// The history gallery: a virtualized, lazy-loading thumbnail grid over the whole
// timeline (new TrontSnap shots on top, ShareX archive scrolling in below).
//
// Only the rows the ScrollArea actually shows get laid out (show_rows), and each
// visible cell requests its thumbnail on demand — so 17k shots scroll smoothly and
// nothing is decoded until you scroll to it.

use std::collections::HashSet;
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
    // INK: source dots, labels and legend text on the panel. The verbatim pick
    // lives on `t().accent` and is free to be pure black; this is its guaranteed
    // legible form (ladder step 9), which is what marks on a ground need.
    crate::theme::accent_ink()
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
    // Stills only: run the shot through Windows OCR, text to clipboard.
    CopyText(PathBuf),
    // Stills only: open the annotation editor (its own process).
    Edit(PathBuf),
    // Multi-select: toggle one (Ctrl+click), extend from the anchor
    // (Shift+click), and the context-menu bulk ops, which resolve the CURRENT
    // visible selection inside apply() rather than snapshotting it per cell.
    ToggleSelect(PathBuf),
    RangeSelect(PathBuf),
    CopySelectedPaths,
    DeleteSelected,
}

/// Bulk deletes larger than this get a confirm dialog first. Recycle Bin makes
/// everything recoverable anyway; the dialog exists so a stray Delete over a
/// "select all" filter can't silently sweep thousands of shots.
const CONFIRM_THRESHOLD: usize = 5;

