# Installing TrontSnap

Download from the [latest release](https://github.com/TrentSterling/trontsnap/releases/latest).
Each release lists a `SHA256SUMS` file; `sha256sum -c SHA256SUMS --ignore-missing` (Linux)
or `Get-FileHash` (Windows) checks what you downloaded.

| You are on | Get this | Notes |
|---|---|---|
| Windows, just try it | `TrontSnap.exe` | portable, no admin, nothing installed |
| Windows, full install | `TrontSnap-vX.Y.Z-windows.zip` | adds a signed hotkey broker so bare `PrtSc` works over elevated windows |
| Linux, any distro | `TrontSnap-vX.Y.Z-x86_64.AppImage` | one file, can update itself |
| Debian / Ubuntu | `trontsnap_X.Y.Z-1_amd64.deb` | installs to `/usr/bin`, upgraded by installing the next one |
| Linux, no root | `trontsnap-X.Y.Z-linux-x86_64.tar.gz` | `install.sh` puts it in `~/.local` |

The Linux packages are new. AppImage self-update (AppImageUpdate) and in-place `.deb` upgrades are
designed and partly tested, but not yet proven across two real releases; if one misbehaves, the
manual route always works: download the new file and replace the old one.

Why two Windows modes, and what the broker is: [SETUP.md](SETUP.md). Linux specifics and the
list of what is not done yet: [LINUX.md](LINUX.md).

---

## Windows

### Portable

1. Put `TrontSnap.exe` somewhere permanent, for example `%LOCALAPPDATA%\TrontSnap\`.
2. Run it. It adds itself to start with Windows on the first run (toggle it from the tray menu).

### Installed

1. Unzip `TrontSnap-vX.Y.Z-windows.zip` and double-click **`Install TrontSnap.cmd`**.
2. Approve the one administrator prompt.

It removes a portable copy in `%LOCALAPPDATA%\TrontSnap` (two copies would fight over the same
hotkey), copies both exes to `%ProgramFiles%\TrontSnap`, signs them, points autostart at the
install and launches it. No reboot. Details and the log (`C:\ProgramData\TrontSnap\bootstrap.log`)
are in SETUP.md.

Requirements for this mode: Windows 10/11 and **`signtool.exe` from the Windows SDK** (it signs
the broker on your PC; the release is deliberately shipped unsigned because a key made anywhere
else would not be trusted here). The installer also trusts a self-signed `TrontSnap Dev`
code-signing certificate on your machine, which is how Windows agrees to grant the broker
uiAccess. Portable needs neither.

### Upgrade

- **Portable:** quit TrontSnap (tray icon > Quit), replace `TrontSnap.exe`, run it. Settings and
  captures are untouched.
- **Installed:** unzip the newer bundle and run **`Install TrontSnap.cmd`** again. It closes the
  running app, replaces both exes, signs, and relaunches. Your settings, captures and thumbnail
  cache are kept.
- **Downgrade protection:** if the bundle you run is older than what is installed, the installer
  stops and changes nothing ("refusing to DOWNGRADE"). That is almost always an old unzipped
  folder. To roll back on purpose, run `bootstrap.ps1 -AllowDowngrade` from an elevated
  PowerShell.

### Uninstall

- **Portable:** delete the exe (and the `TrontSnap` value under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` if you let it autostart).
- **Installed:** `Revert to portable.cmd` goes back to the portable build; to remove
  everything, quit the tray app, delete `%ProgramFiles%\TrontSnap` and that `Run` value. The
  `TrontSnap Dev` certificate stays in the machine store; remove it in `certlm.msc` (Trusted Root
  Certification Authorities and Trusted Publishers) if you want it gone.

Captures and settings are never deleted by any of this.

### Hotkeys on Windows

TrontSnap registers them itself with `RegisterHotKey` (the installed broker does it for you in
installed mode). Defaults, all rebindable in **Settings > Hotkeys**:

| Keys | Does |
|---|---|
| `PrtSc` | full screen |
| `Ctrl+PrtSc` | region |
| `Ctrl+Shift+PrtSc` | start / stop recording |
| `Ctrl+Alt+PrtSc` | copy text from a region (OCR) |

Windows will not deliver a bare `PrtSc` to a normal process while an *elevated* window has focus;
that is what installed mode fixes. Portable users can bind a combo with a modifier instead.

### Where things live (Windows)

| What | Where |
|---|---|
| Captures | `Pictures\TrontSnap` in your user profile |
| Settings | registry, `HKCU\Software\TrontSnap` |
| Thumbnail cache | `%LOCALAPPDATA%\TrontSnap\thumbs` |
| App log | `%LOCALAPPDATA%\TrontSnap\trontsnap.log` |
| Installed binaries | `%ProgramFiles%\TrontSnap` |

