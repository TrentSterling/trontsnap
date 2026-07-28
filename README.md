# TrontSnap

Fast Windows screenshot tool with a lazy-loading gallery over your entire capture history. Built in Rust, raised on a decade of ShareX habits.

<!-- SHOTS: this README wants pictures of itself. Capture them with TrontSnap,
     drop the files into docs/media/ with the names in docs/media/SHOTLIST.md,
     then un-comment these lines. -->
<!-- ![The gallery, searchable, over the whole timeline](docs/media/hero.png) -->
<!-- ![Press PrtSc, the toast appears, drag it into Discord](docs/media/demo.gif) -->

## What it is

TrontSnap sits in the system tray, owns its global hotkeys, and copies every capture straight to the clipboard in every format a paste target might look for (terminals, Explorer, Discord/Slack, image editors). It also saves a PNG to `Pictures\TrontSnap`, records MP4 clips with system audio, reads the text out of any region of the screen, and ships an annotation editor with redaction that actually redacts.

## Your ShareX history comes with you

If you have an existing ShareX screenshot folder, TrontSnap merges it into the same timeline as its own captures: a decade of history, one scrolling gallery, nothing to convert, nothing abandoned. Search, OCR and the editor work across all of it, so switching costs you nothing.

### Key features

- **Global hotkeys that work even over elevated windows**: PrintScreen = fullscreen grab, Ctrl+PrintScreen = freeze-frame region picker, Ctrl+Shift+PrintScreen = record, Ctrl+Alt+PrintScreen = copy a region's TEXT. All four rebindable in Settings. TrontSnap uses `RegisterHotKey` (the same approach ShareX takes), and the installed build adds a tiny signed broker so even bare PrtSc keeps firing while Task Manager has focus. Bare PrintScreen is contested by Windows' own Snipping Tool binding since Win10 1809, so on first run TrontSnap frees it via the per-user `PrintScreenKeyForSnippingEnabled` registry flag, exactly like ShareX.
- **Dedicated GDI region picker**: a separate Win32/GDI overlay window, born already fullscreen with the frozen frame painted in, window detection, and a scroll-resizable zoom loupe. No GL context to warm, no flash, no borrowing the main window.
- **Region OCR**: drag a region and its text lands on the clipboard, via the OCR engine Windows already ships. Offline, no cloud, no model download. Also available as "Copy text (OCR)" on any screenshot in the gallery.
- **Annotation editor**: arrow, box, text, crop, and redaction that means it: solid black fill is the default and pixelation stays deliberately chunky, because a gaussian blur over a token can be reversed. Opens from the Edit chip on the capture toast or the gallery's right-click menu; saving overwrites the file and copies the result.
- **Recording**: region-picked H.264 MP4 via DXGI duplication + Media Foundation, hardware encoded, with WASAPI loopback audio. Export any clip to GIF.
- **Multi-format clipboard writer**: one atomic Open/Empty/SetClipboardData/Close session writes CF_DIB, CF_DIBV5 (with alpha), a registered "PNG" format, and CF_HDROP (the saved file itself) so pasting works everywhere, including apps that only accept a dropped/pasted file. Writes are queued on a single owner thread and retried, so a busy clipboard never loses a shot.
- **Virtualized history gallery**: a lazy-loading thumbnail grid over your whole timeline (new TrontSnap shots on top, legacy ShareX archive scrolling in below). Only visible cells decode, even at 18k+ shots. Search by filename, folder or date; Ctrl+click / Shift+click / Ctrl+A select a batch, Delete sends it to the Recycle Bin.
- **Theme engine**: 32 premade palettes, a custom accent picker, a randomizer, and a Discord-style gradient wash, all contrast-checked so text stays readable on any color. One-click swatch in the title bar.
- **Live file-watch refresh**: new captures splice into the gallery instantly via `notify`, no polling or manual refresh.
- **Native OLE drag-out**: drag a thumbnail (or the capture toast itself) straight into another app.
- **Corner toast**: the capture preview IS the toast; hover holds it open, click opens the file, drag sends it, Edit annotates it.
- **Synthesized shutter sound**: a short filtered-noise "camera click" is generated in code (no bundled WAV) and played async on every capture.
- **Run at login**: autostart via the HKCU Run key, launching hidden into the tray.
- **Single instance**: a second launch just pokes the running app to show its window.

## Install

Grab `trontsnap.exe` from the [latest release](https://github.com/TrentSterling/trontsnap/releases/latest): it is a portable single exe, no installer, no admin, no reboot. Run it and it lives in your tray.

The optional installed mode (`packaging/Install TrontSnap.cmd`) signs and places the hotkey broker so bare PrtSc also works while an elevated window has focus; `packaging/verify-install.ps1` checks the whole install with 22 assertions, and `Revert to portable.cmd` undoes it. See `SETUP.md` for details.

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
| `ocr` | One-shot region picker; the region's TEXT lands on the clipboard (Windows OCR, offline) |
| `edit <path>` | Annotation editor for an image (arrow/box/redact/pixelate/text/crop); save overwrites |
| `toast <path>` | Internal: shows the corner toast for a capture (spawned as its own process) |

Release builds matter here: `opt-level = 3`, LTO, single codegen unit, since the gallery decodes a lot of images.

## Tech stack

- Rust, `eframe`/`egui` for the gallery UI and the annotation editor
- `xcap` + `image` for screen capture and decoding, `ab_glyph` to bake editor text into the saved PNG
- `windows` crate for direct Win32 and WinRT: RegisterHotKey, GDI region overlay, clipboard formats (CF_DIB/CF_DIBV5/PNG/CF_HDROP/CF_UNICODETEXT), DXGI duplication + Media Foundation recording, `Windows.Media.Ocr`
- `tray-icon` for the system tray, `winreg` for the autostart registry entry
- `notify` for the live capture file watcher, `walkdir` for the initial history scan
- `drag` for native OLE drag-out, `trash`/`opener` for file ops
- `crossbeam-channel` for cross-thread event plumbing

See `CHANGELOG.md` for the version history.
