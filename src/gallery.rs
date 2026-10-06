// The history gallery: a virtualized, lazy-loading thumbnail grid over the whole
// timeline (new TrontSnap shots on top, ShareX archive scrolling in below).
//
// Only the rows the ScrollArea actually shows get laid out (show_rows), and each
// visible cell requests its thumbnail on demand — so 17k shots scroll smoothly and
// nothing is decoded until you scroll to it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, Rect, Sense, Stroke};

use crate::capture;
use crate::index::{self, Shot, Source};
use crate::thumbs::ThumbCache;
use crate::watcher::{CaptureWatcher, WatchEvent};

// OS wording: the same code path says "Recycle Bin" on Windows and "Trash" on Linux.
const TRASH: &str = if cfg!(windows) { "Recycle Bin" } else { "Trash" };
const REVEAL: &str = if cfg!(windows) { "Reveal in Explorer" } else { "Show in file manager" };
// Shown on the shot count so the selection gestures are discoverable BEFORE a first selection.
const SELECT_HINT: &str = "Click copies. Ctrl+click toggles a selection, Shift+click selects a range, \
     Ctrl+A selects all shown, Ctrl+C copies the selection, Delete trashes it, Esc clears. \
     While something is selected, a plain click toggles instead of copying.";

/// Width of the title bar's right-hand side that filter_bar_ui must leave alone: the caption
/// buttons (app.rs CAPTION_W) plus the theme swatch and its gaps.
const CHROME_RIGHT_RESERVE: f32 = 172.0;
/// Room the two bare legend dots need.
const LEGEND_DOTS_W: f32 = 36.0;
/// Both legend entries with their words; below this much room they collapse to dots.
const LEGEND_FULL_W: f32 = 150.0;

/// How long a status message stays up.
const STATUS_TTL: Duration = Duration::from_millis(4000);
/// Trash this many files per `trash::delete_all` call: progress is reported between chunks and a
/// bad file only costs its own chunk a per-file retry.
const TRASH_CHUNK: usize = 200;

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
    // `additive` = Ctrl+Shift+click: add the range to the selection instead of replacing it.
    RangeSelect(PathBuf, bool),
    CopySelectedPaths,
    // Multi-select as FILES (uri-list + gnome-copied-files), Linux only: the
    // Windows clipboard writer takes one path at a time.
    #[cfg(not(windows))]
    CopySelectedFiles,
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
    // Message + wall-clock deadline (a frame countdown never expired on an idle
    // window, and ran out in 3 s while scrolling).
    status: Option<(String, Instant)>,
    watcher: Option<CaptureWatcher>,
    // Background jobs (GIF export, reveal, ...) report completion here; drained in ui().
    notice_tx: crossbeam_channel::Sender<String>,
    notice_rx: Receiver<String>,
    // Wakes the UI when a worker posts a notice (egui only repaints on events).
    ctx: egui::Context,
    // Bulk trash runs on a worker; it reports back here. `deleting` = one batch at a time.
    del_tx: crossbeam_channel::Sender<DeleteMsg>,
    del_rx: Receiver<DeleteMsg>,
    deleting: bool,
    // Deletes asked for while a batch is still moving: run when it finishes, in order.
    delete_queue: Vec<PathBuf>,
    // Clipboard writes run on ONE worker, in click order. HARD-WON: each copy used to get its own thread,
    // and the owner that claimed CLIPBOARD last won, which is not the click that came last.
    copy_tx: crossbeam_channel::Sender<CopyJob>,
    // moved / failed summed over a chain of queued batches, reported once at the end.
    delete_tally: (usize, usize),
    // The search box held keyboard focus at the start of this frame (set by
    // filter_bar_ui, which runs BEFORE ui()): the Esc that drops its focus must not
    // also clear the selection.
    search_active: bool,
    // The ShareX archive root exists (always true on Windows). Hides the chip/legend on
    // machines that never had ShareX.
    has_sharex: bool,
    // TRONTSNAP_TEST=1 (the harness guard): log a line whenever the counts/status change,
    // so tests assert exact state instead of guessing from pixels. `last_trace` dedupes.
    trace: bool,
    last_trace: String,
}

type CopyJob = Box<dyn FnOnce() + Send>;

