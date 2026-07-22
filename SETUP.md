# TrontSnap setup

## Install (recommended)

Double-click **`packaging\Install TrontSnap.cmd`**, approve the one admin prompt.

It signs `trontsnap.exe`, installs it to `%ProgramFiles%\TrontSnap`, points autostart
there, and launches it. TrontSnap then starts on every login; open it any time with
**`packaging\Launch TrontSnap.cmd`** or the tray icon.

No reboot. One UAC (just to write into Program Files).

## Why an installer at all?

TrontSnap catches PrtSc with a global keyboard hook. Windows blocks a normal
(Medium-integrity) process's hook from seeing keystrokes sent to an **elevated** window
(like TrontEQ) — an anti-keylogger rule. The fix is **uiAccess**: TrontSnap stays a
normal Medium-integrity app (so drag-out into Discord/browser keeps working and it never
prompts UAC at runtime), but Windows exempts it from that hook restriction. This is the
same mechanism AutoHotkey's uiAccess build uses.

Windows only grants uiAccess to an exe that is **Authenticode-signed** and lives in a
**secure location** (`%ProgramFiles%`). That's the whole job of the installer: sign +
copy there. It reuses the already-trusted TrontEQ dev cert if present, otherwise it
generates and trusts a machine-local one. The private key never ships.

## Developing

`cargo run` / `cargo build` build a plain `asInvoker` binary (no uiAccess) that runs
straight from `target\`, so normal dev iteration works. uiAccess is opt-in behind the
`uiaccess` cargo feature and only the installer turns it on:

```
cargo build --release --features uiaccess
```

A `uiAccess=true` exe cannot be launched by bare `cargo run`/CreateProcess (it fails with
`ERROR_ELEVATION_REQUIRED`), and it only gets uiAccess when signed + in Program Files —
which is exactly why the feature is off by default.

## Uninstall

Delete the HKCU `...\Run` value `TrontSnap`, remove `%ProgramFiles%\TrontSnap`, and quit
the tray app.
