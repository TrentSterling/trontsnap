# TrontSnap Linux test harness

Automated rig for PROVING changes on the Linux port: an isolated GUI instance
you can drive and screenshot, plus an end-to-end recorder test against the live
app. Everything here is plain bash/python3 over tools already on the box (Xvfb,
ffmpeg/ffprobe, xclip, libX11/libXtst via ctypes). No xdotool, no sudo.

Artifacts land in `harness_out/<YYYY-MM-DD>/` (gitignored; override with
`HARNESS_OUT=/path`). Always LOOK at screenshots (Read the PNG) before claiming
a visual result; `imgdiff.py` tells you something changed, not that it is right.

## 0. Is the harness itself healthy? (`selftest.sh`, ~25 s)

```
scripts/linux/harness/selftest.sh            # own rig on :97 / 48797, stops it after
```

Starts a rig, ctrl+clicks two tiles, ctrl+a, Escape, and a plain click (copy);
asserts the pixel diffs + that `image/png` hit the rig clipboard; prints
PASS/FAIL, exit nonzero on FAIL. Screenshots in `harness_out/<date>/selftest/`
(`1_idle.png` vs `2_two_selected.png`: "2 selected", tiles 2+3 outlined).

## 1. Isolated GUI rig (`gui_env.sh`)

```
scripts/linux/harness/gui_env.sh start 99 48799     # Xvfb :99 + app on port 48799
scripts/linux/harness/gui_env.sh status
scripts/linux/harness/gui_env.sh run 99 target/debug/trontsnap toast some.png   # any cmd, rig env
scripts/linux/harness/gui_env.sh stop 99
```

- `N >= 10` (display `:N`), PORT must not be 48761 (the live app). Pick a free
  N/PORT pair per agent so parallel agents don't collide (`status` lists rigs).
- Starts: `Xvfb :N 1366x768x24` (the real screen size), then the app under
  `dbus-run-session` with a temp `HOME` (`harness_out/.../homes/N`), `WAYLAND_DISPLAY`
  unset, `TRONTSNAP_PORT=PORT`, `TRONTSNAP_TEST=1`, `LIBGL_ALWAYS_SOFTWARE=1`.
  Prints `READY ...` (4-10 s) once the port is bound, the window is mapped, it is
  focused (see gotchas) and the first screen has stopped changing (lazy thumbnails).
- Seeds `~/Pictures/TrontSnap` of the temp HOME: 12 PNG copies cycled from the real
  ones (mtimes 90 min apart) + 1 real mp4. Gallery opens on the Gallery tab
  ("13 shots"). `--bare` = empty folder. `--recorder x11|gnome|kms|hook` sets
  `TRONTSNAP_RECORDER`. Re-running `start` re-seeds (wipes only `homes/N`).
- The rig ALWAYS exports `TRONTSNAP_RECORDER` (default `x11`, i.e. x11grab of the Xvfb screen):
  the default backend order is hook > kms > gnome > x11 and `kms` shells out to the passwordless
  capkit-kms helper, so an unpinned rig would record the REAL screen on the VCE. `gui_env.sh run`
  inherits the rig's choice. Only pass `--recorder kms` when the test is about kms.
- `--keep-home` = start without re-seeding (does a setting survive a relaunch?).
  `RIG_APP_ARGS="--startup" gui_env.sh start ...` passes argv to the app (the autostart entry runs `--startup`).
- `RIG_SEED_HOOK=/path/script` runs `script <temp-HOME>` after the default seed and before the app starts
  (the gallery scans once at launch): use it for a ShareX archive (`$HOME/Documents/ShareX/Screenshots/YYYY-MM/`),
  odd filenames, webm, truncated PNGs. Example: `harness_out/2026-10-05/audit-gallery/seed2.sh`.
- Uses `target/debug/trontsnap` (override: `TRONTSNAP_BIN=...`). Rebuild first;
  the rig never builds.
- Logs: `harness_out/.../rigs/N/app.log`, `xvfb.log`.
- `stop` kills by exact pid / process group, removes the X socket + lock. Never
  `pkill -f`.

### Isolation (verified 2026-10-05)

Started WITHOUT the guard (`start 99 48799 --no-guard`) the app really did run
`gsettings set` for its four keybindings plus the PrintScreen free-up, but they
landed in the rig's private dbus + dconf under the temp HOME: the real
`custom-keybindings`, the `trontsnap-record` command (still `.../48761`) and
`~/.config/autostart/` were byte-identical before/after. The guard
`TRONTSNAP_TEST=1` (default on in the rig) additionally makes
`install_gnome_binds()` and `autostart::ensure_first_run()` return immediately.

### Code hooks added for this