/// Worker -> UI messages for a bulk trash.
enum DeleteMsg {
    /// These paths left the folder (trashed, or already gone): drop them from the view.
    Removed { paths: Vec<PathBuf>, done: usize, total: usize },
    /// Worker finished.
    Done { moved: usize, failed: usize },
}

impl Gallery {
    pub fn new(ctx: &egui::Context) -> Self {
        let (notice_tx, notice_rx) = crossbeam_channel::unbounded::<String>();
        let (del_tx, del_rx) = crossbeam_channel::unbounded::<DeleteMsg>();
        let (copy_tx, copy_rx) = crossbeam_channel::unbounded::<CopyJob>();
        std::thread::Builder::new()
            .name("clipboard-copy".into())
            .spawn(move || {
                for job in copy_rx {
                    job();
                }
            })
            .ok();
        let mut g = Self {
            shots: Vec::new(),
            filtered: Vec::new(),
            thumbs: ThumbCache::new(ctx),
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
            ctx: ctx.clone(),
            del_tx,
            del_rx,
            deleting: false,
            delete_queue: Vec::new(),
            copy_tx,
            delete_tally: (0, 0),
            search_active: false,
            has_sharex: sharex_present(),
            trace: std::env::var_os("TRONTSNAP_TEST").is_some(),
            last_trace: String::new(),
        };
        g.start_scan(ctx);
        g
    }

    /// Kick off a fresh background index of both capture roots. The currently
    /// displayed shots stay on screen (and their thumbnails stay cached) until the
    /// new generation's data arrives, so a refresh never flashes an empty grid.
    pub fn start_scan(&mut self, ctx: &egui::Context) {
        let (tx, rx) = crossbeam_channel::unbounded::<(u64, Vec<Shot>)>();
        self.scan_gen += 1;
        let gen = self.scan_gen;
        self.scanning = true;
        self.scan_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Ok(dir) = capture::trontsnap_dir() {
                if dir.exists() {
                    let _ = tx.send((gen, index::scan_root(&dir, Source::TrontSnap)));
                    ctx.request_repaint();
                }
            }
            if let Some(dir) = capture::sharex_dir() {
                if dir.exists() {
                    let _ = tx.send((gen, index::scan_root(&dir, Source::ShareX)));
                }
            }
            drop(tx);
            ctx.request_repaint();
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

    /// Drain the live capture watcher and splice new shots straight into the
    /// timeline (no rescan, no flash) — this is what makes a fresh capture show up
    /// in the gallery instantly. Deletions arrive the same way, so a file trashed
    /// or removed outside the app (or by our own delete) leaves the grid too.
    fn poll_watch(&mut self) {
        let Some(watcher) = &self.watcher else { return };
        let events = watcher.poll();
        if events.is_empty() {
            return;
        }
        for ev in events {
            match ev {
                WatchEvent::Upsert(path) => self.insert_shot(path),
                WatchEvent::Gone(path) => {
                    self.drop_path(&path);
                }
            }
        }
        self.rebuild_filtered();
    }

    /// Insert one freshly-captured TrontSnap shot and keep the list sorted
    /// newest-first. If the path is already present but its file has since been
    /// rewritten (a recording that just finalized), refresh its timestamp and drop
    /// the stale thumbnail so the real one gets decoded — the thumb disk-cache key
    /// includes mtime, so the refresh naturally re-keys it.
    fn insert_shot(&mut self, path: PathBuf) {
        // HARD-WON: a vanished file must NOT be inserted (this used to fall back to
        // now() and mint a blank "ghost" tile that broke Ctrl+A+Delete: trash::delete_all
        // aborts on the first missing path).
        let Ok(taken) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            self.drop_path(&path);
            return;
        };
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

    /// Forget one path everywhere (shot list, thumbnail, selection, shift-click
    /// anchor). Idempotent, so the app's own delete and the watcher's Gone for the
    /// same file can both land. Does NOT rebuild `filtered`; callers batch that.
    fn drop_path(&mut self, path: &Path) -> bool {
        self.thumbs.forget(path);
        self.selected.remove(path);
        if self.sel_anchor.as_deref() == Some(path) {
            self.sel_anchor = None;
        }
        match self.shots.iter().position(|s| s.path == path) {
            Some(i) => {
                self.shots.remove(i);
                true
            }
            None => false,
        }
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

    /// Harness hook (see `trace`): `shots` = whole timeline, `shown` = after filter/search,
    /// `selected` = the whole selection (hidden members included).
    fn trace_state(&mut self) {
        if !self.trace {
            return;
        }
        let status = self.status.as_ref().map_or("", |(m, _)| m.as_str());
        let line = format!(
            "trontsnap-test: gallery shots={} shown={} selected={} status={status:?}",
            self.shots.len(),
            self.filtered.len(),
            self.selected.len()
        );
        if line != self.last_trace {
            eprintln!("{line}");
            self.last_trace = line;
        }
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now() + STATUS_TTL));
    }

