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

use std::sync::OnceLock;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PostThreadMessageW, MSG, WM_APP, WM_HOTKEY, WM_QUIT,
};

use crate::settings::HotkeyAction;

/// Custom thread message telling the pump to unregister + re-register all three
/// hotkeys from the current settings (posted by `reload()` after a rebind). This
/// is a thread message (hwnd == NULL), which GetMessageW still returns normally.
const WM_RELOAD: u32 = WM_APP + 1;

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
static TX: OnceLock<Sender<HotkeyEvent>> = OnceLock::new();

// The hotkey pump thread's OS thread id, set once at the top of `pump_thread`. Any
// thread can read this to post it a message (RegisterHotKey/UnregisterHotKey must
// run on the thread that owns the registration, so a rebind from the UI thread has
// to ask the pump thread to do the actual re-register via WM_RELOAD).
static PUMP_TID: OnceLock<u32> = OnceLock::new();

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
        let _ = PUMP_TID.set(tid);
        let _ = tid_tx.send(tid);

        register_all();

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
            } else if msg.message == WM_RELOAD {
                register_all();
            }
        }

        let _ = UnregisterHotKey(HWND(0), ID_FULL);
        let _ = UnregisterHotKey(HWND(0), ID_REGION);
        let _ = UnregisterHotKey(HWND(0), ID_RECORD);
    }
}

/// (Re)registers all three hotkeys from the persisted settings binds. Always
/// unregisters first (best-effort: nothing may be registered yet, e.g. on first
/// call at startup) so a changed bind doesn't leave the old combo also active.
/// Must run on the pump thread: RegisterHotKey/UnregisterHotKey bind to whichever
/// thread calls them, and that's the thread whose message queue gets WM_HOTKEY.
unsafe fn register_all() {
    let _ = UnregisterHotKey(HWND(0), ID_FULL);
    let _ = UnregisterHotKey(HWND(0), ID_REGION);
    let _ = UnregisterHotKey(HWND(0), ID_RECORD);

    let (mods, vk) = crate::settings::hotkey(HotkeyAction::Full);
    register_or_log(ID_FULL, HOT_KEY_MODIFIERS(mods | MOD_NOREPEAT.0), vk, "Fullscreen");

    let (mods, vk) = crate::settings::hotkey(HotkeyAction::Region);
    register_or_log(ID_REGION, HOT_KEY_MODIFIERS(mods | MOD_NOREPEAT.0), vk, "Region");

    let (mods, vk) = crate::settings::hotkey(HotkeyAction::Record);
    register_or_log(ID_RECORD, HOT_KEY_MODIFIERS(mods | MOD_NOREPEAT.0), vk, "Record");
}

/// Best-effort RegisterHotKey: logs and continues on failure (e.g. another app
/// already owns that combo) instead of aborting the whole hotkey setup.
unsafe fn register_or_log(id: i32, mods: HOT_KEY_MODIFIERS, vk: u32, label: &str) {
    if let Err(e) = RegisterHotKey(HWND(0), id, mods, vk) {
        eprintln!(
            "trontsnap: {label} hotkey unavailable ({e:#}) — another app may own it"
        );
    }
}

/// Tell the hotkey pump thread to unregister + re-register all three hotkeys from
/// the current settings. Call this from the UI thread after a rebind (settings::
/// set_hotkey / reset_hotkeys already persisted the new bind): the actual
/// RegisterHotKey call has to happen on the pump thread, so this just posts it a
/// message rather than registering directly.
pub fn reload() {
    if let Some(tid) = PUMP_TID.get() {
        unsafe {
            let _ = PostThreadMessageW(*tid, WM_RELOAD, WPARAM(0), LPARAM(0));
        }
    }
}
