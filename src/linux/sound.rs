// Shutter feedback, Linux side: same synthesized WAV as Windows (shutter_wav.rs),
// written once to the runtime dir and played fire-and-forget through whichever
// CLI player the system has. paplay/pw-play cover effectively every desktop
// (PulseAudio and PipeWire ship one or the other); aplay is the ALSA last resort.

use std::path::PathBuf;
use std::sync::OnceLock;

static WAV_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

fn wav_path() -> Option<PathBuf> {
    WAV_PATH
        .get_or_init(|| {
            let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
            let p = dir.join("trontsnap-shutter.wav");
            std::fs::write(&p, crate::shutter_wav::build_shutter_wav()).ok().map(|_| p)
        })
        .clone()
}

/// Play the shutter sound. Fire-and-forget, never blocks the caller, and never
/// panics — a missing audio player should not affect capture success.
pub fn play_shutter() {
    let Some(p) = wav_path() else { return };
    std::thread::spawn(move || {
        for player in ["paplay", "pw-play", "aplay"] {
            if let Ok(mut child) = std::process::Command::new(player)
                .arg(&p)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                let _ = child.wait(); // ~110ms clip; waiting here also reaps it
                return;
            }
        }
    });
}
