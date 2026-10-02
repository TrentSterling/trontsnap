// Lazy thumbnail cache — the engine behind "fast scrolling, lazy loading".
//
// The gallery only ever requests thumbnails for cells the virtualized ScrollArea
// actually renders. request() enqueues a decode job (once) and returns immediately;
// worker threads decode + downscale off the UI thread and cache a small JPG on disk
// so the next scroll-by is instant. poll() uploads finished thumbs to GPU textures.
//
// The job queue is LIFO on purpose: when you scroll fast, the cells now on screen
// were requested most recently, so decoding newest-first means the current viewport
// fills in immediately instead of waiting behind everything you scrolled past.
// A bounded LRU keeps at most TEX_CAP textures resident and evicts the rest.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

use crossbeam_channel::Receiver;
use eframe::egui;

const THUMB_PX: u32 = 256;
const TEX_CAP: usize = 800;

struct Job {
    path: PathBuf,
    mtime: u64,
}

struct Done {
    path: PathBuf,
    size: [usize; 2],
    rgba: Vec<u8>,
}

/// LIFO work stack: workers pop the most recently requested job first, so the
/// current viewport wins over stale off-screen requests.
struct JobQueue {
    state: Mutex<QueueState>,
    cv: Condvar,
}

#[derive(Default)]
struct QueueState {
    stack: Vec<Job>,
    closed: bool,
}

impl JobQueue {
    fn new() -> Self {
        Self { state: Mutex::new(QueueState::default()), cv: Condvar::new() }
    }
    fn push(&self, job: Job) {
        let mut state = self.state.lock().unwrap();
        if state.closed { return; }
        state.stack.push(job);
        self.cv.notify_one();
    }
    fn pop(&self) -> Option<Job> {
        let mut s = self.state.lock().unwrap();
        loop {
            if s.closed {
                return None;
            }
            if let Some(job) = s.stack.pop() {
                return Some(job);
            }
            s = self.cv.wait(s).unwrap();
        }
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.stack.clear();
        self.cv.notify_all();
    }
}

pub struct ThumbCache {
    queue: Arc<JobQueue>,
    done_rx: Receiver<Done>,
    textures: HashMap<PathBuf, (egui::TextureHandle, [usize; 2])>,
    lru: VecDeque<PathBuf>,
    pending: HashSet<PathBuf>,
}

impl ThumbCache {
    pub fn new(ctx: &egui::Context) -> Self {
        Self::with_cache(ctx, cache_dir())
    }

    fn with_cache(ctx: &egui::Context, cache_dir: PathBuf) -> Self {
        let queue = Arc::new(JobQueue::new());
        let (done_tx, done_rx) = crossbeam_channel::unbounded::<Done>();
        let _ = std::fs::create_dir_all(&cache_dir);

        let workers = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(2))
            .unwrap_or(4)
            .clamp(4, 12);
        for _ in 0..workers {
            let queue = queue.clone();
            let done_tx = done_tx.clone();
            let cache_dir = cache_dir.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || while let Some(job) = queue.pop() {
                let done = build_thumb(&job, &cache_dir);
                if done_tx.send(done).is_err() {
                    break;
                }
                ctx.request_repaint();
            });
        }

        Self {
            queue,
            done_rx,
            textures: HashMap::new(),
            lru: VecDeque::new(),
            pending: HashSet::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn preview(ctx: &egui::Context, paths: &[PathBuf]) -> Self {
        let (_, done_rx) = crossbeam_channel::unbounded();
        let mut cache = Self {
            queue: Arc::new(JobQueue::new()),
            done_rx,
            textures: HashMap::new(),
            lru: VecDeque::new(),
            pending: HashSet::new(),
        };
        for path in paths {
            let img = image::open(path)
                .expect("public preview image")
                .thumbnail(THUMB_PX, THUMB_PX)
                .to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let handle = ctx.load_texture(
                path.display().to_string(),
                egui::ColorImage::from_rgba_unmultiplied(size, &img),
                egui::TextureOptions::LINEAR,
            );
            cache.textures.insert(path.clone(), (handle, size));
            cache.lru.push_front(path.clone());
        }
        cache
    }

    /// Upload any finished thumbnails to textures and evict past the cap.
    /// Call once per frame before laying out the grid.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.done_rx.try_recv() {
            self.pending.remove(&done.path);
            let ci = egui::ColorImage::from_rgba_unmultiplied(done.size, &done.rgba);
            let handle = ctx.load_texture(
                done.path.to_string_lossy(),
                ci,
                egui::TextureOptions::LINEAR,
            );
            self.textures.insert(done.path.clone(), (handle, done.size));
            self.lru.push_front(done.path);
            while self.lru.len() > TEX_CAP {
                if let Some(old) = self.lru.pop_back() {
                    self.textures.remove(&old);
                }
            }
        }
    }

    /// Return the thumbnail texture for `path` if resident; otherwise enqueue it
    /// (once) and return None so the caller draws a placeholder this frame.
    pub fn request(&mut self, path: &Path, taken: SystemTime) -> Option<(egui::TextureId, [usize; 2])> {
        if let Some((handle, size)) = self.textures.get(path) {
            let id = handle.id();
            let size = *size;
            self.touch(path);
            return Some((id, size));
        }
        if !self.pending.contains(path) {
            self.pending.insert(path.to_path_buf());
            self.queue.push(Job {
                path: path.to_path_buf(),
                mtime: mtime_secs(taken),
            });
        }
        None
    }

    /// Drop a thumbnail entirely (e.g. after delete).
    pub fn forget(&mut self, path: &Path) {
        self.textures.remove(path);
        self.pending.remove(path);
        if let Some(i) = self.lru.iter().position(|p| p == path) {
            self.lru.remove(i);
        }
    }

    fn touch(&mut self, path: &Path) {
        if let Some(i) = self.lru.iter().position(|p| p == path) {
            if i != 0 {
                if let Some(p) = self.lru.remove(i) {
                    self.lru.push_front(p);
                }
            }
        }
    }
}

