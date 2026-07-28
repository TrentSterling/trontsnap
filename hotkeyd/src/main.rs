//! TrontSnap hotkey broker.
//!
//! THE PROBLEM THIS EXISTS TO SOLVE. Windows will not deliver a modifier-less
//! hotkey (a bare PrtSc) to a Medium-integrity process while an ELEVATED window
//! has focus, which is why TrontSnap could never capture Task Manager. The
//! documented fix is uiAccess, but uiAccess is granted by handing out a raised
//! token that lands at HIGH integrity, and Windows blocks OLE drag-and-drop
//! across an integrity boundary. Putting uiAccess on the UI therefore buys
//! Task Manager captures and costs dragging a screenshot into Discord. Measured
//! both ways on 2026-07-27; it is not a bug, it is the same token doing both.
//!
//! So the two jobs are split across two processes, which is how everything else
//! solves this (AutoHotkey ships a separate uiAccess binary; Everything runs a
//! Medium GUI plus a privileged service):
//!
//!   trontsnap.exe          Medium, no uiAccess -> UI, gallery, capture, DRAG-OUT
//!   trontsnap-hotkeys.exe  uiAccess, signed    -> owns RegisterHotKey, no UI
//!
//! This process has no window, no tray icon and no UI. It registers the three
//! binds, and when one fires it opens a loopback connection to the UI's existing
//! single-instance port and writes one word. That port already existed for
//! second-launch handling, so this adds a vocabulary rather than a new channel.
//!
//! It reads its binds from the SAME registry values the UI writes, and watches
//! that key, so rebinding in Settings > Hotkeys takes effect here without any
//! extra IPC.

#![windows_subsystem = "windows"]

use std::io::Write as _;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY,
    REG_NOTIFY_CHANGE_LAST_SET,
};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentThreadId, WaitForSingleObject};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PostThreadMessageW, MSG, WM_APP, WM_HOTKEY, WM_QUIT,
};
use winreg::enums::{HKEY_CURRENT_USER as WR_HKCU, KEY_READ};
use winreg::RegKey;

/// The UI's single-instance port (app.rs `PORT`). Connecting to it and writing a
/// command is the whole IPC.
const UI_PORT: u16 = 48761;

/// Our own port. It does double duty: holding it is our single-instance lock
/// (two brokers would fight over the same registrations), and the UI connects to
/// it to check we actually came up before it cedes the hotkeys to us.
///
/// NOT 48762: that is TrontEQ's single-instance port, and picking it made the
/// bind fail, which made this process exit instantly, which left the UI having
/// handed its hotkeys to a corpse. Ports are a shared namespace across the whole
/// fleet; check before adding one.
///   48761 trontsnap UI
///   48762 tronteq
///   48771 this broker
pub const BROKER_PORT: u16 = 48771;

const APP_KEY: &str = r"Software\TrontSnap";

const ID_FULL: i32 = 1;
const ID_REGION: i32 = 2;
const ID_RECORD: i32 = 3;

/// Re-read the binds and re-register. Must run ON the pump thread, because
/// RegisterHotKey binds to whichever thread called it.
const WM_RELOAD: u32 = WM_APP + 1;

/// A (modifiers, vk) pair. Modifier bits are raw HOT_KEY_MODIFIERS without
/// MOD_NOREPEAT; that is OR'd in at registration time, mirroring keyhook.rs.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Bind {
    mods: u32,
    vk: u32,
}

/// Defaults must match settings.rs: bare PrtSc / Ctrl+PrtSc / Ctrl+Shift+PrtSc.
fn read_binds() -> [Bind; 3] {
    let mut binds = [
        Bind { mods: 0x0000, vk: 0x2C },
        Bind { mods: 0x0002, vk: 0x2C },
        Bind { mods: 0x0002 | 0x0004, vk: 0x2C },
    ];
    let Ok(key) = RegKey::predef(WR_HKCU).open_subkey_with_flags(APP_KEY, KEY_READ) else {
        return binds;
    };
    let names = [
        ("HotkeyFullMods", "HotkeyFullVk"),
        ("HotkeyRegionMods", "HotkeyRegionVk"),
        ("HotkeyRecordMods", "HotkeyRecordVk"),
    ];
    for (i, (m, v)) in names.iter().enumerate() {
        if let Ok(x) = key.get_value::<u32, _>(m) {
            binds[i].mods = x;
        }
        if let Ok(x) = key.get_value::<u32, _>(v) {
            binds[i].vk = x;
        }
    }
    binds
}

