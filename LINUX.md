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

### Stable install on this box (`~/.local/bin/trontsnap`)

The GNOME keybindings and the autostart entry run whichever binary the app was started from, and every
`cargo build` replaces `target/debug/trontsnap` under them. So the live app runs from a fixed path:

```
cargo build --release                  # fat LTO on the 4-core A6: ~13 min, nothing else building
scripts/linux/install_local.sh         # copy, timestamped .bak of the old one, fix autostart, restart live app
```

`install_local.sh` stops the live app by the pid that owns the command port (never a pattern kill), refuses while
a recording is in progress, relaunches `~/.local/bin/trontsnap --startup` detached (log `~/.cache/trontsnap/app.log`),
and the relaunch rewrites the five GNOME bindings from `current_exe`. The **autostart entry is NOT rewritten by the
app** (it is written once, `AutostartInit`), so the script edits its `Exec=` line itself. `--no-restart` only copies.
Installed 2026-10-06 (release, 14 MB, stripped); bindings and `~/.config/autostart/trontsnap.desktop` checked to
point at it. Heads-up: starting ANY other build (`cargo run`, `target/debug/trontsnap`) as the app repoints the
bindings at that binary until the installed one is relaunched.

Build deps beyond Rust: the usual eframe set (`libgtk-3-dev` is NOT needed;
`libxcb`/`libxkbcommon`/`libssl-dev` usually already present on a dev box).

## Runtime dependencies (small, deliberate shell-outs)

