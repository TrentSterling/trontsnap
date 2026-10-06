# TrontSnap on Linux

The Linux port (first landed 2026-10-05, branch `linux-port`). Same binary idea
as Windows: one portable executable, no installer. The UI layer (gallery, theme
engine, editor, toast) is shared code; the OS layer lives in `src/linux/` and is
selected at compile time (see the module table in `src/main.rs`).

## Build

```
cargo build --release
./packaging/linux/appimage.sh        # -> target/TrontSnap-vX.Y.Z-x86_64.AppImage
```

Build deps beyond Rust: the usual eframe set (`libgtk-3-dev` is NOT needed;
`libxcb`/`libxkbcommon`/`libssl-dev` usually already present on a dev box).

## Runtime dependencies (small, deliberate shell-outs)

| Tool | For | Install |
|---|---|---|
| `wl-clipboard` (Wayland) or `xclip` (X11) | clipboard writes that outlive the one-shot capture process | `sudo apt install wl-clipboard` |
| `gnome-screenshot` / `spectacle` / `grim` | screen grab fallback when xcap can't capture your compositor | usually preinstalled |
| `ffmpeg` | recording, video thumbnails, GIF export | `sudo apt install ffmpeg` |
| `tesseract-ocr` | region OCR (optional) | `sudo apt install tesseract-ocr` |
| `paplay` / `pw-play` / `aplay` | shutter sound | preinstalled everywhere |

## What works (verified on GNOME Wayland, Ubuntu)

- `trontsnap` — the full app: gallery over your whole history (ShareX archive
  merge included), search, themes, annotation editor, settings
- `trontsnap full` — one-shot fullscreen grab → PNG + clipboard + shutter + toast
- `trontsnap region` / `ocr` — freeze-frame picker (drag a region, click a
  window where enumeration works, Esc cancels)
- Live gallery splice via inotify, capture toast with Edit chip
- Autostart via `~/.config/autostart/trontsnap.desktop`
- Settings persist to `~/.config/trontsnap/settings.conf`
- Clipboard: `image/png` for shots, `text/uri-list` for recordings, plain text
  for OCR — all survive the capturing process exiting

## Hotkeys

On GNOME the app installs four custom keybindings on first app launch (and
clears GNOME's own bare-Print screenshot UI binding, same move as freeing the
key from Snipping Tool on Windows):

| Keys | Runs |
|---|---|
| `Print` | `trontsnap full` |
| `Ctrl+Print` | `trontsnap region` |
| `Ctrl+Shift+Print` | `echo -n record > /dev/tcp/127.0.0.1:48761` (toggles the running app's recorder) |
| `Ctrl+Alt+Print` | `trontsnap ocr` |

Other desktops: bind those same commands in your DE's keyboard settings.
The one-shots don't need the app running.

## Recording

X11 sessions: ffmpeg x11grab, full screen, 30fps, libx264 ultrafast (+ Pulse
audio when "Record audio" is on). Wayland: not wired yet — drop a
`~/.config/trontsnap/record-start.sh` that records to `$1` (and optionally a
`record-stop.sh`); TrontSnap handles start/stop/clipboard/toast around it.

## Known gaps vs Windows (telling the truth here, in rough priority order)

1. No tray icon (needs a gtk loop beside winit) — the window stays open; Close quits
2. No drag-out from gallery/toast (no XDND backend yet) — paste covers most of it
3. Recording is full-screen only, and Wayland needs the script hook (portal/PipeWire is the real fix)
4. No loupe in the region picker yet, and no per-key rebinding (the Settings hotkey UI edits Windows VK codes)
5. Multi-monitor untested (same primary-monitor scope as Windows)
6. Toast docks wherever the Wayland compositor puts it (bottom-right positioning works on X11 only)
7. A second `trontsnap` launch surfaces the window if it's open, but can't un-minimize it on Wayland (no xdg-shell unminimize) — restore it from the dock

