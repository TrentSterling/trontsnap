// Shutter feedback, Linux side: same synthesized WAV as Windows (shutter_wav.rs),
// written once to the runtime dir and played fire-and-forget through whichever
// CLI player the system has. paplay/pw-play cover effectively every desktop
// (PulseAudio and PipeWire ship one or the other); aplay is the ALSA last resort.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static WAV_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
static TICK_PATHS: OnceLock<[Option<PathBuf>; 2]> = OnceLock::new();

fn wav_path() -> Option<PathBuf> {
    WAV_PATH
        .get_or_init(|| {
            let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
            let p = dir.join("trontsnap-shutter.wav");
            std::fs::write(&p, crate::shutter_wav::build_shutter_wav()).ok().map(|_| p)
        })
        .clone()
}

/// Fire-and-forget through the first CLI player that exists.
fn play_file(p: PathBuf) {
    std::thread::spawn(move || {
        for player in ["paplay", "pw-play", "aplay"] {
            if let Ok(mut child) = std::process::Command::new(player)
                .arg(&p)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                let _ = child.wait(); // short clip; waiting here also reaps it
                return;
            }
        }
    });
}

/// Play the shutter sound. Fire-and-forget, never blocks the caller, and never
/// panics — a missing audio player should not affect capture success.
pub fn play_shutter() {
    let Some(p) = wav_path() else { return };
    play_file(p);
}

/// A soft 60 ms blip: low (520 Hz) = recording is starting, high (880 Hz) = stop was
/// heard. The recorder's feedback for the seconds when nothing else moves (a kms clip
/// starts ~3 s after the press; delivery lags the stop by 1-3 s). Both land in the
/// part of the audio the recorder trims away.
pub fn play_tick(high: bool) {
    let paths = TICK_PATHS.get_or_init(|| {
        let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        let make = |name: &str, hz: f32| -> Option<PathBuf> {
            let p = dir.join(name);
            std::fs::write(&p, tick_wav(hz)).ok().map(|_| p)
        };
        [make("trontsnap-tick-lo.wav", 520.0), make("trontsnap-tick-hi.wav", 880.0)]
    });
    if let Some(p) = &paths[high as usize] {
        play_file(Path::new(p).to_path_buf());
    }
}

/// 44.1 kHz 16-bit mono sine blip with a fast attack and exponential decay.
fn tick_wav(hz: f32) -> Vec<u8> {
    const RATE: u32 = 44_100;
    let n = (RATE as usize) * 60 / 1000;
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let attack = (i as f32 / 120.0).min(1.0);
        let s = (2.0 * std::f32::consts::PI * hz * t).sin() * attack * (-t / 0.018).exp() * 0.35;
        pcm.extend_from_slice(&((s * i16::MAX as f32) as i16).to_le_bytes());
    }
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
}
