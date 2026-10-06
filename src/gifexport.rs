// Gallery -> GIF export for recordings. Decodes frames with videothumb::VideoReader
// (Media Foundation on Windows, ffmpeg on Linux), samples down to ~12fps, caps width at
// 800px, and encodes with the image crate's GifEncoder (NeuQuant palette per frame).
//
// Per-frame quantization is the slow part (~tens of ms per frame at 800px), so this
// runs on a background thread and the gallery reports completion via its status line;
// the exported .gif lands next to the source MP4, where the live watcher picks it up
// and it appears in the timeline like any capture.

use std::path::{Path, PathBuf};

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, DynamicImage, Frame};

use crate::videothumb::VideoReader;

const GIF_FPS: u32 = 12;
const MAX_W: u32 = 800;

/// Export `src` (an MP4) to a sibling .gif; returns the written path.
pub fn export(src: &Path) -> anyhow::Result<PathBuf> {
    let dst = src.with_extension("gif");
    export_to(src, &dst)?;
    Ok(dst)
}

pub fn export_to(src: &Path, dst: &Path) -> anyhow::Result<()> {
    // Linux: ffmpeg resamples to GIF_FPS and scales to MAX_W itself, so a 30 fps (or VFR
    // webm) recording crosses the pipe as ~12 small frames per second instead of every
    // full-size source frame (a 10 s VFR clip used to push ~10,000 duplicated 1366x768
    // frames: 7+ minutes). The Media Foundation reader on Windows decodes natively.
    #[cfg(not(windows))]
    let mut reader = VideoReader::open_resampled(src, GIF_FPS, MAX_W)?;
    #[cfg(windows)]
    let mut reader = VideoReader::open(src)?;
    let file = std::io::BufWriter::new(std::fs::File::create(dst)?);
    // Speed 30 = fastest NeuQuant sampling. Screen recordings are flat UI colors, so
    // the palette loss is invisible while cutting export time roughly in half
    // (measured: 72s -> ~30s for a ~40s clip at speed 12 vs 30).
    let mut enc = GifEncoder::new_with_speed(file, 30);
    enc.set_repeat(Repeat::Infinite)?;

    let step = 10_000_000i64 / GIF_FPS as i64;
    let mut next_t = i64::MIN; // accept the first frame wherever the stream starts
    let mut first_t: Option<i64> = None;
    let mut emitted_cs = 0i64;
    // A frame's delay is only known once the NEXT kept frame arrives (the real gap), so
    // each kept frame waits one step in `pending` (image, its timestamp).
    let mut pending: Option<(DynamicImage, i64)> = None;
    let mut frames = 0usize;
    while let Some((img, t)) = reader.read_frame()? {
        if t < next_t {
            continue; // between GIF ticks: skip (30fps source -> ~12fps GIF)
        }
        // HARD-WON: advance on a fixed grid, not `t + step` from the frame just kept. With
        // 33 ms source frames that kept every 3rd frame (100 ms apart) but labelled each
        // 83 ms, so a 30 fps recording played back 1.2x too fast.
        next_t = if next_t == i64::MIN {
            t + step
        } else {
            let mut n = next_t + step;
            while n <= t {
                n += step;
            }
            n
        };
        let first = *first_t.get_or_insert(t);
        let img = if img.width() > MAX_W {
            DynamicImage::ImageRgba8(img).thumbnail(MAX_W, MAX_W * 2)
        } else {
            DynamicImage::ImageRgba8(img)
        };
        if let Some((prev, _)) = pending.replace((img, t)) {
            put_frame(&mut enc, prev, t, first, &mut emitted_cs)?;
            frames += 1;
        }
    }
    if let (Some((last, last_t)), Some(first)) = (pending, first_t) {
        // The final frame has no successor: show it for one grid step.
        put_frame(&mut enc, last, last_t + step, first, &mut emitted_cs)?;
        frames += 1;
    }
    drop(enc);
    if frames == 0 {
        let _ = std::fs::remove_file(dst);
        anyhow::bail!("no frames decoded");
    }
    Ok(())
}

/// Encode one frame that stays up until `t_next` (100ns units). Delays are whole centiseconds;
/// deriving each from the CUMULATIVE timestamp keeps the rounding from drifting over a long
/// clip, and the 2 cs floor is there because browsers promote anything shorter to 10 cs.
fn put_frame<W: std::io::Write>(
    enc: &mut GifEncoder<W>,
    img: DynamicImage,
    t_next: i64,
    first: i64,
    emitted_cs: &mut i64,
) -> image::ImageResult<()> {
    let want = (t_next - first + 50_000) / 100_000;
    let cs = (want - *emitted_cs).max(2);
    *emitted_cs += cs;
    enc.encode_frame(Frame::from_parts(img.to_rgba8(), 0, 0, Delay::from_numer_denom_ms(cs as u32 * 10, 1)))
}

#[cfg(test)]
mod tests {
    /// Manual smoke test against a real recording:
    /// TRONTSNAP_TEST_MP4=<mp4> TRONTSNAP_TEST_GIF=<out.gif> cargo test gif_export_smoke -- --ignored
    #[test]
    #[ignore]
    fn gif_export_smoke() {
        let Ok(src) = std::env::var("TRONTSNAP_TEST_MP4") else {
            return;
        };
        let Ok(dst) = std::env::var("TRONTSNAP_TEST_GIF") else {
            return;
        };
        super::export_to(std::path::Path::new(&src), std::path::Path::new(&dst))
            .expect("gif export");
        let meta = std::fs::metadata(&dst).expect("gif written");
        assert!(meta.len() > 1024, "gif suspiciously small: {} bytes", meta.len());
    }
}