impl Drop for ThumbCache {
    fn drop(&mut self) {
        self.queue.close();
    }
}


fn build_thumb(job: &Job, cache_dir: &Path) -> Done {
    let cache_file = cache_dir.join(format!("{}.jpg", cache_key(&job.path, job.mtime)));

    // Fast path: a cached thumb already exists — decode the small JPG.
    if let Ok(img) = image::open(&cache_file) {
        let rgba = img.to_rgba8();
        return Done {
            path: job.path.clone(),
            size: [rgba.width() as usize, rgba.height() as usize],
            rgba: rgba.into_raw(),
        };
    }

    // Slow path: decode the original, downscale, persist the thumb for next time.
    // Videos decode their first frame via Media Foundation; stills via image::open.
    let decoded = if crate::index::is_video(&job.path) {
        crate::videothumb::first_frame(&job.path)
            .ok()
            .map(image::DynamicImage::ImageRgba8)
    } else {
        image::open(&job.path).ok()
    };
    match decoded {
        Some(full) => {
            let thumb = full.thumbnail(THUMB_PX, THUMB_PX);
            let rgb = image::DynamicImage::ImageRgb8(thumb.to_rgb8());
            let _ = rgb.save(&cache_file);
            let rgba = thumb.to_rgba8();
            Done {
                path: job.path.clone(),
                size: [rgba.width() as usize, rgba.height() as usize],
                rgba: rgba.into_raw(),
            }
        }
        // Unreadable file (or a recording still in progress): a tiny gray tile so we
        // stop retrying. Finished recordings recover because the watcher's mtime
        // refresh forgets this texture and re-requests under a new disk-cache key.
        None => Done {
            path: job.path.clone(),
            size: [2, 2],
            rgba: std::iter::repeat([40u8, 44, 52, 255]).take(4).flatten().collect(),
        },
    }
}

fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("TrontSnap")
        .join("thumbs")
}

fn cache_key(path: &Path, mtime: u64) -> String {
    let mut h = DefaultHasher::new();
    path.to_string_lossy().hash(&mut h);
    mtime.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn mtime_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_queue_wakes_blocked_workers_and_discards_pending_work() {
        let queue = Arc::new(JobQueue::new());
        let worker_queue = queue.clone();
        let worker = std::thread::spawn(move || worker_queue.pop().is_none());
        queue.close();
        assert!(worker.join().unwrap());
        queue.push(Job {
            path: "unused.png".into(),
            mtime: 0,
        });
        assert!(queue.pop().is_none());
    }

    #[test]
    fn thumbnail_completion_wakes_the_ui_without_a_polling_timer() {
        let dir = PathBuf::from("target/thumbnail-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("public-fixture.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([90, 209, 255, 255]))
            .save(&path)
            .unwrap();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            let _ = ctx.run(Default::default(), |_| {});
        }
        let (tx, rx) = crossbeam_channel::unbounded();
        ctx.set_request_repaint_callback(move |_| {
            let _ = tx.send(());
        });
        let mut cache = ThumbCache::with_cache(&ctx, dir.join("cache"));
        assert!(cache.request(&path, SystemTime::UNIX_EPOCH).is_none());
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker must wake the UI");
        cache.poll(&ctx);
        assert!(cache.request(&path, SystemTime::UNIX_EPOCH).is_some());
    }
}
