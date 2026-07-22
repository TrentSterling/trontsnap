// The history gallery: a virtualized, lazy-loading thumbnail grid over the whole
// timeline (new TrontSnap shots on top, ShareX archive scrolling in below).
//
// Only the rows the ScrollArea actually shows get laid out (show_rows), and each
// visible cell requests its thumbnail on demand — so 17k shots scroll smoothly and
// nothing is decoded until you scroll to it.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, Rect, Sense, Stroke};

use crate::capture;
use crate::index::{self, Shot, Source};
use crate::thumbs::ThumbCache;
use crate::watcher::CaptureWatcher;

const CELL: f32 = 172.0;
const GAP: f32 = 10.0;
// Source colors follow the live theme (accent = TrontSnap, amber = ShareX archive)
// so the legend dots, cell badges, and hover glow restyle with the palette.
fn accent() -> Color32 {
    crate::theme::t().accent
}
fn amber() -> Color32 {
    crate::theme::t().amber
}

#[derive(PartialEq, Clone, Copy)]
enum Filter {
    All,
    TrontSnap,
    ShareX,
}

enum Action {
    Copy(PathBuf),
    CopyPath(PathBuf),
    Open(PathBuf),
    Reveal(PathBuf),
    Delete(PathBuf),
    Drag(PathBuf),
    ExportGif(PathBuf),
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
    // Background jobs (GIF export) report completion here; drained in ui().
    notice_tx: crossbeam_channel::Sender<String>,
    notice_rx: Receiver<String>,
}

