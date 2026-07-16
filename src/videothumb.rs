// First-frame extraction from recorded MP4s via the Media Foundation Source Reader —
// the read-side twin of recorder.rs's SinkWriter (still no ffmpeg, no new crates).
// Feeds the gallery thumbnail cache and the capture toast.
//
// A file still being written by an active recording has no moov atom yet, so opening
// it simply errors — callers treat that as "no thumb yet" (the gallery re-requests
// when the watcher sees the finished file's mtime change).

use std::path::Path;

use image::RgbaImage;
use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaBuffer, IMFSample, IMFSourceReader, MFCreateAttributes,
    MFCreateMediaType, MFCreateSourceReaderFromURL, MFShutdown, MFStartup, MFMediaType_Video,
    MFVideoFormat_RGB32, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::recorder::MF_VERSION;

/// MF_SOURCE_READER_FIRST_VIDEO_STREAM (the enum constant, as the DWORD the APIs take).
const FIRST_VIDEO_STREAM: u32 = 0xFFFF_FFFC;
const READERF_ENDOFSTREAM: u32 = 0x2;

/// Paired CoInitialize+MFStartup for the lifetime of a reader. Ref-counted process-
/// wide, so nesting with the recorder's own session is fine.
struct MfSession;

impl MfSession {
    fn new() -> anyhow::Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, 0)?;
        }
        Ok(Self)
    }
}

impl Drop for MfSession {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

/// A sequential RGBA frame reader over a video file. Powers the gallery/toast
/// first-frame thumbnails and the GIF exporter.
pub struct VideoReader {
    // Field order matters: `reader` must drop before `_mf` shuts MF down.
    reader: IMFSourceReader,
    _mf: MfSession,
    pub w: u32,
    pub h: u32,
    stride: i32,
}

impl VideoReader {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let mf = MfSession::new()?;
        unsafe {
            let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 1)?;
            let attrs = attrs.ok_or_else(|| anyhow::anyhow!("no MF attributes"))?;
            // Lets the reader insert the video processor so H.264 -> RGB32 conversion
            // is its problem, not ours.
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
            let reader: IMFSourceReader =
                MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), Some(&attrs))?;

            let want = MFCreateMediaType()?;
            want.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            want.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            reader.SetCurrentMediaType(FIRST_VIDEO_STREAM, None, &want)?;

            // Actual negotiated geometry (never trust the requested type).
            let cur = reader.GetCurrentMediaType(FIRST_VIDEO_STREAM)?;
            let fs = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let w = (fs >> 32) as u32;
            let h = (fs & 0xFFFF_FFFF) as u32;
            if w == 0 || h == 0 {
                anyhow::bail!("zero-sized video");
            }
            // Positive stride = top-down rows, negative = bottom-up; absent = assume
            // packed top-down (verified against our own recordings by the smoke test).
            let stride = cur
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|v| v as i32)
                .unwrap_or((w * 4) as i32);
            Ok(Self { reader, _mf: mf, w, h, stride })
        }
    }

    /// Next decoded frame + its timestamp (100ns units). Ok(None) at end of stream.
    pub fn read_frame(&mut self) -> anyhow::Result<Option<(RgbaImage, i64)>> {
        unsafe {
            // A few reads may deliver only stream ticks before the first real frame.
            for _ in 0..64 {
                let mut flags = 0u32;
                let mut t: i64 = 0;
                let mut s: Option<IMFSample> = None;
                self.reader.ReadSample(
                    FIRST_VIDEO_STREAM,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut t),
                    Some(&mut s),
                )?;
                if flags & READERF_ENDOFSTREAM != 0 {
                    return Ok(None);
                }
                if let Some(sample) = s {
                    return Ok(Some((self.convert(&sample)?, t)));
                }
            }
            Ok(None)
        }
    }

    /// BGRA sample buffer -> top-down RGBA image, honoring the stride sign.
    unsafe fn convert(&self, sample: &IMFSample) -> anyhow::Result<RgbaImage> {
        let (w, h) = (self.w, self.h);
        let pitch = self.stride.unsigned_abs() as usize;
        let buf: IMFMediaBuffer = sample.ConvertToContiguousBuffer()?;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut ptr, None, Some(&mut len))?;
        let data = std::slice::from_raw_parts(ptr, len as usize);

        let row_bytes = (w * 4) as usize;
        let mut out = vec![0u8; row_bytes * h as usize];
        for y in 0..h as usize {
            let src_row = if self.stride >= 0 { y } else { h as usize - 1 - y };
            let start = src_row * pitch;
            if start + row_bytes > data.len() {
                break; // short buffer: keep what we have rather than panicking
            }
            let src = &data[start..start + row_bytes];
            let dst = &mut out[y * row_bytes..(y + 1) * row_bytes];
            for x in 0..w as usize {
                let o = x * 4;
                dst[o] = src[o + 2]; // R <- B position (BGRA -> RGBA)
                dst[o + 1] = src[o + 1];
                dst[o + 2] = src[o];
                dst[o + 3] = 255;
            }
        }
        let _ = buf.Unlock();
        RgbaImage::from_raw(w, h, out).ok_or_else(|| anyhow::anyhow!("frame assembly failed"))
    }
}

/// Decode the first video frame of `path` as RGBA. Blocking (fast — one frame);
/// call off the UI thread. Errors for unreadable/incomplete files.
pub fn first_frame(path: &Path) -> anyhow::Result<RgbaImage> {
    let mut reader = VideoReader::open(path)?;
    match reader.read_frame()? {
        Some((img, _)) => Ok(img),
        None => anyhow::bail!("no video sample in file"),
    }
}

#[cfg(test)]
mod tests {
    /// Manual smoke test against a real recording (orientation + color sanity):
    /// TRONTSNAP_TEST_MP4=<path> TRONTSNAP_TEST_OUT=<png> cargo test -- --ignored
    #[test]
    #[ignore]
    fn first_frame_smoke() {
        let Ok(p) = std::env::var("TRONTSNAP_TEST_MP4") else {
            return;
        };
        let img = super::first_frame(std::path::Path::new(&p)).expect("decode first frame");
        assert!(img.width() > 0 && img.height() > 0);
        if let Ok(out) = std::env::var("TRONTSNAP_TEST_OUT") {
            img.save(out).expect("save png");
        }
    }
}
