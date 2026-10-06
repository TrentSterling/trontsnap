# Windows handoff: the `linux-port` branch

For the Claude session on the Windows box ("Test"). Your own CLAUDE.md is about other
work (Bedrock mods); this is a different project: **TrontSnap**, a Rust eframe/egui
screenshot tool, repo `TrentSterling/trontsnap`. It is a checklist, not an essay. Nothing
below is pre-approved: pushing, merging and releasing happen when Trent says so.

## What this branch is

`linux-port` adds an Ubuntu/Linux build of TrontSnap and a cross-platform release pipeline.
`main` has not moved since the branch point (`86944f8`, v0.25.0), so it fast-forwards.
Size as of 2026-10-05 (re-run `git diff main...linux-port --stat | tail -1`; it keeps growing):
about 76 files, 7,600 insertions, 200 deletions, of which most is new Linux-only code,
packaging, CI, docs and a test harness. Linux itself is documented in `LINUX.md`; install paths in `INSTALL.md`;
the release flow in `RELEASING.md`.

The branch has been built and run on Ubuntu only. **No line of the Windows build has been
compiled since the refactor** apart from whatever CI's `windows-latest` job reports once the
branch is pushed. That is your job.

## What changed in shared code, and the `cfg` boundary

`git diff main...linux-port --stat -- src hotkeyd Cargo.toml Cargo.lock` is the whole
Windows-relevant surface. **Every Win32 module not in that list is byte-identical to `main`**
(`clipboard.rs`, `keyhook.rs`, `recorder.rs`, `overlay.rs`, `region_win32.rs`, `audio.rs`,
`ocr.rs`, `autostart.rs`, `printscreen.rs`, `cursor.rs`, `shellexec.rs`, `videothumb.rs`).

| Piece | What it is |
|---|---|
| `src/linux/*.rs` | Linux twins of those modules, same names and public API. `main.rs` picks the file with `#[cfg(windows)] mod x;` vs `#[cfg(not(windows))] #[path = "linux/x.rs"] mod x;`. Windows never compiles them |
| `src/main.rs` | the module table above; `audio` is `cfg(windows)` only; `kvstore` and `shutter_wav` are new shared modules |
| `src/kvstore.rs` | settings store. Windows branch is the **same registry key and value names/types** as before (`HKCU\Software\TrontSnap`, DWORD / REG_SZ); Linux writes `~/.config/trontsnap/settings.conf` |
| `src/settings.rs` | every `RegKey` call replaced by `kvstore::get_*/set_*`; no value names changed |
| `src/shutter_wav.rs`, `src/sound.rs` | the synthesized shutter WAV moved out of `sound.rs` into a shared module; Windows `sound.rs` now calls `crate::shutter_wav::build_shutter_wav` |
| `src/app.rs` | the biggest edit: the whole tray block is now `#[cfg(windows)] let (tray, auto_i, cursor_i, audio_i, menu_rx) = { ... };`, `hwnd_cell`/`show_flag` were hoisted above it, tray/broker fields and helpers are `#[cfg(windows)]`, `PORT` became `port()` (honours `TRONTSNAP_PORT`, default still 48761), Windows close-to-tray is unchanged |
| `src/toast.rs` | the Win32 polling/positioning helpers are `#[cfg(windows)]`; Linux gets its own `launch` and positioning. Windows paths should be unchanged |
| `src/capture.rs` | `grab_primary` now wraps `grab_xcap`; on Windows the error path is returned as before |
| `src/index.rs`, `src/watcher.rs` | `is_video` also accepts `webm`, but only `cfg!(not(windows))`; mp4 behaviour unchanged |
| `src/theme.rs`, `src/gallery.rs` | `windows_prefers_dark` gets a Linux twin; `reveal` gets a non-Windows body |
| `Cargo.toml` | `tray-icon`, `winreg`, `drag` (and the existing `windows` crate block) moved under `[target.'cfg(windows)'.dependencies]`; `zbus` is Linux-only; `opener` gains the `reveal` feature everywhere |
| `hotkeyd/` | deps gated to `cfg(windows)`; `src/main.rs` body wrapped in `mod broker { ... pub fn main_impl() }` with `#[cfg(windows)] fn main() { broker::main_impl() }` and an empty non-Windows `main` |
| `Cargo.lock` | `zbus` added under `trontsnap`, `url` under `opener` (both unused on Windows) |
| `.github/`, `scripts/release.*`, `packaging/linux/`, docs | new: CI, release workflow, release scripts, Linux packaging. No Windows runtime code |