---

## Linux

### What it needs

TrontSnap shells out to a few small tools instead of bundling them. The `.deb` pulls the
required ones in for you; the AppImage and tarball do not.

| Tool | For | Without it | Install |
|---|---|---|---|
| `ffmpeg` | recording, GIF export, video thumbnails | those three features | `sudo apt install ffmpeg` |
| `xclip` or `wl-clipboard` | putting the capture on the clipboard | the PNG is still saved, but nothing is copied | `sudo apt install xclip wl-clipboard` |
| `gnome-screenshot`, `grim` or `spectacle` | screen grab fallback when the compositor will not talk to the built-in grabber | capture can fail on Wayland | usually preinstalled on GNOME |
| `tesseract-ocr` | copy text from a region (optional) | OCR is unavailable | `sudo apt install tesseract-ocr` |
| `paplay`, `pw-play` or `aplay` | shutter sound | silent captures | preinstalled almost everywhere |

It also needs an X11 or XWayland session (every desktop has one) and the usual GL / xkbcommon
client libraries. The AppImage and tarball need a glibc at least as new as the machine that
built them (CI builds on Ubuntu 22.04; see the release notes).

On a typical Ubuntu desktop: `sudo apt install ffmpeg xclip tesseract-ocr`.

### AppImage

```
chmod +x TrontSnap-vX.Y.Z-x86_64.AppImage
./TrontSnap-vX.Y.Z-x86_64.AppImage
```

If it dies with a FUSE error, run it with `--appimage-extract-and-run` (no FUSE needed). On first
run it keeps a copy of itself at `~/.local/share/trontsnap/bin/trontsnap`, because the GNOME
keybindings and autostart entry it creates must point at a path that exists next time: you can
keep the AppImage anywhere. **Upgrade:** AppImageUpdate (or `appimageupdatetool <file>`) pulls
the newest release as a delta; or download the new AppImage and delete the old one. Restart a
running TrontSnap to pick it up.

### .deb

```
sudo apt install ./trontsnap_X.Y.Z-1_amd64.deb      # the leading ./ matters
```

**Upgrade:** `sudo apt install ./trontsnap_<newer>-1_amd64.deb`; the running app is asked to exit
first, restart it afterwards. **Remove:** `sudo apt remove trontsnap`. That leaves your per-user
autostart entry and GNOME keybindings (dpkg cannot reach your home); clean them as under
"Uninstall" below.

### Tarball

```
tar xf trontsnap-X.Y.Z-linux-x86_64.tar.gz
cd trontsnap-X.Y.Z-linux-x86_64
./install.sh                    # ~/.local/bin/trontsnap + menu entry; PREFIX=/opt/ts ./install.sh to move it
```

It tells you which of the tools above it did not find. **Upgrade:** unpack the newer tarball and
run `./install.sh` again (safe while the app is running; restart it afterwards). **Uninstall:**
`~/.local/share/trontsnap/uninstall.sh` removes the files, the autostart entry and the GNOME
keybindings and gives the Print key back to GNOME (`--keep-desktop-settings` to leave those).

### Hotkeys on Linux

