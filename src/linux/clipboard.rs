// Linux clipboard writer. Same public API as the Win32 multi-format writer, but
// the mechanics are completely different: on Linux the clipboard is OWNED by a
// process that must stay alive to serve paste requests, and TrontSnap's one-shot
// capture modes (`trontsnap full` / `region`) exit within seconds. So the owner
// is always a separate daemon that outlives us:
//
//   image / file(s) -> `trontsnap clip-serve` (clipserve.rs), a multi-target X11
//                      owner: image/png + text/uri-list + x-special/gnome-copied-files
//                      for shots, Nautilus's own set (+ bare-path text) for files.
//                      Falls back to the single-target tools below on ANY failure.
//   text (OCR, paths)-> xclip / wl-copy, which fork a tiny daemon that keeps
//                      serving after we exit — the same reason those tools exist.
//
// No retry thread either: "another app holds the clipboard open" is a
// Windows-only failure mode.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

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

/// `which` without the fork: scan PATH for an executable file.
fn have(cmd: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|d| {
            std::fs::metadata(d.join(cmd)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

#[derive(Clone, Copy)]
enum Tool {
    WlCopy,
    Xclip,
}

/// Which single-target tool to use, resolved once per process (it used to fork
/// `which` up to four times per write).
/// HARD-WON: GNOME's Mutter has no wlr-data-control, so wl-copy there falls back
/// to mapping a focus-grabbing surface, which is fragile from a hotkey. xclip
/// through XWayland is bridged by Mutter and verified end to end, so on GNOME
/// (with XWayland) xclip wins; wl-copy only where data-control exists (wlroots, KDE).
fn tool() -> Option<Tool> {
    static TOOL: OnceLock<Option<Tool>> = OnceLock::new();
    *TOOL.get_or_init(|| {
        let gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.to_ascii_uppercase().contains("GNOME"));
        let x = std::env::var_os("DISPLAY").is_some() && have("xclip");
        let wl = on_wayland() && have("wl-copy");
        if gnome && x {
            Some(Tool::Xclip)
        } else if wl {
            Some(Tool::WlCopy)
        } else if x {
            Some(Tool::Xclip)
        } else {
            None
        }
    })
}

/// Pipe `bytes` into the session's clipboard tool under MIME `mime`.
/// The tool forks and keeps owning the selection after this returns.
fn pipe_to_clipboard(mime: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let mut cmd = match tool() {
        Some(Tool::WlCopy) => {
            let mut c = Command::new("wl-copy");
            c.args(["--type", mime]);
            c
        }
        Some(Tool::Xclip) => {
            let mut c = Command::new("xclip");
            c.args(["-selection", "clipboard", "-t", mime]);
            c
        }
        None if on_wayland() => anyhow::bail!(
            "no clipboard tool — install xclip (GNOME) or wl-clipboard (`sudo apt install xclip wl-clipboard`)"
        ),
        None => anyhow::bail!("no clipboard tool — install xclip (`sudo apt install xclip`)"),
    };

    // stderr goes to an unlinked-after-use file, not a pipe: the forked daemon
    // keeps its copy of the fd for its whole life, so reading a pipe to EOF would
    // block forever and closing it could SIGPIPE the daemon on a late message.
    let err_path = dirs::runtime_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("trontsnap-clip-{}.err", std::process::id()));
    let err_file = std::fs::File::create(&err_path)?;
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(err_file).spawn()?;
    let wrote = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdin on clipboard tool"))?
        .write_all(bytes);
    // wl-copy/xclip fork to background and exit the foreground process once the
    // selection is claimed, so this wait is quick and proves the handoff happened.
    let status = child.wait()?;
    let mut msg = String::new();
    if wrote.is_err() || !status.success() {
        let _ = std::fs::File::open(&err_path).and_then(|mut f| f.read_to_string(&mut msg));
    }
    let _ = std::fs::remove_file(&err_path);
    if !status.success() || wrote.is_err() {
        let msg = msg.trim();
        anyhow::bail!(
            "clipboard tool exited with {status}{}{}",
            if msg.is_empty() { "" } else { ": " },
            msg
        );
    }
    Ok(())
}

/// file:// URI with every byte outside the RFC 3986 unreserved set (and '/')
/// percent-encoded. HARD-WON: the old raw `file://{path}` broke on '#' (GTK/Nautilus
/// truncate at the fragment) and on literal %XX (Firefox/Nautilus decode it) —
/// recordings with unusual names, or a home path with either, would not paste.
pub(crate) fn uri_of(p: &Path) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt as _;
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut s = String::from("file://");
    for &b in abs.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => s.push(b as char),
            _ => {
                let _ = write!(s, "%{b:02X}");
            }
        }
    }
    s
}

