# TrontSnap setup

TrontSnap ships as **two builds off one codebase**. Both are fully supported; pick the
one that matches how you want to run it.

| | Portable | Installed |
|---|---|---|
| Get it running | download `trontsnap.exe`, run it | `Install TrontSnap.cmd`, one admin prompt |
| Admin needed | never | once, to write into Program Files |
| Fullscreen hotkey | `Alt+PrtSc` | `PrtSc` |
| Works over elevated windows | yes, on every bind | yes, on every bind |
| Uninstall | delete the exe | delete the folder + the Run key |

Everything else is identical: same features, same settings, same gallery. Screenshots
always live in `Pictures\TrontSnap` and settings in `HKCU\Software\TrontSnap`, so you can
switch between the builds without losing anything.

## Why two builds, and why the different default key

TrontSnap binds its hotkeys with `RegisterHotKey`. Windows will not deliver a
**modifier-less** hotkey (a bare `PrtSc`) to a normal Medium-integrity process while an
**elevated** window has focus. Combos *with* modifiers (`Ctrl+PrtSc`,
`Ctrl+Shift+PrtSc`, `Alt+PrtSc`) are delivered fine. That is a UIPI anti-keylogger rule,
not a bug in the app.

So:

- The **portable** build cannot be granted an exemption, so it defaults Fullscreen to
  `Alt+PrtSc`. Every bind then works everywhere, including over Task Manager and elevated
  terminals, with no install and no admin.
- The **installed** build is Authenticode-signed and placed in `%ProgramFiles%`, which is
  what lets Windows grant it **uiAccess**. uiAccess bypasses UIPI, so bare `PrtSc` works
  everywhere. The process still runs at Medium integrity, so drag-out into Discord and the
  browser keeps working.

Either way you can rebind all three hotkeys in **Settings > Hotkeys**.

## Install

Double-click **`packaging\Install TrontSnap.cmd`** and approve the one admin prompt.

It removes any portable copy in `%LOCALAPPDATA%\TrontSnap` (two copies would race for the
same hotkey registration and the loser dies silently), signs `trontsnap.exe`, installs it
to `%ProgramFiles%\TrontSnap`, points autostart there, and launches it. No reboot.

The launch at the end deliberately happens from the *non-elevated* window: Windows only
grants uiAccess when the exe is started at Medium integrity, so launching it from inside
the elevated installer would run it High and defeat the point. Open it later with
**`packaging\Launch TrontSnap.cmd`**, the tray icon, or just let it start on login.

Signing reuses the already-trusted TrontEQ dev cert if present, otherwise it generates and
trusts a machine-local one. The private key never ships.

## Developing

`cargo run` / `cargo build` produce the plain `asInvoker` portable binary, so normal dev
iteration works with no ceremony. uiAccess is opt-in behind a cargo feature:

```
cargo build --release                      # portable
cargo build --release --features uiaccess  # installed
```

A `uiAccess=true` exe cannot be launched by bare `cargo run`/CreateProcess (it fails with
`ERROR_ELEVATION_REQUIRED`, 740), and it only actually receives uiAccess when signed AND
in Program Files. That is exactly why the feature is off by default. `bootstrap.ps1`
refuses to install an exe whose manifest lacks `uiAccess="true"`, so you cannot
accidentally ship the portable binary through the installer path.

## Uninstall

Portable: delete the exe.

Installed: quit the tray app, delete `%ProgramFiles%\TrontSnap`, and remove the HKCU
`Software\Microsoft\Windows\CurrentVersion\Run` value named `TrontSnap`.
