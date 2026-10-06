// Live capture watcher: the moment a new screenshot lands in Pictures\TrontSnap,
// forward its path to the gallery — no polling, no rescan.
//
// notify's RecommendedWatcher on Windows is ReadDirectoryChangesWatcher (native,
// event-driven). Windows delivers a Create then one or more Modify events for a
// single new file in quick succession, and can fire the Create before the writer
// has finished flushing bytes — so this module de-dupes repeats of the same path
// and delays forwarding by 150ms (on a short-lived helper thread, not notify's
// event thread) to give the writer time to finish before the gallery tries to
// decode a thumbnail.
//
// Both directions are forwarded: Upsert for a file that exists, Gone for one that was
// deleted, trashed or renamed away. Dropping Gone events used to leave blank "ghost"
// tiles behind every delete (see WatchEvent).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use eframe::egui::Context;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

/// What the gallery should do about one path.
pub enum WatchEvent {
    /// New or rewritten file that existed when the debounce ended.
    Upsert(PathBuf),
    /// Deleted, trashed or renamed away.
    Gone(PathBuf),
}

/// Watches one directory (non-recursive — TrontSnap never nests) and forwards
/// created/modified/removed image paths on an internal channel, deduped and
/// debounced. Keep this alive for as long as you want the watch active.
pub struct CaptureWatcher {
    // Kept alive only to hold the OS watch open; never read directly.
    _watcher: Option<RecommendedWatcher>,
    rx: Receiver<WatchEvent>,
}

impl CaptureWatcher {
    /// Start watching `dir`. `ctx` is used to wake the UI the instant a new
    /// file is confirmed, so the gallery updates even while idling.
    pub fn start(dir: &Path, ctx: Context) -> Self {
        let (path_tx, path_rx) = crossbeam_channel::unbounded::<WatchEvent>();
        let mut seen: HashMap<PathBuf, Instant> = HashMap::new();

        let handler = move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                return;
            }
            for (idx, path) in event.paths.iter().enumerate() {
                if !is_image(path) {
                    continue;
                }
                // HARD-WON: trash::delete moves the file (IN_MOVED_FROM), which notify
                // reports as Modify(Name(From)), not Remove. Forwarding every Modify as
                // "new capture" made the gallery re-insert the vanished path as a blank
                // tile stamped now(). Name(Both) carries [from, to].
                let gone = match event.kind {
                    EventKind::Remove(_) => true,
                    EventKind::Modify(ModifyKind::Name(RenameMode::From)) => true,
                    EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => idx == 0,
                    _ => false,
                };
                if gone {
                    seen.remove(path); // a re-created file must not be swallowed by the dedupe
                    if path_tx.send(WatchEvent::Gone(path.clone())).is_ok() {
                        ctx.request_repaint();
                    }
                    continue;
                }
                let now = Instant::now();
                if let Some(t) = seen.get(path) {
                    if now.duration_since(*t) < Duration::from_millis(800) {
                        continue; // already scheduled/sent for this path recently
                    }
                }
                seen.insert(path.clone(), now);

                let tx = path_tx.clone();
                let ctx = ctx.clone();
                let path = path.clone();
                std::thread::spawn(move || {
                    // Give the writer a moment to finish flushing the PNG before
                    // the gallery tries to decode a thumbnail for it. Existence is
                    // judged AFTER the wait: a file made and deleted inside the
                    // window is Gone, never a ghost.
                    std::thread::sleep(Duration::from_millis(150));
                    let ev = if path.exists() { WatchEvent::Upsert(path) } else { WatchEvent::Gone(path) };
                    if tx.send(ev).is_ok() {
                        ctx.request_repaint();
                    }
                });
            }
        };

        let watcher = notify::recommended_watcher(handler).and_then(|mut w| {
            w.watch(dir, RecursiveMode::NonRecursive)?;
            Ok(w)
        });

        let watcher = match watcher {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("trontsnap: capture watcher unavailable: {e:#}");
                None
            }
        };

        Self { _watcher: watcher, rx: path_rx }
    }

    /// Drain any events that have arrived since the last poll, in arrival order.
    pub fn poll(&self) -> Vec<WatchEvent> {
        self.rx.try_iter().collect()
    }
}

fn is_image(path: &Path) -> bool {
    // Videos (mp4, + webm on Linux): a finished (or still-recording) capture — the
    // gallery draws videos as film tiles without decoding, so surfacing them early is safe.
    crate::index::is_video(path)
        || matches!(
            path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
            Some("png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif")
        )
}

// inotify semantics (a move-out is Name(From)); Windows' backend has its own events.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// Collect events until `want` returns true or 3 s pass.
    fn wait_for(w: &CaptureWatcher, want: impl Fn(&[String]) -> bool) -> Vec<String> {
        let mut got: Vec<String> = Vec::new();
        for _ in 0..60 {
            for ev in w.poll() {
                got.push(match ev {
                    WatchEvent::Upsert(p) => format!("up:{}", p.file_name().unwrap().to_string_lossy()),
                    WatchEvent::Gone(p) => format!("gone:{}", p.file_name().unwrap().to_string_lossy()),
                });
            }
            if want(&got) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        got
    }

    // The ghost-tile regression, against the real inotify backend: a file moved
    // out of the folder (which is how trash::delete works) must come back as Gone,
    // never as an Upsert for a path that no longer exists.
    #[test]
    fn moved_out_and_removed_files_are_gone_not_upserts() {
        let base = std::env::temp_dir().join(format!("trontsnap-wtest-{}", std::process::id()));
        let (dir, bin) = (base.join("watched"), base.join("trash"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let w = CaptureWatcher::start(&dir, Context::default());

        std::fs::write(dir.join("a.png"), b"x").unwrap();
        let got = wait_for(&w, |g| g.iter().any(|e| e == "up:a.png"));
        assert!(got.contains(&"up:a.png".to_string()), "create -> Upsert, got {got:?}");

        std::fs::rename(dir.join("a.png"), bin.join("a.png")).unwrap(); // = trash::delete
        let got = wait_for(&w, |g| g.iter().any(|e| e == "gone:a.png"));
        assert!(got.contains(&"gone:a.png".to_string()), "move-out -> Gone, got {got:?}");
        assert!(!got.contains(&"up:a.png".to_string()), "ghost Upsert for a moved file: {got:?}");

        std::fs::write(dir.join("b.png"), b"x").unwrap();
        wait_for(&w, |g| g.iter().any(|e| e == "up:b.png"));
        std::thread::sleep(Duration::from_millis(1000)); // clear the dedupe window
        std::fs::remove_file(dir.join("b.png")).unwrap();
        let got = wait_for(&w, |g| g.iter().any(|e| e == "gone:b.png"));
        assert!(got.contains(&"gone:b.png".to_string()), "rm -> Gone, got {got:?}");

        // renamed within the folder: old name Gone, new name Upsert
        std::fs::write(dir.join("c.png"), b"x").unwrap();
        wait_for(&w, |g| g.iter().any(|e| e == "up:c.png"));
        std::fs::rename(dir.join("c.png"), dir.join("d.png")).unwrap();
        let got = wait_for(&w, |g| g.iter().any(|e| e == "up:d.png") && g.iter().any(|e| e == "gone:c.png"));
        assert!(got.contains(&"gone:c.png".to_string()) && got.contains(&"up:d.png".to_string()), "rename, got {got:?}");

        drop(w);
        let _ = std::fs::remove_dir_all(&base);
    }
}
