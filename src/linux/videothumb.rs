// Video frame extraction, Linux side: ffprobe for geometry, ffmpeg piping raw
// RGBA frames on stdout for the sequential reader. Same public surface as the
// Media Foundation twin (VideoReader {w, h}, read_frame -> (frame, t) with t in
// 100ns units — gifexport's sampling math depends on that unit).

use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use anyhow::Context as _;
use image::RgbaImage;

pub struct VideoReader {
    child: Child,
    pub w: u32,
    pub h: u32,
    /// 100ns units per frame, from the probed frame rate.
    frame_step: i64,
    next_index: i64,
}

impl VideoReader {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let out = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,r_frame_rate",
                "-of",
                "csv=p=0",
            ])
            .arg(path)
            .stderr(Stdio::null())
            .output()
            .context("ffprobe not found — install ffmpeg for video thumbnails/GIF export")?;
        if !out.status.success() {
            anyhow::bail!("ffprobe failed on {}", path.display());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().next().unwrap_or_default();
        let mut parts = line.trim().split(',');
        let w: u32 = parts.next().and_then(|s| s.parse().ok()).context("no width")?;
        let h: u32 = parts.next().and_then(|s| s.parse().ok()).context("no height")?;
        let fps = parts
            .next()
            .and_then(|s| {
                let (num, den) = s.split_once('/')?;
                let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
                (den > 0.0 && num > 0.0).then(|| num / den)
            })
            .unwrap_or(30.0);
        if w == 0 || h == 0 {
            anyhow::bail!("zero-sized video");
        }

        let child = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("ffmpeg not found — install ffmpeg for video thumbnails/GIF export")?;

        Ok(Self { child, w, h, frame_step: (10_000_000f64 / fps) as i64, next_index: 0 })
    }

    /// Next decoded frame + its timestamp (100ns units). Ok(None) at end of stream.
    pub fn read_frame(&mut self) -> anyhow::Result<Option<(RgbaImage, i64)>> {
        let Some(stdout) = self.child.stdout.as_mut() else { return Ok(None) };
        let mut buf = vec![0u8; (self.w * self.h * 4) as usize];
        match stdout.read_exact(&mut buf) {
            Ok(()) => {
                let t = self.next_index * self.frame_step;
                self.next_index += 1;
                let img = RgbaImage::from_raw(self.w, self.h, buf)
                    .ok_or_else(|| anyhow::anyhow!("frame assembly failed"))?;
                Ok(Some((img, t)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for VideoReader {
    fn drop(&mut self) {
        // Reader abandoned mid-stream (first-frame use): reap the decoder.
        let _ = self.child.kill();
        let _ = self.child.wait();
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
