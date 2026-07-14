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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use eframe::egui::Context;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

/// Watches one directory (non-recursive — TrontSnap never nests) and forwards
/// newly-created/modified image paths on an internal channel, deduped and
/// debounced. Keep this alive for as long as you want the watch active.
pub struct CaptureWatcher {
    // Kept alive only to hold the OS watch open; never read directly.
    _watcher: Option<RecommendedWatcher>,
    rx: Receiver<PathBuf>,
}

impl CaptureWatcher {
    /// Start watching `dir`. `ctx` is used to wake the UI the instant a new
    /// file is confirmed, so the gallery updates even while idling.
    pub fn start(dir: &Path, ctx: Context) -> Self {
        let (path_tx, path_rx) = crossbeam_channel::unbounded::<PathBuf>();
        let mut seen: HashMap<PathBuf, Instant> = HashMap::new();

        let handler = move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                return;
            }
            for path in &event.paths {
                if !is_image(path) {
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
                    // the gallery tries to decode a thumbnail for it.
                    std::thread::sleep(Duration::from_millis(150));
                    if tx.send(path).is_ok() {
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

    /// Drain any paths that have arrived since the last poll.
    pub fn poll(&self) -> Vec<PathBuf> {
        self.rx.try_iter().collect()
    }
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "bmp" | "webp")
    )
}
