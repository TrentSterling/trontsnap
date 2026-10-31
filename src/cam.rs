// TrontSnap Cam (`trontsnap cam`): a GifCam-style recorder frame. A see-through
// frame you drop over anything (drag the toolbar to move, the ring to resize),
// a one-row toolbar for fps / size / format / output scale, and Record. Space,
// Enter or Ctrl+F9 (global, works while minimized) start and stop.
//
// The window's REGION is the ring plus the toolbar, so the inside of the frame is
// not part of the window at all: clicks, hover and scroll reach the game or app
// underneath. The whole window is WDA_EXCLUDEFROMCAPTURE, so neither the ring nor
// the toolbar ever lands in a recording; the toolbar can sit inside the frame when
// a fullscreen record leaves no room outside it.
//
// Capture + encode is recorder::record_rect (DXGI duplication -> Media Foundation
// MP4). GIF, WEBM and scaled output are a vidconv pass after Stop.
//
// `trontsnap rec ...` is the same pipeline with no window, for scripts and tests.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CombineRgn, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
    CreateRectRgn, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect,
    GetMonitorInfoW, InvalidateRect, MonitorFromPoint, ScreenToClient, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, ClientToScreen, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE,
    DT_VCENTER, HDC, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, RGN_DIFF, RGN_OR, SRCCOPY,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, RegisterHotKey, ReleaseCapture, SetCapture, UnregisterHotKey, MOD_CONTROL,
    MOD_NOREPEAT, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F9, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT,
    VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::vidconv::{Format, Scale};

const BORDER: i32 = 6; // ring thickness (96 dpi px)
const BAR_H: i32 = 34;
const MIN_HOLE: i32 = 32;
const WM_CAM_DONE: u32 = WM_APP + 1;
const WM_CAM_TICK: u32 = WM_APP + 2;
const HOTKEY_ID: i32 = 1;

const FPS_PRESETS: [u32; 8] = [10, 15, 20, 24, 25, 30, 50, 60];
const SIZE_PRESETS: [(i32, i32); 8] =
    [(320, 240), (480, 360), (512, 512), (640, 360), (640, 480), (800, 600), (1280, 720), (1920, 1080)];

/// 0 idle, 1 recording, 2 encoding.
static PHASE: AtomicU8 = AtomicU8::new(0);
static HWND_CELL: AtomicIsize = AtomicIsize::new(0);
static REC_START: Mutex<Option<Instant>> = Mutex::new(None);
static STATUS: Mutex<String> = Mutex::new(String::new());
static LAST_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
static CLOSE_PENDING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Btn {
    Rec,
    Fps,
    Size,
    Format,
    Scale,
    Status,
    Min,
    Close,
}

/// (button, width in 96-dpi px; 0 = takes the leftover width).
const BUTTONS: [(Btn, i32); 8] = [
    (Btn::Rec, 118),
    (Btn::Fps, 62),
    (Btn::Size, 100),
    (Btn::Format, 58),
    (Btn::Scale, 70),
    (Btn::Status, 0),
    (Btn::Min, 32),
    (Btn::Close, 32),
];
const STATUS_MIN: i32 = 130;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub fps: u32,
    pub format: Format,
    pub scale: Scale,
}

struct Cam {
    hwnd: HWND,
    /// The record area in virtual-desktop px.
    hole: RECT,
    set: Settings,
    layout: Layout,
    drag: Option<Drag>,
    hover: Option<Btn>,
}

#[derive(Clone, Copy)]
struct Drag {
    /// Edge mask (1 left, 2 right, 4 top, 8 bottom); 0 = move.
    edges: u8,
    start: POINT,
    hole0: RECT,
    from: Option<Btn>,
    moved: bool,
}

#[derive(Clone, Default)]
struct Layout {
    /// Window rect, screen px.
    win: RECT,
    /// Window-local.
    ring: RECT,
    hole: RECT,
    bar: RECT,
    buttons: Vec<(Btn, RECT)>,
    s: f32,
}

thread_local! {
    static CAM: RefCell<Option<Cam>> = const { RefCell::new(None) };
}

