// Recorder output formats. Capture always records H.264 MP4 through Media
// Foundation (hardware encoder, audio, no dependencies). GIF, WEBM and resized
// output are an optional ffmpeg pass over that finished file after Stop.
//
// WHY A POST-PASS AND NOT A LIVE PIPE: VP9 can't keep up with 1080p60 in real
// time, and a palettegen/paletteuse graph fed from stdin buffers the whole stream
// in RAM anyway. Converting the finished file costs a few seconds and never drops
// frames. It also means nothing breaks without ffmpeg: MP4 still records, GIF
// falls back to the built-in encoder (gifexport.rs), and WEBM says why it's MP4.
//
// ffmpeg is never bundled (small is the point). It is found on PATH, next to
// trontsnap.exe, or at TRONTSNAP_FFMPEG.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Gif,
    Mp4,
    Webm,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Gif, Format::Mp4, Format::Webm];
    pub fn ext(self) -> &'static str {
        match self {
            Format::Gif => "gif",
            Format::Mp4 => "mp4",
            Format::Webm => "webm",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::Gif => "GIF",
            Format::Mp4 => "MP4",
            Format::Webm => "WEBM",
        }
    }
    pub fn parse(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.ext().eq_ignore_ascii_case(s.trim()))
    }
}

/// Output size relative to what was captured.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scale {
    Native,
    /// Percent of the captured size.
    Pct(u32),
    /// Fixed output height (width follows the aspect); never upscales.
    Height(u32),
}

impl Scale {
    pub const PRESETS: [Scale; 8] = [
        Scale::Native,
        Scale::Pct(75),
        Scale::Pct(50),
        Scale::Pct(33),
        Scale::Height(1080),
        Scale::Height(720),
        Scale::Height(480),
        Scale::Height(360),
    ];
    pub fn label(self) -> String {
        match self {
            Scale::Native => "Native".into(),
            Scale::Pct(p) => format!("{p}%"),
            Scale::Height(h) => format!("{h}p"),
        }
    }
    /// "native", "50%", "50", "720p".
    pub fn parse(s: &str) -> Option<Scale> {
        let s = s.trim().to_ascii_lowercase();
        if s == "native" || s == "100%" || s == "100" {
            return Some(Scale::Native);
        }
        if let Some(h) = s.strip_suffix('p') {
            return h.parse().ok().filter(|h| *h >= 16).map(Scale::Height);
        }
        s.trim_end_matches('%').parse().ok().filter(|p| (1..100).contains(p)).map(Scale::Pct)
    }
    /// The ffmpeg scale filter, or None for native.
    fn filter(self) -> Option<String> {
        match self {
            Scale::Native => None,
            // Even dims keep H.264/VP9 happy; lanczos keeps pixel art and UI crisp.
            Scale::Pct(p) => Some(format!("scale=trunc(iw*{p}/200)*2:-2:flags=lanczos")),
            Scale::Height(h) => Some(format!("scale=-2:'min({h},ih)':flags=lanczos")),
        }
    }
}

/// Does this format/scale need a pass after capture?
pub fn needs_pass(fmt: Format, scale: Scale) -> bool {
    !(fmt == Format::Mp4 && scale == Scale::Native)
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TRONTSNAP_FFMPEG").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        let p = dir.join("ffmpeg.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("ffmpeg.exe")).find(|p| p.is_file())
}

/// Where the raw capture goes when a pass will follow (out of the gallery's
/// watched folder, so the intermediate never shows up as a capture).
pub fn temp_raw() -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join("TrontSnap");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("rec_{}.mp4", chrono::Local::now().format("%Y%m%d_%H%M%S_%3f"))))
}

