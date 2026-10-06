# Releasing TrontSnap

One tag push builds everything and leaves a **draft** GitHub release. You write the
changelog, run one script, push, then set the title by hand and publish.

```
# 1. write '## vX.Y.Z' in CHANGELOG.md and commit it
# 2. bump + commit + tag, locally (dry-run first)
scripts/release.sh X.Y.Z --dry-run        # Windows: scripts\release.ps1 X.Y.Z -DryRun
scripts/release.sh X.Y.Z
# 3. push the branch and the tag together; the tag starts CI
git push --atomic origin main vX.Y.Z
# 4. Actions > Release finishes -> Releases > the DRAFT -> set title -> Publish
```

The scripts never push, never publish and never call cargo. Everything outward-facing
(step 3 and 4) is yours.

## The flow, in order

### 0. Before you start

- You are on the branch you are releasing (`main`), with a clean tree.
- **CI is green on that commit** (Actions > CI: `cargo build/test --locked --workspace`
  on ubuntu-22.04 and windows-latest). The release workflow rebuilds from the tag but does
  not run the tests, so a red CI commit can still produce a draft.
- For anything touching Windows-only code (tray, hotkeys, broker, installer): run
  `packaging\verify-install.ps1` and `packaging\verify-window-lifecycle.ps1` on a Windows
  box first (see SETUP.md). CI cannot exercise a desktop session.

### 1. Write the changelog section

Add `## vX.Y.Z` at the top of `CHANGELOG.md` and **commit it**. The section is cut verbatim
(from that heading to the next `## `) and becomes the draft release notes, and the file
also ships inside the Windows zip. Match the existing style: a few bullets or paragraphs,
and a closing `Verification:` line like v0.25.0's.

The release script will not invent notes and will not run on a dirty tree, which is why
the changelog has to be committed before you start.

### 2. Run the release script

```
scripts/release.sh 0.26.0 --dry-run
scripts/release.sh 0.26.0
```

PowerShell 5.1 on the Windows box (no pwsh needed). `release.ps1` has not been executed yet
(there is no PowerShell on the Linux box): do the `-DryRun` first and use that spelling, not
`--dry-run`. `release.sh` (Git Bash works) does the same job and is the tested one.

```
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 0.26.0 -DryRun
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 0.26.0
```

It refuses, and lists every reason it found, unless:

- the tree is clean (untracked files count: a forgotten new source file is how "builds
  here, fails in CI" happens), HEAD is on a branch, and git has an identity;
- `X.Y.Z` is greater than the current version (numeric compare: `0.100.0 > 0.25.0`) and
  tag `vX.Y.Z` does not exist locally;
- `Cargo.toml`, `hotkeyd/Cargo.toml` and the two `Cargo.lock` entries all agree today;
- `CHANGELOG.md` has a non-empty `## vX.Y.Z` section.

Then it edits those four version fields **by text** (the `[package]` table of both
manifests, and the `trontsnap` / `trontsnap-hotkeys` entries of `Cargo.lock`), so it works
offline in about 100 ms and cannot disturb a build in `target/`. Only those two workspace
crates change in the lock, which is the whole diff cargo itself would write. It edits
copies first, checks the result (read-back, exactly 1/1/2 changed lines, BOM intact, and
CI's own `.github/scripts/verify-release.sh` on the copies), and only then touches the
tree. Then it commits `Release TrontSnap X.Y.Z` (those three files only) and creates the
annotated tag `vX.Y.Z`. It ends by printing, not running, the push command.

If the commit fails (a hook), the version files are restored and nothing is committed.

Both manifests start with a UTF-8 BOM and CI's verifier trips on a naive edit; that is
handled, and `release.ps1` edits the files as Latin-1 text so the BOM and any CRLF
checkout survive byte for byte.

Tags are `vMAJOR.MINOR.PATCH` only. CI's verify job rejects `-rc1` style tags, and so does
the script. There is no pre-release channel yet.

### 3. (Recommended) Rehearse the builds before the tag exists

**Prerequisite: `release.yml` must already be on `main`.** GitHub lists "Run workflow" only
for a workflow file that exists on the default branch (it is not offered for a file that
lives only on a feature branch, so `linux-port` cannot rehearse itself). Merge the branch
first (WINDOWS-HANDOFF.md has the order), then:

Push **the branch only**: `git push origin main`. Then Actions > **Release** > Run
workflow, with:

- Use workflow from: `main`, or any branch that carries the commit (hotfix branches work
  once `main` has the file)
- tag: `vX.Y.Z` (the version the commit already carries)
- dry_run: ticked (the default)

It runs the full verify + Windows + Linux jobs and keeps the outputs as workflow artifacts
(`windows-dist`, `linux-dist`, `release-notes`, 14 days). No release, no tag needed. Download
them from the run page; `upload-artifact` drops file modes, so `chmod +x` the AppImage
before trying it.

Rehearsing from `main` also fills the build cache for the tag run: tag runs cannot save one
(`save-if` is branch-only) and can only restore caches made on the default branch, so
without a dry run the first tag build is a cold fat-LTO compile on both platforms. Budgets
are 60 min (Windows) and 45 min (Linux); nobody has measured a real run yet.

**What the first dry run has to prove** (none of it can run off GitHub; treat the first
run as the test, and fix what it finds before any tag leaves your machine):

- the Windows job end to end: `winresource` finds `rc.exe`, `assemble_windows.py` passes its
  manifest/version/CRLF gates, `sha256sum` and `$GITHUB_STEP_SUMMARY` work under Git Bash;
- `cargo test --workspace` on a clean runner is green (`ci.yml`; some egui and thumbnail tests
  touch the filesystem, so this is unproven, not assumed);
- `desktop-file-validate` and `appstreamcli` on ubuntu-22.04 are older than the ones used
  locally and `build-all.sh` treats their failures as hard errors;
- the pinned action majors (checkout, upload/download-artifact v4, setup-python v5,
  rust-cache v2) and the `ubuntu-22.04` label still run without deprecation errors; revisit
  them when GitHub announces retirements.

If the rehearsal turns up a problem, fix it on the branch. The local tag still points at the
old release commit and was never pushed, so moving it is safe:
`git tag -d vX.Y.Z && git tag -a vX.Y.Z -m "TrontSnap X.Y.Z"` once the fix is committed.

### 4. Push the branch and the tag

```
git push --atomic origin main vX.Y.Z
```

`--atomic` makes it all or nothing, so the tag can never arrive without its commit. The
branch push runs CI; the tag push runs the release workflow.

### 5. What CI does (.github/workflows/release.yml)

| Job | Does | Produces |
|---|---|---|
| `verify` | tag is `vX.Y.Z`; it equals `Cargo.toml`, `hotkeyd/Cargo.toml` and both `Cargo.lock` entries; `CHANGELOG.md` has the section | `release-notes.md` |
| `windows` | `cargo build --release --locked -p trontsnap -p trontsnap-hotkeys`, then `.github/scripts/assemble_windows.py` | `TrontSnap.exe`, `TrontSnap-vX.Y.Z-windows.zip`, `Rajdhani-OFL.txt` |
| `linux` | ubuntu-22.04 (glibc floor), `cargo build --release --locked -p trontsnap`, then `packaging/linux/build-all.sh` | AppImage + `.zsync`, `.deb`, `.tar.gz` |
| `publish` | the only job with `contents: write`; makes `SHA256SUMS` and a **draft** release | draft titled `TrontSnap X.Y.Z`, notes = the changelog section |

`publish` runs only for a real release (tag push, or a manual run from the tag with dry_run
off). If a **draft** for the tag already exists it only uploads assets (`--clobber`) and
leaves the title and notes you edited alone. If the release is already **published** it
refuses and fails: replacing the AppImage and `.zsync` under people who are updating from them
is never something CI should do quietly. `SHA256SUMS` is regenerated over exactly the merged
asset set and verified with `sha256sum -c` before upload.

Assets on a finished release:

| File | For |
|---|---|
| `TrontSnap.exe` | Windows portable UI (byte-identical to `portable/trontsnap.exe` in the zip) |
| `TrontSnap-vX.Y.Z-windows.zip` | Windows installer bundle: both exes, `bootstrap.ps1`, the `.cmd` wrappers, verifiers, README/SETUP/CHANGELOG |
| `Rajdhani-OFL.txt` | font licence |
| `TrontSnap-vX.Y.Z-x86_64.AppImage`, `.AppImage.zsync` | Linux, any distro; the `.zsync` is what makes self-update work |
| `trontsnap_X.Y.Z-1_amd64.deb` | Debian/Ubuntu (the `-1` is the Debian revision) |
| `trontsnap-X.Y.Z-linux-x86_64.tar.gz` | Linux tarball, `install.sh` into `~/.local` |
| `SHA256SUMS` | checksums |

### 6. Set the title and publish (by hand, on purpose)

Open the draft (Releases page, "Draft" badge).

1. Title: CI writes `TrontSnap X.Y.Z`. Make it `TrontSnap X.Y.Z: <headline>`, as in
   `TrontSnap 0.25.0: grab the whole title bar`. A person writes the headline, CI never will.
2. Notes: the changelog section is already there; edit freely.
3. Check the asset list against the table above (all of them, `.zsync` included).
4. Leave **Set as the latest release** ticked and **pre-release** unticked for a normal
   release: AppImageUpdate follows "latest" (see Hotfixes for the exception).
5. Publish.

Afterwards: download the AppImage and the zip from the public page on a clean machine, and
confirm an older install upgrades (see "Upgrades" below).

## Dry-running CI without releasing

Actions > Release > Run workflow (only offered once `release.yml` is on `main`, see step 3):

| Field | Value |
|---|---|
| Use workflow from | a **branch** for a rehearsal; the **tag** to publish by hand |
| `tag` | `vX.Y.Z`, must match `Cargo.toml`, `hotkeyd/Cargo.toml`, `Cargo.lock` and `CHANGELOG.md` of that ref |
| `dry_run` | ticked = artifacts only. Unticked also requires starting from the tag ref (CI rejects a branch, so a release is always built from the tag itself) |

Unticked from the tag is also the way to rebuild the draft if the original tag-push run was
lost. Locally you can rehearse the gate itself with no CI at all:
`bash .github/scripts/verify-release.sh vX.Y.Z notes.md` (offline; it writes the notes file, delete it after).

## Windows: signing, uiAccess, and why CI ships the unsigned bundle

TrontSnap has two Windows modes (SETUP.md has the user view):

- **Portable**: `TrontSnap.exe` anywhere. It registers its own hotkeys with
  `RegisterHotKey`. No admin, no signature involved.
- **Installed**: adds `trontsnap-hotkeys.exe`, a tiny broker that owns the hotkeys so a bare
  `PrtSc` still works over elevated windows (Task Manager, admin terminals). That needs the
  `uiAccess="true"` manifest on the broker, and **Windows only grants uiAccess to an exe that
  is Authenticode-signed with a certificate chaining to a root that machine trusts, and that
  sits in a secure location such as Program Files**. The UI itself stays `uiAccess=false`
  (Medium integrity) so drag-out into Discord and browsers keeps working; `build.rs` and
  `hotkeyd/build.rs` embed the two opposite manifests, which is why the broker is its own crate.

`bootstrap.ps1` (run by `Install TrontSnap.cmd`, one UAC prompt) does the signing **on the
user's machine**:

1. refuses a UI with `uiAccess=true` and a broker without it, and refuses to downgrade;
2. picks a certificate: reuses `C:\ProgramData\TrontEq\dev-cert.pfx` if present (already
   trusted there), otherwise generates a self-signed `CN=TrontSnap Dev` code-signing cert and
   trusts it in `LocalMachine\Root` and `TrustedPublisher`;
3. copies both exes into `%ProgramFiles%\TrontSnap` (hash-verified), then signs both with
   `signtool` from the Windows SDK (`/tr` timestamp so the signature outlives the cert; it
   retries untimestamped when offline), and requires both signatures to read `Valid`;
4. repoints the HKCU `Run` autostart value, and only then deletes the portable exe.

So the release bundle **cannot** be signed in CI in any way that helps: a key made in CI is
not trusted on anyone's PC, and the trust step (importing the cert into that machine's Root
store) is exactly what `bootstrap.ps1` does locally. The bundle ships unsigned, `assemble_windows.py`
runs the same manifest/version gates `bootstrap.ps1` would (UI `uiAccess=false`, broker
`uiAccess=true`, the version embedded in the exes), and the signature is made at install time.
Consequences worth knowing:

- Installed mode needs `signtool.exe` (Windows SDK) on the user's PC. Portable does not.
- The portable `TrontSnap.exe` is unsigned, so Windows SmartScreen may warn on first run of a
  downloaded copy.
- The embedded version matters: the downgrade guard reads the exe's `FileVersion`, which is why
  `assemble_windows.py` fails a build whose exes do not contain the tag's version.
- The dev cert is machine-local; uninstalling does not remove it (see INSTALL.md).

Not built: signing in CI with a real code-signing certificate (OV/EV, or Azure Trusted
Signing). It would remove the SDK requirement and the SmartScreen warning, but needs a paid
identity and a secret in the repo.

## Cutting a hotfix

- **`main` is releasable** (nothing half-done on it): it is just the next patch.
  Write `## vX.Y.(Z+1)`, run the script, push. Done.
- **`main` has unreleased work for the next minor**: branch from the released tag instead.

  ```
  git switch -c hotfix/0.26.1 v0.26.0
  git cherry-pick <fix>                # or commit the fix directly
  # add '## v0.26.1' to CHANGELOG.md on this branch, commit
  scripts/release.sh 0.26.1
  git push --atomic origin hotfix/0.26.1 v0.26.1
  ```

  `release.sh` compares against the version on the branch you are on, so this works even
  though `v0.27.0` may already exist. CI (`ci.yml`) only watches `main`, `linux-port` and
  PRs, so rehearse with the Release workflow's dry run from the hotfix branch (works because
  `release.yml` is on `main`), or open a PR.
  Afterwards forward-port the fix and its changelog entry to `main`.

  If a newer release is already out, **untick "Set as the latest release"** on the hotfix
  draft. GitHub, not semver, decides what "latest" is, and AppImageUpdate follows it: an
  older line's hotfix as "latest" would "upgrade" newer AppImages to the old version.
  (This is GitHub behaviour, not something this repo controls; confirm it on the first
  old-line hotfix you cut.)
