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
| `ffmpeg` + `ffprobe` | recording (audio sidecar, mux, crop), video thumbnails, GIF export | `sudo apt install ffmpeg` |
| `pkexec` + capkit's `capkit-kms` / `capkit-stop` | the fast **kms** recording backend (root helper, passwordless via capkit's polkit rule). Absent = TrontSnap falls back to GNOME screencast / x11grab | capkit (`~/capkit`) |
| `notify-send` | one-shot CLI notices (`record` starting the app, OCR). The recorder's own banners go over D-Bus | preinstalled |
| `tesseract-ocr` | region OCR (`Ctrl+Alt+Print`, gallery "Copy text"). **Not preinstalled on Ubuntu**: without it the hotkey shows an "OCR unavailable" notification instead of a picker | `sudo apt install tesseract-ocr` |
| `paplay` / `pw-play` / `aplay` | shutter sound + the recorder's start/stop ticks (this box has `pw-play`, no `paplay`) | preinstalled everywhere |

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

On GNOME the app installs five custom keybindings on first app launch (and
clears GNOME's own bare-Print screenshot UI binding, same move as freeing the
key from Snipping Tool on Windows):

| Keys | Runs |
|---|---|
| `Print` | `trontsnap full` |
| `Ctrl+Print` | `trontsnap region` |
| `Ctrl+Shift+Print` | `trontsnap record` (toggles the recorder; **starts the app first if it isn't running**) |
| `Ctrl+Shift+Alt+Print` | `trontsnap record region` (drag a rectangle, then record just that) |
| `Ctrl+Alt+Print` | `trontsnap ocr` |

`record` hands one command to the running app over its loopback port (48761 by default;
`TRONTSNAP_PORT` overrides, and the binding then carries `env TRONTSNAP_PORT=N`). The old binding was a bare
`bash -c "echo -n record > /dev/tcp/..."`, which failed silently when the app was down (an XWayland restart
takes it down): now a refused connection starts the app (`--startup`, detached, log in
`~/.cache/trontsnap/app.log`), says "TrontSnap is starting", and delivers the command; a cold start is a START.

Other desktops: bind those same commands in your DE's keyboard settings.
The one-shots (`full`, `region`, `ocr`, `record`) don't need the app running.

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

One hotkey toggles a full-screen recording (or a picked region). Stop delivers the file: clipboard file
reference + shutter + toast, same contract as Windows. The backend is picked per recording, first that applies
(`TRONTSNAP_RECORDER=hook|kms|gnome|x11` forces one):

| Backend | What it is | Measured on the G50-45 |
|---|---|---|
| `hook` | your `~/.config/trontsnap/record-start.sh <out>` (+ optional `record-stop.sh`); `$TRONTSNAP_REGION` = `x,y,w,h` | yours |
| **`kms`** | capkit's root helper: kmsgrab -> VCE hardware H.264 | every displayed frame, ~0.1 core. **The default here.** |
| `gnome` | `org.gnome.Shell.Screencast` (webm/VP8, CPU) | 2.9 fps under heavy full-screen motion, ~14 fps under moderate; 1-2 cores |
| `x11` | ffmpeg x11grab + libx264 ultrafast | ~3 cores on a busy screen |

What the `kms` path costs you, honestly:

- **The clip begins ~2-3 s after the press** (pkexec + VAAPI init, then the helper trims the first second of
  warmup). A low tick sounds at the press, a "Starting recording" banner shows during the dead time, and the
  banner closes with a second tick the moment frames flow. Hit record a beat before the action.
- **No mouse pointer**: kmsgrab reads the primary plane and the cursor lives on the hardware cursor plane
  (Settings > Capture cursor does nothing for kms; screenshots and the gnome backend do draw it). User-level
  option, NOT applied here: `MUTTER_DEBUG_DISABLE_HW_CURSORS=1` in `~/.config/environment.d/` (needs a re-login;
  the pointer is then composited by the GPU, a compositor frame per move on a weak APU).
- **No banners in the picture.** GNOME pins banners top-centre *inside* the frame and ignores `notify-send -t`, so the
  recorder uses ONE banner it replaces and closes over D-Bus, and cuts the post-stop footage (the stop helper's ~1 s
  round trip) from the end of the clip, so the stop tick and "Saving recording" banner never appear in it. The two
  ticks can leak into the first/last fraction of the audio track.

**Audio** ("Record audio", default on) = system audio (the default sink's monitor), encoded live by a sidecar
(AAC `.m4a`, Opus for webm) and stream-copied in at the end, so delivery doesn't wait on a long encode. The mux is
END-aligned and both streams are cut at the same instant (the audio when the stop helper returns, the video by its
own SIGINT), which measured **audio-minus-video 0.105 s on the real kms path** (the beep player's own spawn latency
is 0.12-0.16 s; the old behaviour was +1.53 s late). External stops (capkit's own hotkey) cut the audio at kmsgrab's
exit minus an estimated 0.3 s flush.

**Region recording** (`trontsnap record region`, Ctrl+Shift+Alt+Print, Capture menu): the picker's rectangle goes
to the app as `record x,y,w,h`. x11 passes it to x11grab, gnome calls `ScreencastArea`, kms records the whole
screen and crops afterwards with one libx264 pass (about 0.2x realtime here, so a minute costs ~12 s of "Saving"
and the audio is still copied).

**Feedback while it runs:** the title bar shows `Starting...` / `REC mm:ss` (click = stop) / `Saving...` when the
window is open (there is still no tray icon), and the Capture menu item reads "Stop recording".

**Safety:** a stub or unreadable output (the helper `mv`s a 0-byte file when the stop beats the first frame) is a
failed recording ("Recording cancelled" / "too short"), never a clip. Pressing record again within ~1.5 s of a
stop waits for the old kmsgrab to exit instead of refusing; a *capkit* recording in progress is refused
("Press Super+Alt+R to stop it") and capkit's `Super+Alt+R` during a TrontSnap recording is a normal stop. A crash
mid-recording is salvaged at the next app start. **Opening a video from the gallery/toast is deferred while a kmsgrab
recording runs**: VLC/mpv default to VA-API hardware decode, and UVD decode + VCE encode at once hung this radeon
(desktop frozen, hard reset). `TRONTSNAP_OPEN_WHILE_RECORDING=1` overrides; for playback in general set
`avcodec-hw=none` in `~/.config/vlc/vlcrc` (or `vlc --avcodec-hw=none`) on this machine.

Windows' blinking REC outline and capture-excluded HUD have no Linux twin (no capture exclusion exists here).

## Test harness

`scripts/linux/harness/` (see its README): isolated Xvfb GUI rig + XTest input
driver + screenshot/clipboard tools (`gui_env.sh`, `xinput.py`, `shot.sh`,
`clip.sh`, `selftest.sh`), `paste_test.sh` (what lands on the clipboard, asserted:
file set, URI escaping, Ctrl+C, INCR, fallback, OCR notice) `rec_test.sh`, an
end-to-end recorder test against the live app (A/V content sync included), `rec_fake_test.py` (the whole kms code path
against a fake root helper in a rig: cues, tail trim, restart, double press, empty output, region, cold-start CLI, CPU;
no GPU/root, run it freely), `gif_test.py` (GIF export timing on a 30 fps mp4 and a real VFR screencast), and `gallery_test.sh` (deletes/ghost tiles, selection
gestures, Edit/Open/Reveal, the status pill, bulk-delete progress; asserts the app's `TRONTSNAP_TEST`
state trace + Trash/file facts). `TRONTSNAP_PORT` overrides the command port (default 48761);
`TRONTSNAP_TEST=1` stops an instance from installing keybindings/autostart.

## Known gaps vs Windows (telling the truth here, in rough priority order)

1. No tray icon (needs a gtk loop beside winit) — the window stays open; Close quits
2. No drag-out from gallery/toast (no XDND backend yet) — paste covers most of it. A press-and-move
   over ~6 px on a tile therefore does nothing (egui's own click rule, independent of the tile's Sense)
   and a plain double-click still Copies on its first click before it Opens
3. Recording: the pointer is missing from kms recordings and a kms clip starts ~2-3 s after the press (see Recording);
   the gnome fallback draws the pointer but is slow under motion. No capture-excluded REC HUD like Windows
4. No loupe in the region picker yet, and no per-key rebinding (the Settings hotkey UI edits Windows VK codes)
5. Multi-monitor untested (same primary-monitor scope as Windows)
6. The toast docks bottom-right via XWayland (measured 340x188+1002+532, exactly the intended spot); GNOME's own
   banners (OCR/recorder notices) stay top-centre, which the app can't move
7. A second `trontsnap` launch surfaces the window if it's open, but can't un-minimize it on Wayland (no xdg-shell unminimize) — restore it from the dock

