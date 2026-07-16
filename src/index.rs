// Screenshot index: walk the capture roots and build a time-sorted list.
//
// Two roots feed one continuous timeline:
//   - TrontSnap (Pictures\TrontSnap)  -> new captures, newest, shown on top
//   - ShareX archive                  -> legacy history, read-only, scrolls in below
// The scan is metadata-only (path + mtime), so even ~17k files walk in a second or two.

use std::path::PathBuf;
use std::time::SystemTime;

use walkdir::WalkDir;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    TrontSnap,
    ShareX,
}

#[derive(Clone)]
pub struct Shot {
    pub path: PathBuf,
    pub taken: SystemTime,
    pub source: Source,
}

impl Shot {
    pub fn is_video(&self) -> bool {
        is_video(&self.path)
    }
}

/// Videos ride the same timeline as stills but are never sent to the image
/// decoder — the gallery draws them as a film tile instead.
pub fn is_video(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("mp4")
    )
}

fn is_media(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif" | "mp4")
    )
}

/// Walk one root and collect its media files (stills + recordings) with
/// modified-times.
pub fn scan_root(root: &std::path::Path, source: Source) -> Vec<Shot> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_media(path) {
            continue;
        }
        let taken = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        out.push(Shot { path: path.to_path_buf(), taken, source });
    }
    out
}

/// Sort newest-first, in place.
pub fn sort_newest_first(shots: &mut [Shot]) {
    shots.sort_by(|a, b| b.taken.cmp(&a.taken));
}