Wayland has no way for an app to grab a global key, so TrontSnap does not try. Instead, on
**GNOME** it installs four custom keybindings the first time the app is launched (and clears
GNOME's own bare-`Print` screenshot UI binding so `Print` is free):

| Keys | Runs |
|---|---|
| `Print` | `trontsnap full` |
| `Ctrl+Print` | `trontsnap region` |
| `Ctrl+Shift+Print` | start / stop recording in the running app |
| `Ctrl+Alt+Print` | `trontsnap ocr` |

The one-shot commands (`full`, `region`, `ocr`) work with the app closed. Recording needs the
app running (it starts at login, hidden).

On **any other desktop** (KDE, XFCE, Sway, ...) bind the same commands yourself in the keyboard
settings. Use the full path of your install: `/usr/bin/trontsnap` (deb),
`~/.local/bin/trontsnap` (tarball) or `~/.local/share/trontsnap/bin/trontsnap` (AppImage, after
its first run). The record toggle is:

```
bash -c "echo -n record > /dev/tcp/127.0.0.1/48761"
```

The Settings window's hotkey editor speaks Windows key codes and does not rebind the Linux
shortcuts; change those in your desktop's own keyboard settings.

### Where things live (Linux)

| What | Where |
|---|---|
| Captures | `~/Pictures/TrontSnap` (your XDG Pictures folder; `~/TrontSnap` if there is none) |
| Settings | `~/.config/trontsnap/settings.conf` (plain `Key=value`) |
| Autostart | `~/.config/autostart/trontsnap.desktop` |
| Thumbnail cache | `~/.cache/TrontSnap/thumbs` |
| In-progress recordings | `~/.cache/trontsnap/rec` (moved to Captures when finished) |
| App log | `~/.local/share/trontsnap/trontsnap.log` |
| Optional recorder hook | `~/.config/trontsnap/record-start.sh` (+ `record-stop.sh`) |

### Uninstall (Linux)

Captures and settings are never deleted. By package:

- **tarball:** `~/.local/share/trontsnap/uninstall.sh`.
- **.deb:** `sudo apt remove trontsnap`, then the per-user cleanup below.
- **AppImage:** delete the file, `rm -r ~/.local/share/trontsnap/bin`, then the per-user cleanup.

Per-user cleanup (deb and AppImage): delete `~/.config/autostart/trontsnap.desktop`, delete the
four `trontsnap-*` custom shortcuts in Settings > Keyboard > Keyboard Shortcuts > Custom Shortcuts, and give
Print back to GNOME with `gsettings reset org.gnome.shell.keybindings show-screenshot-ui`.

---

## Troubleshooting (Linux)

**The capture toast is in the wrong place.** Wayland compositors ignore the position an app asks
for. TrontSnap works around it by running the toast as an XWayland window, which can be placed:
it docks bottom-right with a margin. That needs XWayland (`echo $DISPLAY` should print
something). The main window's position is the compositor's call and cannot be controlled. The
other popup you see when recording starts ("Recording ... to stop") is a GNOME notification and
GNOME pins those top-centre. A second launch of TrontSnap cannot un-minimize its window under
Wayland; restore it from the dock.

**Nothing is on the clipboard after a capture.** The capture is saved either way. Clipboard
writes go through `wl-copy` (Wayland) or `xclip`, because the clipboard has to be owned by a
process that outlives the one-shot capture. Ubuntu's GNOME images often have `xclip` but not
`wl-clipboard`; with neither, the log says `no clipboard tool`. Fix: `sudo apt install xclip`
(`wl-clipboard` too if you like: `wl-copy` is used first on Wayland, `xclip` is the fallback and
works through XWayland). Shots paste as an image, recordings as a file, OCR as text.

**`Print` still opens GNOME's screenshot UI, or the shortcuts are missing.** The bindings are
written the first time the app itself runs (not by `trontsnap full` alone), and only on GNOME. Open
TrontSnap once from the app menu. To check or restore the stock key:
`gsettings get org.gnome.shell.keybindings show-screenshot-ui`.

**Hotkeys stopped working after I moved or deleted the app.** The bindings and autostart entry
hold the app's absolute path. Start the app once from its new location (tarball: re-run
`./install.sh`) so they are rewritten.

**Recording on GNOME.** TrontSnap picks a recorder in this order and logs it
(`trontsnap: recording via ...` on stderr; run `trontsnap` from a terminal to see it):

1. your own `~/.config/trontsnap/record-start.sh <output-file>` (plus optional `record-stop.sh`),
   if present; TrontSnap still handles start/stop, clipboard and the toast around it;
2. a hardware-encode helper (`/usr/local/bin/capkit-kms`) that only exists on the author's
   machine; you will not have it and do not need it;
3. **GNOME Wayland: GNOME Shell's own screencast** over D-Bus. No root, no setup, but the shell
   encodes on the CPU (VP8 `.webm`), so a weak machine records at a low frame rate;
4. **X11:** `ffmpeg` x11grab + libx264 (`.mp4`).

Non-GNOME Wayland has no built-in recorder: use the hook script. `TRONTSNAP_RECORDER=hook|gnome|x11`
forces one. "Recording didn't start" notifications carry the reason (missing `ffmpeg`, GNOME
refusing a second screencast, ...). If TrontSnap dies mid-recording, the next launch rescues the
clip into your captures folder.

**No audio in recordings.** Turn on "Record audio" in Settings. It records *system audio* (what you
hear: the default output's monitor, via PulseAudio or PipeWire's Pulse layer) with `ffmpeg`, not a
microphone, and muxes it in after you stop. Test the source with
`ffmpeg -f pulse -i @DEFAULT_MONITOR@ -t 3 /tmp/t.wav`; if that fails, so will TrontSnap. Audio is
best-effort: a recording without sound beats no recording.

**The AppImage will not start.** Try `./TrontSnap-...AppImage --appimage-extract-and-run`. If the
window never appears at all, check for an X11/XWayland session and the GL libraries.

**Something else is using port 48761.** TrontSnap uses it for single-instance and the record
toggle. Set `TRONTSNAP_PORT` for the app and the record shortcut if you must change it.