impl Gallery {
    pub fn new(ctx: &egui::Context) -> Self {
        let (notice_tx, notice_rx) = crossbeam_channel::unbounded::<String>();
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
            notice_tx,
            notice_rx,
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
    /// newest-first. If the path is already present but its file has since been
    /// rewritten (a recording that just finalized), refresh its timestamp and drop
    /// the stale thumbnail so the real one gets decoded — the thumb disk-cache key
    /// includes mtime, so the refresh naturally re-keys it.
    fn insert_shot(&mut self, path: PathBuf) {
        let taken = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| std::time::SystemTime::now());
        if let Some(i) = self.shots.iter().position(|s| s.path == path) {
            if self.shots[i].taken != taken {
                self.shots[i].taken = taken;
                self.thumbs.forget(&path);
                index::sort_newest_first(&mut self.shots);
            }
            return;
        }
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

    /// Filter chips + shot count + spinner + source legend + status message. Drawn
    /// inline in the app's single top chrome row (gallery tab only) instead of a
    /// separate header strip inside the gallery body — see `App::title_bar`.
    pub fn filter_bar_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
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
            ui.separator();
            // Legend: explains the little source dot on the corner of every
            // thumbnail (cyan = shot by TrontSnap, amber = imported ShareX archive).
            let dot = |ui: &mut egui::Ui, color: Color32, label: &str| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(11.0, 11.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
                ui.label(egui::RichText::new(label).small().color(Color32::from_gray(170)));
            };
            dot(ui, accent(), "TrontSnap");
            dot(ui, amber(), "ShareX");
            // No manual Refresh button: the live file watcher splices new captures in
            // automatically (see poll_watch). start_scan() still runs once on launch.
            // Plain inline label (not right-to-left) now that this bar shares its row
            // with the window buttons — a right-aligned child here would fight theirs.
            if let Some((msg, _)) = &self.status {
                ui.separator();
                ui.colored_label(accent(), msg.clone());
            }
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, hwnd: Option<isize>) {
        self.poll_scan();
        self.poll_watch();
        self.thumbs.poll(ctx);
        while let Ok(msg) = self.notice_rx.try_recv() {
            self.set_status(msg);
        }
        if let Some((_, n)) = &mut self.status {
            if *n == 0 {
                self.status = None;
            } else {
                *n -= 1;
            }
        }

        // Grid. Thin, right-anchored (egui's default), themed scrollbar — set it
        // locally so this is the source of truth regardless of the global style.
        let scroll_style = egui::style::ScrollStyle {
            bar_width: 8.0,
            floating: false,
            ..egui::style::ScrollStyle::solid()
        };
        ui.style_mut().spacing.scroll = scroll_style;

        // Compute columns against the width actually left over ONCE the vertical
        // scrollbar reserves its own strip (egui shrinks the ScrollArea's content
        // area by exactly `allocated_width()`) — using the pre-reservation width
        // here is what left a ragged, non-centered gap on the right before.
        let avail = (ui.available_width() - scroll_style.allocated_width()).max(CELL);
        let cols = (((avail + GAP) / (CELL + GAP)).floor() as usize).max(1);
        let content_w = cols as f32 * CELL + cols.saturating_sub(1) as f32 * GAP;
        let side_margin = ((avail - content_w) / 2.0).max(0.0);

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
                        ui.add_space(side_margin); // centers the block (equal L/R margins)
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
                    // Videos can't be pixel formats — copy the FILE (CF_HDROP), which
                    // still pastes into Discord/Explorer/terminals.
                    let r = if index::is_video(&p) {
                        crate::clipboard::set_file(&p)
                    } else {
                        capture::copy_path(&p)
                    };
                    if let Err(e) = r {
                        eprintln!("trontsnap: copy failed: {e:#}");
                    }
                });
                self.set_status("Copied");
            }
            Action::CopyPath(_path) => {
                // The actual clipboard write happens right at the menu click (needs
                // `ui.ctx()`, not available here) — this just surfaces the toast.
                self.set_status("Path copied");
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
                    self.set_status("Dragging...");
                }
            }
            Action::ExportGif(path) => {
                self.set_status("Exporting GIF...");
                let tx = self.notice_tx.clone();
                std::thread::spawn(move || match crate::gifexport::export(&path) {
                    Ok(gif) => {
                        let name = gif
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let _ = tx.send(format!("GIF saved: {name}"));
                    }
                    Err(e) => {
                        eprintln!("trontsnap: gif export failed: {e:#}");
                        let _ = tx.send("GIF export failed (see log)".into());
                    }
                });
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

    if shot.is_video() {
        // Videos: first-frame thumbnail (decoded via Media Foundation in the same
        // worker pool) with a play badge; a film plate while it's pending.
        match thumbs.request(&shot.path, shot.taken) {
            Some((id, size)) => {
                let fitted = fit(rect.shrink(4.0), size);
                let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(id, fitted, uv, Color32::WHITE);
                // Play badge over the frame.
                let c = fitted.center();
                painter.circle_filled(c, 19.0, Color32::from_black_alpha(150));
                painter.circle_stroke(c, 19.0, Stroke::new(1.0, accent()));
                let r = 10.0;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(c.x - r * 0.55, c.y - r),
                        egui::pos2(c.x + r, c.y),
                        egui::pos2(c.x - r * 0.55, c.y + r),
                    ],
                    accent(),
                    Stroke::NONE,
                ));
            }
            None => {
                let plate = rect.shrink(4.0);
                painter.rect_filled(plate, 4.0, crate::theme::t().widget_bg);
                let c = plate.center();
                let r = 22.0;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(c.x - r * 0.6, c.y - r),
                        egui::pos2(c.x + r, c.y),
                        egui::pos2(c.x - r * 0.6, c.y + r),
                    ],
                    accent(),
                    Stroke::NONE,
                ));
            }
        }
        painter.text(
            rect.shrink(4.0).left_top() + egui::vec2(6.0, 6.0),
            egui::Align2::LEFT_TOP,
            "MP4",
            egui::FontId::proportional(12.0),
            Color32::from_gray(200),
        );
    } else {
        match thumbs.request(&shot.path, shot.taken) {
            Some((id, size)) => {
                let fitted = fit(rect.shrink(4.0), size);
                let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(id, fitted, uv, Color32::WHITE);
            }
            None => {
                painter.rect_filled(rect.shrink(4.0), 4.0, crate::theme::t().widget_bg);
            }
        }
    }

    // Source dot: accent = new TrontSnap shot, amber = ShareX archive.
    let color = if shot.source == Source::TrontSnap { accent() } else { amber() };
    painter.circle_filled(rect.left_bottom() + egui::vec2(10.0, -10.0), 3.5, color);

    if resp.hovered() {
        // Subtle lift: a faint accent wash + soft outer glow under the crisp
        // inner outline, so hover reads as "raised" without shouting.
        painter.rect_filled(rect, 6.0, Color32::from_rgba_unmultiplied(90, 209, 255, 12));
        painter.rect_stroke(
            rect.expand(1.5),
            7.0,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(90, 209, 255, 70)),
        );
        painter.rect_stroke(rect, 6.0, Stroke::new(1.5, accent()));
    }

    let name = shot.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let resp = resp.on_hover_ui(|ui| hover_tooltip(ui, shot, &name));

    if resp.drag_started() {
        *action = Some(Action::Drag(shot.path.clone()));
    } else if resp.double_clicked() {
        *action = Some(Action::Open(shot.path.clone()));
    } else if resp.clicked() {
        *action = Some(Action::Copy(shot.path.clone()));
    }

    resp.context_menu(|ui| {
        if ui.button("Copy to clipboard").clicked() {
            *action = Some(Action::Copy(shot.path.clone()));
            ui.close_menu();
        }
        if ui.button("Copy path").clicked() {
            // Written straight to the OS clipboard here (needs `ui.ctx()`); the
            // Action only drives the status toast.
            ui.ctx().copy_text(shot.path.display().to_string());
            *action = Some(Action::CopyPath(shot.path.clone()));
            ui.close_menu();
        }
        if ui.button("Open").clicked() {
            *action = Some(Action::Open(shot.path.clone()));
            ui.close_menu();
        }
        if ui.button("Reveal in Explorer").clicked() {
            *action = Some(Action::Reveal(shot.path.clone()));
            ui.close_menu();
        }
        if shot.is_video() && ui.button("Export GIF").clicked() {
            *action = Some(Action::ExportGif(shot.path.clone()));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Delete (Recycle Bin)").clicked() {
            *action = Some(Action::Delete(shot.path.clone()));
            ui.close_menu();
        }
    });
}