/// Send one command to the UI. Returns false if nothing is listening, which is
/// how we notice the UI has exited.
fn send(cmd: &str) -> bool {
    match TcpStream::connect((Ipv4Addr::LOCALHOST, UI_PORT)) {
        Ok(mut s) => {
            let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
            s.write_all(cmd.as_bytes()).is_ok()
        }
        Err(_) => false,
    }
}

fn register_all(binds: &[Bind; 3]) {
    for (id, b) in [ID_FULL, ID_REGION, ID_RECORD].iter().zip(binds.iter()) {
        unsafe {
            let _ = UnregisterHotKey(HWND(0), *id);
            // Failure is not fatal: another app may own that combo. The other two
            // still work, which beats refusing to start.
            let _ = RegisterHotKey(
                HWND(0),
                *id,
                HOT_KEY_MODIFIERS(b.mods) | MOD_NOREPEAT,
                b.vk,
            );
        }
    }
}

/// Watch HKCU\Software\TrontSnap and poke the pump to re-register when the user
/// rebinds in Settings. Reading the same values the UI writes is the entire sync
/// mechanism; there is no separate rebind message to keep in step.
fn spawn_registry_watcher(pump_tid: u32) {
    std::thread::spawn(move || unsafe {
        let mut hkey = HKEY::default();
        let path: Vec<u16> = APP_KEY.encode_utf16().chain(std::iter::once(0)).collect();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(path.as_ptr()),
            0,
            KEY_NOTIFY,
            &mut hkey,
        )
        .is_err()
        {
            return;
        }
        let Ok(event) = CreateEventW(None, false, false, None) else { return };
        loop {
            if RegNotifyChangeKeyValue(hkey, false, REG_NOTIFY_CHANGE_LAST_SET, event, true)
                .is_err()
            {
                return;
            }
            WaitForSingleObject(event, u32::MAX);
            let _ = PostThreadMessageW(pump_tid, WM_RELOAD, WPARAM(0), LPARAM(0));
        }
    });
}

/// Exit once the UI is gone. Without this the broker would keep holding the
/// hotkeys after you quit TrontSnap from the tray, so PrtSc would go dead with
/// nothing visibly running to explain why.
///
/// The probe writes "ping", which the UI's acceptor ignores. It is deliberately
/// NOT a bare connection: the acceptor treats an unrecognised payload as
/// "surface the window", so a wordless health check would pop the gallery open
/// every few seconds.
fn spawn_ui_watchdog() {
    std::thread::spawn(|| {
        let mut misses = 0;
        loop {
            std::thread::sleep(Duration::from_secs(3));
            if send("ping") {
                misses = 0;
            } else {
                misses += 1;
                // Three strikes covers a UI restart (settings change, crash
                // self-heal) without tearing the broker down needlessly.
                if misses >= 3 {
                    std::process::exit(0);
                }
            }
        }
    });
}

fn main() {
    // Single instance, and the liveness beacon the UI probes. Held for the whole
    // process lifetime; dropping out here means another broker already owns the
    // hotkeys, which is a success case, not an error.
    let Ok(_beacon) = TcpListener::bind((Ipv4Addr::LOCALHOST, BROKER_PORT)) else {
        return;
    };

    let pump_tid = unsafe { GetCurrentThreadId() };
    spawn_registry_watcher(pump_tid);
    spawn_ui_watchdog();

    let mut binds = read_binds();
    register_all(&binds);

    // RegisterHotKey with a null HWND posts WM_HOTKEY to the REGISTERING THREAD,
    // so the pump has to live on this same thread as the registrations.
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(0), 0, 0).as_bool() {
            match msg.message {
                WM_HOTKEY => {
                    let cmd = match msg.wParam.0 as i32 {
                        ID_FULL => "full",
                        ID_REGION => "region",
                        ID_RECORD => "record",
                        _ => continue,
                    };
                    // If the UI is gone the watchdog will retire us shortly; drop
                    // this press rather than blocking the pump on a dead socket.
                    let _ = send(cmd);
                }
                WM_RELOAD => {
                    let next = read_binds();
                    if next != binds {
                        binds = next;
                        register_all(&binds);
                    }
                }
                WM_QUIT => break,
                _ => {}
            }
        }
        for id in [ID_FULL, ID_REGION, ID_RECORD] {
            let _ = UnregisterHotKey(HWND(0), id);
        }
    }
}