/// Run `f` on the cam state unless it is already borrowed (re-entrant messages
/// from SetWindowPos or a menu's modal loop just skip).
fn with<R>(f: impl FnOnce(&mut Cam) -> R) -> Option<R> {
    CAM.with(|c| c.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn sc(v: i32, s: f32) -> i32 {
    (v as f32 * s).round() as i32
}

fn monitor_rect(p: POINT) -> RECT {
    unsafe {
        let m = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(m, &mut mi);
        mi.rcMonitor
    }
}

fn center(r: &RECT) -> POINT {
    POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }
}

fn rect_wh(r: &RECT) -> (i32, i32) {
    (r.right - r.left, r.bottom - r.top)
}

/// Lay the ring around `hole` and the toolbar below it (above when there's no
/// room, inside the hole's bottom edge when neither fits, e.g. fullscreen).
fn compute_layout(hole: RECT, s: f32) -> Layout {
    let mon = monitor_rect(center(&hole));
    let b = sc(BORDER, s);
    let bh = sc(BAR_H, s);
    let ring = RECT { left: hole.left - b, top: hole.top - b, right: hole.right + b, bottom: hole.bottom + b };
    let fixed: i32 = BUTTONS.iter().map(|(_, w)| sc(*w, s)).sum();
    let bar_w = (ring.right - ring.left).max(fixed + sc(STATUS_MIN, s));
    let top = if ring.bottom + bh <= mon.bottom {
        ring.bottom
    } else if ring.top - bh >= mon.top {
        ring.top - bh
    } else {
        hole.bottom - bh
    };
    let mut left = ring.left.max(mon.left);
    if left + bar_w > mon.right {
        left = (mon.right - bar_w).max(mon.left);
    }
    let bar = RECT { left, top, right: left + bar_w, bottom: top + bh };
    let win = RECT {
        left: ring.left.min(bar.left),
        top: ring.top.min(bar.top),
        right: ring.right.max(bar.right),
        bottom: ring.bottom.max(bar.bottom),
    };
    let local = |r: RECT| RECT { left: r.left - win.left, top: r.top - win.top, right: r.right - win.left, bottom: r.bottom - win.top };
    let bar_l = local(bar);
    let mut buttons = Vec::new();
    let flex = bar_w - fixed;
    let mut x = bar_l.left;
    for (btn, w) in BUTTONS {
        let w = if w == 0 { flex } else { sc(w, s) };
        buttons.push((btn, RECT { left: x, top: bar_l.top, right: x + w, bottom: bar_l.bottom }));
        x += w;
    }
    Layout { win, ring: local(ring), hole: local(hole), bar: bar_l, buttons, s }
}

fn apply_layout(hwnd: HWND, l: &Layout) {
    unsafe {
        let (w, h) = rect_wh(&l.win);
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, l.win.left, l.win.top, w, h, SWP_NOACTIVATE);
        let shape = CreateRectRgn(l.ring.left, l.ring.top, l.ring.right, l.ring.bottom);
        let inner = CreateRectRgn(l.hole.left, l.hole.top, l.hole.right, l.hole.bottom);
        let bar = CreateRectRgn(l.bar.left, l.bar.top, l.bar.right, l.bar.bottom);
        CombineRgn(shape, shape, inner, RGN_DIFF);
        CombineRgn(shape, shape, bar, RGN_OR);
        let _ = DeleteObject(inner);
        let _ = DeleteObject(bar);
        SetWindowRgn(hwnd, shape, true); // takes ownership of `shape`
        let _ = InvalidateRect(hwnd, None, false);
    }
}

/// Recompute and apply the layout for the current hole.
fn relayout() {
    let Some((hwnd, l)) = with(|c| {
        let s = unsafe { GetDpiForWindow(c.hwnd) } as f32 / 96.0;
        c.layout = compute_layout(c.hole, if s > 0.0 { s } else { 1.0 });
        (c.hwnd, c.layout.clone())
    }) else {
        return;
    };
    apply_layout(hwnd, &l);
}

// ---------------------------------------------------------------- persistence

fn reg() -> Option<winreg::RegKey> {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey("Software\\TrontSnap\\Cam")
        .ok()
        .map(|(k, _)| k)
}

pub fn load_settings() -> Settings {
    let k = reg();
    let get_s = |n: &str| k.as_ref().and_then(|k| k.get_value::<String, _>(n).ok());
    Settings {
        fps: k
            .as_ref()
            .and_then(|k| k.get_value::<u32, _>("Fps").ok())
            .filter(|f| (1..=120).contains(f))
            .unwrap_or(30),
        format: get_s("Format").and_then(|v| Format::parse(&v)).unwrap_or(Format::Gif),
        scale: get_s("Scale").and_then(|v| Scale::parse(&v)).unwrap_or(Scale::Native),
    }
}

fn save(set: &Settings, hole: &RECT) {
    if let Some(k) = reg() {
        let _ = k.set_value("Fps", &set.fps);
        let _ = k.set_value("Format", &set.format.ext().to_string());
        let _ = k.set_value("Scale", &set.scale.label());
        let _ = k.set_value("Rect", &format!("{},{},{},{}", hole.left, hole.top, hole.right - hole.left, hole.bottom - hole.top));
    }
}

