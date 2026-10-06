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
| `xclip` | text clipboard writes (OCR, copied paths) and the fallback for everything else; used on X11 AND on GNOME Wayland (via XWayland, which Mutter bridges) | `sudo apt install xclip` |
| `wl-clipboard` | only for wlroots/KDE compositors that implement data-control; GNOME has none, so wl-copy there is not used when xclip exists | `sudo apt install wl-clipboard` |
| `gnome-screenshot` / `spectacle` / `grim` | screen grab fallback when xcap can't capture your compositor | usually preinstalled |
| `ffmpeg` | recording, video thumbnails, GIF export | `sudo apt install ffmpeg` |
| `tesseract-ocr` | region OCR (`Ctrl+Alt+Print`, gallery "Copy text"). **Not preinstalled on Ubuntu**: without it the hotkey shows an "OCR unavailable" notification instead of a picker | `sudo apt install tesseract-ocr` |
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
- Clipboard, all of it surviving the capturing process exiting:
  - shots: `image/png` + the saved file (`text/uri-list` + `x-special/gnome-copied-files`),
    so Firefox/file-manager-style destinations get a real named file; no text flavor
    on purpose (a path next to the picture could win in a rich-text field — unverified
    either way, so we kept the safe side)
  - recordings and gallery "Copy N files": exactly what Nautilus offers on Ctrl+C
    (uri-list, gnome-copied-files, bare-path text), so terminals paste the path too
  - OCR text and "Copy path(s)": plain text via xclip
  - URIs are RFC 3986 percent-encoded (a `#` or `%20` in a name used to break paste)
- Gallery: click = copy, Ctrl+click / Shift+click = select, Ctrl+A, Delete, and
  **Ctrl+C copies the selection** (1 = the image, N = the files). Details below.

### Gallery behaviour worth knowing

- **Selection.** Ctrl+click toggles, Shift+click REPLACES the selection with the range from the
  anchor (Ctrl+Shift+click adds it), Ctrl+A selects what is shown, Esc clears. While something is
  selected a plain click toggles instead of copying (no way to select with the mouse alone
  otherwise). Selections hidden by a search/filter stay selected and show as `+K hidden`.
  Esc in the search box only leaves the box. Ctrl+click twice quickly toggles back off; it no
  longer Opens the file.
- **Deleting** moves files to the XDG Trash ("Recycle Bin" on Windows) on a worker thread, 200 per
  call, with `Moving to the Trash: 400/1500` progress. A tile leaves only once its file is really gone;
  a path that vanished meanwhile is dropped silently, a file that cannot be trashed is retried alone and
  counted in `N failed (see log)`. The folder watcher forwards deletes/moves-out as `Gone`, so a file
  removed or trashed outside the app leaves the grid too (it used to come back as a blank "ghost"
  tile that broke Ctrl+A + Delete).
- **Status messages** are a pill at the bottom-centre of the window (they used to draw under the
  window buttons), 4 s by wall clock, and expire by themselves on an idle window.
- **Reveal** ("Show in file manager") runs on a worker: with a cold Nautilus the D-Bus calls can take
  up to ~10 s before the folder-open fallback. It still ends in the parent folder when no file manager
  implements `FileManager1.ShowItems`.
- **Edit (annotate)**, **Open**: real argv / a reaped `xdg-open` (no quoted-path or zombie bugs).
- **Copy text (OCR)** is greyed out with a hint when `tesseract` is not installed (restart the app after
  installing it: the probe is cached).
- Tiles say `MP4` / `WEBM` for what they are; the ShareX chip and legend entry only exist if
  `~/Documents/ShareX/Screenshots` does; the legend drops to bare dots (tooltips) when the title bar is
  too narrow for the words.

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

## How the clipboard works here (and why it is a daemon)

On Linux the clipboard is owned by a live process; the one-shot `trontsnap full`
exits in seconds. `xclip`/`wl-copy` fork a daemon but serve ONE mime type, so shots
and files go through `trontsnap clip-serve` (`src/linux/clipserve.rs`): a small
detached X11 selection owner on XWayland offering several targets at once. It
exits by itself when anything else takes the clipboard. If it can't start (no
`DISPLAY`, exec failure, no ownership within 4 s) the writer falls back to the
single-target tool and logs why to `~/.local/share/trontsnap/trontsnap.log`;
`TRONTSNAP_CLIPSERVE=off` forces that fallback (used by the harness). Measured
consumers: native-Wayland GTK4 (texture + real file path), GTK3/VTE (text + uris),
Nautilus 46 (pastes a `Screen Rec #1 (final) 100%é.mp4` that used to error).
Not checked on this box: Discord/Electron/Chromium, Qt/KDE.

`trontsnap full` hands gnome-screenshot's PNG to the clipboard and disk untouched
(no decode/re-encode), ~1.7 s in the debug build the GNOME hotkeys run (4.8 s
before; gnome-screenshot alone is ~1.6 s). The GNOME bindings run whichever binary the
app was launched from (here `target/debug/trontsnap`). A release build would be faster
still, but the bindings deliberately don't prefer `target/release` when it exists: it would
silently be a stale binary after every debug rebuild.

## Recording

X11 sessions: ffmpeg x11grab, full screen, 30fps, libx264 ultrafast (+ Pulse
audio when "Record audio" is on). Wayland: not wired yet — drop a
`~/.config/trontsnap/record-start.sh` that records to `$1` (and optionally a
`record-stop.sh`); TrontSnap handles start/stop/clipboard/toast around it.

## Test harness

`scripts/linux/harness/` (see its README): isolated Xvfb GUI rig + XTest input
driver + screenshot/clipboard tools (`gui_env.sh`, `xinput.py`, `shot.sh`,
`clip.sh`, `selftest.sh`), `paste_test.sh` (what lands on the clipboard, asserted:
file set, URI escaping, Ctrl+C, INCR, fallback, OCR notice) and `rec_test.sh`, an
end-to-end recorder test against the live app, and `gallery_test.sh` (deletes/ghost tiles, selection
gestures, Edit/Open/Reveal, the status pill, bulk-delete progress; asserts the app's `TRONTSNAP_TEST`
state trace + Trash/file facts). `TRONTSNAP_PORT` overrides the command port (default 48761);
`TRONTSNAP_TEST=1` stops an instance from installing keybindings/autostart.

## Known gaps vs Windows (telling the truth here, in rough priority order)

1. No tray icon (needs a gtk loop beside winit) — the window stays open; Close quits
2. No drag-out from gallery/toast (no XDND backend yet) — paste covers most of it. A press-and-move
   over ~6 px on a tile therefore does nothing (egui's own click rule, independent of the tile's Sense)
   and a plain double-click still Copies on its first click before it Opens
3. Recording is full-screen only, and Wayland needs the script hook (portal/PipeWire is the real fix)
4. No loupe in the region picker yet, and no per-key rebinding (the Settings hotkey UI edits Windows VK codes)
5. Multi-monitor untested (same primary-monitor scope as Windows)
6. Toast docks wherever the Wayland compositor puts it (bottom-right positioning works on X11 only)
7. A second `trontsnap` launch surfaces the window if it's open, but can't un-minimize it on Wayland (no xdg-shell unminimize) — restore it from the dock

