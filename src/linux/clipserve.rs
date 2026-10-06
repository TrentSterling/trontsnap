// `trontsnap clip-serve`: a multi-target X11 clipboard owner (XWayland on GNOME).
//
// WHY this exists: xclip/wl-copy serve exactly ONE mime type per process, so a
// screenshot could only be an image and a recording only a uri-list. Real paste
// destinations want a SET (what Nautilus offers when you copy a file):
//   - terminals (VTE, GTK3) read text/plain / the file trio, and got nothing
//   - Firefox turns image/png + uri-list into the real file instead of "image.png"
//   - file managers want x-special/gnome-copied-files
// A single selection owner can offer all of them, and the Mutter XWayland bridge
// carries every target to native-Wayland clients (measured, see LINUX.md).
//
// The owner has to outlive the one-shot `trontsnap full`, so it is a detached
// child of this same binary: the parent pipes the payload in, waits for READY
// (ownership confirmed), and leaves. The child exits when something else takes
// the clipboard (SelectionClear) or the X connection drops. The parent falls
// back to xclip/wl-copy on ANY failure here, so this is strictly an upgrade.
//
//   image : clip-serve image <saved-path>   stdin = PNG bytes
//           -> image/png + text/uri-list + x-special/gnome-copied-files
//           (deliberately NO text/plain: a path text flavor next to the image
//           could make a rich-text field paste the path instead of the picture)
//   files : clip-serve files <path>...
//           -> Nautilus's own set: uri-list, gnome-copied-files, bare-path text

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
    SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, NONE};

use crate::clipboard::uri_of;

/// What to put on the clipboard.
pub enum Offer<'a> {
    /// A PNG plus the file it was saved to (so Firefox/Nautilus get a real file).
    Image { png: &'a [u8], path: &'a Path },
    /// One or more files (recordings, gallery multi-select).
    Files(&'a [PathBuf]),
}

// === Parent side ===

/// Spawn the detached owner and wait until it holds the selection. Err on any
/// problem (no DISPLAY, exec failure, timeout); the caller falls back to xclip.
pub fn serve(offer: Offer<'_>) -> anyhow::Result<()> {
    if std::env::var_os("TRONTSNAP_CLIPSERVE").is_some_and(|v| v == "off") {
        bail!("disabled by TRONTSNAP_CLIPSERVE=off");
    }
    if std::env::var_os("DISPLAY").is_none() {
        bail!("no DISPLAY (no XWayland)");
    }
    // HARD-WON: a rebuilt binary leaves /proc/self/exe as ".../trontsnap (deleted)"
    // in the long-running app; the path without the suffix is the fresh build.
    let exe = std::env::current_exe().context("current_exe")?;
    let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));

    let mut cmd = Command::new(exe);
    cmd.arg("clip-serve");
    let payload: &[u8] = match &offer {
        Offer::Image { png, path } => {
            cmd.arg("image").arg(path);
            png
        }
        Offer::Files(paths) => {
            cmd.arg("files").args(paths.iter());
            &[]
        }
    };
    // Own process group: the owner must survive the hotkey's process-group cleanup.
    let mut child = cmd
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn clip-serve")?;

    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let wrote = stdin.write_all(payload);
    drop(stdin); // EOF = payload complete
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });

    let ready = wrote.is_ok()
        && matches!(rx.recv_timeout(Duration::from_millis(4000)), Ok(l) if l.trim() == "READY");
    if !ready {
        let _ = child.kill();
        let _ = child.wait();
        bail!("clip-serve did not report READY");
    }
    // Reap it when it eventually exits (clipboard replaced) so the long-running
    // app doesn't collect zombies; in a one-shot this thread just dies with us.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// === Child side ===

struct Entry {
    target: Atom,
    ty: Atom,
    data: Vec<u8>,
}

/// One in-flight INCR transfer to a requestor.
struct Incr {
    entry: usize,
    off: usize,
    done_sent: bool,
    started: Instant,
}