| Tool | For | Install |
|---|---|---|
| `xclip` | text clipboard writes (OCR, copied paths) and the fallback for everything else; used on X11 AND on GNOME Wayland (via XWayland, which Mutter bridges) | `sudo apt install xclip` |
| `wl-clipboard` | only for wlroots/KDE compositors that implement data-control; GNOME has none, so wl-copy there is not used when xclip exists | `sudo apt install wl-clipboard` |
| `gnome-screenshot` / `spectacle` / `grim` | screen grab fallback when xcap can't capture your compositor | usually preinstalled |
| `ffmpeg` + `ffprobe` | recording (audio sidecar, mux, crop, tail trim), video thumbnails, GIF export. Without them a kms/GNOME clip is still saved as captured (no audio, trim or crop) and a one-time "ffmpeg is missing" banner says why; it is never deleted as "too short" | `sudo apt install ffmpeg` |
| `pkexec` + capkit's `capkit-kms` / `capkit-stop` | the fast **kms** recording backend (root helper, passwordless via capkit's polkit rule). Absent = TrontSnap falls back to GNOME screencast / x11grab | capkit (`~/capkit`) |
| `notify-send` | only the fallback when there is no notification bus. Every notice (CLI one-shots, the recorder's banners) goes over D-Bus and is CLOSED explicitly: GNOME Shell ignores `-t` and holds a banner until the next keystroke | preinstalled |
| `tesseract-ocr` | region OCR (`Ctrl+Alt+Print`, gallery "Copy text"). **Not preinstalled on Ubuntu**: without it the hotkey shows an "OCR unavailable" notification instead of a picker | `sudo apt install tesseract-ocr` |
| `paplay` / `pw-play` / `aplay` | shutter sound + the recorder's start/stop ticks (this box has `pw-play`, no `paplay`) | preinstalled everywhere |

## What works (verified on GNOME Wayland, Ubuntu)

- `trontsnap` — the full app: gallery over your whole history (ShareX archive
  merge included), search, themes, annotation editor, settings
- `trontsnap full` — one-shot fullscreen grab → PNG + clipboard + shutter + toast
- `trontsnap region` / `ocr` — freeze-frame picker (drag a region, click a
  window where enumeration works, Esc cancels)
- Live gallery splice via inotify, capture toast with Edit chip
- Tray icon + menu (StatusNotifierItem), Close hides to the tray, recording badge on the icon: "The tray icon and
  the window" below
- Autostart via `~/.config/autostart/trontsnap.desktop` (`--startup`: no window at login, see
  "The tray icon and the window" below)
- Settings persist to `~/.config/trontsnap/settings.conf`
- Region picker with the Windows picker's loupe: 8x magnifier + crosshair beside the pointer, mouse
  wheel resizes it (96-528 px, remembered; Settings > Region picker). Where other windows can't be
  enumerated (native Wayland) a plain click takes the whole screen and the hint says so
- "Capture cursor" draws the pointer into screenshots: the grabber does it on Wayland
  (`gnome-screenshot -p`, `spectacle -p`, `grim -c`), XFixes does it on an X11 session. Recordings
  honour it as before (kms can't, see Recording)
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
  counted in `N failed (see log)`. A second Delete pressed while a batch is still moving is queued behind it
  (one summary at the end), and clipboard copies go through one worker in click order. The folder watcher forwards deletes/moves-out as `Gone`, so a file
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

On GNOME the app installs five custom keybindings on every app start (and
clears GNOME's own bare-Print screenshot UI binding, same move as freeing the
key from Snipping Tool on Windows). These are the DEFAULTS: **Settings > Hotkeys rebinds them for
real** (the stored Windows modifier/VK pair is translated to a GNOME accelerator; `Win` reads `Super`;
the Record-region chord is always Record + Alt and is disabled while Record already uses Alt).
Changes are written ~0.6 s after the last click, off the UI thread. Unticking "Use the PrintScreen key"
gives bare Print back to GNOME, is remembered across launches, and blanks our bare-Print slot.

| Keys | Runs |
|---|---|
| `Print` | `trontsnap full` |
| `Ctrl+Print` | `trontsnap region` |
| `Ctrl+Shift+Print` | `trontsnap record` (toggles the recorder; **starts the app first if it isn't running**) |
| `Ctrl+Shift+Alt+Print` | `trontsnap record region` (drag a rectangle, then record just that; pressed while a recording runs - or is still starting - it STOPS it, no picker) |
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

`trontsnap full` hands gnome-screenshot's PNG to the clipboard and disk untouched (no decode/re-encode), so it costs
about 0.1 s over the grabber itself; it no longer forks `gsettings` on every press either (only the app frees the Print key).
The grabber is the floor, and on this box it varies with what is on screen: 1.6 s measured once, 4.3 s (2.7 s of CPU) on
2026-10-06 with the shader-heavy terminal animating (`full` 4.45 s vs `gnome-screenshot -f` 4.33 s, same minute). A grabber
that hangs is killed after 20 s and the hotkey says "Screenshot failed" (the one-shots notify; stderr goes nowhere from a
keybinding). The GNOME bindings run whichever binary the app was launched from: `~/.local/bin/trontsnap` since 2026-10-06 (see
"Stable install"). They deliberately don't prefer `target/release` when it exists: it would silently be a stale binary
after every rebuild. Release `full` through the installed binary: 4.4 s wall (the grabber is the floor, see above).

## The tray icon and the window (close, autostart, second launch)

TrontSnap has a real tray icon when the desktop offers one (`src/linux/sni.rs`): a StatusNotifierItem plus a
`com.canonical.dbusmenu` menu, hand-rolled on zbus (no gtk / libappindicator, so the AppImage stays simple). KDE,
most trays and **GNOME with the `ubuntu-appindicators` extension** (Ubuntu ships and enables it) draw it. Plain
GNOME without that extension has no tray at all; then everything below falls back to "no tray".

- **Icon**: the rainbow app icon (ARGB pixmaps, 16-64 px). While a recording is live it carries a **red badge**;
  tooltip "TrontSnap is recording / Click to stop".
- **Menu**: Show TrontSnap, Capture fullscreen, Capture region, Record screen (reads **Stop recording** while
  recording), Record region, Copy text (OCR), Capture cursor, Record audio, Start at login (the three checks are the
  same settings as Settings > ..., live both ways), Quit TrontSnap.
- **Clicks** (checked against the real extension in a private shell, not just a mock): right click = menu. On GNOME a
  single left click ALSO opens the menu (the extension waits for a double click first) and **double click =
  Activate = open the window**; KDE-style hosts send Activate on a single click. While recording, Activate stops the
  recording instead (the badge is the cue). Middle click = region capture.
- **Close (the title-bar X, Alt+F4) hides to the tray** when the tray is registered with the watcher AND the main
  window is an X11 window; the process, the hotkeys and the command port keep running (hidden = 0 CPU ticks in 5 s),
  a live recording keeps going. Show it again with the tray (double click / menu), a second `trontsnap` launch, or
  the dock icon. **Quit** is in the tray menu.
- **No tray (or the watcher went away): Close quits**, same path as Alt+F4 (live recording stopped and finalized
  first; with a recording running and no tray, X minimizes instead). If the watcher vanishes while the window is
  hidden (extension switched off, shell restarted) the window comes back by itself so the app can't be stranded.
  Nothing is lost by quitting: the capture keybindings are one-shot processes and `trontsnap record` starts the app
  on demand.

**Why the main window runs on XWayland when a tray exists.** HARD-WON: winit-wayland's `set_visible` is a no-op and
"unminimizing is ignored", so a native Wayland window can neither hide to a tray nor be brought back. X11 map/unmap
is real, and GNOME runs XWayland (the toast already lives there). So at startup, **before the window exists**,
`app::linux_window_setup` brings the tray up and, if it registered and an X server is reachable, forces winit onto X11
(`EventLoopBuilderExtX11::with_x11`; a direct `winit` dependency, already in the tree). Without a tray the window
stays native Wayland, exactly as before. `TRONTSNAP_NATIVE_WAYLAND=1` keeps the native window (no close-to-tray);
`TRONTSNAP_TRAY=off` disables the tray; a `TRONTSNAP_TEST` instance has no tray unless `TRONTSNAP_TRAY=on` (a test
sharing the REAL session bus must not put an icon in the real tray). At login the extension can load after our
autostart entry, so `--startup` on GNOME waits up to 3 s for the watcher; any other start answers at once.
Costs of XWayland: no fractional-scale crispness (this 1366x768 box is 1x; a HiDPI user can opt out), and Mutter
auto-maximizes a window this size. Showing it: winit maps and raises it, then `x11win.rs` asks the window manager
to activate it like a pager (`_NET_ACTIVE_WINDOW`, source 2) because winit cannot focus a window it just mapped
and Mutter's focus-stealing rules would otherwise leave it behind the focused one. Verified live: hidden window
unmapped, Activate -> mapped + `_NET_WM_STATE_FOCUSED` over a fullscreen terminal.

- **`--startup`** (what the autostart entry and `record` run) starts without a visible window: with a tray
  that is an unmapped X11 window (a vendored-eframe backport, see `vendor/eframe/VENDOR.md`); without one, X11 still
  stays unmapped, and a native Wayland client cannot hide, so it starts **minimized** (open it from the dock).
- **A second `trontsnap` launch** pokes the running app. With an X11 window (tray case, or a real X11 session) that
  maps, raises and focuses it. On native Wayland winit can't unminimize or focus, so the app asks GNOME to raise it
  with an xdg-activation request: a buried window gets the "TrontSnap is ready" notification (click it). A
  **minimized native Wayland window cannot be raised this way**: it gets no frame callbacks, so its event loop never
  runs the request. Click its dock icon instead.
- Every window sends `app_id = trontsnap` (X11: WM_CLASS) so GNOME ties it to `trontsnap.desktop` /
  `StartupWMClass=trontsnap` (dock icon, grouping, pinning).

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

- **The clip begins ~2-4 s after the press** (pkexec + VAAPI init, then the helper trims the first second of
  warmup; the first record after an app restart measured 4.2 s to frames). A low tick sounds at the press and a "Starting recording" banner shows during the dead time; the banner
  closes and the tray dot appears the moment frames flow (no sound then: it would be recorded into the clip). Hit
  record a beat before the action.
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
exit minus an estimated 0.2 s flush.

**Region recording** (`trontsnap record region`, Ctrl+Shift+Alt+Print, Capture menu): the picker's rectangle goes
to the app as `record x,y,w,h`. x11 passes it to x11grab, gnome calls `ScreencastArea`, kms records the whole
screen and crops afterwards with one libx264 pass (about 0.2x realtime here, so a minute costs ~12 s of "Saving"
and the audio is still copied).

**Feedback while it runs:** the tray icon (a StatusNotifierItem: GNOME's AppIndicator extension, KDE and most
trays) grows a red badge from the moment frames flow to the stop press (tooltip "TrontSnap is recording", Activate
= stop, the menu item reads "Stop recording"), and the title bar shows `Starting...` / `REC mm:ss` (click = stop) /
`Saving...` when the window is open; the Capture menu item reads "Stop recording". The badge is in the top bar, so a
full-screen recording shows it too, and a `hidetopbar`-style extension (enabled on this box) hides the bar, badge
included, until the pointer touches the top edge. Verified in a private GNOME Shell with the real extension: the
badge appears when frames flow and clears at the stop (a synthetic `record-start.sh` hook, no screen grab).

**Safety:** a stub or unreadable output (the helper `mv`s a 0-byte file when the stop beats the first frame) is a
failed recording ("Recording cancelled" / "too short"), never a clip. Pressing record again within ~1.5 s of a
stop waits for the old kmsgrab to exit instead of refusing; a *capkit* recording in progress is refused
("Press Super+Alt+R to stop it") and capkit's `Super+Alt+R` during a TrontSnap recording is a normal stop. A crash
mid-recording is salvaged at the next app start. The salvage reads the dead session's marker BEFORE the command
port can start a new session and then touches only that session's own files (`TrontSnap_<stamp>.*` in the stage
dir, and the capkit clip whose name stamp is the helper's start time): a `record` arriving right after the
relaunch is not eaten, and a Super+Alt+R clip made meanwhile stays in `~/Videos/captures`. A kms start waits
(banner "Recovering the last recording", ~5 s) while the dead session's orphan kmsgrab is being stopped. **Opening a video from the gallery/toast is deferred while a kmsgrab
recording runs**: VLC/mpv default to VA-API hardware decode, and UVD decode + VCE encode at once hung this radeon
(desktop frozen, hard reset). `TRONTSNAP_OPEN_WHILE_RECORDING=1` overrides; for playback in general set
`avcodec-hw=none` in `~/.config/vlc/vlcrc` (or `vlc --avcodec-hw=none`) on this machine.

Windows' blinking REC outline and capture-excluded HUD have no Linux twin (no capture exclusion exists here).

**The pointer in kms recordings is a pending decision, not a bug being fixed.** kmsgrab reads the primary plane; the
pointer is on the hardware cursor plane. Options, none applied: (1) `MUTTER_DEBUG_DISABLE_HW_CURSORS=1` in
`~/.config/environment.d/` (re-login; the GPU then composites the pointer, a compositor frame per move on this APU);
(2) record with the `gnome` backend whenever "Capture cursor" is on (draws it, but 3-14 fps on this APU); (3) leave as is
(the Settings text and the first-recording banner already say so). Nothing can overlay a sampled pointer position:
on Wayland no client can read it outside its own windows.

## Test harness

`scripts/linux/harness/` (see its README; `parity_test.sh` = window lifecycle, hotkey rebinding, PrintScreen
opt-out, gsettings spawn count, picker loupe, pointer in screenshots on BOTH an Xvfb rig and a real native-Wayland
private GNOME Shell): isolated Xvfb GUI rig + XTest input
driver + screenshot/clipboard tools (`gui_env.sh`, `xinput.py`, `shot.sh`,
`clip.sh`, `selftest.sh`), `paste_test.sh` (what lands on the clipboard, asserted:
file set, URI escaping, Ctrl+C, INCR, fallback, OCR notice) `rec_test.sh`, an
end-to-end recorder test against the live app (A/V content sync included), `rec_fake_test.py` (the whole kms code path
against a fake root helper in a rig: cues, tail trim, restart, double press, empty output, region, cold-start CLI, CPU;
no GPU/root, run it freely), `gif_test.py` (GIF export timing on a 30 fps mp4 and a real VFR screencast), and `gallery_test.sh` (deletes/ghost tiles, selection
gestures, Edit/Open/Reveal, the status pill, bulk-delete progress; asserts the app's `TRONTSNAP_TEST`
state trace + Trash/file facts), and `tray_test.sh` (the tray icon + close-to-tray against a mock watcher:
registration, menu layout, Close hides/shows, the recording badge and label flip, check items, watcher loss, Quit; 37 checks). `TRONTSNAP_PORT` overrides the command port (default 48761);
`TRONTSNAP_TEST=1` stops an instance from installing keybindings/autostart (and from registering a tray item unless
`TRONTSNAP_TRAY=on`).

`run_all.sh` runs the whole set sequentially (one rig at a time) and checks the kernel log for UVD/VCE/radeon errors
at the end; `RIG_N` / `RIG_PORT` pick the display and port. `rec_fake_test.py` also covers: no ffprobe on PATH (S12),
crash + relaunch + immediate `record` (S13), what the salvage may touch (S14), `record region` during a recording
(S15), one-shot notices closing, failed/hung grabs (S16). `gallery_test.sh` G13 = a second Delete queued behind a
running one; `paste_test.sh` K = 20000 paths over stdin, and a 36 MB copy then a small one end in click order (this
guards the FIFO; the old thread-per-copy race needs two clicks inside ~100 ms, which XTest pacing here cannot make, so
the pre-fix binary passes it too). Rigs started with
`RIG_KEEP_BUS=1` survive `kill -9` of the app (the crash tests).

**Last full run, 2026-10-06** (`run_all.sh` on the debug build of `6c7e851`, 18 min, kernel log clean afterwards;
checks counted from the suite logs): selftest 5, paste 43, gallery 38, parity 28, tray 36, notice 4, hotkey 9,
recfake 91 = all PASS, 0 FAIL. Real session on the kms/VCE path through `rec_test.sh`: case A 18/18 (8 s clip,
29.8 fps, A/V content sync +0.090 s, ends at the stop press) and case D 17/17 (kill -9 mid-recording, relaunch,
6.7 s clip salvaged). Cases B, C1, C2 (capkit collision, external stop) were NOT re-run that night: see the budget below.

**Machine safety, learned the hard way (2026-10-05, the desktop froze and needed a hard reset).** Never DECODE video
on the GPU while kmsgrab encodes on the VCE: UVD hung ("UVD not responding"), then
`radeon_dpm_change_power_state_locked` took the compositor with it. The harness motion sources are generated
(`ffplay -f lavfi -i testsrc2`, which `rec_perf.py` uses; `mpv --hwdec=no`, a terminal animation), never a played clip with
hardware decode; ffmpeg/ffprobe software decode for analysis is fine. Budget for real kms clips: <= 3 per session,
<= 15 s each, >= 60 s apart, never while anything else records. `rec_test.sh` with no arguments makes FIVE real clips
(A, B, C1, C2, D), so run it by case (`--only A`, `--only D`, `--only B,C`). After any GPU-touching test:
`journalctl -k -b --since "-5 min" | grep -i -E "uvd|vce|radeon.*error|hung"` must print nothing. Rigs pin
`TRONTSNAP_RECORDER=x11` so a rig instance can never start the real kms helper.

## Verified only in rigs, still owed a physical run

Everything above was driven in Xvfb rigs, a fake root helper, private headless GNOME Shells and (for the recorder, at
most three short clips) the real session. On 2026-10-06 the INSTALLED release binary was also run on the real session:
`~/.local/bin/trontsnap full` (1366x768 PNG on disk + `image/png`/uri-list on the clipboard, 4.4 s wall) and
`~/.local/bin/trontsnap record` twice, 9 s apart, i.e. the exact commands the bindings run (1366x768 h264 + aac,
video 6.54 s / audio 6.52 s, a real frame of the desktop, kernel log clean). Those were typed commands, not physical key presses. What a private shell DOES cover (`hotkey_wl_test.sh`, `notice_wl_test.sh`):
the five bindings pressed as real GNOME custom keybindings (a private `gsd-media-keys` + real key events), and a
banner disappearing by itself. What it cannot, and what was NOT done with real keys on the real desktop; do this once
and record the result here:

- press each of the five bindings with the physical keys on the real session, double-tap Ctrl+Shift+Print, record 10 s then restart within
  2 s (the real kms helper, not the fake one: only sync, region+crop and external stop ran against it);
- Esc in the region picker on the real session (the private shell gives no freshly mapped window keyboard focus, so
  only the right-click cancel is proven);
- Ctrl+click multi-select + Ctrl+C + paste into Nautilus and a terminal in the live gallery (XWayland window);
- watch the tray dot on the real top bar (the `hidetopbar` extension hides it until the pointer touches the edge).

Not verifiable on this one-output laptop: multi-monitor (PAR-14), Windows runtime of the shared gallery changes
(CHANGELOG has the smoke list), a 5-minute clip's stop-to-delivery time, the gnome backend at lower fps.

## Known gaps vs Windows (telling the truth here, in rough priority order)

1. Tray: needs a StatusNotifier host (KDE, GNOME with `ubuntu-appindicators`); a bare GNOME has none and Close quits.
   On GNOME a single left click opens the menu and a double click opens the window (the extension's rule). With a
   tray the main window is an XWayland window (see above): fine at 1x, not fractional-scale-crisp. A hidden
   window is brought back by the tray or a second `trontsnap` launch (what an installed `trontsnap.desktop` / dock
   click runs; this box has no installed entry, so the dock path itself is untested)
2. No drag-out from gallery/toast (no XDND backend yet) — paste covers most of it. A press-and-move
   over ~6 px on a tile therefore does nothing (egui's own click rule, independent of the tile's Sense)
   and a plain double-click still Copies on its first click before it Opens
3. Recording: the pointer is missing from kms recordings and a kms clip starts ~2-4 s after the press (see Recording);
   the gnome fallback draws the pointer but is slow under motion. No capture-excluded REC HUD like Windows
4. The picker has no "click for a window" on native Wayland (no window enumeration), and no faint window-rect map on any backend
5. Multi-monitor untested and unverifiable on this one-output laptop (same primary-monitor scope as Windows; static review worry: the CLI grabber's multi-monitor PNG is cropped with xcap's scaled geometry, and the picker is fullscreen on whichever output the compositor picks)
6. The toast docks bottom-right via XWayland (measured 340x188+1002+532, exactly the intended spot); GNOME's own
   banners (OCR/recorder notices) stay top-centre, which the app can't move
7. Without a tray (native Wayland window), a second `trontsnap` launch can't un-minimize a minimized Wayland window (no xdg-shell unminimize, and a minimized surface gets no redraw to act on the request) — restore it from the dock. A buried but mapped window is raised through GNOME's notification
8. Bindings follow whichever binary was started last: `cargo run` or `target/debug/trontsnap` as the app repoints the five
   GNOME bindings at itself (the installed copy fixes that on its next launch; `install_local.sh` relaunches it). Autostart is
   written once, so only `install_local.sh` (or one click on Start at login in the tray: a stale entry reads as unticked, ticking rewrites it) repoints it
9. No `--version` flag (`trontsnap --version` prints the usage line); the build stamp is the exe's mtime, shown in Settings > About
