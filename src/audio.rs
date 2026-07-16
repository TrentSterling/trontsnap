// WASAPI loopback capture of the default output device — "record what you hear" for
// the screen recorder's audio track.
//
// Two classic loopback gotchas are handled here:
//   1. A shared-mode loopback stream only produces packets while something is
//      RENDERING; total silence = no packets = holes in the AAC timeline. Fix: we
//      open a render client on the same device and continuously play silence (the
//      well-known keep-alive trick OBS-style recorders use), so packets always flow
//      and the sample clock is continuous.
//   2. Timestamps: blocks are stamped off a pure sample counter anchored when the
//      first packet arrives relative to the recording epoch, so the audio timeline
//      is gapless and monotonic — exactly what the SinkWriter wants.
//
// The mix format is typically float32 @ 44.1/48kHz; we convert to 16-bit PCM here so
// the recorder can hand blocks straight to the MF AAC encoder.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

/// One block of captured audio, ready for the encoder.
pub struct AudioBlock {
    /// Start time on the recording timeline, 100ns units.
    pub t100ns: i64,
    pub dur100ns: i64,
    /// 16-bit interleaved PCM.
    pub pcm: Vec<u8>,
}

#[derive(Clone, Copy)]
pub struct AudioFormat {
    pub rate: u32,
    pub channels: u16,
}

/// Start loopback capture on a dedicated thread. Blocks are timestamped relative to
/// `epoch` (the recording start). The thread exits when `stop` goes true. Errors mean
/// "no usable audio" — the recorder falls back to video-only.
pub fn start(epoch: Instant, stop: Arc<AtomicBool>) -> anyhow::Result<(AudioFormat, Receiver<AudioBlock>)> {
    // Negotiate the format on the caller's thread so the recorder can set up the
    // encoder stream before any samples arrive; the capture loop then re-opens COM
    // on its own thread.
    let fmt = probe_format()?;
    if fmt.channels != 1 && fmt.channels != 2 {
        anyhow::bail!("unsupported channel count {}", fmt.channels);
    }
    if fmt.rate != 44_100 && fmt.rate != 48_000 {
        // The MF AAC encoder only accepts 44.1k/48k; resampling is out of MVP scope.
        anyhow::bail!("unsupported device sample rate {}", fmt.rate);
    }

    let (tx, rx) = crossbeam_channel::unbounded::<AudioBlock>();
    std::thread::Builder::new()
        .name("trontsnap-audio".into())
        .spawn(move || {
            if let Err(e) = capture_loop(epoch, stop, fmt, tx) {
                eprintln!("trontsnap: audio capture ended with error: {e:#}");
            }
        })?;
    Ok((fmt, rx))
}

/// Open the default render device just long enough to read its mix format.
fn probe_format() -> anyhow::Result<AudioFormat> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let pfmt = client.GetMixFormat()?;
        let fmt = AudioFormat {
            rate: (*pfmt).nSamplesPerSec,
            channels: (*pfmt).nChannels,
        };
        CoTaskMemFree(Some(pfmt as *const _));
        Ok(fmt)
    }
}

fn capture_loop(
    epoch: Instant,
    stop: Arc<AtomicBool>,
    fmt: AudioFormat,
    tx: crossbeam_channel::Sender<AudioBlock>,
) -> anyhow::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        struct ComGuard;
        impl Drop for ComGuard {
            fn drop(&mut self) {
                unsafe { CoUninitialize() }
            }
        }
        let _com = ComGuard;

        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;

        // --- capture side (loopback of the render mix) ---------------------------
        let cap_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let pfmt = cap_client.GetMixFormat()?;
        let bits = (*pfmt).wBitsPerSample;
        let block_align = (*pfmt).nBlockAlign as usize;
        cap_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            2_000_000, // 200ms buffer
            0,
            pfmt,
            None,
        )?;
        let capture: IAudioCaptureClient = cap_client.GetService()?;

        // --- silence keep-alive (see module docs) --------------------------------
        let render_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        render_client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 2_000_000, 0, pfmt, None)?;
        let render: IAudioRenderClient = render_client.GetService()?;
        let render_buf_frames = render_client.GetBufferSize()?;
        CoTaskMemFree(Some(pfmt as *const _));

        render_client.Start()?;
        cap_client.Start()?;

        // Sample-counter clock, anchored at the first captured packet.
        let mut base_t100: Option<i64> = None;
        let mut samples_sent: u64 = 0;
        let out_block = 2usize * fmt.channels as usize; // bytes per frame, 16-bit PCM

        while !stop.load(Ordering::SeqCst) {
            // Keep the render stream fed with silence so loopback never starves.
            let padding = render_client.GetCurrentPadding()?;
            let free = render_buf_frames.saturating_sub(padding);
            if free > 0 {
                let p = render.GetBuffer(free)?;
                std::ptr::write_bytes(p, 0, free as usize * block_align);
                render.ReleaseBuffer(free, 0)?;
            }

            // Drain everything the capture side has.
            loop {
                let packet = capture.GetNextPacketSize()?;
                if packet == 0 {
                    break;
                }
                let mut pdata: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                capture.GetBuffer(&mut pdata, &mut frames, &mut flags, None, None)?;
                if frames > 0 {
                    let n = frames as usize;
                    let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                    let mut pcm = vec![0u8; n * out_block];
                    if !silent {
                        convert_to_i16(pdata, n * fmt.channels as usize, bits, &mut pcm);
                    }
                    let t0 = *base_t100.get_or_insert_with(|| {
                        (epoch.elapsed().as_nanos() / 100) as i64
                    });
                    let t = t0 + (samples_sent as i64) * 10_000_000 / fmt.rate as i64;
                    let dur = (frames as i64) * 10_000_000 / fmt.rate as i64;
                    samples_sent += frames as u64;
                    let _ = tx.send(AudioBlock { t100ns: t, dur100ns: dur, pcm });
                }
                capture.ReleaseBuffer(frames)?;
            }

            std::thread::sleep(Duration::from_millis(10));
        }

        let _ = cap_client.Stop();
        let _ = render_client.Stop();
        Ok(())
    }
}

/// Convert `count` samples (not frames) of the device's native format to i16 LE.
/// Mix formats are float32 in practice; 16-bit PCM passes straight through.
unsafe fn convert_to_i16(src: *const u8, count: usize, bits: u16, out: &mut [u8]) {
    match bits {
        32 => {
            let f = std::slice::from_raw_parts(src as *const f32, count);
            for (i, s) in f.iter().enumerate() {
                let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                out[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
        16 => {
            let bytes = std::slice::from_raw_parts(src, count * 2);
            out[..count * 2].copy_from_slice(bytes);
        }
        _ => {} // unsupported depth: leave silence rather than screech
    }
}