fn text_set(paths: &[PathBuf]) -> Vec<u8> {
    paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n").into_bytes()
}

/// uri-list is CRLF-terminated per line (RFC 2483); gnome-copied-files is
/// "copy" + LF-separated URIs with no trailing newline (what Nautilus emits).
fn uri_blobs(paths: &[PathBuf]) -> (Vec<u8>, Vec<u8>) {
    let uris: Vec<String> = paths.iter().map(|p| uri_of(p)).collect();
    let list: String = uris.iter().map(|u| format!("{u}\r\n")).collect();
    let gnome = format!("copy\n{}", uris.join("\n"));
    (list.into_bytes(), gnome.into_bytes())
}

/// Child entry point: argv = clip-serve (image <path> | files <path>...).
pub fn run() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(2);
    let kind = args.next().ok_or_else(|| anyhow!("missing kind"))?;
    let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();

    // (target name, reply type name, bytes); resolved to atoms below.
    let mut set: Vec<(&str, &str, Vec<u8>)> = Vec::new();
    if kind == OsString::from("image") {
        let mut png = Vec::new();
        std::io::stdin().read_to_end(&mut png)?;
        set.push(("image/png", "image/png", png));
        if !paths.is_empty() {
            let (list, gnome) = uri_blobs(&paths);
            set.push(("text/uri-list", "text/uri-list", list));
            set.push(("x-special/gnome-copied-files", "x-special/gnome-copied-files", gnome));
        }
    } else if kind == OsString::from("files") {
        if paths.is_empty() {
            bail!("files: no paths");
        }
        let (list, gnome) = uri_blobs(&paths);
        set.push(("text/uri-list", "text/uri-list", list));
        set.push(("x-special/gnome-copied-files", "x-special/gnome-copied-files", gnome));
        let text = text_set(&paths);
        set.push(("text/plain;charset=utf-8", "text/plain;charset=utf-8", text.clone()));
        set.push(("text/plain", "text/plain", text.clone()));
        set.push(("UTF8_STRING", "UTF8_STRING", text.clone()));
        set.push(("STRING", "STRING", text.clone()));
        // ICCCM: TEXT converts to whatever fits; hand back UTF-8.
        set.push(("TEXT", "UTF8_STRING", text));
    } else {
        bail!("unknown kind {kind:?}");
    }

    let (conn, screen_num) = RustConnection::connect(None).context("connect to X (DISPLAY)")?;
    let root = conn.setup().roots[screen_num].root;
    let intern = |name: &str| -> anyhow::Result<Atom> {
        Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
    };
    let a_clipboard = intern("CLIPBOARD")?;
    let a_targets = intern("TARGETS")?;
    let a_timestamp = intern("TIMESTAMP")?;
    let a_incr = intern("INCR")?;
    let a_multiple = intern("MULTIPLE")?;
    let mut entries: Vec<Entry> = Vec::new();
    for (target, ty, data) in set {
        entries.push(Entry { target: intern(target)?, ty: intern(ty)?, data });
    }
    let by_target: HashMap<Atom, usize> =
        entries.iter().enumerate().map(|(i, e)| (e.target, i)).collect();

    // Unmapped 1x1 window: only an addressable owner + a property for the
    // timestamp round trip. PropertyChange is needed for it and for INCR.
    let win: Window = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;
    conn.change_property8(PropMode::APPEND, win, AtomEnum::WM_NAME, AtomEnum::STRING, &[])?;
    conn.flush()?;
    let ts = loop {
        if let Event::PropertyNotify(e) = conn.wait_for_event()? {
            if e.window == win {
                break e.time;
            }
        }
    };

    conn.set_selection_owner(win, a_clipboard, ts)?;
    conn.flush()?;
    if conn.get_selection_owner(a_clipboard)?.reply()?.owner != win {
        bail!("did not get CLIPBOARD ownership");
    }
    // xclip uses max_request/4 per chunk; same idea, capped so one ChangeProperty
    // stays well inside any server's limit.
    let chunk = (conn.maximum_request_bytes() / 16).clamp(64 * 1024, 1 << 20);
    println!("READY");
    std::io::stdout().flush()?;

    let mut transfers: HashMap<(Window, Atom), Incr> = HashMap::new();
    loop {
        let ev = conn.wait_for_event().context("X connection lost")?;
        match ev {
            // Someone else owns the clipboard now: our job is done.
            Event::SelectionClear(e) if e.selection == a_clipboard => return Ok(()),
            Event::SelectionRequest(e) => {
                transfers.retain(|_, t| t.started.elapsed() < Duration::from_secs(60));
                let prop = if e.property == NONE { e.target } else { e.property };
                let mut answer = NONE;
                if e.selection == a_clipboard {
                    if e.target == a_targets {
                        let mut list = vec![a_targets, a_timestamp];
                        list.extend(entries.iter().map(|x| x.target));
                        conn.change_property32(
                            PropMode::REPLACE,
                            e.requestor,
                            prop,
                            AtomEnum::ATOM,
                            &list,
                        )?;
                        answer = prop;
                    } else if e.target == a_timestamp {
                        conn.change_property32(
                            PropMode::REPLACE,
                            e.requestor,
                            prop,
                            AtomEnum::INTEGER,
                            &[ts],
                        )?;
                        answer = prop;
                    } else if e.target != a_multiple {
                        if let Some(&i) = by_target.get(&e.target) {
                            answer = send_entry(&conn, &e, prop, i, &entries[i], chunk, a_incr, &mut transfers)?;
                        }
                    }
                }
                notify(&conn, &e, answer)?;
            }
            // INCR: the requestor deleting the property is its "send the next chunk" signal.
            Event::PropertyNotify(e) if e.state == Property::DELETE => {
                let key = (e.window, e.atom);
                let Some(t) = transfers.get_mut(&key) else { continue };
                if t.done_sent {
                    transfers.remove(&key);
                    continue;
                }
                let en = &entries[t.entry];
                let end = (t.off + chunk).min(en.data.len());
                conn.change_property8(PropMode::REPLACE, e.window, e.atom, en.ty, &en.data[t.off..end])?;
                conn.flush()?;
                if t.off >= en.data.len() {
                    t.done_sent = true; // that was the zero-length terminator
                }
                t.off = end;
            }
            _ => {}
        }
    }
}

