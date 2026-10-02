# TrontSnap setup

TrontSnap ships as **two builds off one codebase**. Both are fully supported; pick the
one that matches how you want to run it.

| | Portable | Installed |
|---|---|---|
| Get it running | download `trontsnap.exe`, run it | `Install TrontSnap.cmd`, one admin prompt |
| Admin needed | never | once, to write into Program Files |
| Fullscreen hotkey | `PrtSc` | `PrtSc` |
| Bare `PrtSc` over elevated windows | requires the installed broker | yes |
| Uninstall | delete the exe | delete the folder + the Run key |

Everything else is identical: same features, same settings, same gallery. Screenshots
always live in `Pictures\TrontSnap` and settings in `HKCU\Software\TrontSnap`, so you can
switch between the builds without losing anything.

## Why the installed hotkey broker

TrontSnap binds its hotkeys with `RegisterHotKey`. Windows will not deliver a
**modifier-less** hotkey (a bare `PrtSc`) to a normal Medium-integrity process while an
**elevated** window has focus. Combos *with* modifiers (`Ctrl+PrtSc`,
`Ctrl+Shift+PrtSc`, `Alt+PrtSc`) are delivered fine. That is a UIPI anti-keylogger rule,
not a bug in the app.

So:

- The **portable** build defaults Fullscreen to `PrtSc`, just like the installed mode.
  Bare `PrtSc` cannot reach it while an elevated window has focus. Rebinding Fullscreen
  to a modifier combination such as `Alt+PrtSc` is an option in Settings.
- The **installed** mode keeps the gallery UI at Medium integrity and adds a tiny,
  Authenticode-signed hotkey broker in `%ProgramFiles%`. Windows grants **uiAccess** only
  to that broker. It owns `RegisterHotKey` and relays captures to the UI over loopback, so
  bare `PrtSc` works everywhere while drag-out from the UI into Discord and the browser
  keeps working.

Either way you can rebind all four hotkeys in **Settings > Hotkeys**.

## Install

Extract the Windows release bundle and double-click **`Install TrontSnap.cmd`**, or run
**`packaging\Install TrontSnap.cmd`** from a source checkout. Approve the one admin
prompt.

It removes any portable copy in `%LOCALAPPDATA%\TrontSnap` (two copies would race for the
same hotkey registration and the loser dies silently), signs the UI and hotkey broker,
installs both to `%ProgramFiles%\TrontSnap`, points autostart at the UI, and launches it.
No reboot.

The launch at the end deliberately happens from the *non-elevated* window so the gallery
UI stays at Medium integrity. The UI then launches the installed broker, whose signed
`uiAccess=true` manifest is what lets it receive bare hotkeys over elevated windows. Open
the UI later with **`packaging\Launch TrontSnap.cmd`**, the tray icon, or just let it start
on login.

Signing reuses the already-trusted TrontEQ dev cert if present, otherwise it generates and
trusts a machine-local one. The private key never ships.

## Developing

`cargo run` / `cargo build` produce the plain `asInvoker` UI binary, so normal dev
iteration works with no ceremony. The broker has its own manifest and package:

```
cargo build --release -p trontsnap          # portable UI, uiAccess=false
cargo build --release -p trontsnap-hotkeys  # broker, uiAccess=true
```

A `uiAccess=true` exe cannot be launched by bare `cargo run`/CreateProcess (it fails with
`ERROR_ELEVATION_REQUIRED`, 740), and it only receives uiAccess when signed and installed
in a secure location such as Program Files. `bootstrap.ps1` enforces the split: it rejects
a UI with `uiAccess=true`, rejects a broker without it, installs both, signs both, and
verifies both signatures before changing autostart.

## Verify an installed build

Run this for the install, signatures, hotkey broker, capture, and clipboard checks:

```
powershell -NoProfile -ExecutionPolicy Bypass -File .\verify-install.ps1
```

It deliberately takes a fullscreen capture and replaces the current clipboard contents.

Run this from an interactive, non-elevated desktop with the installed gallery open and
restored and its TrontSnap notification icon visible:

```
powershell -NoProfile -ExecutionPolicy Bypass -File .\verify-window-lifecycle.ps1
```

It physically moves and clicks the mouse to verify the custom minimize and close buttons,
real tray-icon double-click restore, and the Gallery, Settings, and About title-bar
controls. The script restores the original cursor position when it finishes.

## Uninstall

Portable: delete the exe.

Installed: quit the tray app, delete `%ProgramFiles%\TrontSnap`, and remove the HKCU
`Software\Microsoft\Windows\CurrentVersion\Run` value named `TrontSnap`.
