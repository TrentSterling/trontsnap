// Instant replay for the cam frame: while it's on, the frame area is recorded
// continuously in short MP4 segments under %TEMP%\TrontSnap\replay, and only
// enough of them to cover the last N seconds are kept. "Save replay" ends the
// current segment early, stream-copies the tail into one clip (vidconv::concat_tail)
// and hands it to the cam's save handler, which converts it to the chosen format.
//
// Segments on disk instead of frames in RAM: 30 s of raw 1080p is ~7 GB, while
// 30 s of hardware H.264 is a few MB. The cost is a ~0.1 s seam between segments
// while the duplication restarts, which is fine for "save what just happened".

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::RECT;

const SEGMENT: Duration = Duration::from_secs(5);

static ON: AtomicBool = AtomicBool::new(false);
static SAVE: AtomicBool = AtomicBool::new(false);
static KEEP_S: AtomicU32 = AtomicU32::new(30);
/// (record rect, fps, audio), read at the start of every segment.
static TARGET: Mutex<Option<(RECT, u32, bool)>> = Mutex::new(None);
static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
type Handler = Box<dyn Fn(anyhow::Result<PathBuf>) + Send + Sync>;
static ON_SAVE: OnceLock<Handler> = OnceLock::new();

/// Called with every saved replay (a temp MP4 the handler owns), or the error.
pub fn set_handler(f: impl Fn(anyhow::Result<PathBuf>) + Send + Sync + 'static) {
    let _ = ON_SAVE.set(Box::new(f));
}

pub fn set_target(rect: RECT, fps: u32, audio: bool) {
    if let Ok(mut g) = TARGET.lock() {
        *g = Some((rect, fps, audio));
    }
}

pub fn keep_secs() -> u32 {
    KEEP_S.load(Ordering::SeqCst)
}

pub fn set_keep_secs(s: u32) {
    KEEP_S.store(s.clamp(5, 300), Ordering::SeqCst);
}

pub fn is_on() -> bool {
    ON.load(Ordering::SeqCst)
}

pub fn start() {
    if ON.swap(true, Ordering::SeqCst) {
        return;
    }
    let h = std::thread::spawn(buffer_loop);
    if let Ok(mut g) = THREAD.lock() {
        *g = Some(h);
    }
}

/// Stop buffering and wait for the current segment to finish (well under a second).
pub fn stop() {
    if !ON.swap(false, Ordering::SeqCst) {
        return;
    }
    crate::recorder::request_stop();
    if let Some(h) = THREAD.lock().ok().and_then(|mut g| g.take()) {
        let _ = h.join();
    }
}

/// Save the last N seconds. No-op unless buffering.
pub fn request_save() {
    if is_on() {
        SAVE.store(true, Ordering::SeqCst);
        crate::recorder::request_stop(); // end the current segment now
    }
}

fn buffer_loop() {
    let dir = std::env::temp_dir().join("TrontSnap").join("replay");
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("trontsnap: replay dir: {e}");
        ON.store(false, Ordering::SeqCst);
        return;
    }
    // (file, seconds, size)
    let mut segs: VecDeque<(PathBuf, f64, (i32, i32))> = VecDeque::new();
    let mut n = 0u64;
    while ON.load(Ordering::SeqCst) {
        let Some((rect, fps, audio)) = TARGET.lock().ok().and_then(|g| *g) else {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        let path = dir.join(format!("seg_{n:06}.mp4"));
        n += 1;
        match crate::recorder::record_rect_for(rect, fps, audio, path.clone(), Some(SEGMENT)) {
            Ok(p) => {
                let wh = (rect.right - rect.left, rect.bottom - rect.top);
                segs.push_back((p, crate::recorder::last_secs(), wh));
            }
            Err(e) => {
                eprintln!("trontsnap: replay segment failed: {e:#}");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        // Keep just enough segments to cover the window.
        let keep = keep_secs() as f64;
        while segs.len() > 1 && segs.iter().skip(1).map(|s| s.1).sum::<f64>() >= keep {
            if let Some((old, _, _)) = segs.pop_front() {
                let _ = std::fs::remove_file(old);
            }
        }
        if SAVE.swap(false, Ordering::SeqCst) {
            // Only the trailing run recorded at the current size (resizing the frame
            // mid-buffer starts a new run; mixed sizes can't be stream-copied).
            let res = match segs.back() {
                None => Err(anyhow::anyhow!("nothing buffered yet")),
                Some(&(_, _, wh)) => {
                    let run: Vec<_> = segs.iter().rev().take_while(|s| s.2 == wh).collect();
                    let total: f64 = run.iter().map(|s| s.1).sum();
                    let files: Vec<PathBuf> = run.iter().rev().map(|s| s.0.clone()).collect();
                    crate::vidconv::concat_tail(&files, total, keep)
                }
            };
            if let Some(h) = ON_SAVE.get() {
                h(res);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `trontsnap replay-test <secs-buffered> <keep>`: buffer the monitor under the mouse,
/// save, print the clip. Headless check of the whole replay path.
pub fn replay_test(buffer_s: u64, keep: u32) -> anyhow::Result<()> {
    let m = crate::capture::cursor_monitor()?;
    let rect = RECT { left: m.x, top: m.y, right: m.x + 640, bottom: m.y + 360 };
    let (tx, rx) = crossbeam_channel::bounded(1);
    set_handler(move |r| {
        let _ = tx.send(r.map_err(|e| e.to_string()));
    });
    set_keep_secs(keep);
    set_target(rect, 30, false);
    start();
    std::thread::sleep(Duration::from_secs(buffer_s));
    request_save();
    let res = rx.recv_timeout(Duration::from_secs(30))?;
    stop();
    match res {
        Ok(p) => {
            println!("{}", p.display());
            Ok(())
        }
        Err(e) => anyhow::bail!(e),
    }
}
