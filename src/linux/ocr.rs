// Region OCR, Linux side: shell out to tesseract (the de-facto system OCR).
// Offline, no cloud — same positioning as Windows.Media.Ocr on the other side.
// Missing tesseract is a message, never a panic: the picker must not die
// because OCR is unavailable.

use anyhow::Context as _;
use image::RgbaImage;

/// Is tesseract on PATH? Checked before the picker so a missing engine is a
/// message, not a drag that silently produces nothing. Cached: the gallery asks
/// every frame its context menu is open, and a probe is a fork+exec. (The one-shot
/// `trontsnap ocr` process is fresh each hotkey press, so it always probes anew;
/// the long-lived app needs a restart after `apt install tesseract-ocr`.)
pub fn available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        std::process::Command::new("tesseract")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    })
}

/// Recognize the text in `img`. Blocks (tesseract runs to completion); run on a
/// worker thread, never the UI thread. Returns Ok("") when the region simply
/// contains no words.
pub fn recognize(img: &RgbaImage) -> anyhow::Result<String> {
    let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    let png = dir.join(format!("trontsnap-ocr-{}.png", std::process::id()));
    img.save(&png).context("write OCR temp image")?;

    let out = std::process::Command::new("tesseract")
        .arg(&png)
        .arg("stdout")
        .stderr(std::process::Stdio::null())
        .output();
    let _ = std::fs::remove_file(&png);

    let out = out.context(
        "tesseract not found — install it for region OCR \
         (e.g. `sudo apt install tesseract-ocr`)",
    )?;
    if !out.status.success() {
        anyhow::bail!("tesseract failed ({})", out.status);
    }
    // tesseract already emits one line per text line; trim the trailing blank
    // lines it likes to append but keep interior layout (what makes pasted
    // code/logs/tables usable).
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}