pub struct Gallery {
    shots: Vec<Shot>,
    filtered: Vec<usize>,
    thumbs: ThumbCache,
    filter: Filter,
    // Search box text (chrome row, next to the chips). Matches filename, parent
    // folder name, and the capture date; see shot_haystack. Session state only.
    query: String,
    // Multi-select, keyed by PATH rather than index so it survives rescans,
    // watcher inserts and refilters without ever pointing at the wrong shot.
    // Every op acts on the selection's intersection with the visible
    // (filtered) view; hidden-but-selected shots are never touched.
    selected: HashSet<PathBuf>,
    // Shift-click extends from the most recent Ctrl/plain selection here.
    sel_anchor: Option<PathBuf>,
    // Bulk delete parked while its confirm dialog is up.
    confirm_delete: Option<Vec<PathBuf>>,
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
            query: String::new(),
            selected: HashSet::new(),
            sel_anchor: None,
            confirm_delete: None,
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
        // Terms are ANDed: "2026-07 clip" means a July shot with clip in the
        // name. Only runs when the query/filter/scan changes, never per frame,
        // so formatting ~18k dates stays off the paint path.
        let query = self.query.to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        self.filtered = self
            .shots
            .iter()
            .enumerate()
            .filter(|(_, s)| match filter {
                Filter::All => true,
                Filter::TrontSnap => s.source == Source::TrontSnap,
                Filter::ShareX => s.source == Source::ShareX,
            })
            .filter(|(_, s)| {
                terms.is_empty() || {
                    let hay = shot_haystack(s);
                    terms.iter().all(|t| hay.contains(t))
                }
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

    /// The selection restricted to what the current filter/search shows, in
    /// timeline order. This is the set every bulk op works on.
    fn visible_selected(&self) -> Vec<PathBuf> {
        self.filtered
            .iter()
            .map(|&i| &self.shots[i])
            .filter(|s| self.selected.contains(&s.path))
            .map(|s| s.path.clone())
            .collect()
    }

    /// Bulk delete entry point: small batches go straight to the bin, large
    /// ones park in `confirm_delete` for the dialog.
    fn request_delete(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if paths.len() > CONFIRM_THRESHOLD {
            self.confirm_delete = Some(paths);
        } else {
            self.delete_many(paths);
        }
    }

    /// Recycle-Bin the batch in ONE trash op (one undoable unit, and hugely
    /// faster than per-file), then drop the shots via retain: a per-path
    /// `position` scan would be O(selection x shots), which at select-all
    /// scale is 300M+ compares.
    fn delete_many(&mut self, paths: Vec<PathBuf>) {
        let n = paths.len();
        match trash::delete_all(&paths) {
            Ok(()) => {
                let del: HashSet<&PathBuf> = paths.iter().collect();
                self.shots.retain(|s| !del.contains(&s.path));
                for p in &paths {
                    self.thumbs.forget(p);
                    self.selected.remove(p);
                }
                self.rebuild_filtered();
                self.set_status(format!("Moved {n} to Recycle Bin"));
            }
            Err(e) => {
                eprintln!("trontsnap: bulk delete failed: {e:#}");
                self.set_status("Delete failed (see log)");
            }
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
            ui.add_space(6.0);
            // Search over the whole timeline: filename, parent folder (the
            // ShareX archive buckets by "YYYY-MM"), or capture date as
            // YYYY-MM-DD. The shot count to the right is the live result count.
            let search = ui
                .add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search")
                        .desired_width(150.0),
                )
                .on_hover_text("Filename, folder, or date, like 2026-07");
            if search.changed() {
                self.rebuild_filtered();
            }
            // Ctrl+F jumps here: the muscle-memory route to a search box.
            if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                search.request_focus();
            }
            if !self.query.is_empty() {
                // Hand-drawn clear glyph, same rule as the window buttons: a
                // font "x" renders as tofu on some systems.
                let (crect, cresp) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
                if cresp.hovered() {
                    ui.painter().rect_filled(crect, 3.0, crate::theme::t().widget_hover);
                }
                let s = Stroke::new(1.2, crate::theme::t().text_muted);
                let c = crect.center();
                ui.painter().line_segment(
                    [egui::pos2(c.x - 3.5, c.y - 3.5), egui::pos2(c.x + 3.5, c.y + 3.5)],
                    s,
                );
                ui.painter().line_segment(
                    [egui::pos2(c.x - 3.5, c.y + 3.5), egui::pos2(c.x + 3.5, c.y - 3.5)],
                    s,
                );
                if cresp.on_hover_text("Clear search").clicked() {
                    self.query.clear();
                    self.rebuild_filtered();
                }
            }
            ui.separator();
            ui.label(format!("{} shots", self.filtered.len()));
            if self.scanning {
                ui.spinner();
            }
            // Visible-selection count (18k hash probes is well under a
            // millisecond; not worth caching against every mutation site).
            let sel_n = self
                .filtered
                .iter()
                .filter(|&&i| self.selected.contains(&self.shots[i].path))
                .count();
            if sel_n > 0 {
                ui.separator();
                ui.colored_label(accent(), format!("{sel_n} selected")).on_hover_text(
                    "Ctrl+click toggles, Shift+click extends, Ctrl+A selects all shown, \
                     Delete removes (Recycle Bin), Esc clears.",
                );
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

        // Selection keyboard ops, gated off text focus so Ctrl+A in the search
        // box selects text there, not 18k shots (and the Esc that drops the
        // box's focus doesn't also nuke a selection). Held while the confirm
        // dialog is up; that has its own Esc handling below.
        if !ctx.wants_keyboard_input() && self.confirm_delete.is_none() {
            let (del, all, esc) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Delete),
                    i.modifiers.command && i.key_pressed(egui::Key::A),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if all {
                for &i in &self.filtered {
                    self.selected.insert(self.shots[i].path.clone());
                }
            }
            if esc && !self.selected.is_empty() {
                self.selected.clear();
                self.sel_anchor = None;
            }
            if del {
                let vis = self.visible_selected();
                self.request_delete(vis);
            }
        }

        // The bulk-delete confirm. Center-anchored (immovable is FINE for a
        // modal; the anchor lesson is about tool panels), Esc cancels, and the
        // count stays in the button so there is no "OK to what?" moment.
        // Rendered before the grid so it survives the empty-state early return.
        if self.confirm_delete.is_some() {
            let n = self.confirm_delete.as_ref().map(Vec::len).unwrap_or(0);
            let mut decision: Option<bool> = None;
            egui::Window::new("Delete shots")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(format!("Move {n} shots to the Recycle Bin?"));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button(format!("Delete {n}")).clicked() {
                            decision = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decision = Some(false);
                        }
                    });
                });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                decision = Some(false);
            }
            match decision {
                Some(true) => {
                    if let Some(paths) = self.confirm_delete.take() {
                        self.delete_many(paths);
                    }
                }
                Some(false) => self.confirm_delete = None,
                None => {}
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

        // A search that matches nothing gets told so, not an ambiguous blank
        // grid (which otherwise looks identical to "still scanning").
        if n == 0 && !self.query.trim().is_empty() && !self.scanning {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new(format!("No shots match \"{}\"", self.query.trim()))
                        .color(crate::theme::t().text_muted),
                );
            });
            return;
        }

        // Count once per frame, not per cell: the context menu labels need it.
        let sel_count = self
            .filtered
            .iter()
            .filter(|&&i| self.selected.contains(&self.shots[i].path))
            .count();
        let shots = &self.shots;
        let filtered = &self.filtered;
        let thumbs = &mut self.thumbs;
        let selected_set = &self.selected;
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
                            let is_sel = selected_set.contains(&shot.path);
                            draw_cell(ui, shot, thumbs, is_sel, sel_count, &mut action);
                        }
                    });
                }
            });

        if let Some(action) = action {
            self.apply(action, hwnd, ctx);
        }
    }

    fn apply(&mut self, action: Action, hwnd: Option<isize>, ctx: &egui::Context) {
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
                        crate::clipboard::log_line(&format!(
                            "gallery copy failed for {}: {e:#}",
                            p.display()
                        ));
                    }
                });
                self.set_status("Copied");
            }
            Action::CopyPath(path) => {
                ctx.copy_text(path.display().to_string());
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
            Action::Edit(path) => {
                crate::editor::launch(&path);
            }
            Action::CopyText(path) => {
                // Decode + OCR off-thread (a full-res PNG decode is tens of
                // ms and RecognizeAsync more); the result comes back through
                // the same notice channel GIF export uses.
                self.set_status("Reading text...");
                let tx = self.notice_tx.clone();
                std::thread::spawn(move || {
                    let r: anyhow::Result<String> = (|| {
                        let img = image::open(&path)?.to_rgba8();
                        crate::ocr::recognize(&img)
                    })();
                    match r {
                        Ok(text) if text.trim().is_empty() => {
                            let _ = tx.send("No text found".into());
                        }
                        Ok(text) => {
                            let n = text.lines().count();
                            let _ = crate::clipboard::set_text(&text);
                            let _ = tx.send(format!("Copied {n} lines of text"));
                        }
                        Err(e) => {
                            eprintln!("trontsnap: gallery ocr failed: {e:#}");
                            crate::clipboard::log_line(&format!("gallery ocr failed: {e:#}"));
                            let _ = tx.send("OCR failed (see log)".into());
                        }
                    }
                });
            }
            Action::ToggleSelect(path) => {
                if !self.selected.insert(path.clone()) {
                    self.selected.remove(&path);
                }
                self.sel_anchor = Some(path);
            }
            Action::RangeSelect(path) => {
                // Extend from the anchor within the CURRENT view, both ends
                // resolved against filtered order. If the anchor is filtered
                // out (or there never was one), Shift+click degrades to a
                // plain select-and-anchor instead of guessing a range.
                let anchor = self.sel_anchor.clone().unwrap_or_else(|| path.clone());
                let a = self.filtered.iter().position(|&i| self.shots[i].path == anchor);
                let b = self.filtered.iter().position(|&i| self.shots[i].path == path);
                match (a, b) {
                    (Some(a), Some(b)) => {
                        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                        for &fi in &self.filtered[lo..=hi] {
                            self.selected.insert(self.shots[fi].path.clone());
                        }
                    }
                    _ => {
                        self.selected.insert(path.clone());
                        self.sel_anchor = Some(path);
                    }
                }
            }
            Action::CopySelectedPaths => {
                let vis = self.visible_selected();
                let n = vis.len();
                ctx.copy_text(
                    vis.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"),
                );
                self.set_status(format!("Copied {n} paths"));
            }
            Action::DeleteSelected => {
                let vis = self.visible_selected();
                self.request_delete(vis);
            }
        }
    }
}

