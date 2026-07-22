# Changelog

All notable changes to TrontSnap. Newest first.

## v0.11.0 (2026-07-22)

### Added
- **Tabbed UI with custom, frameless window chrome.** The single gallery window is
  now four tabs (Gallery / Capture / Settings / About) under a hand-drawn title bar:
  wordmark + tab strip + minimize/maximize/close, since `.with_decorations(false)`
  drops the OS chrome entirely. Window move is a drag on the title bar's empty space
  (double-click to maximize); resize is 8 thin interact strips along the edges and
  corners (`ViewportCommand::StartDrag` / `BeginResize`), since decorations-off also
  removes the OS resize border.
- **Capture tab**: three big buttons (Fullscreen / Region / Record) that trigger the
  exact same paths as the hotkeys. They're a universal fallback for windows that
  block a bare PrintScreen key: elevated apps, fullscreen games, anything eating
  input before the OS hotkey ever fires. A mouse click on this tab, or the tray icon,
  always lands.
- **Settings tab**: the same four toggles the tray menu already had (capture cursor,
  record audio, start at login), plus a new one, "Use the PrintScreen key" — flips
  the per-user `PrintScreenKeyForSnippingEnabled` registry flag live, so you can hand
  PrintScreen back to Windows' Snipping Tool without editing the registry yourself.