- **Every release must carry the Linux assets**, `.zsync` included, for the same reason: if a
  Windows-only release becomes "latest", AppImageUpdate finds no matching asset and reports
  nothing to update. There is no such thing as a Windows-only hotfix release any more.

## When CI fails

Start from the failed job's log; every gate prints what it wanted. Reproduce the verify
gate locally with `bash .github/scripts/verify-release.sh vX.Y.Z notes.md`.

| Where | Message / symptom | Cause and fix |
|---|---|---|
| verify | `tag ... is not vMAJOR.MINOR.PATCH` | stray `-rc1`/typo in the tag |
| verify | `<file> is 'A' but the tag says B` | the tag and the commit it points at disagree; the script prevents this, so someone tagged by hand or the tag was moved |
| verify | `CHANGELOG.md has no '## vX.Y.Z' section` | section missing from the **tagged commit** |
| windows | build error | read it; CI (`ci.yml`) builds the workspace on Windows on every push to `main`, so this should have shown up there first |
| windows | `lacks uiAccess=true` / `carries uiAccess=true` / `does not embed version` | manifest or stale-build problem caught by `assemble_windows.py`; see "Windows" above |
| linux | `N non-empty files match dist/<name> (want exactly 1)` | the workflow's expected file name and what `packaging/linux/*.sh` actually write drifted (`.github/scripts/check-assets.sh` does the check; the deb is matched as `trontsnap_X.Y.Z-*_amd64.deb`). Fix whichever side is wrong |
| linux | `update information not embedded` | `appimage.sh` could not read the update string back from the AppImage it just built; a stray `APPIMAGE_EXTRACT_AND_RUN` in the environment was the first cause (it is now unset for that one call) |
| linux | appimagetool download or sha256 mismatch | network blip (re-run), or the pinned tool changed (`packaging/linux/appimage.sh` refuses to run a tool whose hash differs) |
| publish | `N non-empty files match dist/<name> (want exactly 1)` | an earlier job's artifact upload is short a file; check the artifact list on the run page |
| publish | `release vX.Y.Z is already PUBLISHED` | the draft was published before this run reached `publish` (a re-run, or a dispatch from the tag). Nothing was uploaded. Cut the next patch, or fix the assets by hand |
| any job | a runner or network flake | Actions > the run > **Re-run failed jobs**; it rebuilds the same tag commit |