fn load_hole() -> Option<RECT> {
    let v: String = reg()?.get_value("Rect").ok()?;
    let n: Vec<i32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    let [x, y, w, h] = n[..] else { return None };
    Some(RECT { left: x, top: y, right: x + w, bottom: y + h })
}

/// Keep the hole fully on the monitor holding its center, at least MIN_HOLE.
fn clamp_hole(mut r: RECT) -> RECT {
    let mon = monitor_rect(center(&r));
    let (mw, mh) = rect_wh(&mon);
    let w = (r.right - r.left).clamp(MIN_HOLE, mw);
    let h = (r.bottom - r.top).clamp(MIN_HOLE, mh);
    r.left = r.left.clamp(mon.left, mon.right - w);
    r.top = r.top.clamp(mon.top, mon.bottom - h);
    r.right = r.left + w;
    r.bottom = r.top + h;
    r
}

fn centered_on_cursor(w: i32, h: i32) -> RECT {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    let mon = monitor_rect(p);
    let c = center(&mon);
    clamp_hole(RECT { left: c.x - w / 2, top: c.y - h / 2, right: c.x - w / 2 + w, bottom: c.y - h / 2 + h })
}

// ---------------------------------------------------------------- CLI helpers

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn parse_wh(s: &str) -> Option<(i32, i32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Overrides from `--fps N --format gif|mp4|webm --scale native|50%|720p`.
fn apply_cli(set: &mut Settings, args: &[String]) {
    if let Some(f) = flag(args, "--fps").and_then(|v| v.parse().ok()).filter(|f: &u32| (1..=120).contains(f)) {
        set.fps = f;
    }
    if let Some(f) = flag(args, "--format").and_then(|v| Format::parse(&v)) {
        set.format = f;
    }
    if let Some(s) = flag(args, "--scale").and_then(|v| Scale::parse(&v)) {
        set.scale = s;
    }
}

/// `trontsnap rec [--rect X,Y,W,H | --monitor N] [--secs S] [--fps N] [--format F]
/// [--scale S]`: record with no window, print the finished file. Defaults to the
/// cam's saved settings and the whole monitor under the mouse.
pub fn rec_cli(args: &[String]) -> anyhow::Result<()> {
    let mut set = load_settings();
    apply_cli(&mut set, args);
    let secs: f64 = flag(args, "--secs").and_then(|v| v.parse().ok()).unwrap_or(5.0);
    let rect = if let Some(r) = flag(args, "--rect") {
        let n: Vec<i32> = r.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        let [x, y, w, h] = n[..] else { anyhow::bail!("--rect wants X,Y,W,H") };
        RECT { left: x, top: y, right: x + w, bottom: y + h }
    } else {
        let mons = crate::capture::monitors()?;
        let m = match flag(args, "--monitor").and_then(|v| v.parse::<usize>().ok()) {
            Some(n) => mons.get(n.saturating_sub(1)).cloned().ok_or_else(|| anyhow::anyhow!("no such monitor"))?,
            None => crate::capture::cursor_monitor()?,
        };
        RECT { left: m.x, top: m.y, right: m.x + m.w as i32, bottom: m.y + m.h as i32 }
    };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs_f64(secs));
        crate::recorder::request_stop();
    });
    let t = Instant::now();
    let (path, note) = record_and_finish(rect, set)?;
    println!("{}", path.display());
    if let Some(n) = note {
        println!("note: {n}");
    }
    eprintln!("trontsnap: rec {:?} done in {:.1}s", set, t.elapsed().as_secs_f64());
    Ok(())
}

/// Capture `rect` until request_stop(), then convert. Blocking.
fn record_and_finish(rect: RECT, set: Settings) -> anyhow::Result<(PathBuf, Option<String>)> {
    let direct = !crate::vidconv::needs_pass(set.format, set.scale);
    let raw = if direct { crate::recorder::out_path("mp4")? } else { crate::vidconv::temp_raw()? };
    // A GIF has no audio track, so don't capture one.
    let audio = set.format != Format::Gif && crate::settings::record_audio();
    let raw = crate::recorder::record_rect(rect, set.fps, audio, raw)?;
    PHASE.store(2, Ordering::SeqCst);
    post(WM_CAM_TICK);
    crate::vidconv::finish(&raw, set.format, set.scale, set.fps)
}

fn post(msg: u32) {
    let h = HWND_CELL.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(HWND(h), msg, WPARAM(0), LPARAM(0));
        }
    }
}

fn set_status(s: impl Into<String>) {
    if let Ok(mut g) = STATUS.lock() {
        *g = s.into();
    }
}

// ---------------------------------------------------------------- record toggle

