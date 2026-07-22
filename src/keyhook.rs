// Global PrintScreen hotkeys via RegisterHotKey — the same mechanism ShareX uses.
//
// WHY THIS WORKS NOW (it didn't used to): bare PrintScreen registered via
// RegisterHotKey is contested since Win10 1809 by Windows' own "PrintScreen
// opens Snipping Tool" shell feature, which owns that binding by default. The
// fix is the per-user, no-admin registry flag `PrintScreenKeyForSnippingEnabled`
// (see printscreen.rs), which frees the key so our registration actually wins
// it. Once that's set, RegisterHotKey is rock solid and — critically — it is
// NEVER UIPI-blocked, so a plain Medium-integrity, unsigned, portable exe
// receives it even while an elevated window (TrontEQ, Task Manager, an
// elevated terminal) has focus. That's the whole reason the previous
// WH_KEYBOARD_LL hook + uiAccess manifest + signed installer existed; none of
// it is needed anymore.
//
// THREADING: RegisterHotKey posts WM_HOTKEY to the thread that registered it
// (hwnd = NULL), so we spawn one dedicated thread that registers all three
// hotkeys and runs its own GetMessageW pump.

use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
    VK_SNAPSHOT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, PostThreadMessageW, MSG, WM_HOTKEY, WM_QUIT};

/// What the hotkey pump saw: plain PrintScreen, PrintScreen with Ctrl held, or
/// PrintScreen with Ctrl+Shift held (video record toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Full,
    Region,
    Record,
}

const ID_FULL: i32 = 1;
const ID_REGION: i32 = 2;
const ID_RECORD: i32 = 3;

// The pump thread is the only place these ids are registered/unregistered, but the
// send side of the channel is set once at install time and read from there.
static TX: std::sync::OnceLock<Sender<HotkeyEvent>> = std::sync::OnceLock::new();

/// Owns the hotkey pump thread's lifetime. Dropping it posts WM_QUIT to the pump
/// thread (which unregisters the hotkeys and exits) and joins it.
pub struct KeyboardHook {
    thread_id: u32,
    join: Option<JoinHandle<()>>,
}

impl KeyboardHook {
    /// Register the three PrintScreen hotkeys + start the pump thread. Returns a
    /// receiver that yields one HotkeyEvent per press. Call once per process.
    pub fn install() -> anyhow::Result<(Self, Receiver<HotkeyEvent>)> {
        let (tx, rx) = crossbeam_channel::unbounded();
        TX.set(tx)
            .map_err(|_| anyhow::anyhow!("KeyboardHook::install called more than once"))?;

        let (tid_tx, tid_rx) = std::sync::mpsc::channel::<u32>();
        let join = std::thread::Builder::new()
            .name("trontsnap-hotkeys".into())
            .spawn(move || pump_thread(tid_tx))?;

        let thread_id = tid_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| anyhow::anyhow!("hotkey pump thread did not start in time"))?;

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

/// Runs on its own OS thread: registers the hotkeys, pumps messages (required
/// for WM_HOTKEY to be delivered), unregisters on WM_QUIT and returns.
fn pump_thread(tid_tx: std::sync::mpsc::Sender<u32>) {
    unsafe {
        let tid = GetCurrentThreadId();
        let _ = tid_tx.send(tid);

        register_or_log(ID_FULL, MOD_NOREPEAT, "PrtSc");
        register_or_log(ID_REGION, MOD_CONTROL | MOD_NOREPEAT, "Ctrl+PrtSc");
        register_or_log(ID_RECORD, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, "Ctrl+Shift+PrtSc");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(0), 0, 0).as_bool() {
            if msg.message == WM_HOTKEY {
                let ev = match msg.wParam.0 as i32 {
                    ID_FULL => Some(HotkeyEvent::Full),
                    ID_REGION => Some(HotkeyEvent::Region),
                    ID_RECORD => Some(HotkeyEvent::Record),
                    _ => None,
                };
                if let (Some(ev), Some(tx)) = (ev, TX.get()) {
                    let _ = tx.send(ev);
                }
            }
        }

        let _ = UnregisterHotKey(HWND(0), ID_FULL);
        let _ = UnregisterHotKey(HWND(0), ID_REGION);
        let _ = UnregisterHotKey(HWND(0), ID_RECORD);
    }
}

/// Best-effort RegisterHotKey: logs and continues on failure (e.g. another app
/// already owns that combo) instead of aborting the whole hotkey setup.
unsafe fn register_or_log(id: i32, mods: HOT_KEY_MODIFIERS, label: &str) {
    if let Err(e) = RegisterHotKey(HWND(0), id, mods, VK_SNAPSHOT.0 as u32) {
        eprintln!(
            "trontsnap: {label} hotkey unavailable ({e:#}) — another app may own it"
        );
    }
}
