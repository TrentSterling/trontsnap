// The history gallery: a virtualized, lazy-loading thumbnail grid over the whole
// timeline (new TrontSnap shots on top, ShareX archive scrolling in below).
//
// Only the rows the ScrollArea actually shows get laid out (show_rows), and each
// visible cell requests its thumbnail on demand — so 17k shots scroll smoothly and
// nothing is decoded until you scroll to it.

use std::path::{Path, PathBuf};

use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, Rect, Sense, Stroke};

use crate::capture;
use crate::index::{self, Shot, Source};
use crate::thumbs::ThumbCache;
use crate::watcher::CaptureWatcher;

const CELL: f32 = 172.0;
const GAP: f32 = 10.0;
const ACCENT: Color32 = Color32::from_rgb(90, 209, 255);
const AMBER: Color32 = Color32::from_rgb(255, 183, 77);

#[derive(PartialEq, Clone, Copy)]
enum Filter {
    All,
    TrontSnap,
    ShareX,
}

enum Action {
    Copy(PathBuf),
    Open(PathBuf),
    Reveal(PathBuf),
    Delete(PathBuf),
    Drag(PathBuf),
}

pub struct Gallery {
    shots: Vec<Shot>,
    filtered: Vec<usize>,
    thumbs: ThumbCache,
    filter: Filter,
    scan_rx: Option<Receiver<(u64, Vec<Shot>)>>,
    scan_gen: u64,
    displayed_gen: u64,
    scanning: bool,
    status: Option<(String, u32)>,
    watcher: Option<CaptureWatcher>,
}

impl Gallery {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut g = Self {
            shots: Vec::new(),
            filtered: Vec::new(),
            thumbs: ThumbCache::new(),
            filter: Filter::All,
            scan_rx: None,
            scan_gen: 0,
            displayed_gen: 0,
            scanning: false,
            status: None,
            // Live-refresh: watch Pictures\TrontSnap so new captures appear instantly.
            watcher: capture::trontsnap_dir()
                .ok()
                .map(|dir| CaptureWatcher::start(&dir, ctx.clone())),
        };
        g.start_scan();
        g
    }

    /// Kick off a fresh background index of both capture roots. The currently
    /// displayed shots stay on screen (and their thumbnails stay cached) until the
    /// new generation's data arrives, so a refresh never flashes an empty grid.
    pub fn start_scan(&mut self) {
        let (tx, rx) = crossbeam_channel::unbounded::<(u64, Vec<Shot>)>();
        self.scan_gen += 1;
        let gen = self.scan_gen;
        self.scanning = true;
        self.scan_rx = Some(rx);
        std::thread::spawn(move || {
            if let Ok(dir) = capture::trontsnap_dir() {
                if dir.exists() {
                    let _ = tx.send((gen, index::scan_root(&dir, Source::TrontSnap)));
                }
            }
            if let Some(dir) = capture::sharex_dir() {
                if dir.exists() {
                    let _ = tx.send((gen, index::scan_root(&dir, Source::ShareX)));
                }
            }
        });
    }

    fn poll_scan(&mut self) {
        let Some(rx) = self.scan_rx.clone() else { return };
        let mut changed = false;
        let mut disconnected = false;
        loop {
            match rx.try_recv() {
                Ok((gen, mut batch)) => {
                    if gen != self.scan_gen {
                        continue; // stale scan, ignore
                    }
                    if self.displayed_gen != gen {
                        self.shots.clear();
                        self.displayed_gen = gen;
                    }
                    self.shots.append(&mut batch);
                    changed = true;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if changed {
            index::sort_newest_first(&mut self.shots);
            self.rebuild_filtered();
        }
        if disconnected {
            self.scanning = false;
            self.scan_rx = None;
        }
    }

    /// Drain the live capture watcher and splice any new shots straight into the
    /// timeline (no rescan, no flash) — this is what makes a fresh capture show up
    /// in the gallery instantly.
    fn poll_watch(&mut self) {
        let Some(watcher) = &self.watcher else { return };
        let mut changed = false;
        for path in watcher.poll() {
            self.insert_shot(path);
            changed = true;
        }
        if changed {
            self.rebuild_filtered();
        }
    }

    /// Insert one freshly-captured TrontSnap shot and keep the list sorted
    /// newest-first. Skips anything already present (a Create and a Modify event
    /// can both surface the same path).
    fn insert_shot(&mut self, path: PathBuf) {
        if self.shots.iter().any(|s| s.path == path) {
            return;
        }
        let taken = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| std::time::SystemTime::now());
        self.shots.push(Shot { path, taken, source: Source::TrontSnap });
        index::sort_newest_first(&mut self.shots);
    }

    fn rebuild_filtered(&mut self) {
        let filter = self.filter;
        self.filtered = self
            .shots
            .iter()
            .enumerate()
            .filter(|(_, s)| match filter {
                Filter::All => true,
                Filter::TrontSnap => s.source == Source::TrontSnap,
                Filter::ShareX => s.source == Source::ShareX,
            })
            .map(|(i, _)| i)
            .collect();
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), 180));
    }

    fn remove_shot(&mut self, path: &Path) {
        if let Some(i) = self.shots.iter().position(|s| s.path == path) {
            self.shots.remove(i);
            self.rebuild_filtered();
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, hwnd: Option<isize>) {
        self.poll_scan();
        self.poll_watch();
        self.thumbs.poll(ctx);
        if let Some((_, n)) = &mut self.status {
            if *n == 0 {
                self.status = None;
            } else {
                *n -= 1;
            }
        }

        // Toolbar.
        ui.horizontal(|ui| {
            ui.heading("TrontSnap");
            ui.separator();
            if ui.selectable_label(self.filter == Filter::All, "All").clicked() {
                self.filter = Filter::All;
                self.rebuild_filtered();
            }
            if ui.selectable_label(self.filter == Filter::TrontSnap, "New").clicked() {
                self.filter = Filter::TrontSnap;
                self.rebuild_filtered();
            }
            if ui.selectable_label(self.filter == Filter::ShareX, "ShareX").clicked() {
                self.filter = Filter::ShareX;
                self.rebuild_filtered();
            }
            ui.separator();
            ui.label(format!("{} shots", self.filtered.len()));
            if self.scanning {
                ui.spinner();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⟳ Refresh").clicked() {
                    self.start_scan();
                }
                if let Some((msg, _)) = &self.status {
                    ui.colored_label(ACCENT, msg.clone());
                }
            });
        });
        ui.separator();

        // Grid.
        let cols = (((ui.available_width() + GAP) / (CELL + GAP)).floor() as usize).max(1);
        let n = self.filtered.len();
        let rows = n.div_ceil(cols);
        let row_h = CELL + GAP;

        let shots = &self.shots;
        let filtered = &self.filtered;
        let thumbs = &mut self.thumbs;
        let mut action: Option<Action> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show_rows(ui, row_h, rows, |ui, range| {
                ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
                for row in range {
                    ui.horizontal(|ui| {
                        for c in 0..cols {
                            let fi = row * cols + c;
                            if fi >= n {
                                break;
                            }
                            let shot = &shots[filtered[fi]];
                            draw_cell(ui, shot, thumbs, &mut action);
                        }
                    });
                }
            });

        if let Some(action) = action {
            self.apply(action, hwnd);
        }
    }

    fn apply(&mut self, action: Action, hwnd: Option<isize>) {
        match action {
            Action::Copy(path) => {
                let p = path.clone();
                std::thread::spawn(move || {
                    let _ = capture::copy_path(&p);
                });
                self.set_status("Copied ✓");
            }
            Action::Open(path) => {
                let _ = opener::open(&path);
            }
            Action::Reveal(path) => reveal(&path),
            Action::Delete(path) => {
                if trash::delete(&path).is_ok() {
                    self.remove_shot(&path);
                    self.thumbs.forget(&path);
                    self.set_status("Moved to Recycle Bin");
                }
            }
            Action::Drag(path) => {
                if crate::app::start_file_drag(hwnd, &path) {
                    self.set_status("Dragging…");
                }
            }
        }
    }
}