fn toggle() {
    match PHASE.load(Ordering::SeqCst) {
        1 => crate::recorder::request_stop(),
        2 => {} // encoding; nothing to stop
        _ => {
            let Some((hole, set)) = with(|c| (c.hole, c.set)) else { return };
            PHASE.store(1, Ordering::SeqCst);
            if let Ok(mut g) = REC_START.lock() {
                *g = Some(Instant::now());
            }
            set_status("");
            std::thread::spawn(move || {
                match record_and_finish(hole, set) {
                    Ok((path, note)) => {
                        let clip = crate::clipboard::set_file(&path).is_ok();
                        crate::sound::play_shutter();
                        crate::toast::launch(&path, clip);
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        set_status(match note {
                            Some(n) => format!("{n}  ({name})"),
                            None => format!("Saved + copied  {name}"),
                        });
                        if let Ok(mut g) = LAST_FILE.lock() {
                            *g = Some(path);
                        }
                    }
                    Err(e) => {
                        eprintln!("trontsnap: cam recording failed: {e:#}");
                        set_status(format!("Failed: {e}"));
                    }
                }
                PHASE.store(0, Ordering::SeqCst);
                post(WM_CAM_DONE);
            });
        }
    }
    unsafe {
        let _ = InvalidateRect(HWND(HWND_CELL.load(Ordering::SeqCst)), None, false);
    }
}

// ---------------------------------------------------------------- entry

/// Open the cam window in its own process (or bring the open one forward).
pub fn launch() {
    let Ok(exe) = std::env::current_exe() else { return };
    if !crate::shellexec::run(&exe.to_string_lossy(), "cam") {
        eprintln!("trontsnap: cam launch refused by the shell");
    }
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let class: Vec<u16> = "TrontSnapCam\0".encode_utf16().collect();
    unsafe {
        // One cam at a time: a second launch brings the first one back.
        let existing = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null());
        if existing.0 != 0 {
            let _ = ShowWindow(existing, SW_RESTORE);
            let _ = SetForegroundWindow(existing);
            return Ok(());
        }

        let mut set = load_settings();
        apply_cli(&mut set, args);
        let hole = match flag(args, "--size").and_then(|v| parse_wh(&v)) {
            Some((w, h)) => centered_on_cursor(w, h),
            None => load_hole().map(clamp_hole).unwrap_or_else(|| centered_on_cursor(640, 360)),
        };

        let hinstance = GetModuleHandleW(None)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc_),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            hIcon: LoadIconW(hinstance, PCWSTR(1 as *const u16)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title: Vec<u16> = "TrontSnap Cam\0".encode_utf16().collect();
        let layout = compute_layout(hole, 1.0);
        let (w, h) = rect_wh(&layout.win);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_APPWINDOW,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX,
            layout.win.left,
            layout.win.top,
            w,
            h,
            HWND(0),
            None,
            hinstance,
            None,
        );
        if hwnd.0 == 0 {
            anyhow::bail!("could not create the cam window");
        }
        HWND_CELL.store(hwnd.0, Ordering::SeqCst);
        CAM.with(|c| *c.borrow_mut() = Some(Cam { hwnd, hole, set, layout, drag: None, hover: None }));
        // Never in a recording or screenshot, ours or anyone else's.
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        if RegisterHotKey(hwnd, HOTKEY_ID, MOD_CONTROL | MOD_NOREPEAT, VK_F9.0 as u32).is_err() {
            set_status("Ctrl+F9 is taken by another app; use Space or Enter");
        }
        relayout();
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        SetTimer(hwnd, 1, 250, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(0), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- menus

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Pop a menu of (id, label, checked) under `anchor` (window-local); returns the
/// chosen id. Must be called with no cam borrow held (modal loop re-enters proc).
fn popup(hwnd: HWND, anchor: RECT, items: &[(usize, String, bool)], sub: Option<(&str, &[(usize, String, bool)])>) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let add = |m: HMENU, id: usize, label: &str, checked: bool| {
            let w = wide(label);
            if id == 0 {
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            } else {
                let flags = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
                let _ = AppendMenuW(m, flags, id, PCWSTR(w.as_ptr()));
            }
        };
        for (id, label, checked) in items {
            add(menu, *id, label, *checked);
        }
        if let Some((title, subitems)) = sub {
            if let Ok(sm) = CreatePopupMenu() {
                for (id, label, checked) in subitems {
                    add(sm, *id, label, *checked);
                }
                if subitems.is_empty() {
                    let w = wide("(no windows)");
                    let _ = AppendMenuW(sm, MF_STRING | MF_GRAYED, 1, PCWSTR(w.as_ptr()));
                }
                let w = wide(title);
                let _ = AppendMenuW(menu, MF_POPUP, sm.0 as usize, PCWSTR(w.as_ptr()));
            }
        }
        let mut pt = POINT { x: anchor.left, y: anchor.bottom };
        let _ = ClientToScreen(hwnd, &mut pt);
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, 0, hwnd, None);
        let _ = DestroyMenu(menu); // destroys the submenu too
        id.0 as usize
    }
}