- `TRONTSNAP_PORT=<n>` overrides the single-instance/command port (`app::port()`,
  default 48761 unchanged). The GNOME record keybinding command now follows
  `app::port()` too (so a non-default port can't leave a dead `/dev/tcp/.../48761`).
- `TRONTSNAP_TEST=1` = never touch the session (keybindings/autostart/tray; `TRONTSNAP_TRAY=on` re-enables
  the tray for the tray tests).

## 2. Input (`xinput.py`)

XTest through ctypes. Always pass the display: `-d :99` (it refuses to guess).

```
xinput.py -d :99 move 400 300
xinput.py -d :99 click 400 300          # position optional (current pointer)
xinput.py -d :99 rightclick 400 300
xinput.py -d :99 ctrlclick 400 300      # also shiftclick / altclick / ctrlshiftclick / doubleclick
xinput.py -d :99 drag 100 100 300 300
xinput.py -d :99 scroll down 3
xinput.py -d :99 key ctrl+a Delete Escape ctrl+c    # combos: ctrl/shift/alt/super + keysym name
xinput.py -d :99 type "hello"
xinput.py -d :99 pos                    # "X Y"
xinput.py -d :99 windows                # top-level windows
xinput.py -d :99 find "TrontSnap Toast" # whole-tree search, abs root coords (works on :0 XWayland too)
xinput.py -d :99 wmclose               # WM_DELETE_WINDOW = the X button / Alt+F4 (close-to-tray tests)
xinput.py -d :99 focus [TITLE]
```

Park the pointer away from tiles (`move 1270 700`) before screenshotting a
selection, or the hover highlight/tooltip pollutes the diff.

**Flaky clicks / lost Ctrl on a busy box:** with the debug build under llvmpipe a frame can take
100+ ms, and the default pacing then drops roughly 1 click in 3 (pointer enters and presses inside
one frame) and loses Ctrl/Shift (modifier lands after the click or key; Ctrl+click degraded to a
plain copy, Ctrl+A did nothing). Prefix multi-select / shortcut tests with `XINPUT_SLOW=1`
(0.5 s settle, 0.3 s modifier lead; ~1 s per ctrl-click) and ALWAYS screenshot to confirm the
selection took before trusting a later step.

## 3. Screenshots, diff, clipboard

```
shot.sh 99 out.png [--crop WxH+X+Y] [--settle]   # ffmpeg x11grab, 1 frame; prints the path
                                        # --settle: wait for two identical consecutive frames
imgdiff.py a.png b.png [--out mask.png] # changed pixel count + bbox; exit 1 if identical
clip.sh 99                              # clipboard TARGETS on :99
clip.sh 99 image/png out.png            # dump a target to a file
```

`clip.sh` = `xclip -display :N -selection clipboard -t TARGETS -o` / `-t TYPE -o`.
List TARGETS first: asking xclip for a type the owner doesn't offer can hand back
a different one. Don't print binary targets to the terminal.

## 3b. Paste test (`paste_test.sh`, ~80 s)

```
scripts/linux/harness/paste_test.sh            # own rig :92 / 48792, stopped after
TRONTSNAP_BIN=/path/to/old/trontsnap scripts/linux/harness/paste_test.sh   # negative control
```

Asserts what a paste destination would get, on the rig's X clipboard: A) video tile with a
name needing URI escaping (`Screen Rec #1 (final) 100%é.mp4`, seeded by `paste_seed.sh`) -> the
Nautilus file set, percent-encoded, CRLF; B) still tile -> image/png byte-identical to the file
its own URI names; C/D/E) Ctrl+C with 2 / 1 selected, and NOT from the search box; F) `trontsnap
full` -> image/png == the saved file; G) `TRONTSNAP_CLIPSERVE=off` fallback + log line; H) a 10 MB
PNG through INCR x3 and the owner exiting when replaced; I) `trontsnap ocr` without tesseract ->
a notification (stand-in `fake_bin/notify-send`; `fake_bin/paplay` etc. keep the rig's shutter
off the real speakers) and an untouched clipboard; J) "Copy 2 files" / "Copy 2 paths" menu items,
and the copied paths still there after the app is killed. Run against the pre-fix binary it fails
24 checks (kept log: `harness_out/2026-10-05/fix-paste/paste_test_BEFORE.log`). Tile geometry:
tiles are 182 px apart from x=128,y=134; the F/G captures become tiles 1-2, which is why J
works on those; a still's context menu puts "Copy N files" at pointer+(45,285), "Copy N paths"
at +(49,319) (a video's menu has two fewer rows, so offsets shift).

## 3c. Gallery test (`gallery_test.sh`, ~2 min)

```
scripts/linux/harness/gallery_test.sh              # own rig :91 / 48791, stops it after
TRONTSNAP_BIN=/path/to/older/trontsnap HARNESS_OUT=/elsewhere scripts/linux/harness/gallery_test.sh
```

Seeds the rig with `seed_gallery.sh` (ShareX archive, a webm, a name with `#` and `%`, a truncated PNG: 25 files),
then asserts: Esc in the search box keeps the selection; plain click toggles once something is selected; Shift+click
replaces the range; a fast Ctrl+click x2 never Opens; Open reaps its `xdg-open` (no zombie child); Edit (annotate)
opens the editor; the status pill paints bottom-centre, leaves the title bar alone and expires on an idle window;
Reveal does not freeze the window while a slow file manager answers (`fm1_mock.py`); a Delete leaves no ghost tile
(key, context menu, external `rm`, and Ctrl+A + Delete + confirm with a file vanishing under the dialog); a
1500-file bulk delete reports chunked progress.

- State comes from `TRONTSNAP_TEST=1`: the app prints `trontsnap-test: gallery shots= shown= selected= status=`
  to stderr whenever one changes (`rigs/N/app.log`). A binary without that trace (an older build) runs the
  file-system / xdg-open / window checks and SKIPs the rest: that is how the pre-fix binary was shown to FAIL
  (2026-10-05: G6, G9 zombie, G2, G5, G4, G1 bulk delete).
- `fake_bin/xdg-open` logs to `$FAKE_OPEN_LOG` instead of launching anything (the test puts `fake_bin` on PATH).
- `fm1_mock.py` owns `org.freedesktop.FileManager1` + `org.freedesktop.portal.Desktop` on the rig's PRIVATE bus and
  answers ShowItems after `--delay`. Use it for ANY test that clicks Reveal: without it dbus activates the real
  Nautilus, which the rig's bus hands to the real `systemd --user` (tracker, portals, gvfs, all running with the rig HOME).
  `gui_env.sh stop` now also kills strays that inherited the rig's exact bus address.
- Tile centres assume the default window (x = 129 + 182 col, y = 134 + 182 row); seeded order is deterministic.

## 3d. Parity test (`parity_test.sh`, ~6 min)

```
scripts/linux/harness/parity_test.sh                 # X half (Xvfb :94 / 48794) then W half (private GNOME Shell)
scripts/linux/harness/parity_test.sh --only X        # or --only W / --no-wayland
TRONTSNAP_BIN=/path/to/older/trontsnap scripts/linux/harness/parity_test.sh    # negative control
```

The 2026-10-05 parity audit (PAR-02..13), asserted. **X** (Xvfb, no WM): `--startup` leaves the window
*unmapped* and a second launch maps it; the title-bar X exits the process and frees the port; WM_CLASS is
`trontsnap`; stored hotkeys (settings.conf seeded by a `RIG_SEED_HOOK`) become the right GNOME accelerators
in the rig's PRIVATE dconf (`<Ctrl>F12`, `<Super>Print`, record-region disabled when Record has Alt...); the
PrintScreen opt-out survives a launch (`PrintKeyOptOut=1` -> GNOME's `show-screenshot-ui` untouched, our
bare-Print slot blank); a counting `gsettings` shim sees <= 2 spawns for 20 pointer moves on the Settings tab
(36 before); the region picker paints a 136 px loupe box beside the pointer and nowhere else; `full` with
"Capture cursor" on differs from off by the XFixes pointer image. **W** (`wl_env.sh`, real native Wayland):
`--startup` sends `set_app_id("trontsnap")` + `set_minimized` and the desktop is empty; a second launch with the
window buried sends an xdg-activation token request; the X button quits; `full` with the pointer on differs from
off by exactly the sprite; the wheel grows the loupe 132 -> 180 (box measured 182 px), clamps at 96 and 528 and
persists `LoupeSize`.

- Against the pre-fix binary (`negctl/`, copied out of the running live app) it FAILS 14 of the X checks and 11 of
  the W checks (logs `parity_negctl_X.log` / `parity_negctl_W.log`); the Settings spawn count reads `delta=36`.
- Needs the app's binary rebuilt first (the rig never builds). Screenshots: `harness_out/<date>/parity_test/`
  (`x6_loupe.png`, `x7_cursor_*.png`, `w1_startup.png`, `w5_loupe_180.png`...): LOOK at them.
- Gotchas learned: the headless shell opens the Activities overview some seconds after its socket is up (an early
  Escape is lost and every click then lands in the overview: `leave_overview` polls for it); pointer/wheel
  injection needs an `abs X Y` in the SAME `wl_env.sh input` call (a fresh RemoteDesktop session has no pointer
  focus otherwise); `shot.sh` paints the pointer unless `SHOT_MOUSE=0`; a capture adds a tile to the live gallery, so
  pointer diffs look at an empty strip, not the whole frame.

## 3e. Tray test (`tray_test.sh`, ~3 min)

```
scripts/linux/harness/tray_test.sh               # own rig :86 / 48786 + a mock watcher, stops it after
scripts/linux/harness/tray_test.sh --only A      # A no tray, B/C/D the registered tray (see the header)
```

The Linux tray icon (`src/linux/sni.rs`) and close-to-tray, asserted. `sni_mock.py` is the watcher on the rig's
PRIVATE bus (what GNOME's AppIndicator extension is on the real session) and `tray_probe.py` is the HOST: it reads
the item, walks the dbusmenu (`GetLayout`), clicks (`Event`, `Activate`) and listens for `NewIcon` / `LayoutUpdated`.
`xinput.py wmclose` sends WM_DELETE_WINDOW, exactly what the X button / Alt+F4 does (Xvfb has no WM and the X button
is a custom-drawn pixel target). Covers: no watcher -> Close quits; the default `TRONTSNAP_TEST` guard never touches
the bus; late-watcher registration; icon pixmaps + WindowId type; menu labels; Close hides (process, port, ~0 CPU,
`full` over the port still captures, window unmapped not destroyed); Activate / menu Show / second launch map it
again; recording while hidden flips the label to "Stop recording", greys "Record region", badges the icon and fires
`NewIcon`, Activate stops it; the check items flip real settings and the autostart file; watcher vanishing while
hidden brings the window back and Close quits again; menu Quit exits. Needs `TRONTSNAP_TRAY=on` (a rig is
`TRONTSNAP_TEST`, which keeps the tray off so a test can't put an icon in the REAL tray); the script sets it.

What it can NOT show: what GNOME's extension and Mutter do with the item. That was checked by hand in a private
headless GNOME Shell (`wl_env.sh start`, then `gsettings set org.gnome.shell enabled-extensions
"['ubuntu-appindicators@ubuntu.com']"` on ITS bus, then the app with `env TRONTSNAP_TRAY=on`): the icon is drawn from
the ARGB pixmap, right click shows the menu (check marks, separators, a greyed item), a left click opens the menu
and a DOUBLE click is what sends `Activate`, a menu click toggles `settings.conf`, and a `record-start.sh` hook (an
`ffmpeg -re -f lavfi -i testsrc2` generator, no screen grab, no GPU) puts the red badge on the icon and the menu's
"Stop recording" ends it. Gotchas there: `wl_input.py` opens a RemoteDesktop session that shows an ORANGE pill in
the top bar, which shifts the tray icon ~62 px left while input is injected (aim at x=1204, not 1266), and a press
on that pill ends the session ("Object does not exist ... Session/uN").

## 4. Recorder test vs the LIVE app (`rec_test.sh`)

```
scripts/linux/harness/rec_test.sh                 # everything (~3 min): A B C D
scripts/linux/harness/rec_test.sh --only A --secs 8
scripts/linux/harness/rec_test.sh --no-crash      # skip D (D kill -9s + relaunches the live app)
scripts/linux/harness/rec_test.sh --only A --secs 8 [--no-sync]   # --no-sync: no fullscreen beacon window / beep
scripts/linux/harness/rec_test.sh --only C1                       # ONE real recording: stopped by capkit's hotkey, A/V sync
```

Drives the live app over port 48761 exactly like the keybinding
(`echo -n record > /dev/tcp/127.0.0.1/48761`), so it records the REAL screen,
plays the shutter and pops a toast per delivered clip. Prints PASS/FAIL/SKIP per
check; exit 1 on any FAIL.

| Case | What it proves |
|---|---|
| preflight | live app up on 48761, no kmsgrab/`@DEFAULT_MONITOR@` ffmpeg, stage dir empty; backend + audio setting read from the app env / settings.conf |
| A basic | new clip in `~/Pictures/TrontSnap`; h264 (vp8 for gnome); kms fps >= 27 (packets/duration); duration sane; audio stream iff `RecordAudio` on; `abs(video_dur - audio_dur) < 0.15s`; stage dir `~/.cache/trontsnap/rec` empty; no leftover kmsgrab / `@DEFAULT_MONITOR@` ffmpeg |
| B capkit first | `~/capkit/kms-toggle` running -> TrontSnap refuses (log line), still exactly one kmsgrab, no audio sidecar, nothing delivered, capkit's clip still completes |
| C trontsnap first | `kms-toggle` stops TrontSnap's recording -> TrontSnap still delivers a valid clip with audio + capkit's rec_*.mp4 moved out of `~/Videos/captures`; next press starts fresh and delivers normally |
| A/V sync (inside A, C1, and R) | a `rec_clock.py` beacon flashes a block and plays a 1 kHz beep at the same instant; the delivered clip's flash frame vs beep onset must read `0.14 +- 0.12 s` (the beep player's own latency is 0.12-0.16 s). Duration-only checks passed while every sound landed 1.4 s late. A also asserts a kms clip ends at the stop press (tail trimmed) |
| R region (`--only R`) | `record 0,600,1366,168` through the live app: clip exactly that size, h264, audio kept, sync as above |
| D crash salvage | marker written; `kill -9` the app (exact pid); relaunch like the lab does; clip salvaged into Pictures/TrontSnap, no orphans, stage empty, app back on the port |

Only kms-specific cases (B, C) SKIP on other backends. Cleanup: clips THIS run
created are MOVED to `harness_out/.../rec_test_clips/`, capkit files it caused to
`.../capkit_files/` (never deletes, never touches files it didn't create).
If aborted mid-recording a stray kmsgrab is stopped via `capkit-stop`.

Don't run builds or other heavy work while it runs: the fps check measures real
frame delivery on a 4-core 1.8 GHz box.

## 4b. Recorder logic WITHOUT the GPU (`rec_fake_test.py`, ~4 min, run it freely)

```
scripts/linux/harness/rec_fake_test.py [--only S1,S3] [--no-audio] [--keep]     # own rig :93 / 48793
```

The kms code path against a FAKE root helper, in an isolated rig: `fake_kms/capkit-kms` + `capkit-stop` (same contract:
`-stats` on stderr, a `kmsgrab`-argv ffmpeg process, 1 s trim, `SAVED:`; knobs in `$FAKE_KMS_DIR/cfg`: `INIT`,
`STOP_DELAY`, `MODE=empty`), a wall-clock frame source (`fake_kms/gen_frames.py`, same strip encoding as `rec_clock.py`),
and `notify_mock.py` owning `org.freedesktop.Notifications` on the rig's PRIVATE bus (logs every Notify/Close with an
epoch time, honours `replaces_id`). The rig puts `fake_bin` first on PATH (no ticks/shutter through the real speakers, `notify-send`/`xdg-open` logged to
`$FAKE_NOTIFY_LOG`/`$FAKE_OPEN_LOG`). The app runs the fakes directly only when `TRONTSNAP_TEST` is set
(`TRONTSNAP_KMS_HELPER` / `TRONTSNAP_KMS_STOP`), so a rig can never reach pkexec or the real helper.

| Case | Asserts |
|---|---|
| S1 | "Starting recording" at the press, closed when frames flow, "Saving recording" at the stop, nothing stacked; the clip's first frame vs the cue; the clip ENDS at the stop press (tail trimmed); audio stream = video length; audio sidecar cut AFTER the stop helper SIGINTed kmsgrab |
| S2 | record twice 0.2 s apart: no clip, "Recording cancelled", no orphan, next recording works |
| S3 | stop, record again 0.6 s later: no "Another recording is running" refusal, both clips delivered, stage drains |
| S4 | helper leaves a 0-byte file: "Recording too short", nothing delivered, stub removed |
| S5 | `record x,y,w,h`: clip exactly that size, audio kept; malformed region ignored |
| S6 | `trontsnap record` with NO app running starts it and the recording; a second call stops it |
| S7 | `trontsnap record region` picker, `drag` -> a 320x180 clip |
| S8 | no recorder helper thread above 0.02 core while recording (the 20 Hz /proc poll was 0.16 core) |
| S10 | tray dot: no dot in the dead time, registers when frames flow (`sni_mock.py` = a watcher on the rig bus, logs the properties a host reads), red-dot ARGB pixmaps, Activate (a click) stops the recording, the dot vanishes at the stop |
| S11 | Open on a video tile during a kms recording is deferred (no xdg-open, a "Not opening the video yet" notice); after the recording it launches the player |
| X1 | x11 backend (ffmpeg x11grab of the rig's Xvfb): full screen 1366x768 with audio as long as the video, and a region clip whose pixels equal that rectangle of the screen |

S1 screenshots (`harness_out/<date>/fake/shots/`): `1_recording.png` (title-bar `REC 00:00`), `2_saving.png`. READ them.

## 4d. GNOME screencast backend on the real session (`rec_gnome_test.py`, ~1 min, CPU only)

A throwaway TrontSnap instance (own Xvfb :93 for its window/toast/clipboard, temp HOME, `TRONTSNAP_TEST=1`,
`GSETTINGS_BACKEND=memory`, silent fake players, port 48792) that talks to the REAL session bus, so the shell really
records the real screen (VP8 on the CPU; no kmsgrab, no VCE, no video decode). G1: webm/vp8 + opus, audio as long as the
video, flash+beep content sync within 0.14 +- 0.15 s; G2: `record x,y,w,h` through `ScreencastArea` is exactly that
size. The beacon beeps once per run and the shell burns 1-2 cores while it records: run it sparingly.

## 4c. GIF export (`gif_test.py`, ~1.5 min)

`gif_test.py` exports a 30 fps mp4 and a real VFR GNOME screencast (`GIF_TEST_WEBM=<path>`; r_frame_rate=1000/1)
through the real context menu in a rig and asserts the GIF's duration equals the source's (+-0.4 s), ~12 frames per
source second, 800 px wide, finished in under 60 s. Before the fix: 108 frames x 83 ms for a 10.98 s clip (1.27x fast),
and 7 min 21 s for a 10 s VFR webm in the debug build.

## 5. Toast placement probe (`toast_probe.sh`)

```
toast_probe.sh 99          # toast in the rig (Xvfb, no WM): where does the window land?
toast_probe.sh --live      # toast on the REAL session for ~3 s (pops a real popup)
```

Spawns `trontsnap toast <png>` the way `toast::launch` does (WAYLAND_DISPLAY
dropped so it runs on X/XWayland), polls `xinput.py find "TrontSnap Toast"` for the
window's absolute root-coordinate geometry, and compares it with the intended
bottom-right dock (`screen - 340x188 - (24, 48)`). Rig mode also screenshots.

## Gotchas

- **Focus is mandatory.** No window manager on Xvfb means nothing is ever focused
  (`focus: PointerRoot`), and egui then silently drops keyboard events and
  Ctrl/Shift state (typing and ctrl+click did nothing until `focus`). `gui_env.sh
  start` focuses once; `xinput.py key/type/*click with modifiers` re-focus if
  needed. After a window is recreated run `xinput.py -d :N focus`.
- Without a WM the app window sits at +0+0 at its own size (1180x760 of the 1366x768
  screen); the right ~186 px strip is bare black root.
- The Xvfb app is a software-GL debug build: allow ~0.5 s after an input for the
  next frame before screenshotting (scripts here sleep 0.6-0.8 s).
- A plain tile click IS "copy to clipboard" (gallery.rs), so a click with a dropped
  Ctrl (see focus) silently copies instead of selecting. Ctrl+C on a selection copies
  it (1 = image, N = files); it only fires when no text field has focus.
- egui repaints on events: after a non-input state change (a file appearing,
  a thumbnail decoding) nudge the pointer 1 px or use `shot.sh --settle` before
  trusting a shot. Pixel-diff only against shots taken with `--settle`.
- The cyan chip at the top-right of the title bar (~x 1027,y 20) is always there, even
  idle; it is not part of any harness state.
- Rig and live app are both `target/debug/trontsnap`; `rec_test.sh` finds the live
  one by excluding processes with `TRONTSNAP_TEST`/`TRONTSNAP_PORT` in their env.

## Findings so far (2026-10-05, from building this)

- **Toast placement (the "popup in the wrong spot" report):** measured, not
  guessed. `toast_probe.sh --live` on the real XWayland session (1366x768, scale 1):
  `340x188+1002+532` = exactly the intended bottom-right dock, dx=0 dy=0; same in
  the Xvfb rig; and a real frame from a kms recording shows it at the bottom-right.
  So the code docks it where designed. The other popup per RECORDING is GNOME's
  `notify-send` banner ("Recording / Ctrl+Shift+PrtSc to stop"), which GNOME pins
  top-centre and the app cannot move. If "wrong" means "not where Trent wants it",
  it is a design call (the 48 px bottom margin was copied from Windows' taskbar
  gap; GNOME has no bottom bar), not a positioning bug. Verify any change with
  `toast_probe.sh --live`.
- **Delete leaves a ghost tile (repro with the rig):** `ctrlclick` a tile, press
  `Delete` -> file goes to the (temp-HOME) Trash correctly, but the gallery keeps
  "13 shots" and shows a blank grey placeholder as the newest tile. Cause (read, not
  yet fixed): `watcher.rs` forwards every `EventKind::Modify(_)`; inotify reports the
  rename-out-of-the-folder that `trash` performs as `Modify(Name(From))`, so the
  vanished path is spliced in as a "new capture". Skip non-existent paths / `Name(From)`.
- Gallery strings are Windows words on Linux: "Reveal in Explorer", "Delete
  (Recycle Bin)", "Move N shots to the Recycle Bin?".

## Clipboard / paste probes (2026-10-05 paste audit)

What can a real destination actually consume? Tools, all read-only against the
clipboard unless noted. A selection only exists while its owner lives.

- `gtk4_clip_probe.py [--tag T --out DIR]` : native GTK4 window; dumps advertised mimes,
  per-mime bytes (size+sha1), `read_texture`, `read_text`, `Gdk.FileList`. On the LIVE
  session this is a native Wayland client (needs focus; it maps a window briefly). In a rig
  run it with `GDK_BACKEND=x11 GSK_RENDERER=cairo`. HARD-WON: reads are async; a sync stream
  read on X11 deadlocks the main loop.
- `gtk3_text_probe.py [--window]` : what VTE / gedit-style GTK3 text consumers get
  (`text`, `uris`, `image_available`). `GDK_BACKEND=wayland --window` = gnome-terminal's path.
- `clip_owner_proto.py OFFER...` : PROTOTYPE of a one-process multi-target owner
  (`image/png=@file`, `text/uri-list=uri:/path`, `x-special/gnome-copied-files=gnome:/path`,
  `text/plain;charset=utf-8=path:/path`). xclip/wl-copy can only serve one type.
- `ff_paste_case.sh N WINID NAME -- <owner cmd>` : sets the clipboard, ctrl+v into
  `paste_page.html` in a Firefox running on rig :N, screenshots the page's report of
  `clipboardData.types/items/files`. Snap Firefox needs the rig launched as
  `snap run --shell firefox -c "DISPLAY=:N GDK_BACKEND=x11 MOZ_ENABLE_WAYLAND=0 .../firefox --no-remote --profile P file://.../paste_page.html"`
  (plain `firefox` says "cannot open display"; the snap wrapper must be bypassed).
  Focus it with `xinput.py -d :N focus 0x<winid>` (its X name is empty).

## Windows compile check (`wincheck.sh`) and the real-Wayland rig (`wl_env.sh`) (2026-10-05 parity audit)

`wincheck.sh` = `cargo check --target x86_64-pc-windows-gnu --workspace --bins --tests` from this Linux box
(rust-std for the target is already installed; no mingw: check never links, and a fake `windres` + host `ar`
satisfy winresource, so `build.rs` is untouched). Own `CARGO_TARGET_DIR` (`harness_out/<date>/wincheck-target`,
~1.3 GB, never takes the lock on `target/`). Cold ~10 min, warm ~10 s-1.5 min. Proven non-vacuous: an error
injected into a `cfg(windows)` fn is caught, one injected into a `cfg(not(windows))` fn is invisible to it.
It proves the tree COMPILES for Windows (gnu ABI, not msvc); it cannot run anything. Run it before pushing any
shared-code edit (anything outside `src/linux/`). CI (`.github/workflows/ci.yml`) is the real Windows build.

`wl_env.sh` = a private headless GNOME Shell (mutter 46) so TrontSnap runs as a genuine native-Wayland client,
with scripted pointer/keys (`wl_env.sh input ...` over mutter's RemoteDesktop D-Bus API) and screenshots
(`wl_env.sh shot out.png`). Xvfb cannot reproduce Wayland: winit-wayland's `set_visible` is a no-op,
`set_minimized(false)` is ignored, `focus_window` is empty, and no `app_id` is sent.
```
H/wl_env.sh start harness_out/<date>/homes/94   # optional seed home (Pictures + settings)
(setsid H/wl_env.sh run target/debug/trontsnap > app.log 2>&1 < /dev/null &)   # port 48794, TEST=1, x11 recorder
H/wl_env.sh input key Escape                    # leave the Activities overview once
H/wl_env.sh input abs 1342 52 wait 0.5 click    # walk the pointer to X,Y and click
H/wl_env.sh input abs 700 400 wheel 1           # wheel notch (positive = forward/up); needs the abs in the same call
H/wl_env.sh shot out.png ; H/wl_env.sh stop     # kill the client by its port pid first
```
Safety: headless mutter opens the radeon render node (no modesetting) with `LIBGL_ALWAYS_SOFTWARE=1`; no video
decode is involved, but check `journalctl -k -b --since "-2 min" | grep -i -E "uvd|vce|radeon.*error|hung"` after
`start` and stop at the first hit. XWayland clients do not work in it (no systemd --user on the private bus), so the
toast (which strips WAYLAND_DISPLAY to dock on XWayland) cannot be tested there. Never pin the pointer top-left (hot corner).


## Recorder probes on the REAL session (2026-10-05 recording audit)

`rec_test.sh` proves the plumbing; these measure what the user gets. All talk to the
live app on 48761 (or to `org.gnome.Shell.Screencast`), so they record the real screen.

```
rec_lat.py  --secs 12 --out DIR [--restart-probe 0.4] [--move-pointer X Y AT]
                                  press -> first frame in the clip, stop -> file/toast, A/V sync
                                  (flash + 1 kHz beep vs the clip), notifications, cursor-in-clip
rec_perf.py --backend kms|gnome --secs 10 --out DIR [--noise 0]
                                  CPU per process / per app thread + fps + lost source frames under
                                  generated full-screen motion (frame counter read back out of the clip)
rec_race.py --gap 0.2 --out DIR   press record twice 0.2 s apart; what is left running / delivered
gnome_area_probe.py X Y W H --out DIR   ScreencastArea region recording (CPU-only VP8) + cursor check
rec_clock.py                      fullscreen wall-clock beacon (ms digits + 40-bit binary strip) used by rec_lat
```

MACHINE SAFETY (learned the hard way, this box hard-reset once):
- Never play video with hardware decode while kmsgrab encodes: on this radeon (Mullins)
  mpv `--hwdec=vaapi` hung the UVD ("UVD not responding"), then
  `radeon_dpm_change_power_state_locked` deadlocked and the desktop froze. The motion source
  here is `ffplay -f lavfi` (generated, nothing decoded). Do not add a media player.
- Real kms recordings: at most 3 per agent, <= 15 s each, >= 60 s apart, never while
  anything else records. After GPU work: `journalctl -k -b --since "-5 min" | grep -i -E "uvd|vce|radeon.*error|hung"`.
- The gnome backend is CPU-only VP8 here (no gst-plugins-bad, so no VA encoder), so
  `--backend gnome` never touches the VCE.
- A relative template path makes GNOME Shell write under `~/Videos/`; both scripts pass an
  absolute one.

## Everything at once, and the crash/notice additions (2026-10-06)

```
run_all.sh [--only selftest,paste,gallery,parity,tray,notice,recfake] [--skip ...]   # RIG_N=95 RIG_PORT=48795
```

Runs the suites above sequentially (ONE rig at a time: 6 GB RAM), logs each to `$HARNESS_OUT/run_all/<suite>.log`,
checks the kernel log for `uvd|vce|radeon.*error|hung` at the end, writes `summary.txt`, exits 1 on any failure.
Refuses to start if its display/port is busy. `rec_test.sh` (real screen, real VCE, capped) is deliberately NOT in it.
`selftest.sh` with no arguments walks up from :97 to the first free display (an orphan Xvfb used to block it).

- `RIG_KEEP_BUS=1 gui_env.sh start ...`: the app is a child of a waiting shell instead of the `dbus-run-session`
  payload, so `kill -9 app` leaves the private bus and its mocks alive (crash tests relaunch with
  `gui_env.sh run N env TRONTSNAP_TRAY=on target/debug/trontsnap --startup`). `rec_fake_test.py` always sets it.
- `rec_fake_test.py` S12-S16: no ffprobe on PATH (`fake_noprobe/ffprobe` exits 127), crash + relaunch + `record`
  0.3 s after bind (the two clips are told apart by the wall clock inside their frames), what the salvage may touch,
  `record region` during a recording, one-shot notices closing / failed and hung grabs. `RIG_N` / `RIG_PORT` pick the rig.
  Negative control: `TRONTSNAP_BIN=<old binary>` fails them (14 of the 21 checks in S12-S15 failed on the pre-fix binary).
- `notice_wl_test.sh`: a private headless GNOME Shell shows a plain `notify-send -t 2000` banner (still up 9 s later:
  GNOME holds banners until user input) and `trontsnap ocr`'s notice (gone by itself 9 s later). Read the PNGs.
- `<cache>/trontsnap/test_trash_ms` holding a number (TRONTSNAP_TEST instances only, read per delete batch) stretches
  each 200-file trash chunk by that many ms, so `gallery_test.sh` G13 can land a second Delete while a batch is
  provably still moving (the test writes it for that one step and removes it).
- `hotkey_wl_test.sh`: the five GNOME bindings PRESSED. A private headless GNOME Shell + a private `gsd-media-keys`
  (it complains about the missing session manager and works anyway) + the app without the TEST guard (it installs its
  bindings into that session's dconf) + real key events through mutter's RemoteDesktop (`wl_input.py key
  ctrl+shift+Print`). Asserts: Print -> a 1366x768 PNG, Ctrl+Shift+Print x2 -> a clip (hook recorder, `ffmpeg -re -f
  lavfi`), Ctrl+Shift+Alt+Print during a recording -> no picker and the clip is delivered, Ctrl+Alt+Print -> the OCR
  notice, Ctrl+Print / Ctrl+Shift+Alt+Print -> a picker process. Pickers are closed with a right click: nothing in the
  headless shell gets keyboard focus for a freshly mapped window (zenity ignores Escape too), so a KEY cannot reach it.
