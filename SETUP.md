# TrontSnap setup

TrontSnap is a portable single exe. There is no installer.

## Run it

Download `trontsnap.exe` from the latest release and run it. It drops into the
system tray and applies your hotkeys right away. Nothing is copied anywhere and
nothing asks for elevation.

Press `PrtSc` for a full screenshot, `Ctrl+PrtSc` for a freeze-frame region, or
`Ctrl+Shift+PrtSc` to record a clip. You can also left-click the tray icon.

## Start at login

Right-click the tray icon and toggle "Start at login" if you want TrontSnap
running automatically. This writes a normal per-user autostart entry; nothing
else on the system changes.

## Why it works over elevated windows

TrontSnap catches its hotkeys with `RegisterHotKey`, the same approach ShareX
uses, so PrtSc keeps firing even when an elevated window (Task Manager, an
admin terminal) has focus. It runs at normal (Medium) integrity the whole
time, it never elevates itself, and drag-out into other apps keeps working.

## PrintScreen setting

The first time it needs to, TrontSnap turns on
`PrintScreenKeyForSnippingEnabled`, a per-user Windows setting that routes the
PrtSc key to snipping-style tools instead of the OS default. It is the exact
setting ShareX turns on too, it only affects your user account, and it is
fully reversible.

## Uninstall

Untick "Start at login" in the tray menu if you had it on, quit TrontSnap from
the tray, then delete `trontsnap.exe`. That's the whole uninstall.

## Developing

`cargo run` / `cargo build` build and run the plain portable exe straight from
`target\`, no extra steps or feature flags needed.