fn on_button(btn: Btn) {
    let Some((hwnd, rect, set, hole)) = with(|c| {
        let r = c.layout.buttons.iter().find(|(b, _)| *b == btn).map(|(_, r)| *r).unwrap_or_default();
        (c.hwnd, r, c.set, c.hole)
    }) else {
        return;
    };
    let idle = PHASE.load(Ordering::SeqCst) == 0;
    match btn {
        Btn::Rec => toggle(),
        Btn::Min => unsafe {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
        },
        Btn::Close => close(hwnd),
        Btn::Status => {
            if let Some(p) = LAST_FILE.lock().ok().and_then(|g| g.clone()) {
                crate::shellexec::run("explorer.exe", &format!("/select,\"{}\"", p.display()));
            }
        }
        _ if !idle => {} // settings are locked while recording / encoding
        Btn::Fps => {
            let items: Vec<_> = FPS_PRESETS.iter().map(|f| (100 + *f as usize, format!("{f} fps"), *f == set.fps)).collect();
            let id = popup(hwnd, rect, &items, None);
            if id > 100 {
                with(|c| c.set.fps = (id - 100) as u32);
            }
        }
        Btn::Format => {
            let items: Vec<_> = Format::ALL
                .iter()
                .enumerate()
                .map(|(i, f)| (200 + i, f.label().to_string(), *f == set.format))
                .collect();
            let id = popup(hwnd, rect, &items, None);
            if id >= 200 {
                with(|c| c.set.format = Format::ALL[id - 200]);
            }
        }
        Btn::Scale => {
            let items: Vec<_> = Scale::PRESETS
                .iter()
                .enumerate()
                .map(|(i, s)| (300 + i, format!("Output {}", s.label()), *s == set.scale))
                .collect();
            let id = popup(hwnd, rect, &items, None);
            if id >= 300 {
                with(|c| c.set.scale = Scale::PRESETS[id - 300]);
            }
        }
        Btn::Size => {
            let (w, h) = rect_wh(&hole);
            let mut items: Vec<_> = SIZE_PRESETS
                .iter()
                .enumerate()
                .map(|(i, (pw, ph))| (400 + i, format!("{pw} x {ph}"), (*pw, *ph) == (w, h)))
                .collect();
            items.push((0, String::new(), false));
            items.push((490, "Full screen (this monitor)".into(), false));
            items.push((491, "Center on screen".into(), false));
            let wins: Vec<(String, RECT)> = crate::overlay::enumerate_windows_titled()
                .into_iter()
                .filter(|(t, r)| t != "TrontSnap Cam" && r.right - r.left >= MIN_HOLE && r.bottom - r.top >= MIN_HOLE)
                .take(24)
                .collect();
            let sub: Vec<_> = wins
                .iter()
                .enumerate()
                .map(|(i, (t, r))| {
                    let t: String = t.chars().take(48).collect();
                    (500 + i, format!("{t}   ({} x {})", r.right - r.left, r.bottom - r.top), false)
                })
                .collect();
            let id = popup(hwnd, rect, &items, Some(("Snap to window", &sub)));
            let new_hole = match id {
                400..=407 => {
                    let (pw, ph) = SIZE_PRESETS[id - 400];
                    Some(clamp_hole(RECT { left: hole.left, top: hole.top, right: hole.left + pw, bottom: hole.top + ph }))
                }
                490 => Some(monitor_rect(center(&hole))),
                491 => Some(centered_on_cursor(w, h)),
                500.. => wins.get(id - 500).map(|(_, r)| clamp_hole(*r)),
                _ => None,
            };
            if let Some(nh) = new_hole {
                with(|c| c.hole = nh);
                relayout();
            }
        }
    }
    with(|c| save(&c.set, &c.hole));
    unsafe {
        let _ = InvalidateRect(hwnd, None, false);
    }
}