    /// Worker-thread handle: posts a status message and wakes the UI (egui only
    /// repaints on events, so a bare send would sit unread until the mouse moved).
    fn notifier(&self) -> impl Fn(String) + Send + 'static {
        let (tx, ctx) = (self.notice_tx.clone(), self.ctx.clone());
        move |msg| {
            let _ = tx.send(msg);
            ctx.request_repaint();
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

    /// Trash the batch on a worker thread (about 3.4 ms per file here, so a
    /// select-all of the archive would otherwise freeze the window for a minute).
    /// The UI drops each chunk's shots as they leave the folder (`poll_delete`),
    /// never on a bare `Ok`: what counts is what is actually gone from disk.
    fn delete_many(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.deleting {
            // Queued, not refused: a second Delete press during a big batch used to be dropped silently
            // apart from a status line.
            let n = paths.len();
            self.delete_queue.extend(paths);
            self.set_status(format!("Still moving the last batch to the {TRASH}; {n} more queued"));
            return;
        }
        self.deleting = true;
        self.set_status(format!("Moving {} to the {TRASH}...", paths.len()));
        let (tx, ctx) = (self.del_tx.clone(), self.ctx.clone());
        // Harness knob (TRONTSNAP_TEST instances only): a ms count in <cache>/trontsnap/test_trash_ms stretches
        // every chunk, so a test can land a second Delete while a batch is provably still moving (real
        // trashing is ~1 ms a file). A file, not an env var: the test turns it on for one step only.
        let slow = std::env::var_os("TRONTSNAP_TEST")
            .and_then(|_| dirs::cache_dir())
            .and_then(|d| std::fs::read_to_string(d.join("trontsnap").join("test_trash_ms")).ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(std::time::Duration::from_millis);
        std::thread::spawn(move || {
            let total = paths.len();
            let (mut moved, mut failed, mut done) = (0usize, 0usize, 0usize);
            for chunk in paths.chunks(TRASH_CHUNK) {
                // Already gone (deleted behind our back) = nothing to trash, but it
                // must still leave the view.
                let (live, vanished): (Vec<&PathBuf>, Vec<&PathBuf>) =
                    chunk.iter().partition(|p| p.exists());
                let mut removed: Vec<PathBuf> = vanished.into_iter().cloned().collect();
                if !live.is_empty() {
                    if let Err(e) = trash::delete_all(live.iter().copied()) {
                        // HARD-WON: delete_all stops at the first bad path with the
                        // earlier ones already moved, so retry per file and judge each
                        // by whether it is still on disk.
                        eprintln!("trontsnap: bulk delete chunk failed, retrying per file: {e:#}");
                        for p in live {
                            let r = trash::delete(p);
                            if !p.exists() {
                                moved += 1;
                                removed.push(p.clone());
                            } else {
                                failed += 1;
                                eprintln!(
                                    "trontsnap: delete failed for {}: {:#}",
                                    p.display(),
                                    r.err().map(|e| e.to_string()).unwrap_or_default()
                                );
                            }
                        }
                    } else {
                        moved += live.len();
                        removed.extend(live.into_iter().cloned());
                    }
                }
                done += chunk.len();
                let _ = tx.send(DeleteMsg::Removed { paths: removed, done, total });
                ctx.request_repaint();
                if let Some(d) = slow {
                    std::thread::sleep(d);
                }
            }
            let _ = tx.send(DeleteMsg::Done { moved, failed });
            ctx.request_repaint();
        });
    }

    /// Apply the delete worker's progress to the view.
    fn poll_delete(&mut self) {
        let mut changed = false;
        while let Ok(msg) = self.del_rx.try_recv() {
            match msg {
                DeleteMsg::Removed { paths, done, total } => {
                    let gone: HashSet<&PathBuf> = paths.iter().collect();
                    // retain, not a per-path scan: select-all is 300M+ compares otherwise
                    self.shots.retain(|s| !gone.contains(&s.path));
                    for p in &paths {
                        self.thumbs.forget(p);
                        self.selected.remove(p);
                    }
                    if self.sel_anchor.as_ref().is_some_and(|a| gone.contains(a)) {
                        self.sel_anchor = None;
                    }
                    changed = true;
                    if done < total {
                        self.set_status(format!("Moving to the {TRASH}: {done}/{total}"));
                    }
                }
                DeleteMsg::Done { moved, failed } => {
                    self.deleting = false;
                    self.delete_tally = (self.delete_tally.0 + moved, self.delete_tally.1 + failed);
                    if !self.delete_queue.is_empty() {
                        // Next batch right away (it sets its own progress status); what the queued
                        // paths overlap with this batch is already gone and drops out as "vanished".
                        let next = std::mem::take(&mut self.delete_queue);
                        self.delete_many(next);
                        continue;
                    }
                    let (moved, failed) = std::mem::take(&mut self.delete_tally);
                    if failed > 0 {
                        self.set_status(format!("Moved {moved} to the {TRASH}, {failed} failed (see log)"));
                    } else if moved == 0 {
                        self.set_status("Those files were already gone");
                    } else {
                        self.set_status(format!("Moved {moved} to the {TRASH}"));
                    }
                }
            }
        }
        if changed {
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
            if self.has_sharex && ui.selectable_label(self.filter == Filter::ShareX, "ShareX").clicked() {
                self.filter = Filter::ShareX;
                self.rebuild_filtered();
            }
            ui.add_space(6.0);
            // Search over the whole timeline: filename, parent folder (the
            // ShareX archive buckets by "YYYY-MM"), or capture date as
            // YYYY-MM-DD. The shot count to the right is the live result count.
            // Focus is sampled BEFORE the TextEdit runs: Esc makes it surrender focus
            // inside this very call, so by the time ui() runs the key would look
            // unclaimed and clear the selection too.
            let search_id = ui.make_persistent_id("gallery_search");
            let was_focused = ui.memory(|m| m.has_focus(search_id));
            let search = ui
                .add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(search_id)
                        .hint_text("Search")
                        .desired_width(150.0),
                )
                .on_hover_text("Filename, folder, or date, like 2026-07");
            self.search_active = was_focused || search.has_focus() || search.lost_focus();
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
            ui.label(format!("{} shots", self.filtered.len())).on_hover_text(SELECT_HINT);
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
            if !self.selected.is_empty() {
                ui.separator();
                // Selections hidden by a filter/search stay selected; say so instead of
                // letting them reappear unannounced when the filter clears.
                let hidden = self.selected.len().saturating_sub(sel_n);
                let text = match (sel_n, hidden) {
                    (_, 0) => format!("{sel_n} selected"),
                    (0, h) => format!("+{h} hidden"), // short: this row has no room to spare
                    (n, h) => format!("{n} selected +{h} hidden"),
                };
                ui.colored_label(accent(), text).on_hover_text(SELECT_HINT);
            }
            ui.separator();
            // Legend: explains the little source dot on the corner of every
            // thumbnail (cyan = shot by TrontSnap, amber = imported ShareX archive).
            // Dots only when the words do not fit: at the default 1180 px window the
            // legend text slid under the theme swatch (and always does once a selection
            // adds its own label), so the room left of the swatch decides.
            let legend_w = if self.has_sharex { LEGEND_FULL_W } else { LEGEND_FULL_W / 2.0 };
            let compact = ui.available_width() - CHROME_RIGHT_RESERVE < legend_w;
            let dot = |ui: &mut egui::Ui, color: Color32, label: &str| {
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(11.0, 11.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
                if compact {
                    resp.on_hover_text(label);
                } else {
                    ui.label(egui::RichText::new(label).small().color(Color32::from_gray(170)));
                }
            };
            // Not even the dots when the row is that tight (three-digit counts plus a
            // selection): the second one would slide under the swatch.
            if ui.available_width() - CHROME_RIGHT_RESERVE >= LEGEND_DOTS_W {
                dot(ui, accent(), "TrontSnap");
                if self.has_sharex {
                    dot(ui, amber(), "ShareX");
                }
            }
            // The status message is NOT drawn here: the row has no room left of the
            // window buttons at the default width (it showed as "Copied 8 p" under
            // them). See status_ui().
        });
    }

    /// Transient feedback ("Copied", "Moved 3 to the Trash", errors) as a pill
    /// anchored bottom-centre of the window, clear of the crowded chrome row. Expires
    /// by wall clock: schedules its own repaint so an idle window clears it too.
    fn status_ui(&mut self, ctx: &egui::Context) {
        let Some((msg, until)) = &self.status else { return };
        let now = Instant::now();
        if now >= *until {
            self.status = None;
            return;
        }
        ctx.request_repaint_after(*until - now);
        let t = crate::theme::t();
        egui::Area::new(egui::Id::new("gallery_status"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -18.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(t.widget_bg)
                    .stroke(Stroke::new(1.0, accent()))
                    .rounding(8.0)
                    .inner_margin(egui::Margin::symmetric(14.0, 7.0))
                    .show(ui, |ui| {
                        // extend(): an Area sizes itself from the PREVIOUS message, so a
                        // longer one wrapped into a two-line pill.
                        ui.add(egui::Label::new(egui::RichText::new(msg.as_str()).color(accent())).extend());
                    });
            });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, hwnd: Option<isize>) {
        self.poll_scan();
        self.poll_watch();
        self.poll_delete();
        self.thumbs.poll(ctx);
        while let Ok(msg) = self.notice_rx.try_recv() {
            self.set_status(msg);
        }
        self.status_ui(ctx);

        // Selection keyboard ops, gated off text focus so Ctrl+A in the search
        // box selects text there, not 18k shots (and the Esc that drops the
        // box's focus doesn't also nuke a selection). Held while the confirm
        // dialog is up; that has its own Esc handling below.
        // HARD-WON: wants_keyboard_input() alone is not enough for Esc: the search
        // box runs earlier in the frame (title bar), surrenders its focus on Esc, and
        // this gate then sees no text focus. search_active remembers it.
        if !ctx.wants_keyboard_input() && !self.search_active && self.confirm_delete.is_none() {
            // HARD-WON: egui-winit turns Ctrl+C into Event::Copy and returns BEFORE
            // pushing the Key event, so key_pressed(Key::C) is never true. The
            // wants_keyboard_input gate above keeps Ctrl+C inside the search box.
            let copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
            let (del, all, esc) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Delete),
                    i.modifiers.command && i.key_pressed(egui::Key::A),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if copy {
                let mut vis = self.visible_selected();
                match vis.len() {
                    0 => {}
                    1 => self.apply(Action::Copy(vis.remove(0)), hwnd, ctx),
                    #[cfg(not(windows))]
                    _ => self.apply(Action::CopySelectedFiles, hwnd, ctx),
                    #[cfg(windows)]
                    _ => self.apply(Action::CopySelectedPaths, hwnd, ctx),
                }
            }
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

        self.trace_state();

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
                    ui.label(format!("Move {n} shots to the {TRASH}?"));
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

    /// Queue a clipboard write behind the earlier ones (FIFO, one worker).
    fn copy_job(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.copy_tx.send(Box::new(job));
    }

    /// Text -> system clipboard. HARD-WON: on Linux egui's copy_text ties the
    /// selection to the app WINDOW (X11 owner / Wayland serial), so the text
    /// vanished when the app closed (Close quits there) and could be refused
    /// without a fresh input serial. The daemon-owned path (the one OCR uses)
    /// outlives the app. Windows keeps egui's, which is a real OS clipboard.
    fn copy_text(&self, ctx: &egui::Context, text: String) {
        #[cfg(windows)]
        ctx.copy_text(text);
        #[cfg(not(windows))]
        {
            let _ = ctx;
            let notify = self.notifier();
            self.copy_job(move || {
                if let Err(e) = crate::clipboard::set_text(&text) {
                    eprintln!("trontsnap: copy text failed: {e:#}");
                    notify("Copy failed (see log)".into());
                }
            });
        }
    }

    fn apply(&mut self, action: Action, hwnd: Option<isize>, ctx: &egui::Context) {
        match action {
            Action::Copy(path) => {
                let p = path.clone();
                let notify = self.notifier();
                self.copy_job(move || {
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
                        notify("Copy failed (see log)".into());
                    }
                });
                self.set_status("Copied");
            }
            Action::CopyPath(path) => {
                self.copy_text(ctx, path.display().to_string());
                self.set_status("Path copied");
            }
            Action::Open(path) => {
                if !open_path(&path) {
                    eprintln!("trontsnap: open failed for {}", path.display());
                    self.set_status("Open failed (see log)");
                }
            }
            Action::Reveal(path) => {
                if !path.exists() {
                    // Stale tile (deleted behind our back): say so, and sweep it.
                    self.drop_path(&path);
                    self.rebuild_filtered();
                    self.set_status("That file no longer exists");
                } else {
                    reveal(&path, self.notifier());
                    #[cfg(not(windows))]
                    self.set_status("Opening the file manager...");
                }
            }
            // Same worker as the bulk delete: errors surface, the selection is cleaned,
            // and the tile leaves only once the file is really gone.
            Action::Delete(path) => self.delete_many(vec![path]),
            Action::Drag(path) => {
                if crate::app::start_file_drag(hwnd, &path) {
                    self.set_status("Dragging...");
                }
            }
            Action::ExportGif(path) => {
                self.set_status("Exporting GIF...");
                let notify = self.notifier();
                std::thread::spawn(move || match crate::gifexport::export(&path) {
                    Ok(gif) => {
                        let name = gif
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        notify(format!("GIF saved: {name}"));
                    }
                    Err(e) => {
                        eprintln!("trontsnap: gif export failed: {e:#}");
                        notify("GIF export failed (see log)".into());
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
                let notify = self.notifier();
                std::thread::spawn(move || {
                    let r: anyhow::Result<String> = (|| {
                        let img = image::open(&path)?.to_rgba8();
                        crate::ocr::recognize(&img)
                    })();
                    match r {
                        Ok(text) if text.trim().is_empty() => {
                            notify("No text found".into());
                        }
                        Ok(text) => {
                            let n = text.lines().count();
                            let _ = crate::clipboard::set_text(&text);
                            notify(format!("Copied {n} lines of text"));
                        }
                        Err(e) => {
                            eprintln!("trontsnap: gallery ocr failed: {e:#}");
                            crate::clipboard::log_line(&format!("gallery ocr failed: {e:#}"));
                            notify("OCR failed (see log)".into());
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
            Action::RangeSelect(path, additive) => {
                // Extend from the anchor within the CURRENT view, both ends
                // resolved against filtered order. If the anchor is filtered
                // out (or there never was one), Shift+click degrades to a
                // plain select-and-anchor instead of guessing a range.
                let anchor = self.sel_anchor.clone().unwrap_or_else(|| path.clone());
                let a = self.filtered.iter().position(|&i| self.shots[i].path == anchor);
                let b = self.filtered.iter().position(|&i| self.shots[i].path == path);
                // Plain Shift+click REPLACES the selection with the range (Explorer /
                // Nautilus); Ctrl+Shift+click adds it.
                if !additive {
                    self.selected.clear();
                }
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
                self.copy_text(
                    ctx,
                    vis.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"),
                );
                self.set_status(format!("Copied {n} paths"));
            }
            #[cfg(not(windows))]
            Action::CopySelectedFiles => {
                let vis = self.visible_selected();
                let n = vis.len();
                let notify = self.notifier();
                self.copy_job(move || {
                    if let Err(e) = crate::clipboard::set_files(&vis) {
                        eprintln!("trontsnap: copy files failed: {e:#}");
                        notify("Copy failed (see log)".into());
                    }
                });
                self.set_status(format!("Copied {n} files"));
            }
            Action::DeleteSelected => {
                let vis = self.visible_selected();
                self.request_delete(vis);
            }
        }
        self.trace_state();
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
    // Read once up front; the click ladder below needs it for single AND double clicks.
    let mods = ui.input(|i| i.modifiers);
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
            video_label(&shot.path),
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
    } else if resp.clicked() || resp.double_clicked() {
        // HARD-WON: modifiers are checked BEFORE the double-click, or a quick
        // Ctrl+click twice (toggle on, toggle off) opened the file instead.
        // Plain click stays copy-to-clipboard (the flagship gesture) until something
        // is selected; then it toggles, so a selection can be built with the mouse alone.
        *action = Some(if mods.shift {
            Action::RangeSelect(shot.path.clone(), mods.command)
        } else if mods.command {
            Action::ToggleSelect(shot.path.clone())
        } else if resp.double_clicked() {
            Action::Open(shot.path.clone())
        } else if sel_count > 0 {
            Action::ToggleSelect(shot.path.clone())
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
        if ui.button(REVEAL).clicked() {
            *action = Some(Action::Reveal(shot.path.clone()));
            ui.close_menu();
        }
        if shot.is_video() && ui.button("Export GIF").clicked() {
            *action = Some(Action::ExportGif(shot.path.clone()));
            ui.close_menu();
        }
        if !shot.is_video() {
            // Greyed out (not hidden) when the engine is missing: the reason is the
            // tooltip, and the OCR failure used to be one easily-missed status line.
            let ocr_ok = crate::ocr::available();
            if ui
                .add_enabled(ocr_ok, egui::Button::new("Copy text (OCR)"))
                .on_disabled_hover_text("Needs tesseract-ocr (sudo apt install tesseract-ocr), then restart TrontSnap")
                .clicked()
            {
                *action = Some(Action::CopyText(shot.path.clone()));
                ui.close_menu();
            }
        }
        ui.separator();
        if ui.button(format!("Delete ({TRASH})")).clicked() {
            *action = Some(Action::Delete(shot.path.clone()));
            ui.close_menu();
        }
        // Bulk section, only when this cell is part of a multi-selection, so
        // the everyday single-item menu stays uncluttered. Both resolve the
        // live visible selection inside apply().
        if selected && sel_count > 1 {
            ui.separator();
            #[cfg(not(windows))]
            if ui.button(format!("Copy {sel_count} files")).clicked() {
                *action = Some(Action::CopySelectedFiles);
                ui.close_menu();
            }
            if ui.button(format!("Copy {sel_count} paths")).clicked() {
                *action = Some(Action::CopySelectedPaths);
                ui.close_menu();
            }
            if ui.button(format!("Delete {sel_count} ({TRASH})")).clicked() {
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
fn reveal(path: &Path, _notify: impl Fn(String) + Send + 'static) {
    crate::shellexec::run("explorer.exe", &format!("/select,\"{}\"", path.display()));
}

/// Linux: opener::reveal goes through the file manager's D-Bus ShowItems where
/// available (Nautilus etc.), falling back to opening the containing folder.
/// HARD-WON: on a worker thread. It waits up to 5 s on FileManager1 and another 5 s on
/// the OpenURI portal when the file manager has to cold-start, which froze the whole
/// window for ~10 s when it ran inside the click handler.
#[cfg(not(windows))]
fn reveal(path: &Path, notify: impl Fn(String) + Send + 'static) {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        if let Err(e) = opener::reveal(&path) {
            eprintln!("trontsnap: reveal failed: {e:#}");
            notify("Reveal failed (see log)".into());
        }
    });
}

/// Open a file in the default app. Windows keeps opener (ShellExecute inside).
/// Linux goes through a reaped xdg-open: opener drops the Child unwaited, one zombie
/// per Open for the app's whole lifetime.
fn open_path(path: &Path) -> bool {
    #[cfg(windows)]
    {
        opener::open(path).is_ok()
    }
    #[cfg(not(windows))]
    {
        crate::shellexec::open(path) || opener::open(path).is_ok()
    }
}

/// The corner label of a video tile: its container, not a hard-coded "MP4"
/// (GNOME's screencast writes webm).
fn video_label(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_uppercase())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "VIDEO".into())
}

/// Does a ShareX archive exist to browse? Always true on Windows (the app's home turf);
/// elsewhere the "ShareX" chip and legend dot only mean something if the folder is there.
fn sharex_present() -> bool {
    cfg!(windows) || capture::sharex_dir().is_some_and(|d| d.exists())
}

#[cfg(test)]
#[path = "gallery/preview.rs"]
mod preview;

#[cfg(test)]
#[path = "gallery/tests.rs"]
mod tests;
