# Changelog

All notable changes to TrontSnap. Newest first.

## v0.4.2 (2026-07-14)

### Fixed
- **Region capture "stopped working" after entering it from the tray.** Root
  cause: winit's fullscreen-exit restored the repurposed main window to a bogus
  off-screen rect (observed live: `-21333,-21333 @ 107x19`, still visible and
  topmost) whenever region mode was entered while the window was hidden in the
  tray. Every subsequent `Ctrl+PrtSc` then fullscreened onto no on-screen
  monitor, so the picker was invisible. Plain `PrtSc` was unaffected because full
  capture never touches the window.
  - On region exit, hard-restore a known-good on-screen rect (and clear the
    lingering topmost bit) via Win32 `SetWindowPos`, applied a few frames after
    `Fullscreen(false)` so it lands last. The pre-region gallery rect is
    remembered so the window returns to the same spot with no jump.
  - On region entry, park the window on the primary monitor before going
    fullscreen, so the picker always covers a real monitor.
  - Drain the capture channel every frame so a fresh `Ctrl+PrtSc` always
    re-enters and kicks a stuck picker back on-screen instead of being swallowed.
  - Unit test guards the corrupt-rect discriminator (`rect_is_sane`).

### Added
- Toolbar **legend** for the per-thumbnail source dots: cyan = shot by TrontSnap,
  amber = imported from the ShareX archive.

## v0.4.1 (2026-07-13) — initial private release

- Tray-resident ShareX replacement in Rust + eframe/egui.
- Global hotkeys via a `WH_KEYBOARD_LL` hook (`PrtSc` = fullscreen,
  `Ctrl+PrtSc` = freeze-frame region), swallowing the key so Snipping Tool never
  also pops.
- In-process, flash-free region picker with a zoom loupe and a smart rect map
  (smallest window under the cursor wins; full-screen fallback).
- Virtualized gallery over the whole history: new TrontSnap shots on top, the
  ShareX archive (~17.5k) scrolling in below; LIFO thumbnail decode, disk-cached
  256px JPGs, filters, copy / reveal / delete.
- Multi-format clipboard (CF_DIB, CF_DIBV5, PNG, CF_HDROP) so paste lands in
  terminals, Claude, Explorer, Discord.
- Native OLE drag-out of thumbnails, live file-watch gallery refresh, synthesized
  shutter sound, corner toast, tray + autostart, single-instance.
