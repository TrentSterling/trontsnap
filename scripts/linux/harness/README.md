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
- `TRONTSNAP_TEST=1` = never touch the session (keybindings/autostart).

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

## 4. Recorder test vs the LIVE app (`rec_test.sh`)

```
scripts/linux/harness/rec_test.sh                 # everything (~3 min): A B C D
scripts/linux/harness/rec_test.sh --only A --secs 8
scripts/linux/harness/rec_test.sh --no-crash      # skip D (D kill -9s + relaunches the live app)
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
| D crash salvage | marker written; `kill -9` the app (exact pid); relaunch like the lab does; clip salvaged into Pictures/TrontSnap, no orphans, stage empty, app back on the port |

Only kms-specific cases (B, C) SKIP on other backends. Cleanup: clips THIS run
created are MOVED to `harness_out/.../rec_test_clips/`, capkit files it caused to
`.../capkit_files/` (never deletes, never touches files it didn't create).
If aborted mid-recording a stray kmsgrab is stopped via `capkit-stop`.

Don't run builds or other heavy work while it runs: the fps check measures real
frame delivery on a 4-core 1.8 GHz box.

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
  Ctrl (see focus) silently copies instead of selecting. There is no Ctrl+C handler
  in the gallery grid; use click / context menu "Copy to clipboard".
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