fn close(hwnd: HWND) {
    if PHASE.load(Ordering::SeqCst) == 1 {
        // Stop first; the file finishes and the window closes once it's saved.
        crate::recorder::request_stop();
        CLOSE_PENDING.store(true, Ordering::SeqCst);
        set_status("Finishing the recording...");
        return;
    }
    if PHASE.load(Ordering::SeqCst) == 2 {
        return; // let the encode finish
    }
    with(|c| save(&c.set, &c.hole));
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

// ---------------------------------------------------------------- input

fn lparam_pt(lp: LPARAM) -> POINT {
    POINT { x: (lp.0 & 0xffff) as i16 as i32, y: ((lp.0 >> 16) & 0xffff) as i16 as i32 }
}

fn inside(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// What's under window-local point `p`: a toolbar button, or ring edges.
fn hit(l: &Layout, p: POINT) -> (Option<Btn>, u8) {
    if let Some((b, _)) = l.buttons.iter().find(|(_, r)| inside(r, p)) {
        return (Some(*b), 0);
    }
    if inside(&l.ring, p) && !inside(&l.hole, p) {
        let grab = sc(BORDER * 2, l.s);
        let mut e = 0u8;
        if p.x < l.hole.left + grab {
            e |= 1;
        }
        if p.x >= l.hole.right - grab {
            e |= 2;
        }
        if p.y < l.hole.top + grab {
            e |= 4;
        }
        if p.y >= l.hole.bottom - grab {
            e |= 8;
        }
        return (None, e);
    }
    (None, 0)
}

fn cursor_for(edges: u8) -> PCWSTR {
    match edges {
        1 | 2 => IDC_SIZEWE,
        4 | 8 => IDC_SIZENS,
        5 | 10 => IDC_SIZENWSE,
        6 | 9 => IDC_SIZENESW,
        _ => IDC_ARROW,
    }
}

fn nudge(vk: u16) {
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let d = if shift { 10 } else { 1 };
    let (dx, dy) = match vk {
        v if v == VK_LEFT.0 => (-d, 0),
        v if v == VK_RIGHT.0 => (d, 0),
        v if v == VK_UP.0 => (0, -d),
        v if v == VK_DOWN.0 => (0, d),
        _ => return,
    };
    with(|c| {
        let mut h = c.hole;
        if ctrl {
            // Ctrl+arrows resize from the bottom-right corner.
            h.right += dx;
            h.bottom += dy;
        } else {
            h.left += dx;
            h.right += dx;
            h.top += dy;
            h.bottom += dy;
        }
        c.hole = clamp_hole(h);
    });
    relayout();
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            with(|c| paint(hdc, c));
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_TIMER | WM_CAM_TICK => {
            if PHASE.load(Ordering::SeqCst) != 0 || msg == WM_CAM_TICK {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_CAM_DONE => {
            let _ = InvalidateRect(hwnd, None, false);
            // Close was pressed mid-recording: now that the file is saved, go.
            if CLOSE_PENDING.swap(false, Ordering::SeqCst) {
                close(hwnd);
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            if wp.0 as i32 == HOTKEY_ID {
                toggle();
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u16;
            if vk == VK_SPACE.0 || vk == VK_RETURN.0 {
                toggle();
            } else if vk == VK_ESCAPE.0 {
                close(hwnd);
            } else if PHASE.load(Ordering::SeqCst) == 0 {
                nudge(vk);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let _ = ScreenToClient(hwnd, &mut p);
            let idle = PHASE.load(Ordering::SeqCst) == 0;
            let edges = with(|c| hit(&c.layout, p).1).unwrap_or(0);
            let id = if idle { cursor_for(edges) } else { IDC_ARROW };
            SetCursor(LoadCursorW(None, id).unwrap_or_default());
            LRESULT(1)
        }
        WM_LBUTTONDOWN => {
            let p = lparam_pt(lp);
            with(|c| {
                let (btn, edges) = hit(&c.layout, p);
                let mut sp = p;
                let _ = ClientToScreen(hwnd, &mut sp);
                // Buttons click; the status strip and ring drag (only when idle).
                if btn.is_some() || edges != 0 {
                    c.drag = Some(Drag { edges, start: sp, hole0: c.hole, from: btn, moved: false });
                }
            });
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let p = lparam_pt(lp);
            let idle = PHASE.load(Ordering::SeqCst) == 0;
            let mut need_layout = false;
            let mut repaint = false;
            with(|c| {
                let hov = hit(&c.layout, p).0;
                if hov != c.hover {
                    c.hover = hov;
                    repaint = true;
                }
                let Some(mut d) = c.drag else { return };
                let mut sp = p;
                let _ = ClientToScreen(hwnd, &mut sp);
                let (dx, dy) = (sp.x - d.start.x, sp.y - d.start.y);
                if !d.moved && dx.abs() + dy.abs() < 4 {
                    return;
                }
                // Only the status strip moves the frame (a button press that slides
                // off just cancels); nothing moves while recording.
                if !idle || (d.edges == 0 && d.from != Some(Btn::Status)) {
                    return;
                }
                d.moved = true;
                let mut h = d.hole0;
                if d.edges == 0 {
                    h = RECT { left: h.left + dx, top: h.top + dy, right: h.right + dx, bottom: h.bottom + dy };
                } else {
                    if d.edges & 1 != 0 {
                        h.left = (h.left + dx).min(h.right - MIN_HOLE);
                    }
                    if d.edges & 2 != 0 {
                        h.right = (h.right + dx).max(h.left + MIN_HOLE);
                    }
                    if d.edges & 4 != 0 {
                        h.top = (h.top + dy).min(h.bottom - MIN_HOLE);
                    }
                    if d.edges & 8 != 0 {
                        h.bottom = (h.bottom + dy).max(h.top + MIN_HOLE);
                    }
                }
                c.hole = h;
                c.drag = Some(d);
                need_layout = true;
            });
            if need_layout {
                relayout();
            } else if repaint {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            let p = lparam_pt(lp);
            let click = with(|c| {
                let d = c.drag.take()?;
                if d.moved {
                    c.hole = clamp_hole(c.hole);
                    save(&c.set, &c.hole);
                    return Some(None);
                }
                // A click lands if released on the same button it started on.
                let (btn, _) = hit(&c.layout, p);
                Some(if btn.is_some() && btn == d.from { btn } else { None })
            })
            .flatten();
            match click {
                Some(Some(b)) => on_button(b),
                Some(None) => relayout(),
                None => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            close(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---------------------------------------------------------------- paint

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

fn fill(hdc: HDC, r: &RECT, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(hdc, r, b);
        let _ = DeleteObject(b);
    }
}

fn text(hdc: HDC, s: &str, r: &RECT, color: COLORREF, center: bool) {
    unsafe {
        let mut w: Vec<u16> = s.encode_utf16().collect();
        let mut rr = *r;
        SetTextColor(hdc, color);
        let align = if center { DT_CENTER } else { DT_LEFT };
        DrawTextW(hdc, &mut w, &mut rr, align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    }
}

fn fmt_time(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

unsafe fn paint(hdc: HDC, c: &Cam) {
    let l = &c.layout;
    let (w, h) = rect_wh(&RECT { left: 0, top: 0, right: l.win.right - l.win.left, bottom: l.win.bottom - l.win.top });
    // Double-buffer: compose off-screen, one blit (no flicker while dragging).
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    let old = SelectObject(mem, bmp);

    let phase = PHASE.load(Ordering::SeqCst);
    let ring = match phase {
        1 => {
            let on = REC_START.lock().ok().and_then(|g| *g).map(|t| (t.elapsed().as_millis() / 500) % 2 == 0).unwrap_or(true);
            if on { rgb(255, 48, 64) } else { rgb(128, 32, 40) }
        }
        2 => rgb(255, 184, 64),
        _ => {
            let a = crate::theme::accent_over_media();
            rgb(a.r(), a.g(), a.b())
        }
    };
    fill(mem, &l.ring, ring);
    // 1px dark inner line so the ring reads on bright content too.
    let ins = RECT { left: l.hole.left - 1, top: l.hole.top - 1, right: l.hole.right + 1, bottom: l.hole.bottom + 1 };
    fill(mem, &ins, rgb(10, 12, 16));

    let plate = rgb(16, 18, 24);
    fill(mem, &l.bar, plate);

    let font = CreateFontW(
        sc(15, l.s),
        0,
        0,
        0,
        600,
        0,
        0,
        0,
        windows::Win32::Graphics::Gdi::DEFAULT_CHARSET.0 as u32,
        windows::Win32::Graphics::Gdi::OUT_DEFAULT_PRECIS.0 as u32,
        windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS.0 as u32,
        windows::Win32::Graphics::Gdi::CLEARTYPE_QUALITY.0 as u32,
        0,
        PCWSTR(wide("Segoe UI").as_ptr()),
    );
    let old_font = SelectObject(mem, font);
    SetBkMode(mem, TRANSPARENT);

    let (hw, hh) = rect_wh(&c.hole);
    let idle = phase == 0;
    let fg = rgb(236, 240, 246);
    let dim = rgb(130, 138, 150);
    let pad = sc(3, l.s);
    for (btn, r) in &l.buttons {
        let inner = RECT { left: r.left + pad, top: r.top + pad, right: r.right - pad, bottom: r.bottom - pad };
        let hover = c.hover == Some(*btn);
        let settings_btn = matches!(btn, Btn::Fps | Btn::Size | Btn::Format | Btn::Scale);
        let (label, bg, col): (String, COLORREF, COLORREF) = match btn {
            Btn::Rec => match phase {
                1 => {
                    let t = REC_START.lock().ok().and_then(|g| *g).map(|t| t.elapsed()).unwrap_or_default();
                    (format!("\u{25A0} Stop  {}", fmt_time(t)), rgb(200, 36, 52), fg)
                }
                2 => ("Encoding...".into(), rgb(120, 86, 24), fg),
                _ => ("\u{25CF} Record".into(), if hover { rgb(230, 52, 70) } else { rgb(190, 38, 56) }, fg),
            },
            Btn::Fps => (format!("{} fps", c.set.fps), rgb(32, 36, 46), if idle { fg } else { dim }),
            Btn::Size => (format!("{hw} x {hh}"), rgb(32, 36, 46), if idle { fg } else { dim }),
            Btn::Format => (c.set.format.label().into(), rgb(32, 36, 46), if idle { fg } else { dim }),
            Btn::Scale => (c.set.scale.label(), rgb(32, 36, 46), if idle { fg } else { dim }),
            Btn::Status => {
                let s = STATUS.lock().map(|s| s.clone()).unwrap_or_default();
                let s = if !s.is_empty() {
                    s
                } else if phase == 1 {
                    "Recording. Space, Enter or Ctrl+F9 stops".into()
                } else {
                    "Space or Ctrl+F9. Drag here to move".into()
                };
                (s, plate, dim)
            }
            Btn::Min => ("_".into(), plate, fg),
            Btn::Close => ("x".into(), if hover { rgb(196, 43, 28) } else { plate }, fg),
        };
        let bg = if hover && settings_btn && idle { rgb(48, 54, 68) } else if hover && *btn == Btn::Min { rgb(40, 44, 56) } else { bg };
        if bg != plate {
            fill(mem, &inner, bg);
        }
        let text_r = if *btn == Btn::Status { RECT { left: inner.left + pad * 2, ..inner } } else { inner };
        text(mem, &label, &text_r, col, *btn != Btn::Status);
    }

    SelectObject(mem, old_font);
    let _ = DeleteObject(font);
    let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_puts_bar_under_hole_and_buttons_fill_it() {
        // Far from any monitor edge so the bar fits below.
        let hole = RECT { left: 200, top: 200, right: 840, bottom: 560 };
        let l = compute_layout(hole, 1.0);
        assert_eq!(l.hole.right - l.hole.left, 640);
        assert!(l.bar.top >= l.ring.bottom - 1 || l.bar.bottom <= l.ring.top + 1 || l.bar.bottom <= l.hole.bottom);
        let last = l.buttons.last().unwrap().1;
        assert_eq!(last.right, l.bar.right);
        assert!(l.buttons.iter().all(|(_, r)| r.right > r.left));
    }

    /// Offscreen render of the toolbar + ring (no window, no desktop input), saved to
    /// target/cam-paint.png for eyeballing. Run: cargo test cam_paint -- --ignored
    #[test]
    #[ignore]
    fn cam_paint_snapshot() {
        use windows::Win32::Graphics::Gdi::{GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
        unsafe {
            let hole = RECT { left: 300, top: 300, right: 940, bottom: 660 };
            let layout = compute_layout(hole, 1.5);
            let cam = Cam {
                hwnd: HWND(0),
                hole,
                set: Settings { fps: 30, format: Format::Gif, scale: Scale::Native },
                layout,
                drag: None,
                hover: Some(Btn::Size),
            };
            let (w, h) = rect_wh(&cam.layout.win);
            let screen = GetDC(HWND(0));
            let dc = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(dc, bmp);
            paint(dc, &cam);
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0 as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut px = vec![0u8; (w * h * 4) as usize];
            SelectObject(dc, old);
            GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS);
            ReleaseDC(HWND(0), screen);
            for p in px.chunks_mut(4) {
                p.swap(0, 2);
                p[3] = 255;
            }
            // Punch the hole the window region would remove, so the PNG reads true.
            let l = &cam.layout;
            for y in l.hole.top..l.hole.bottom {
                for x in l.hole.left..l.hole.right {
                    let i = ((y * w + x) * 4) as usize;
                    px[i..i + 4].copy_from_slice(&[60, 70, 80, 255]);
                }
            }
            image::RgbaImage::from_raw(w as u32, h as u32, px).unwrap().save("target/cam-paint.png").unwrap();
        }
    }

    #[test]
    fn parses_size_and_flags() {
        assert_eq!(parse_wh("800x600"), Some((800, 600)));
        let args: Vec<String> = ["--fps", "50", "--format", "webm", "--scale", "50%"].map(String::from).to_vec();
        let mut s = Settings { fps: 30, format: Format::Gif, scale: Scale::Native };
        apply_cli(&mut s, &args);
        assert_eq!(s, Settings { fps: 50, format: Format::Webm, scale: Scale::Pct(50) });
    }
}