- **About tab**: replaces the old idea of a fixed-size About window (the kind that
  breaks under zoom/resize) with a scrollable, width-clamped tab that always stays
  readable. Explains the hotkeys, the Capture menu, the elevated-window edge case, and
  that it's a portable single exe; links to tront.xyz and the GitHub repo. Auto-opens
  on first launch (and stays on until you turn off "Show this tab when TrontSnap
  starts").

### Rebindable hotkeys
- **The three global hotkeys are now rebindable.** Settings has a Hotkeys section
  with a row per action (Fullscreen / Region / Record): Ctrl/Shift/Alt/Win
  checkboxes plus a key dropdown (PrintScreen, Pause, Insert, Home/End,
  PageUp/Down, F1-F12). Changes apply instantly (the pump thread re-registers
  live, no restart) and there's a "Reset hotkeys to defaults" button. Binds
  persist in `HKCU\Software\TrontSnap` alongside the other settings.

### Region loupe
- **Scrollwheel-resizable magnifier.** During a region capture, scroll the wheel
  to grow or shrink the pixel loupe (96-528px, 8x zoom held constant so a bigger
  box shows more area). The size persists across captures and restarts, and it's
  also a slider in Settings > Region picker.

### UI: one clean chrome, sectioned Settings, focused About
- **Single-row chrome.** Icon + wordmark + Capture menu + Gallery/Settings/About
  tabs + (on Gallery) the filter chips / shot count / source legend inline + the
  window buttons, all in ONE row. No second header strip, no bottom bar, no
  duplicate "TrontSnap".
- **Uniform tab strip.** Capture is a frameless quick-menu (Fullscreen / Region /
  Record) that matches the tabs instead of a boxed accent button; even spacing
  across the whole nav.
- **Painter-drawn window buttons** (crisp min/max/restore/close lines and rects,
  never a tofu glyph; close hovers reddish), and **chrome text is not selectable**.
- **Sectioned Settings**: Capture (cursor, PrintScreen key), Recording (audio),
  Region picker (loupe-size slider), Hotkeys (rebind rows), Startup (start at
  login) - each under an accent header, in a scroll area.
- **Focused About**: hero (icon + name + tagline), a two-tone Shortcuts list, and
  a short author footer. Dropped the "over elevated windows" plumbing and other
  filler.
- **Gallery grid fills and centers** (accounts for the reserved scrollbar width);
  the vertical scrollbar stays on the right, thin and themed.
- **Richer thumbnail hover tooltip**: filename, full path, capture date/time,
  pixel dimensions (decoded lazily, only on hover), file size, and source; plus
  "Copy path" in the right-click menu. The tooltip no longer clips at the window
  edge.

### Contrast + rendering fixes
- **Smart contrast on selected chips.** A selected tab/chip sits on the bright
  cyan accent fill, so its text is dark for readability; every other state (rest,
  hover, and the pressed state that was the old invisible-text bug) stays light on
  the dark panel. No dark-on-dark, no washed-out light-on-cyan.
- **Emoji purged from rendered text.** egui's bundled font only covers a tiny
  emoji subset, so the `Copied` check-glyph and the right-click menu icons
  rendered as tofu boxes; all replaced with plain text.
- **UI zoom is clamped** to a usable range (Ctrl+scroll / Ctrl +/-) so extreme
  zoom no longer thrashes the virtualized grid or resizes the OS window.
- **First run opens on About once**, then every launch after opens on Gallery
  unless you tick "Show this tab when TrontSnap starts".

## v0.10.0 (2026-07-22)

### Changed
- **Portable rework: TrontSnap is a plain, unsigned, single exe again.** Switched
  the global hotkeys from a `WH_KEYBOARD_LL` keyboard hook to `RegisterHotKey`,
  the same mechanism ShareX uses: it frees the PrintScreen key from Windows'
  own "PrintScreen opens Snipping Tool" binding via the per-user, no-admin
  registry flag `PrintScreenKeyForSnippingEnabled` (set automatically at
  startup, no admin prompt), then registers PrtSc / Ctrl+PrtSc / Ctrl+Shift+PrtSc
  directly. RegisterHotKey is never UIPI-blocked, so it still fires over
  elevated windows (TrontEQ, Task Manager, elevated terminals) at plain Medium
  integrity: no uiAccess manifest needed.
- **Dropped the uiAccess manifest, code-signing, and the Program Files
  installer entirely.** There's no `uiaccess` cargo feature anymore, no signed
  build, no `bootstrap.ps1` install step: TrontSnap is now a portable single exe
  you can run from anywhere and uninstall by deleting the file.

## v0.9.0 (2026-07-22)

### Added
- **Hotkeys now fire over elevated windows** (TrontEQ, Task Manager, elevated
  terminals). Previously PrtSc silently did nothing while an admin window had focus:
  a Medium-integrity `WH_KEYBOARD_LL` hook is blocked by Windows UIPI from seeing
  keystrokes headed to a higher-integrity window. Fix = a **uiAccess** build, the same
  trick AutoHotkey's uiAccess build uses: TrontSnap stays Medium integrity (so drag-out
  keeps working and there's no per-launch UAC) but is UIPI-exempt for input. Needs a
  signed exe in a secure location, so there's a one-time installer.
- **One-click installer** (`Install TrontSnap.cmd` + `bootstrap.ps1`): signs the exe and
  installs it to `%ProgramFiles%\TrontSnap`, points autostart there, and launches it at
  Medium so Windows grants uiAccess. One UAC, no reboot. `Launch TrontSnap.cmd` opens it
  any time (it also starts on login). Dev builds (`cargo run`) stay plain asInvoker via
  the opt-in `uiaccess` cargo feature.
- **Distinct rainbow icon.** TrontSnap's icon (tray + window + exe) is now the face inside
  a rainbow capture-frame, so it no longer twins with TrontEQ's bare face.

### Fixed
- The tray "Record Region" item was a dead no-op (its menu id was never handled); it now
  toggles recording, matching the Ctrl+Shift+PrtSc hotkey.

## v0.8.0 (2026-07-16)

### Added
- **Recordings now capture system audio** (new tray toggle "Record audio", on by
  default, persisted like the other settings). WASAPI loopback of the default output
  device — "record what you hear" — converted to 16-bit PCM and encoded as AAC 192kbps
  into the same MP4 by the SinkWriter. Two classic loopback traps handled: a silence
  keep-alive render stream (loopback produces no packets when nothing is playing, which
  would hole the timeline), and sample-counter timestamps anchored to the recording
  epoch so A/V stay aligned. Any audio failure (odd device rate, no endpoint) logs and
  records video-only — audio can never lose a recording. The A/V mux path is covered by
  a headless smoke test (synthesized frames + sine -> encode -> finalize -> reopen,
  assert both tracks).
- **Export GIF**: right-click any recording in the gallery -> "Export GIF". Decodes via
  the Media Foundation reader, samples to ~12fps, caps width at 800px, quantizes with
  the image crate's GIF encoder (no ffmpeg). Runs in the background with a status line;
  the .gif lands next to the MP4 and pops into the timeline via the live watcher.
  Measured on a real ~13s clip: ~35s export, 778x502, 158 frames.

### Known limitations
- Audio requires the output device at 44.1/48kHz (Windows default; exotic rates record
  video-only). Microphone mix-in is a separate follow-up.
- GIF export is CPU-bound (~2-3x clip length); big/long clips make big GIFs.

## v0.7.3 (2026-07-16)

### Fixed
- **The recording HUD ate mouse hover + scroll wheel over the recorded area.**
  Windows routes hover and the "scroll the window under the cursor" wheel by
  hit-testing, and a layered color-key window still hit-tests as SOLID over its
  transparent pixels on Win11 (the documented pass-through did not hold). The HUD
  window now carries a window region (`SetWindowRgn`) shaped as exactly the frame
  ring + the REC tab, so the interior is structurally not part of the window:
  hover, wheel, and clicks inside the recorded region go straight to the app being
  recorded. Clicking the red frame or the tab still stops the recording.

## v0.7.2 (2026-07-15)

### Added
- **Real video thumbnails.** Gallery MP4 tiles now show the recording's first frame
  (with a small play badge) instead of the generic film plate, and the capture toast
  shows it too. Decoding is Media Foundation's Source Reader — the read-side twin of
  the recorder's SinkWriter, so still no ffmpeg and no new dependencies. Verified
  against a real recording (orientation + colors correct, cursor visible in-frame).
- Thumbs for a **recording still in progress** show the film plate, then swap to the
  real first frame when the file finalizes: the watcher's mtime refresh drops the
  stale texture and the mtime-keyed disk cache re-decodes automatically.

## v0.7.1 (2026-07-15)

### Changed
- **Recording HUD: the REC pill grew into a flashing red outline around the recorded
  region** (Trent's ask), with an attached "● REC · stop" tab. One layered color-key
  window: the interior is genuinely click-through (clicks land on whatever you're
  recording), the red frame and the tab are clickable — click either to stop.
- **The HUD never appears in the recording.** The window is marked
  `WDA_EXCLUDEFROMCAPTURE` (the OBS trick): visible on your monitor, invisible to
  DXGI duplication — and to screenshots. This also means the tab can safely sit
  inside the region on fullscreen records.

### Note
- v0.7.0's "Ctrl+Shift+PrtSc takes a normal picture" report was the stale v0.5.8
  binary still running (the release exe can't be replaced while the app is live) —
  not a hotkey bug.

## v0.7.0 (2026-07-15)

### Added
- **Screen recording: `Ctrl+Shift+PrtSc`** (also in the tray menu). First press runs
  the same freeze-frame region picker as screenshots (click a window or drag a rect),
  then recording starts; press again — or click the little "● REC" pill — to stop.
  - **Capture:** DXGI Desktop Duplication (GPU frame grabs, ~1ms) cropped to the
    region, 30fps constant frame rate (a static screen encodes duplicated frames).
  - **Encode:** Media Foundation `IMFSinkWriter` → H.264 MP4 straight to
    `Pictures\TrontSnap` (hardware encoder via NVENC when available). No ffmpeg, no
    new dependencies.
  - **Cursor:** composited per frame with the same GDI `DrawIconEx` path as v0.6.0
    stills, honoring the "Capture cursor" toggle (hotspot cached per cursor handle).
  - **REC pill:** a tiny topmost, non-activating indicator parked just outside the
    region's top-right corner (inside if the region touches the screen edge, where it
    will appear in the recording). Blinks; click to stop.
  - **Delivery:** finished MP4 is put on the clipboard as a file (`CF_HDROP` only), so
    it pastes into Discord/Explorer/terminals; shutter plays; the corner toast says
    "Recording saved" (click to play).
- **Gallery understands videos** (and now also `.gif`): MP4s ride the same timeline as
  stills — drawn as a film tile (accent play triangle, MP4 tag) without ever touching
  the image decoder. Click = copy file, double-click = open in your player, drag-out /
  reveal / delete all work. The live watcher surfaces a recording the moment its file
  appears.

### Known limitations (logged for later)
- Video only — no audio track yet (WASAPI loopback is the follow-up).
- Quitting TrontSnap mid-recording abandons the file un-finalized (unplayable);
  stop first. A fragmented-MP4 mode would fix this properly.
- Primary monitor only, same as stills.

## v0.6.0 (2026-07-15)

### Added
- **Capture the mouse cursor** (new tray toggle "Capture cursor", on by default).
  xcap grabs the screen without the pointer, so both PrtSc and Ctrl+PrtSc were
  silently dropping the cursor. We now composite the live system cursor into the frame
  via GDI `DrawIconEx`, which renders every cursor style correctly (color arrows with
  per-pixel alpha, the monochrome I-beam's AND/XOR mask, link hands, resize arrows).
  Region capture freezes the pointer where it was at Ctrl+PrtSc time, so it lands in
  the crop — the ShareX behaviour. The setting persists in the registry (same
  `HKCU\Software\TrontSnap` key autostart uses).

### Fixed
- **Gallery occasionally restored from the tray/taskbar at a tiny size** (a
  winit/eframe restore desync; it snapped back only after a manual resize). Added a
  debounced self-heal: winit never lets you drag the content below `min_inner_size`, so
  a visible sub-minimum size is provably the desync — it's detected and the last
  known-good size is re-asserted automatically (and logged, to catch the trigger if it
  recurs).

## v0.5.8 (2026-07-15)

### Fixed
- **PrtSc / Ctrl+PrtSc did nothing while the TrontSnap window itself was focused**
  (worked fine from any other window). Instrumented the hook and confirmed the cause:
  Windows does not invoke a global `WH_KEYBOARD_LL` hook for PrintScreen when the
  keystroke is destined for the hook owner's *own* foreground window. Added a
  thread-scoped `WH_KEYBOARD` hook on the UI thread as a fallback: it fires exactly
  when one of TrontSnap's windows has focus (debounced; fires on any snapshot event,
  since PrtSc may deliver only a key-up to a focused app). The global hook covers
  every other case, so the two never double-fire.

## v0.5.7 (2026-07-15)

### Fixed
- **Toast "click to open" didn't work.** egui's click handling doesn't fire on the
  toast's non-activating, always-on-top window. It now detects the click via Win32
  (physical mouse button + cursor-over-window rect test), bypassing egui entirely.

## v0.5.6 (2026-07-15)

### Fixed
- **Toast "click to open" didn't work.** The full-window click target was added
  *before* the thumbnail and labels, so in egui's hit-testing those sat on top of it;
  clicking the (large, obvious) thumbnail hit its hover region and the click never
  reached the handler. The click target is now added last, on top of everything, so a
  click anywhere in the toast opens the capture. Open errors are logged instead of
  silently swallowed.

## v0.5.5 (2026-07-15)

### Changed
- **Removed the manual "Refresh" button** from the gallery toolbar. The live file
  watcher already splices new captures in automatically, so it was redundant.
  `start_scan()` still runs once on launch for the initial load.

## v0.5.4 (2026-07-15)

### Fixed
- **Root cause behind all the tray flakiness: eframe never runs `update()` while the
  window is hidden to the tray.** So anything routed through `update()` (a tray
  left-click region capture, a menu capture, showing the window) silently *queued*
  until the window happened to reappear, then flushed at once. Fix: every tray/menu
  action is now performed **directly in its event handler** (which fires on the UI
  thread when dispatched), not via `update()`:
  - Tray left-click → region capture fires instantly, even while hidden.
  - Menu Open → Win32 restore (cached HWND); Capture Fullscreen/Region → run directly;
    Quit → exits directly.
  - Only the autostart toggle still goes through `update()` (it needs `&self`; applies
    on the next tick).

## v0.5.3 (2026-07-15)

### Fixed
- **Tray "Open TrontSnap" / restore-from-minimized still didn't bring the window
  back** (v0.5.2 woke the loop, but `show()` itself was the problem). `Visible(true)`
  leaves a tray-hidden window stuck behind the focused app (Windows foreground lock)
  and does nothing at all for a *minimized* window (it's iconic, not hidden). `show()`
  now does a real Win32 restore: `ShowWindow(SW_RESTORE)` (un-hides AND un-minimizes)
  plus an attach-thread-input force-foreground so the gallery actually pops to front.

## v0.5.2 (2026-07-15)

### Fixed
- **Tray "Open TrontSnap" did not re-show the window.** The menu popped, but the
  window stayed hidden. Cause: eframe does not reliably call `update()` on the idle
  repaint timer while the window is hidden to the tray, so the menu event sat in its
  global channel unread until some unrelated event woke the loop. Fix: forward tray +
  menu events through our own channels via `set_event_handler` and call
  `request_repaint()` on each (the same wake the single-instance acceptor uses), so
  tray interaction is processed immediately.

### Changed
- **Tray icon: left-click now starts a region capture; right-click opens the menu**
  (`with_menu_on_left_click(false)`). Previously double-click re-showed the window;
  use the right-click menu's "Open TrontSnap" for that now.

## v0.5.1 (2026-07-14)

### Fixed
- **Region picker cursor flickered to the wait/hourglass spinner**, making rect
  selection fiddly. Two causes: while the overlay holds `SetCapture`, Windows stops
  sending `WM_SETCURSOR` so the crosshair class-cursor never applied; and the
  per-mouse-move paint re-darkened the whole screen and re-stroked every window rect,
  making the thread look busy. Fixes: force `SetCursor(crosshair)` on capture and on
  every `WM_MOUSEMOVE`; and pre-compose the static layer (dim backdrop + faint rect
  map) once so each frame only blits the base plus the small dynamic bits. Crosshair
  now stays put and painting is cheaper.

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