fn draw_cell(
    ui: &mut egui::Ui,
    shot: &Shot,
    thumbs: &mut ThumbCache,
    selected: bool,
    sel_count: usize,
    action: &mut Option<Action>,
) {
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
                // On the badge's black scrim, not on the panel: needs the
                // overlay form or it vanishes in light mode.
                painter.circle_stroke(c, 19.0, Stroke::new(1.0, crate::theme::accent_over_media()));
                let r = 10.0;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(c.x - r * 0.55, c.y - r),
                        egui::pos2(c.x + r, c.y),
                        egui::pos2(c.x - r * 0.55, c.y + r),
                    ],
                    crate::theme::accent_over_media(),
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
                    crate::theme::accent_over_media(),
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

    // Selected: the hover treatment, held, in the live accent (a wash plus a
    // heavier ring), so a selection reads at a glance across the grid. Hover
    // still layers its glow on top.
    if selected {
        let ac = accent();
        painter.rect_filled(
            rect,
            6.0,
            Color32::from_rgba_unmultiplied(ac.r(), ac.g(), ac.b(), 26),
        );
        painter.rect_stroke(rect, 6.0, Stroke::new(2.0, ac));
    }

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
        // Plain click stays copy-to-clipboard (the flagship gesture); the
        // modifier clicks are the selection model.
        let mods = ui.input(|i| i.modifiers);
        *action = Some(if mods.command {
            Action::ToggleSelect(shot.path.clone())
        } else if mods.shift {
            Action::RangeSelect(shot.path.clone())
        } else {
            Action::Copy(shot.path.clone())
        });
    }

    resp.context_menu(|ui| {
        if ui.button("Copy to clipboard").clicked() {
            *action = Some(Action::Copy(shot.path.clone()));
            ui.close_menu();
        }
        if ui.button("Copy path").clicked() {
            *action = Some(Action::CopyPath(shot.path.clone()));
            ui.close_menu();
        }
        if ui.button("Open").clicked() {
            *action = Some(Action::Open(shot.path.clone()));
            ui.close_menu();
        }
        if !shot.is_video() && ui.button("Edit (annotate)").clicked() {
            *action = Some(Action::Edit(shot.path.clone()));
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
        if !shot.is_video() && ui.button("Copy text (OCR)").clicked() {
            *action = Some(Action::CopyText(shot.path.clone()));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Delete (Recycle Bin)").clicked() {
            *action = Some(Action::Delete(shot.path.clone()));
            ui.close_menu();
        }
        // Bulk section, only when this cell is part of a multi-selection, so
        // the everyday single-item menu stays uncluttered. Both resolve the
        // live visible selection inside apply().
        if selected && sel_count > 1 {
            ui.separator();
            if ui.button(format!("Copy {sel_count} paths")).clicked() {
                *action = Some(Action::CopySelectedPaths);
                ui.close_menu();
            }
            if ui.button(format!("Delete {sel_count} (Recycle Bin)")).clicked() {
                *action = Some(Action::DeleteSelected);
                ui.close_menu();
            }
        }
    });
}

/// The lowercase text a shot is searchable by: filename, parent folder name
/// (the ShareX archive buckets by "YYYY-MM", TrontSnap by app dir), and the
/// capture date formatted YYYY-MM-DD, so "2026-07", a full day, or a name
/// fragment all narrow the timeline. Deliberately NOT file contents or
/// dimensions: search must never open files (the Shot model is metadata-only
/// by design, and 18k opens per keystroke would be absurd).
fn shot_haystack(shot: &Shot) -> String {
    let name = shot
        .path
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let dir = shot
        .path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let dt: DateTime<Local> = shot.taken.into();
    format!("{name} {dir} {}", dt.format("%Y-%m-%d"))
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

/// Open Explorer with the file selected. Goes through ShellExecuteW for the same
/// reason the toast does: a uiAccess process cannot CreateProcess, so the old
/// `Command::new("explorer")` silently did nothing on the installed build.
#[cfg(windows)]
fn reveal(path: &Path) {
    crate::shellexec::run("explorer.exe", &format!("/select,\"{}\"", path.display()));
}

#[cfg(not(windows))]
fn reveal(_path: &Path) {}
