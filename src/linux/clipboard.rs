// Linux clipboard writer. Same public API as the Win32 multi-format writer, but
// the mechanics are completely different: on Linux the clipboard is OWNED by a
// process that must stay alive to serve paste requests, and TrontSnap's one-shot
// capture modes (`trontsnap full` / `region`) exit within seconds. So writes go
// through wl-copy (Wayland) or xclip (X11), both of which fork a tiny daemon
// that keeps serving the selection after we exit — the same reason those tools
// exist at all. No retry thread either: "another app holds the clipboard open"
// is a Windows-only failure mode.
//
//   image  -> image/png        (what Firefox/Chrome/Discord/GIMP paste targets read)
//   file   -> text/uri-list    (what file managers and chat apps read for a file)
//   text   -> plain text       (the OCR delivery)

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use image::RgbaImage;

/// Append a line to ~/.local/share/trontsnap/trontsnap.log — same rationale as
/// the Windows side: a clipboard failure must leave one real error message
/// somewhere findable, because stderr from a hotkey-spawned process goes nowhere.
pub fn log_line(msg: &str) {
    let Some(base) = dirs::data_local_dir() else { return };
    let dir = base.join("trontsnap");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("trontsnap.log"))
    {
        let _ = writeln!(f, "[{stamp}] {msg}");
    }
}

fn on_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn have(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Pipe `bytes` into the session's clipboard tool under MIME `mime`.
/// The tool forks and keeps owning the selection after this returns.
fn pipe_to_clipboard(mime: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let mut cmd = if on_wayland() && have("wl-copy") {
        let mut c = Command::new("wl-copy");
        c.args(["--type", mime]);
        c
    } else if have("xclip") {
        let mut c = Command::new("xclip");
        c.args(["-selection", "clipboard", "-t", mime]);
        c
    } else if on_wayland() {
        anyhow::bail!("no clipboard tool — install wl-clipboard (`sudo apt install wl-clipboard`)")
    } else {
        anyhow::bail!("no clipboard tool — install xclip (`sudo apt install xclip`)")
    };

    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin on clipboard tool"))?
        .write_all(bytes)?;
    // wl-copy/xclip fork to background and exit the foreground process once the
    // selection is claimed, so this wait is quick and proves the handoff happened.
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("clipboard tool exited with {status}");
    }
    Ok(())
}

/// Put `img` on the clipboard as image/png. `png_bytes` must be the exact PNG
/// encoding of `img` (already computed for the disk write, so this is free);
/// `file_path` is unused here — Linux paste targets read the pixels, and a
/// text/uri-list alongside would require a second selection owner fighting the
/// first.
pub fn set_all(img: &RgbaImage, png_bytes: &[u8], _file_path: &Path) -> anyhow::Result<()> {
    let _ = img;
    pipe_to_clipboard("image/png", png_bytes).inspect_err(|e| log_line(&format!("clipboard image failed: {e:#}")))
}

/// Put a FILE reference on the clipboard (recordings — no pixel formats).
/// text/uri-list is what file managers and chat apps look for.
pub fn set_file(file_path: &Path) -> anyhow::Result<()> {
    let uri = format!("file://{}\n", file_path.display());
    pipe_to_clipboard("text/uri-list", uri.as_bytes())
        .inspect_err(|e| log_line(&format!("clipboard file failed: {e:#}")))
}

/// Put plain TEXT on the clipboard (the OCR delivery).
pub fn set_text(text: &str) -> anyhow::Result<()> {
    pipe_to_clipboard("text/plain;charset=utf-8", text.as_bytes())
        .inspect_err(|e| log_line(&format!("clipboard text failed: {e:#}")))
}