/// Compact multi-line hover tooltip: name, full path, capture time, pixel
/// dimensions (decoded lazily, on hover only — never during the scan), file
/// size, and source. Metadata/dimension reads are best-effort; any failure
/// just omits that line rather than showing an error.
fn hover_tooltip(ui: &mut egui::Ui, shot: &Shot, name: &str) {
    // Bounded + wrapping so egui can keep the whole tooltip inside the window
    // instead of a super-wide strip that overflows past the right/bottom edge
    // when hovering a cell near it.
    ui.set_max_width(320.0);
    ui.label(egui::RichText::new(name).strong());
    ui.add(egui::Label::new(
        egui::RichText::new(shot.path.display().to_string())
            .small()
            .color(Color32::from_gray(150)),
    )
    .wrap());
    ui.add_space(3.0);

    if let Ok(meta) = std::fs::metadata(&shot.path) {
        if let Ok(modified) = meta.modified() {
            let dt: DateTime<Local> = modified.into();
            ui.label(format!("Captured: {}", dt.format("%b %-d, %Y %-I:%M %p")));
        }
        ui.label(format!("Size: {}", human_size(meta.len())));
    }
    if !shot.is_video() {
        if let Ok((w, h)) = image::image_dimensions(&shot.path) {
            ui.label(format!("Dimensions: {w} x {h} px"));
        }
    }

    let source = match shot.source {
        Source::TrontSnap => "TrontSnap",
        Source::ShareX => "ShareX",
    };
    ui.label(egui::RichText::new(format!("Source: {source}")).color(crate::theme::t().text_muted));
}

/// `1.2 MB` / `340 KB` / `812 B` style formatting — no dependency needed for
/// three branches of arithmetic.
fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
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