If you must change code, there are two cases:

- **Nothing public yet** (no published release for the tag): you may move the tag.
  Delete the draft release if one was created, then
  `git push origin :refs/tags/vX.Y.Z`, fix and commit, `git tag -d vX.Y.Z`, re-tag the new
  commit (`git tag -a vX.Y.Z -m "TrontSnap X.Y.Z"`), and push the branch and tag again.
  GitHub keeps only one pending run per concurrency group, so quick re-pushes collapse into
  the newest.
- **Already published**: never move or reuse the tag; people have downloaded it and
  AppImage zsync deltas reference it. Cut `X.Y.(Z+1)`.

## Upgrades, per platform

Settings and captures survive every path below: they live outside the install (see
INSTALL.md for where).

| Platform | Upgrade | Notes |
|---|---|---|
| Windows portable | quit TrontSnap, replace `TrontSnap.exe` with the new one | the exe is locked while running. Settings are in HKCU, captures in `Pictures\TrontSnap`, thumbnail cache in `%LOCALAPPDATA%\TrontSnap\thumbs` |
| Windows installed | unzip the new `TrontSnap-vX.Y.Z-windows.zip`, double-click `Install TrontSnap.cmd` | see below |
| Linux AppImage | AppImageUpdate / `appimageupdatetool <file>`, or download the new AppImage and delete the old | needs the `.zsync` on the latest release |
| Linux `.deb` | `sudo apt install ./trontsnap_X.Y.Z-1_amd64.deb` over the old one | same package name and a higher version = in-place upgrade; the running app is asked to exit first |
| Linux tarball | unpack the new tarball, run `./install.sh` again | atomic replace, safe while running |
| Flatpak | rebuild and reinstall from `packaging/linux/xyz.tront.TrontSnap.yml` | experimental, not in CI, not a supported path (see its header) |

