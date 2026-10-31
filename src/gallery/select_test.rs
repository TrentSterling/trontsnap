//! Multi-select driven through the real gallery grid with synthetic egui input
//! (pointer + modifiers), no window and no desktop input.

use super::*;
use std::cell::RefCell;

thread_local! {
    pub(super) static CELLS: RefCell<Vec<(PathBuf, Rect)>> = const { RefCell::new(Vec::new()) };
    /// OS side effects the gallery would have performed (open, copy, drag).
    pub(super) static OS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

/// What winit reports for Ctrl on Windows: both flags.
const CTRL: egui::Modifiers = egui::Modifiers { alt: false, ctrl: true, shift: false, mac_cmd: false, command: true };

struct Harness {
    ctx: egui::Context,
    g: Gallery,
    paths: Vec<PathBuf>,
}

impl Harness {
    fn new(n: usize) -> Self {
        let ctx = egui::Context::default();
        let paths: Vec<PathBuf> = (0..n).map(|i| std::env::temp_dir().join(format!("trontsnap-fixture-shot{i:02}.png"))).collect();
        for p in &paths {
            if !p.is_file() {
                let _ = image::RgbaImage::from_pixel(8, 8, image::Rgba([40, 80, 120, 255])).save(p);
            }
        }
        let g = Gallery::preview(&ctx, &paths);
        let mut h = Self { ctx, g, paths };
        h.frame(vec![], egui::Modifiers::NONE);
        h
    }

    fn frame(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        CELLS.with(|c| c.borrow_mut().clear());
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
            events,
            modifiers,
            ..Default::default()
        };
        let g = &mut self.g;
        let _ = self.ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| g.ui(ui, ctx, None));
        });
    }

    fn center(&self, i: usize) -> egui::Pos2 {
        let p = &self.paths[i];
        CELLS.with(|c| c.borrow().iter().find(|(q, _)| q == p).map(|(_, r)| r.center()))
            .expect("cell drawn")
    }

    fn click(&mut self, i: usize, modifiers: egui::Modifiers) {
        let pos = self.center(i);
        let btn = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers };
        self.frame(vec![egui::Event::PointerMoved(pos)], modifiers);
        self.frame(vec![btn(true)], modifiers);
        self.frame(vec![btn(false)], modifiers);
        self.frame(vec![], egui::Modifiers::NONE);
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        self.frame(vec![ev(true)], modifiers);
        self.frame(vec![ev(false)], modifiers);
    }

    fn sel(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.paths.len()).filter(|&i| self.g.selected.contains(&self.paths[i])).collect();
        v.sort();
        v
    }
}

#[test]
fn ctrl_click_toggles() {
    let mut h = Harness::new(12);
    h.click(2, CTRL);
    h.click(5, CTRL);
    assert_eq!(h.sel(), vec![2, 5]);
    h.click(2, CTRL);
    assert_eq!(h.sel(), vec![5]);
}

#[test]
fn shift_click_extends_from_anchor() {
    let mut h = Harness::new(12);
    h.click(3, CTRL);
    h.click(7, egui::Modifiers::SHIFT);
    assert_eq!(h.sel(), vec![3, 4, 5, 6, 7]);
}

#[test]
fn ctrl_a_then_esc() {
    let mut h = Harness::new(12);
    h.key(egui::Key::A, CTRL);
    assert_eq!(h.sel().len(), 12);
    h.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(h.sel().is_empty());
}

fn os_log() -> Vec<&'static str> {
    OS.with(|o| o.borrow().clone())
}

/// The reported bug: egui calls any two quick clicks a double-click, even on
/// different cells, so a fast Ctrl+click run opened files instead of selecting.
#[test]
fn quick_ctrl_clicks_never_open() {
    OS.with(|o| o.borrow_mut().clear());
    let mut h = Harness::new(12);
    for i in [1, 4, 6, 9] {
        h.click(i, CTRL);
    }
    assert_eq!(h.sel(), vec![1, 4, 6, 9]);
    assert!(!os_log().contains(&"open"), "{:?}", os_log());
}

#[test]
fn plain_click_selects_one_and_double_click_opens_same_cell_only() {
    OS.with(|o| o.borrow_mut().clear());
    let mut h = Harness::new(12);
    h.click(2, CTRL);
    h.click(3, CTRL);
    h.click(5, egui::Modifiers::NONE); // replaces the selection, copies
    assert_eq!(h.sel(), vec![5]);
    assert_eq!(os_log().last(), Some(&"copy"));
    h.click(8, egui::Modifiers::NONE); // quick click on ANOTHER cell: select, no open
    assert_eq!(h.sel(), vec![8]);
    assert!(!os_log().contains(&"open"));
    h.click(8, egui::Modifiers::NONE); // second quick click on the SAME cell: open
    assert_eq!(os_log().last(), Some(&"open"));
}

#[test]
fn shift_replaces_ctrl_shift_adds() {
    let mut h = Harness::new(12);
    h.click(0, CTRL);
    h.click(1, CTRL); // anchor = 1
    h.click(3, egui::Modifiers::SHIFT);
    assert_eq!(h.sel(), vec![1, 2, 3]);
    h.click(8, CTRL); // anchor = 8, adds 8
    let ctrl_shift = egui::Modifiers { shift: true, ..CTRL };
    h.click(10, ctrl_shift);
    assert_eq!(h.sel(), vec![1, 2, 3, 8, 9, 10]);
}