/// Turn the raw capture into the requested output. Returns the final file and an
/// optional note for the user (a fallback was taken). Removes `raw` when done.
pub fn finish(raw: &Path, fmt: Format, scale: Scale, fps: u32) -> anyhow::Result<(PathBuf, Option<String>)> {
    if !needs_pass(fmt, scale) {
        return Ok((raw.to_path_buf(), None));
    }
    let Some(ffmpeg) = find_ffmpeg() else {
        return fallback(raw, fmt);
    };
    let dst = crate::recorder::out_path(fmt.ext())?;
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-y".into()];
    args.push("-i".into());
    args.push(raw.to_string_lossy().into_owned());
    let scale_f = scale.filter();
    match fmt {
        Format::Gif => {
            // Browsers clamp GIF frame delays under 20ms to 100ms, so cap at 50fps.
            let gfps = fps.clamp(1, 50);
            let mut chain = format!("fps={gfps}");
            if let Some(s) = &scale_f {
                chain.push(',');
                chain.push_str(s);
            }
            // One palette for the whole clip, built from the pixels that change
            // (stats_mode=diff), and only changed rectangles re-dithered: small
            // files for UI and game footage without full-frame shimmer.
            args.push("-vf".into());
            args.push(format!(
                "{chain},split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle"
            ));
            args.push("-loop".into());
            args.push("0".into());
        }
        Format::Mp4 => {
            if let Some(s) = &scale_f {
                args.extend(["-vf".into(), s.clone()]);
            }
            args.extend(
                ["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-c:a", "copy"]
                    .map(String::from),
            );
        }
        Format::Webm => {
            if let Some(s) = &scale_f {
                args.extend(["-vf".into(), s.clone()]);
            }
            args.extend(
                [
                    "-c:v", "libvpx-vp9", "-crf", "32", "-b:v", "0", "-row-mt", "1", "-deadline", "good", "-cpu-used", "4",
                    "-pix_fmt", "yuv420p", "-c:a", "libopus", "-b:a", "128k",
                ]
                .map(String::from),
            );
        }
    }
    args.push(dst.to_string_lossy().into_owned());

    let out = hidden(Command::new(&ffmpeg)).args(&args).output()?;
    if !out.status.success() || !dst.is_file() {
        let _ = std::fs::remove_file(&dst);
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
        eprintln!("trontsnap: ffmpeg failed: {err}");
        let line = line.to_string();
        let (p, _) = fallback(raw, fmt)?;
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("file").to_uppercase();
        return Ok((p, Some(format!("ffmpeg failed ({line}), saved {ext}"))));
    }
    let _ = std::fs::remove_file(raw);
    Ok((dst, None))
}

/// No ffmpeg (or it failed): GIF via the built-in encoder, everything else stays MP4.
fn fallback(raw: &Path, fmt: Format) -> anyhow::Result<(PathBuf, Option<String>)> {
    if fmt == Format::Gif {
        let dst = crate::recorder::out_path("gif")?;
        crate::gifexport::export_to(raw, &dst)?;
        let _ = std::fs::remove_file(raw);
        return Ok((dst, Some("no ffmpeg: built-in GIF (12 fps, 800px max)".into())));
    }
    let dst = crate::recorder::out_path("mp4")?;
    move_file(raw, &dst)?;
    let note = match fmt {
        Format::Webm => "no ffmpeg: saved MP4 instead of WEBM",
        _ => "no ffmpeg: saved at native size",
    };
    Ok((dst, Some(note.into())))
}

fn move_file(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if std::fs::rename(src, dst).is_err() {
        // %TEMP% and Pictures can sit on different volumes.
        std::fs::copy(src, dst)?;
        let _ = std::fs::remove_file(src);
    }
    Ok(())
}

/// No console window flashes up for the ffmpeg child.
fn hidden(mut c: Command) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_values() {
        assert_eq!(Format::parse("GIF"), Some(Format::Gif));
        assert_eq!(Format::parse("webm"), Some(Format::Webm));
        assert_eq!(Format::parse("avi"), None);
        assert_eq!(Scale::parse("native"), Some(Scale::Native));
        assert_eq!(Scale::parse("50%"), Some(Scale::Pct(50)));
        assert_eq!(Scale::parse("720p"), Some(Scale::Height(720)));
        assert_eq!(Scale::parse("0"), None);
        assert!(!needs_pass(Format::Mp4, Scale::Native));
        assert!(needs_pass(Format::Mp4, Scale::Pct(50)));
        assert!(needs_pass(Format::Gif, Scale::Native));
    }
}