## Verify nothing regressed (do this first, in order)

Use the checkout that built v0.25.0 so the incremental `target\` is warm. If `linux-port` is not
on `origin` yet, Trent has to push it from the Linux box first (nothing here pushes for you).

```
git fetch origin
git switch linux-port
git diff main...linux-port --stat -- src hotkeyd Cargo.toml Cargo.lock    # the surface above

cargo build --release --locked -p trontsnap -p trontsnap-hotkeys          # must not touch Cargo.lock
cargo test  --locked --workspace                                          # v0.25.0 baseline: 49 passed, 0 failed
```

- `--locked` passing proves the lockfile matches both manifests with the new gating.
- 49 is the count the v0.25.0 changelog reports. A **lower** number means a Windows test got
  cfg'd out by mistake (check `src/app.rs`: `tray_tests` is `cfg(all(test, windows))`, it must
  still run here). Higher is fine.
- Sanity: the new `trontsnap.exe` should be about the size of the released one
  (v0.25.0: 7,046,656 bytes), and `cargo tree -p trontsnap | findstr /i zbus` should print nothing (zbus is Unix-only).
- The two manifests must not have swapped (the check `bootstrap.ps1` and CI both rely on).
  Each line must print `True`:

  ```
  $ui = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes("$PWD\target\release\trontsnap.exe"))
  $hk = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes("$PWD\target\release\trontsnap-hotkeys.exe"))
  $ui -match 'uiAccess="false"'; $hk -match 'uiAccess="true"'
  ```

Then smoke it as a person would. CI cannot do any of this; the two verifier scripts cover the
install, the rest is by hand.

1. **Portable run** (`target\release\trontsnap.exe`): the gallery opens on your real history and
   your **existing settings are intact** (theme, gradient, hotkeys, "Record audio"). That is the
   kvstore parity check; if settings look reset, the registry branch regressed.
2. Hotkeys: `PrtSc`, `Ctrl+PrtSc` (region), `Ctrl+Shift+PrtSc` (record, stop, plays back with
   audio), `Ctrl+Alt+PrtSc` (OCR). Paste the capture into Discord. The toast docks bottom-right,
   dismisses, and drag-out from it works.
3. Tray: close-to-tray, double-click restore, Quit. mp4s show as film tiles; Delete goes to the
   Recycle Bin; Reveal opens Explorer with the file selected.
4. **Installed mode**: `packaging\Install TrontSnap.cmd` (from the checkout, or the release zip
   once there is one), then, non-elevated:

   ```
   powershell -NoProfile -ExecutionPolicy Bypass -File .\packaging\verify-install.ps1
   powershell -NoProfile -ExecutionPolicy Bypass -File .\packaging\verify-window-lifecycle.ps1
   ```

   `verify-install.ps1` checks signatures, both manifests, UI at Medium without uiAccess, the
   broker running with it, ports 48761/48771, autostart, and a real capture to the clipboard
   (it replaces your clipboard). `verify-window-lifecycle.ps1` moves the real mouse, so leave
   the desktop alone while it runs. Both must exit 0.
5. Optional: `packaging\Revert to portable.cmd` and back, to prove the round trip.

If the branch has been pushed, **check Actions first**: `ci.yml` builds and tests on
`windows-latest` on every push to `linux-port`, which is the first compile of this refactor
on Windows. A red Windows job there is the answer to "what regressed".

## What to look for in review

- `src/app.rs` tray block: handlers still capture `hwnd_cell` / `show_flag` / `ctx`; the tuple
  returned from the block is destructured into the same struct fields; nothing Windows-only
  lost its `#[cfg(windows)]` and nothing shared gained one.
- `src/kvstore.rs` Windows `imp`: key `Software\TrontSnap`, DWORD for `get/set_u32`, string for
  the others, creating the key on write; and that `settings.rs` still names every value it
  used to (diff the `*_VALUE` constants against `main`).
- `src/toast.rs`: the Windows branch of `update()` still calls `position_bottom_right` and uses
  `cursor_over_window` / `left_button_down` exactly as before.
