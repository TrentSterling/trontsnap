# Linux packaging

Everything here turns an already-built `target/release/trontsnap` into release
artifacts. Nothing in this directory runs cargo (except `appimage.sh --build`, a
local convenience).

```
packaging/linux/build-all.sh --version X.Y.Z --out dist/ [--appimage-tool PATH] [--require-zsync]
```

| Output (in `--out`) | Built by | For |
|---|---|---|
| `TrontSnap-vX.Y.Z-x86_64.AppImage` + `.zsync` | `appimage.sh` | any distro; self-updating via AppImageUpdate |
| `trontsnap_X.Y.Z-1_amd64.deb` | `build-deb.sh` | Debian/Ubuntu; installs `/usr/bin/trontsnap` |
| `trontsnap-X.Y.Z-linux-x86_64.tar.gz` | `build-tarball.sh` | everyone else; `install.sh` puts it in `~/.local` |
| `SHA256SUMS` | `build-all.sh` | the four files above |

`--version` must equal Cargo.toml's `[package]` version (override:
`--allow-version-mismatch`). Each script also runs alone with `--version/--out`.
`TRONTSNAP_BIN=/path` packages a different binary (that is how the scripts are
tested without cargo). `SOURCE_DATE_EPOCH` pins mtimes; the `.deb` and tarball are
byte-for-byte reproducible for the same binary and commit.

## Rules the release process has to keep

- **Every GitHub release must carry the Linux assets**, `.zsync` included. The
  AppImage's embedded update string is
  `gh-releases-zsync|TrentSterling|trontsnap|latest|TrontSnap-*x86_64.AppImage.zsync`;
  if a Windows-only release becomes "latest", AppImageUpdate finds nothing.
- **Build the binary on the oldest Ubuntu you want to support.** Nothing is
  bundled, so the glibc of the build machine is the floor. The `.deb` reads it off
  the binary (`libc6 (>= x.y)`).
- CI needs `curl`, `dpkg-deb`, `tar`, `gzip`, `python3` (icons), network (the
  appimage tool is downloaded pinned + sha256-verified into the gitignored
  `tools/`, and appimagetool fetches its runtime). `desktop-file-utils` and
  `appstream` make `build-all.sh` validate the metadata; without them it warns.
- Keep `docs/media/*.png` on `main`: the AppStream screenshots point at them.

## Why things are the way they are

- **Stable exe path.** TrontSnap writes its GNOME keybindings and autostart entry
  from `current_exe()`. Inside an AppImage that is a random `/tmp/.mount_*` path,
  so `AppRun` keeps a copy of the binary at
  `~/.local/share/trontsnap/bin/trontsnap` and runs that (refreshed on upgrade;
  `TRONTSNAP_NO_STABLE_COPY=1` opts out). The deb is `/usr/bin`, the tarball
  `~/.local/bin`; neither may move between releases.
- **dlopen'd libs are listed by hand** in the deb's `Depends` (winit/glutin load
  Wayland, xkbcommon, X11 cursor and GL libs at runtime, so `dpkg-shlibdeps` can't
  see them).
- **`APPIMAGE_EXTRACT_AND_RUN` must not leak into the update-string check.** CI sets it
  job-wide so appimagetool runs without FUSE, but with it set an AppImage extracts and
  runs the app instead of answering `--appimage-updateinformation`. `appimage.sh` unsets
  it for that one call; keep it that way if you touch the check. Never run a built
  AppImage with your real `$HOME` while testing with a stand-in binary: `AppRun` copies
  the binary to `~/.local/share/trontsnap/bin/trontsnap`.
- **Desktop file is installed as `xyz.tront.TrontSnap.desktop`** everywhere (deb,
  AppImage, tarball, Flatpak) to match the AppStream id. `StartupWMClass=trontsnap`
  ties windows to it (winit's X11 class is the exe name).
- The Flatpak manifest is experimental and not built by `build-all.sh`; its header
  says exactly what does not work in the sandbox.

## Not decided / not verified (read before releasing)

- **No LICENSE in the repo.** `copyright` and the metainfo `project_license` say
  "all rights reserved / LicenseRef-proprietary" because that is all the tree
  supports. Pick a licence and replace both.
- `.deb` Maintainer defaults to the repo author; override with `DEB_MAINTAINER`.
- No per-crate third-party notices are generated (`copyright` points at Cargo.lock).
- Uninstalling the `.deb` leaves the per-user autostart entry and GNOME
  keybindings behind (dpkg can't reach `$HOME`). The tarball's `uninstall.sh`
  removes them and gives Print back to GNOME; for a deb install run
  `gsettings reset org.gnome.shell.keybindings show-screenshot-ui` and delete the
  `trontsnap-*` entries in Settings > Keyboard.
