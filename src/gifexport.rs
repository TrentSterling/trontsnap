// Gallery -> GIF export for recordings. Decodes frames with videothumb::VideoReader
// (Media Foundation), samples down to ~12fps, caps width at 800px, and encodes with
// the image crate's GifEncoder (NeuQuant palette per frame). No ffmpeg, no new deps.
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
    let mut reader = VideoReader::open(src)?;
    let file = std::io::BufWriter::new(std::fs::File::create(dst)?);
    // Speed 30 = fastest NeuQuant sampling. Screen recordings are flat UI colors, so
    // the palette loss is invisible while cutting export time roughly in half
    // (measured: 72s -> ~30s for a ~40s clip at speed 12 vs 30).
    let mut enc = GifEncoder::new_with_speed(file, 30);
    enc.set_repeat(Repeat::Infinite)?;

    let step = 10_000_000i64 / GIF_FPS as i64;
    let mut next_t = i64::MIN; // accept the first frame wherever the stream starts
    let mut frames = 0usize;
    while let Some((img, t)) = reader.read_frame()? {
        if t < next_t {
            continue; // between GIF ticks: skip (30fps source -> ~12fps GIF)
        }
        next_t = t + step;
        let img = if img.width() > MAX_W {
            DynamicImage::ImageRgba8(img).thumbnail(MAX_W, MAX_W * 2).to_rgba8()
        } else {
            img
        };
        enc.encode_frame(Frame::from_parts(
            img,
            0,
            0,
            Delay::from_numer_denom_ms(1000, GIF_FPS),
        ))?;
        frames += 1;
    }
    drop(enc);
    if frames == 0 {
        let _ = std::fs::remove_file(dst);
        anyhow::bail!("no frames decoded");
    }
    Ok(())
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