fn draw_cell(ui: &mut egui::Ui, shot: &Shot, thumbs: &mut ThumbCache, action: &mut Option<Action>) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(CELL, CELL), Sense::click_and_drag());
    let painter = ui.painter();
    // Raised card: theme fill + a hairline border so tiles read as surfaces.
    painter.rect_filled(rect, 6.0, crate::theme::card_bg());
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0, crate::theme::stroke()));

    match thumbs.request(&shot.path, shot.taken) {
        Some((id, size)) => {
            let fitted = fit(rect.shrink(4.0), size);
            let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(id, fitted, uv, Color32::WHITE);
        }
        None => {
            painter.rect_filled(rect.shrink(4.0), 4.0, crate::theme::T.widget_bg);
        }
    }

    // Source dot: accent = new TrontSnap shot, amber = ShareX archive.
    let color = if shot.source == Source::TrontSnap { ACCENT } else { AMBER };
    painter.circle_filled(rect.left_bottom() + egui::vec2(10.0, -10.0), 3.5, color);

    if resp.hovered() {
        painter.rect_stroke(rect, 6.0, Stroke::new(1.5, ACCENT));
    }

    let name = shot.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let resp = resp.on_hover_text(&name);

    if resp.drag_started() {
        *action = Some(Action::Drag(shot.path.clone()));
    } else if resp.double_clicked() {
        *action = Some(Action::Open(shot.path.clone()));
    } else if resp.clicked() {
        *action = Some(Action::Copy(shot.path.clone()));
    }

    resp.context_menu(|ui| {
        if ui.button("📋 Copy to clipboard").clicked() {
            *action = Some(Action::Copy(shot.path.clone()));
            ui.close_menu();
        }
        if ui.button("🔍 Open").clicked() {
            *action = Some(Action::Open(shot.path.clone()));
            ui.close_menu();
        }
        if ui.button("📁 Reveal in Explorer").clicked() {
            *action = Some(Action::Reveal(shot.path.clone()));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("🗑 Delete (Recycle Bin)").clicked() {
            *action = Some(Action::Delete(shot.path.clone()));
            ui.close_menu();
        }
    });
}

fn fit(rect: Rect, size: [usize; 2]) -> Rect {
    let (tw, th) = (size[0] as f32, size[1] as f32);
    if tw <= 0.0 || th <= 0.0 {
        return rect;
    }
    let s = (rect.width() / tw).min(rect.height() / th);
    Rect::from_center_size(rect.center(), egui::vec2(tw * s, th * s))
}

#[cfg(windows)]
fn reveal(path: &Path) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn();
}

#[cfg(not(windows))]
fn reveal(_path: &Path) {}