**Not yet proven (as of the first Linux release):** AppImageUpdate delta upgrades and the
"GitHub `latest` drives it" behaviour, and `sudo apt install ./new.deb` over an old one on a
clean machine, are designed and partly tested locally (the deb maintainer scripts, the update
string, the `.zsync` name match) but need two real releases to see end to end. After the second
Linux release, upgrade an older AppImage and an older deb on a clean box and delete this
paragraph, or fix what broke. Until then INSTALL.md also tells users the manual path
(download the new file, replace the old).

**Windows installed in detail.** Re-running the installer is the upgrade. `bootstrap.ps1`
kills the running UI and broker, copies the new exes over (and verifies the copy by hash),
re-signs, repoints autostart and, last, removes any portable exe. It logs
`version: installed vA -> incoming vB` to `C:\ProgramData\TrontSnap\bootstrap.log`. It
rewrites only its own values: HKCU settings and other apps' autostart entries are kept.

The **downgrade guard** compares the incoming exe's `FileVersion` with the installed one and
**refuses** (exit 1, nothing changed) if the incoming one is older. This is deliberate:
running `Install TrontSnap.cmd` from an old unzipped bundle used to silently reinstall a
months-old build. The same version number is allowed and still replaces the binary. To roll
back on purpose, run `bootstrap.ps1` yourself from an elevated PowerShell with
`-AllowDowngrade` (the `.cmd` does not forward the switch). Installing from a source checkout
(no prebuilt exe beside the script) builds first, so it is always current.

**Linux AppImage in detail.** The AppImage carries the update string
`gh-releases-zsync|TrentSterling|trontsnap|latest|TrontSnap-*x86_64.AppImage.zsync`, so
AppImageUpdate fetches the newest release's `.zsync` and delta-downloads. `AppRun` keeps a
stable copy of the binary at `~/.local/share/trontsnap/bin/trontsnap` (the app bakes its own
exe path into the GNOME keybindings and autostart entry, and inside an AppImage that path is
a throwaway mount); the copy is refreshed on the first launch of a different build, which is
how upgrades reach the hotkeys. A running TrontSnap keeps the old version until restarted.

## Not built yet (open questions, clearly future work)

- **An apt repository / PPA.** Today deb users upgrade by downloading the new file. A hosted
  repo would make `apt upgrade` do it. Open question: a signed repo on GitHub Pages or
  tront.xyz (needs a GPG key and a publish step in CI) versus a Launchpad PPA (builds from a
  source package, so it needs Rust dependency vendoring). Not started.
- **An in-app update check.** The app could ask the GitHub Releases API for the latest tag,
  compare it to its own version and show a toast with the download link (and open the
  Windows installer flow, which still needs elevation). It should stay off for deb installs,
  which apt owns. Not started.
- CI code signing for Windows (above), Flathub, and arm64 builds.

## Where things live

| Path | What |
|---|---|
| `scripts/release.sh`, `scripts/release.ps1` | bump + commit + tag, locally |
| `.github/workflows/ci.yml` | build + test on every push to `main`/`linux-port` and PRs |
| `.github/workflows/release.yml` | tag-driven (or manual) release builds and the draft |
| `.github/scripts/verify-release.sh` | the version/changelog gate (also runs inside `release.sh`) |
| `.github/scripts/check-assets.sh` | "exactly one non-empty file per name/glob" check, used by the linux and publish jobs |
| `.gitattributes` | pins CRLF on `*.cmd` / `*.ps1` checkouts (the zip's `.cmd` wrappers; see HARD-WON there) |
| `.github/scripts/assemble_windows.py` | Windows exe + bundle zip + manifest/version gates |
| `packaging/linux/build-all.sh` | AppImage, deb, tarball, checksums (see `packaging/linux/README.md`) |
| `bootstrap.ps1`, `packaging/*.cmd`, `packaging/*.ps1` | the Windows installer, its wrappers and verifiers |
| `INSTALL.md`, `SETUP.md`, `LINUX.md` | user-facing install docs |
