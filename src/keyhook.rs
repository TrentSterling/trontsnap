// A WH_KEYBOARD_LL low-level keyboard hook standing in for global-hotkey.
//
// WHY THIS EXISTS:
// Bare PrintScreen registered via RegisterHotKey (what global-hotkey does under
// the hood) is contested system-wide: since Win10 1809, Windows' own "Print
// Screen opens Snipping Tool" shell feature owns that exact (modifiers=0,
// VK_SNAPSHOT) combo by the time we start, so our registration fails silently.
// A low-level keyboard hook taps every keystroke before that dispatch stage, so
// it sees PrintScreen unconditionally, and returning non-zero + skipping
// CallNextHookEx *consumes* the key so the Snipping Tool binding never also pops.
//
// THREADING: SetWindowsHookExW(WH_KEYBOARD_LL, ...) callbacks are always
// dispatched on the thread that installed the hook, so hook_proc (and PressState)
// only ever run serially on the one dedicated pump thread — never concurrently.
// hook_proc does the absolute minimum and never blocks: every keystroke on the
// whole machine passes through it while installed.

use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Mutex, OnceLock};

// Debounce timestamp for the focused-window fallback hook (see kbd_proc).
static LAST_THREAD_FIRE_MS: AtomicU64 = AtomicU64::new(0);
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use windows::Win32::Foundation::{HMODULE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_SHIFT, VK_SNAPSHOT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// What the hook saw: plain PrintScreen, PrintScreen with Ctrl held, or
/// PrintScreen with Ctrl+Shift held (video record toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Full,
    Region,
    Record,
}

// The hook callback is a bare `extern "system" fn` — no captures — so the send
// side of the channel lives in a process-global set once at install time.
static TX: OnceLock<Sender<HotkeyEvent>> = OnceLock::new();

/// Pure fire-once decision logic for VK_SNAPSHOT down/up events, kept FFI-free so
/// it's directly unit-testable.
///
/// BUG THIS FIXES (intermittent missed Ctrl+PrintScreen, "sometimes nothing,
/// then works next time"): the old logic was a single AtomicBool toggled on
/// down/up. If a VK_SNAPSHOT key-up was EVER not observed (our own
/// AlwaysOnTop+Fullscreen+focus-steal on picker entry perturbing the up, another
/// hook eating it, or the PrintScreen delivery quirk), the flag stuck at `true`
/// forever: the next full press produced ZERO fires and only the press AFTER it
/// worked. Exactly the reported symptom.
///
/// FIX: self-heal via a monotonic gap. If the flag is already `true` when a new
/// down arrives but longer than `stale_ms` has passed since the last down, the
/// key MUST have been released in between (OS auto-repeat never has a gap that
/// large), so treat it as a fresh press instead of eating it.
struct PressState {
    down: bool,
    last_down_ms: u64,
}

impl PressState {
    const fn new() -> Self {
        Self { down: false, last_down_ms: 0 }
    }

    /// Returns true if this event is a new physical press that should fire.
    fn on_event(&mut self, is_down: bool, now_ms: u64, stale_ms: u64) -> bool {
        if is_down {
            let prev = self.last_down_ms;
            self.last_down_ms = now_ms;
            let was_down = self.down;
            self.down = true;
            let stale = was_down && now_ms.saturating_sub(prev) > stale_ms;
            !was_down || stale
        } else {
            // Up fires only if we never saw a matching down (the documented
            // "PrintScreen sometimes only delivers a key-up" quirk).
            !std::mem::replace(&mut self.down, false)
        }
    }
}

// hook_proc always runs serially on the pump thread, so this Mutex is never
// contended — it exists for Send + Sync, not exclusion. unwrap_or_else on poison
// means a poisoned lock can never panic (hook_proc must never panic: release
// builds run panic=abort, which would kill the whole process).
static STATE: Mutex<PressState> = Mutex::new(PressState::new());

// Monotonic millis clock for the staleness check.
static CLOCK_START: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    CLOCK_START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Above Windows' worst-case initial key-repeat delay (~1000ms) — a gap bigger
/// than this proves the key was actually released, not still held.
const REPEAT_STALE_MS: u64 = 1200;

/// Owns the hook's lifetime. Dropping it posts WM_QUIT to the pump thread
/// (unhooking + exiting cleanly) and joins it.
pub struct KeyboardHook {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl KeyboardHook {
    /// Install the hook + start its dedicated pump thread. Returns a receiver that
    /// yields one HotkeyEvent per press. Call once per process.
    pub fn install() -> anyhow::Result<(Self, Receiver<HotkeyEvent>)> {
        let (tx, rx) = crossbeam_channel::unbounded();
        TX.set(tx)
            .map_err(|_| anyhow::anyhow!("KeyboardHook::install called more than once"))?;

        let (tid_tx, tid_rx) = std::sync::mpsc::channel::<u32>();
        let join = std::thread::Builder::new()
            .name("trontsnap-keyhook".into())
            .spawn(move || pump_thread(tid_tx))?;

        let thread_id = tid_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| anyhow::anyhow!("keyboard hook thread did not start in time"))?;

