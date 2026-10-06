// Self-contained "shutter" feedback for every successful capture (full + region).
// No bundled asset: the WAV bytes are synthesized in code — a short filtered-noise
// burst shaped by a fast exponential-decay envelope (two "curtain" clicks plus a
// low thud), 44.1kHz 16-bit mono PCM, ~110ms — and played asynchronously via
// winmm's PlaySoundW(SND_MEMORY|SND_ASYNC) from the `windows` crate.
//
// SND_MEMORY playback reads directly from OUR buffer for as long as the sound is
// audible, with no callback telling us when winmm is done — so the bytes must
// stay valid indefinitely from the point PlaySoundW is called. A
// `static OnceLock<Vec<u8>>` gives us exactly that: once synthesized, the Vec's
// heap allocation lives for the rest of the process.

use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

static SHUTTER_WAV: OnceLock<Vec<u8>> = OnceLock::new();

/// Play the shutter sound. Fire-and-forget, never blocks the caller, and never
/// panics/propagates errors — a missing audio device should not affect capture
/// success. Call right after every successful `capture::deliver(..)`.
pub fn play_shutter() {
    let bytes = SHUTTER_WAV.get_or_init(crate::shutter_wav::build_shutter_wav);

    // Raw pointers aren't Send. The buffer is 'static (it lives in a OnceLock for
    // the whole process), so carrying its address as a usize across the spawn
    // boundary and reconstituting the pointer on the other side is sound. The
    // thread keeps PlaySoundW's synchronous device-open cost off the capture path.
    let addr = bytes.as_ptr() as usize;
    std::thread::spawn(move || unsafe {
        let _ = PlaySoundW(
            PCWSTR(addr as *const u16),
            HMODULE(0),
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    });
}