/// Write one target's bytes to the requestor's property: in one go when small,
/// else start an INCR transfer. Returns the property to report in SelectionNotify.
#[allow(clippy::too_many_arguments)]
fn send_entry(
    conn: &RustConnection,
    e: &SelectionRequestEvent,
    prop: Atom,
    idx: usize,
    en: &Entry,
    chunk: usize,
    a_incr: Atom,
    transfers: &mut HashMap<(Window, Atom), Incr>,
) -> anyhow::Result<Atom> {
    if en.data.len() <= chunk {
        conn.change_property8(PropMode::REPLACE, e.requestor, prop, en.ty, &en.data)?;
        return Ok(prop);
    }
    // Select PropertyChange on the requestor BEFORE announcing INCR, or its
    // first delete can slip past us and the transfer stalls.
    conn.change_window_attributes(
        e.requestor,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;
    conn.change_property32(PropMode::REPLACE, e.requestor, prop, a_incr, &[en.data.len() as u32])?;
    transfers.insert(
        (e.requestor, prop),
        Incr { entry: idx, off: 0, done_sent: false, started: Instant::now() },
    );
    Ok(prop)
}

/// SelectionNotify to the requestor; `property == NONE` means "refused".
fn notify(conn: &RustConnection, e: &SelectionRequestEvent, property: Atom) -> anyhow::Result<()> {
    let ev = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: e.time,
        requestor: e.requestor,
        selection: e.selection,
        target: e.target,
        property,
    };
    conn.send_event(false, e.requestor, EventMask::NO_EVENT, ev)?;
    conn.flush()?;
    Ok(())
}