- `hotkeyd/src/main.rs`: `#![windows_subsystem = "windows"]` is still at crate level (above the
  `mod broker`), and the broker still builds with the uiAccess manifest from `hotkeyd/build.rs`.
- `Cargo.toml`: no Windows dependency lost its features while moving; `[patch.crates-io]`
  (the vendored eframe) untouched.
- Anything that says "HARD-WON" in the new code is a real bug someone already hit on Linux;
  don't "simplify" it away on the Windows side.

## Merge to `main`

When the checks above pass and Trent says go: open a PR `linux-port` -> `main` (CI runs on PRs;
`main` is still at the branch point, so a plain merge or fast-forward both work) and merge it on
GitHub, then `git switch main && git pull --ff-only`. Do not squash if per-commit history of
the Linux twins is wanted; ask.

## Cut the first cross-platform release (v0.26.0 proposed)

Read `RELEASING.md` first; this is the short path. The Windows asset set (`TrontSnap.exe`,
`TrontSnap-v0.26.0-windows.zip`, `Rajdhani-OFL.txt`) is now built by CI to the same layout as the
hand-built v0.25.0 zip, and the Linux assets ride along.

1. On `main`: add `## v0.26.0` to the top of `CHANGELOG.md` (Linux port, packaging, release
   pipeline; no Windows behaviour change; end with a `Verification:` line like v0.25.0's) and
   commit it.
2. Dry run, then for real (Windows PowerShell 5.1 is enough):

   ```
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 0.26.0 -DryRun
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 0.26.0
   ```

   `release.ps1` has never been executed (it was written and reviewed on the Linux box, which has
   no PowerShell), so the `-DryRun` is a real test of the script, not a formality: if it errors or
   prints nonsense, use `scripts/release.sh` from Git Bash or the Linux box instead and tell Trent.
   Use `-DryRun`, not `--dry-run` (the double-dash spelling is untested on 5.1).
   It refuses on a dirty tree, a non-greater version or a missing changelog section; it bumps
   `Cargo.toml`, `hotkeyd\Cargo.toml` and `Cargo.lock` by text, commits `Release TrontSnap 0.26.0`
   and creates the annotated tag, and prints the push command. It never pushes.
3. Strongly recommended first release: with the merge above done (GitHub offers "Run workflow"
   only for a workflow file that is on `main`), push `main`, then Actions > Release > Run
   workflow from `main` with tag `v0.26.0` and dry_run ticked, download `windows-dist` and
   `linux-dist`, and check them. Fix anything before the tag leaves your machine. This run is
   also the first proof of the Windows release job, `cargo test --workspace` on a clean runner,
   and the packaging validators on ubuntu-22.04; none of those could be run off GitHub. It
   also warms the build cache the tag run restores.
4. `git push --atomic origin main v0.26.0`. CI builds both platforms and leaves a **draft**.
5. In the draft: title `TrontSnap 0.26.0: <headline>`, check all assets are there (`.zsync`
   included), leave "latest" ticked, publish.
6. Prove the Windows upgrade path from the published zip: on a machine with v0.25.0 installed,
   unzip the new bundle and run `Install TrontSnap.cmd`. The log should read
   `version: installed v0.25.0  ->  incoming v0.26.0` and `verify-install.ps1` should pass.
   Running it from the old v0.25.0 zip afterwards should be **refused** by the downgrade guard.

The Windows bundle ships unsigned by design: `bootstrap.ps1` signs on the install machine with a
local cert and needs the Windows SDK's `signtool.exe` there. `RELEASING.md` explains why CI
cannot sign it.

## If something fails

| Symptom | Look at |
|---|---|
| Windows job red in CI | the first compile error; start with the files in the table above |
| tests: fewer than 49 | `cfg` attributes on test modules, `src/app.rs` `tray_tests` |
| settings look reset | `src/kvstore.rs` Windows branch, `settings.rs` value names |
| broker not running / hotkeys dead over elevated windows | manifests (`uiAccess`), `hotkeyd/src/main.rs` wrapper, then `bootstrap.log` |
| `release.ps1` refuses | its message lists every reason; nothing was changed |
| release workflow red | `RELEASING.md`, "When CI fails" |
