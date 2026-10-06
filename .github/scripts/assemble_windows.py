#!/usr/bin/env python3
"""Assemble the Windows release assets the way the hand-built v0.25.0 ones are laid out.

  dist/TrontSnap.exe                  portable UI (byte-identical to trontsnap.exe in the zip)
  dist/TrontSnap-vX.Y.Z-windows.zip   installer bundle (layout below)
  dist/Rajdhani-OFL.txt               font license

The bundle ships UNSIGNED on purpose: bootstrap.ps1 signs on the user's machine with a
local cert (TrontEQ's if present, else it generates one), so CI has nothing to sign with.

Usage (repo root, after `cargo build --release -p trontsnap -p trontsnap-hotkeys`):
    python .github/scripts/assemble_windows.py --version 0.26.0 --out dist
"""
import argparse
import shutil
import sys
import zipfile
from pathlib import Path

# (name in the zip, source relative to --root, or relative to --bin-dir when the
# source starts with "bin:"). Order matches the v0.25.0 zip.
ZIP_LAYOUT = [
    ("portable/Rajdhani-OFL.txt", "assets/fonts/OFL.txt"),
    ("portable/trontsnap.exe", "bin:trontsnap.exe"),
    ("bootstrap.ps1", "bootstrap.ps1"),
    ("CHANGELOG.md", "CHANGELOG.md"),
    ("Install TrontSnap.cmd", "packaging/Install TrontSnap.cmd"),
    ("Launch TrontSnap.cmd", "packaging/Launch TrontSnap.cmd"),
    ("Rajdhani-OFL.txt", "assets/fonts/OFL.txt"),
    ("README.md", "README.md"),
    ("Revert to portable.cmd", "packaging/Revert to portable.cmd"),
    ("revert-to-portable.ps1", "packaging/revert-to-portable.ps1"),
    ("SETUP.md", "SETUP.md"),
    ("trontsnap-hotkeys.exe", "bin:trontsnap-hotkeys.exe"),
    ("trontsnap.exe", "bin:trontsnap.exe"),
    ("verify-install.ps1", "packaging/verify-install.ps1"),
    ("verify-window-lifecycle.ps1", "packaging/verify-window-lifecycle.ps1"),
]


def die(msg):
    print("ERROR: " + msg, file=sys.stderr)
    sys.exit(1)


def check_cmd_eol(path, name):
    """HARD-WON: cmd.exe can mis-parse goto labels in LF-only batch files, so the .cmd
    wrappers must ship CRLF. .gitattributes (*.cmd text eol=crlf) does it at checkout;
    this fails the build if that ever stops working (a bare LF anywhere is enough)."""
    data = path.read_bytes()
    if data.count(b"\n") != data.count(b"\r\n"):
        die("%s has LF-only line endings; check .gitattributes (*.cmd text eol=crlf)" % name)


def check_binaries(ui, broker, version):
    """Same gates bootstrap.ps1 applies at install time, moved to build time."""
    ui_b, broker_b = ui.read_bytes(), broker.read_bytes()
    # UI at uiAccess=true is raised to HIGH integrity and loses drag-out into Discord
    # and terminals (that shipped as v0.14.0). bootstrap.ps1 rejects it; fail earlier.
    if b'uiAccess="true"' in ui_b:
        die("trontsnap.exe carries uiAccess=true; the UI must stay Medium (see build.rs)")
    if b'uiAccess="false"' not in ui_b:
        die("trontsnap.exe has no embedded manifest; winresource did not run")
    if b'uiAccess="true"' not in broker_b:
        die("trontsnap-hotkeys.exe lacks uiAccess=true; the broker would be pointless")
    # HARD-WON: bootstrap.ps1's downgrade guard compares the exe's FileVersion, which
    # lives in the version resource as UTF-16LE (a plain-ASCII grep finds nothing).
    needle = version.encode("utf-16-le")
    for name, blob in (("trontsnap.exe", ui_b), ("trontsnap-hotkeys.exe", broker_b)):
        if needle not in blob:
            die("%s does not embed version %s; stale build or wrong tag" % (name, version))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True, help="X.Y.Z, no leading v")
    ap.add_argument("--out", default="dist")
    ap.add_argument("--root", default=".", help="repo root")
    ap.add_argument("--bin-dir", default=None, help="default: <root>/target/release")
    a = ap.parse_args()

    root = Path(a.root)
    bin_dir = Path(a.bin_dir) if a.bin_dir else root / "target" / "release"
    out = Path(a.out)

    def src(spec):
        return bin_dir / spec[4:] if spec.startswith("bin:") else root / spec

    missing = [str(src(s)) for _, s in ZIP_LAYOUT if not src(s).is_file()]
    if missing:
        die("missing inputs:\n  " + "\n  ".join(missing))

    check_binaries(src("bin:trontsnap.exe"), src("bin:trontsnap-hotkeys.exe"), a.version)
    for name, spec in ZIP_LAYOUT:
        if name.endswith(".cmd"):
            check_cmd_eol(src(spec), name)

    out.mkdir(parents=True, exist_ok=True)
    zip_path = out / ("TrontSnap-v%s-windows.zip" % a.version)
    if zip_path.exists():
        zip_path.unlink()
    # HARD-WON: entries use forward slashes. The hand-built v0.25.0 zip has
    # 'portable\trontsnap.exe' (Windows PowerShell 5.1 Compress-Archive), which
    # unzip/7z-on-Linux treat as a literal backslash filename.
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as zf:
        for name, spec in ZIP_LAYOUT:
            zf.write(src(spec), name)

    shutil.copyfile(src("bin:trontsnap.exe"), out / "TrontSnap.exe")
    shutil.copyfile(root / "assets/fonts/OFL.txt", out / "Rajdhani-OFL.txt")

    # Read the zip back: names, integrity, and that the portable exe is the UI exe.
    with zipfile.ZipFile(zip_path) as zf:
        bad = zf.testzip()
        if bad:
            die("zip entry failed CRC: " + bad)
        names = zf.namelist()
        if names != [n for n, _ in ZIP_LAYOUT] or any("\\" in n for n in names):
            die("zip layout drifted: %r" % names)
        if zf.read("portable/trontsnap.exe") != (out / "TrontSnap.exe").read_bytes():
            die("portable exe differs from TrontSnap.exe")

    for p in sorted(out.iterdir()):
        print("%10d  %s" % (p.stat().st_size, p.name))


if __name__ == "__main__":
    main()
