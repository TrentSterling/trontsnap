//! Launching child processes in a way that survives the uiAccess build.
//!
//! A process holding the uiAccess token flag CANNOT start anything with a bare
//! CreateProcess: it fails with ERROR_ELEVATION_REQUIRED (740). That takes out
//! `std::process::Command`, which is how the corner toast and "Reveal in
//! Explorer" used to spawn. ShellExecuteW goes through the shell instead and
//! works from both builds.
//!
//! Used UNCONDITIONALLY rather than cfg'd on the `uiaccess` feature, on purpose:
//! one code path means the portable build exercises the same thing the installed
//! build ships, so this cannot silently rot in whichever configuration nobody
//! ran today.
//!
//! THIS HAS REGRESSED ONCE ALREADY. v0.9.0 hit it and switched to ShellExecuteW;
//! v0.10 dropped uiAccess and "simplified" it back to Command; v0.14.0 brought
//! uiAccess back and re-broke both call sites. Do not revert it again just
//! because the portable build works without it.

#![cfg(windows)]

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Fire-and-forget launch of `file` with the raw parameter string `params`.
///
/// Returns false if the shell refused. ShellExecuteW signals success with a
/// return value ABOVE 32; anything <= 32 is an error code, which is why this is
/// not the usual zero-is-failure test.
///
/// Callers are best-effort by design: a capture is already saved and on the
/// clipboard before any of this runs, so a failed toast never loses work.
pub fn run(file: &str, params: &str) -> bool {
    let f = wide(file);
    let p = wide(params);
    let op = wide("open");
    let rc = unsafe {
        ShellExecuteW(
            HWND(0),
            PCWSTR(op.as_ptr()),
            PCWSTR(f.as_ptr()),
            PCWSTR(p.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    rc.0 as isize > 32
}
