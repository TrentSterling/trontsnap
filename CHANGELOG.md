# Changelog

All notable changes to TrontSnap. Newest first.

## v0.5.0 (2026-07-14)

### Changed (architecture)
- **Region capture is now a dedicated Win32 + GDI fullscreen overlay** (new
  `region_win32.rs`), the ShareX approach, instead of repurposing the app's own
  eframe/GL window. Each `Ctrl+PrtSc` spawns a thread that grabs the screen,
  enumerates targets, and puts up a separate borderless window *born already
  fullscreen* with the frozen frame painted in: GDI double-buffered, force-
  foregrounded (attach-thread-input) so keyboard works regardless of focus,
  blocking-modal, returning the crop. The gallery window is never touched.

### Fixed (all consequences of no longer repurposing the main window)
- No visible maximize / grow into the overlay — it's born fullscreen, appears instantly.
- No black flash on close/cancel — the overlay window is just destroyed; the gallery
  is never resized, restored, or refocused.
- No focus-dependent behavior — the overlay is independent of the gallery, and force-
  foreground guarantees Esc / clicks land regardless of which app was active.
- Phantom drag on entry (the v0.4.3 regression where the picker rubber-banded a
  rectangle before you clicked) is gone: input is driven by real
  `WM_LBUTTONDOWN` / `WM_LBUTTONUP` messages, not raw pointer-down polling.

### Removed
- The entire in-process repurpose machinery: arming, deferred geometry restore,
  off-screen rescue, coverage gate, and the egui `RegionPicker`. `overlay.rs` is now
  just window enumeration, consumed by the GDI picker.

## v0.4.4 (2026-07-14)

### Fixed
- **You could see the window visibly maximize into the overlay, with a delay.** The
  entry sequence showed the window first and then went fullscreen, so the fullscreen
  transition animated on-screen. Now the window is configured to fullscreen *while
  hidden* and only revealed once it is already full-size (a short "arm" countdown),
  so the overlay appears born-fullscreen with no grow animation. This is the
  repurpose-window approximation of the dedicated born-fullscreen overlay window
  ShareX uses.

## v0.4.3 (2026-07-14)

### Fixed
- **Region overlay was confined to a small window when TrontSnap was minimized to
  the taskbar.** The entry sequence never un-minimized the window, so
  `Fullscreen(true)` didn't stick and the picker painted the frozen frame scaled
  into the normal-sized window. Now sends `Minimized(false)` before fullscreen.
- **A quick drag immediately after `Ctrl+PrtSc` flickered and failed** (a slow drag
  worked). `Fullscreen(true)` lands a frame or two after the picker's first frame,
  so early drag coordinates were measured against the pre-fullscreen (small) window
  rect and then jumped. The picker now gates all selection input until the overlay
  actually covers the captured monitor (60-frame safety valve so input is never
  trapped), and drag tracking is manual (press / move / release) so a drag begun
  mid-transition re-anchors cleanly instead of flickering.

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