/// Multi-target owner first, single-target tool as the safety net.
fn serve_or_fallback(
    offer: crate::clipserve::Offer<'_>,
    fallback_mime: &str,
    fallback_bytes: &[u8],
) -> anyhow::Result<()> {
    match crate::clipserve::serve(offer) {
        Ok(()) => Ok(()),
        Err(e) => {
            log_line(&format!("clip-serve failed ({e:#}); single-target fallback"));
            pipe_to_clipboard(fallback_mime, fallback_bytes)
        }
    }
}

/// Put `img` on the clipboard as image/png (+ the saved file, see `set_png`).
/// `png_bytes` must be the exact PNG encoding of `img` (already computed for the
/// disk write, so this is free).
pub fn set_all(img: &RgbaImage, png_bytes: &[u8], file_path: &Path) -> anyhow::Result<()> {
    let _ = img;
    set_png(png_bytes, file_path)
}

/// Same as `set_all` for callers that never decoded pixels (the capture fast
/// path hands gnome-screenshot's PNG bytes straight through). Offers image/png
/// plus the saved FILE (uri-list + gnome-copied-files) so Firefox/Nautilus-style
/// destinations get a real, named file instead of "image.png".
pub fn set_png(png_bytes: &[u8], file_path: &Path) -> anyhow::Result<()> {
    let offer = crate::clipserve::Offer::Image { png: png_bytes, path: file_path };
    serve_or_fallback(offer, "image/png", png_bytes)
        .inspect_err(|e| log_line(&format!("clipboard image failed: {e:#}")))
}

/// Put a FILE reference on the clipboard (recordings — no pixel formats):
/// exactly what Nautilus offers on Ctrl+C, so file managers, browsers, chat apps
/// AND terminals (bare-path text) all get something they understand.
pub fn set_file(file_path: &Path) -> anyhow::Result<()> {
    set_files(std::slice::from_ref(&file_path.to_path_buf()))
}

/// N files at once (gallery multi-select).
pub fn set_files(paths: &[PathBuf]) -> anyhow::Result<()> {
    let list: String = paths.iter().map(|p| format!("{}\r\n", uri_of(p))).collect();
    serve_or_fallback(crate::clipserve::Offer::Files(paths), "text/uri-list", list.as_bytes())
        .inspect_err(|e| log_line(&format!("clipboard file failed: {e:#}")))
}

/// Put plain TEXT on the clipboard (OCR delivery, copied paths). Owned by a
/// forked xclip daemon, so it survives this process (and the app) exiting.
pub fn set_text(text: &str) -> anyhow::Result<()> {
    pipe_to_clipboard("text/plain;charset=utf-8", text.as_bytes())
        .inspect_err(|e| log_line(&format!("clipboard text failed: {e:#}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_encodes_reserved_bytes() {
        let u = uri_of(Path::new("/home/t/Screen Rec #1 (final) 100%é.mp4"));
        assert_eq!(u, "file:///home/t/Screen%20Rec%20%231%20%28final%29%20100%25%C3%A9.mp4");
        // a literal %20 in a name must not be read back as a space
        assert_eq!(uri_of(Path::new("/x/a%20b.mp4")), "file:///x/a%2520b.mp4");
        assert_eq!(uri_of(Path::new("/a-b_c.d~e/f")), "file:///a-b_c.d~e/f");
    }
}