        Ok((Self { thread_id, join: Some(join) }, rx))
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Runs on its own OS thread: installs the hook, pumps messages (required for the
/// hook proc to be invoked), unhooks on WM_QUIT and returns.
fn pump_thread(tid_tx: std::sync::mpsc::Sender<u32>) {
    unsafe {
        let tid = GetCurrentThreadId();
        let _ = tid_tx.send(tid);

        let hmod: HMODULE = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("trontsnap: GetModuleHandleW failed: {e}");
                return;
            }
        };

        let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hmod, 0) {
            Ok(h) => h,
            Err(e) => {
                eprintln!(
                    "trontsnap: SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e:#} \
                     (PrintScreen capture will not work this session)"
                );
                return;
            }
        };

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = UnhookWindowsHookEx(hook);
    }
}

/// The WH_KEYBOARD_LL callback. Fast and panic-free: runs in-line with EVERY
/// keystroke while installed (release builds are panic=abort).
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        if is_down || is_up {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            if kb.vkCode == VK_SNAPSHOT.0 as u32 {
                let now = now_ms();
                let should_fire = {
                    let mut guard = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
                    guard.on_event(is_down, now, REPEAT_STALE_MS)
                };
                if should_fire {
                    fire();
                }
                // Swallow (down AND up) so Windows' own PrintScreen->Snipping
                // Tool binding never sees this keystroke either.
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(HHOOK(0), code, wparam, lparam)
}

/// Read live modifier state and post the right event. Called only on the confirmed
/// leading edge of a PrintScreen press. Ctrl+Shift = record toggle, Ctrl = region,
/// bare = fullscreen.
fn fire() {
    let down = |vk: i32| unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 };
    let ctrl = down(VK_CONTROL.0 as i32);
    let shift = down(VK_SHIFT.0 as i32);
    if let Some(tx) = TX.get() {
        let ev = match (ctrl, shift) {
            (true, true) => HotkeyEvent::Record,
            (true, false) => HotkeyEvent::Region,
            _ => HotkeyEvent::Full,
        };
        let _ = tx.send(ev);
    }
}

/// Install a THREAD-scoped `WH_KEYBOARD` hook on the CALLING (UI) thread. This catches
/// PrintScreen while one of this thread's windows (the gallery) has focus — the exact
/// case where the global `WH_KEYBOARD_LL` hook is NOT invoked (a Windows quirk: an LL
/// keyboard hook can be bypassed for keystrokes going to the hook owner's own
/// foreground window; verified empirically). The two hooks are mutually exclusive (LL
/// for everything else, this one only when we're focused), so no double-fire. Must be
/// called on the UI thread. Best-effort: logs and returns on failure.
pub fn install_focused_fallback() {
    unsafe {
        // hMod = NULL because the proc is in this process and threadId is our own thread.
        if let Err(e) = SetWindowsHookExW(WH_KEYBOARD, Some(kbd_proc), HMODULE(0), GetCurrentThreadId())
        {
            eprintln!("trontsnap: focused-window PrtSc fallback hook unavailable: {e:#}");
        }
        // The hook lives for the process; it's auto-removed on exit. We intentionally
        // don't keep the handle.
    }
}

/// WH_KEYBOARD proc (runs on the UI thread). Fires a capture on any VK_SNAPSHOT
/// keyboard event, debounced, because when a normal app is focused PrintScreen may
/// deliver only a key-up (the documented quirk) — so we can't rely on seeing a keydown.
unsafe extern "system" fn kbd_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && wparam.0 as u32 == VK_SNAPSHOT.0 as u32 {
        let now = now_ms();
        let last = LAST_THREAD_FIRE_MS.swap(now, AtomicOrdering::Relaxed);
        if now.saturating_sub(last) > 250 {
            fire();
        }
    }
    CallNextHookEx(HHOOK(0), code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::PressState;

    const STALE: u64 = 1200;

    #[test]
    fn fires_once_through_repeats_then_up() {
        let mut s = PressState::new();
        assert!(s.on_event(true, 0, STALE));
        assert!(!s.on_event(true, 50, STALE));
        assert!(!s.on_event(true, 100, STALE));
        assert!(!s.on_event(false, 900, STALE));
    }

    #[test]
    fn missed_up_self_heals_on_next_press() {
        let mut s = PressState::new();
        assert!(s.on_event(true, 0, STALE));
        assert!(s.on_event(true, 5000, STALE));
    }

    #[test]
    fn fast_retry_inside_stale_window_is_a_documented_limitation() {
        let mut s = PressState::new();
        assert!(s.on_event(true, 0, STALE));
        assert!(!s.on_event(true, 400, STALE));
        assert!(s.on_event(true, 2000, STALE));
    }

    #[test]
    fn up_only_quirk_always_fires() {
        let mut s = PressState::new();
        assert!(s.on_event(false, 0, STALE));
        assert!(s.on_event(false, 1000, STALE));
    }

    #[test]
    fn rapid_legit_double_tap_fires_twice() {
        let mut s = PressState::new();
        assert!(s.on_event(true, 0, STALE));
        assert!(!s.on_event(false, 60, STALE));
        assert!(s.on_event(true, 180, STALE));
        assert!(!s.on_event(false, 240, STALE));
    }

    #[test]
    fn long_hold_at_slowest_repeat_rate_never_double_fires() {
        let mut s = PressState::new();
        assert!(s.on_event(true, 0, STALE));
        let mut t = 0u64;
        for _ in 0..20 {
            t += 400;
            assert!(!s.on_event(true, t, STALE));
        }
    }

    #[test]
    fn worst_case_initial_repeat_delay_is_not_stale() {
        let mut s = PressState::new();
        assert!(s.on_event(true, 0, STALE));
        assert!(!s.on_event(true, 1000, STALE));
    }
}
