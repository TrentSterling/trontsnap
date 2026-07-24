# TrontSnap

Fast Windows screenshot tool with a lazy-loading gallery over your entire capture history. Built in Rust, raised on a decade of ShareX habits.

## What it is

TrontSnap sits in the system tray, owns its global hotkeys, and copies every capture straight to the clipboard in every format a paste target might look for (terminals, Explorer, Discord/Slack, image editors). It also saves a PNG to `Pictures\TrontSnap` and, if you have a ShareX screenshot folder, shows both histories merged into one scrolling timeline.

### Key features

- **Global hotkeys that work even over elevated windows**: PrintScreen = fullscreen grab, Ctrl+PrintScreen = freeze-frame region picker, Ctrl+Shift+PrintScreen = record. TrontSnap uses `RegisterHotKey` (the same approach ShareX takes), which keeps firing even when an elevated window like Task Manager has focus, without the app itself needing any elevation. Bare PrintScreen is contested by Windows' own Snipping Tool binding since Win10 1809, so on first run TrontSnap frees it via the per-user `PrintScreenKeyForSnippingEnabled` registry flag, exactly like ShareX.
- **Dedicated GDI region picker**: a separate Win32/GDI overlay window, born already fullscreen with the frozen frame painted in, the same approach ShareX uses. No GL context to warm, no flash, no borrowing the main window.
- **Multi-format clipboard writer**: one atomic Open/Empty/SetClipboardData/Close session writes CF_DIB, CF_DIBV5 (with alpha), a registered "PNG" format, and CF_HDROP (the saved file itself) so pasting works everywhere, including dropping the file into apps that only accept a dropped/pasted file.
- **Virtualized history gallery**: a lazy-loading thumbnail grid over your whole timeline (new TrontSnap shots on top, legacy ShareX archive scrolling in below). Only visible cells decode; a LIFO job queue means the current viewport always fills in first, even at 17k+ shots.
- **Live file-watch refresh**: new captures splice into the gallery instantly via `notify`, no polling or manual refresh.
- **Native OLE drag-out**: drag a thumbnail straight out of the gallery into another app.
- **Corner toast**: a small themed always-on-top notification per capture (thumbnail + "Copied to clipboard"), click to open the file, spawned as its own tiny process so it never touches the main app's loop.
- **Synthesized shutter sound**: a short filtered-noise "camera click" is generated in code (no bundled WAV) and played async on every capture.
- **Run at login**: autostart via the HKCU Run key, launching hidden into the tray.
- **Single instance**: a second launch just pokes the running app to show its window.

## Build / run

```
cargo build --release
```

Run modes (first CLI arg):

| Arg | Behavior |
|-----|----------|
| *(none)* / `app` | Persistent tray app: hotkeys + gallery window shown |
| `--startup` / `tray` | Same, but starts hidden in the tray (used by the autostart entry) |
| `region` | One-shot freeze-frame region picker, deliver, exit |
| `full` | One-shot fullscreen grab, deliver, exit |
| `toast <path>` | Internal: shows the corner toast for a capture (spawned as its own process) |

Release builds matter here: `opt-level = 3`, LTO, single codegen unit, since the gallery decodes a lot of images.

## Tech stack

- Rust, `eframe`/`egui` for the gallery UI
- `xcap` + `image` for screen capture and decoding
- `windows` crate for direct Win32: low-level keyboard hook, GDI region overlay, clipboard formats (CF_DIB/CF_DIBV5/PNG/CF_HDROP), DWM window enumeration, winmm shutter playback
- `tray-icon` for the system tray, `winreg` for the autostart registry entry
- `notify` for the live capture file watcher, `walkdir` for the initial history scan
- `drag` for native OLE drag-out, `trash`/`opener` for file ops
- `crossbeam-channel` for cross-thread event plumbing

See `CHANGELOG.md` for the version history.
