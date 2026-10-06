#!/usr/bin/env python3
"""gif_test: gallery "Export GIF" on a 30 fps mp4 and on a VFR webm (what the GNOME screencast
writes: r_frame_rate 1000/1), driven through the real context menu in an isolated rig (:93/48793).

Asserts per clip: the GIF's total duration matches the source (+-0.4 s; the old export played a
30 fps clip back 1.2x too fast), ~12 frames per second, and the export finishes in a sane time
(the old export pushed every duplicated source frame of a VFR clip through a pipe: minutes).
Prints PASS/FAIL, exit 1 on FAIL. ~1.5 min.
"""
import os, subprocess, sys, time
from pathlib import Path
from PIL import Image

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
OUT = Path(os.environ.get("HARNESS_OUT") or REPO / "harness_out" / time.strftime("%Y-%m-%d"))
D = OUT / "gif_test"
HOME = OUT / "homes" / "93"
PIC = HOME / "Pictures" / "TrontSnap"
XIN = [str(HERE / "xinput.py"), "-d", ":93"]
SECS = 6.0
fails = []


def check(label, ok, detail=""):
    print(f"  {'PASS' if ok else 'FAIL'}  {label}" + (f"  [{detail}]" if detail else ""), flush=True)
    if not ok:
        fails.append(label)
    return ok


def make_clips():
    D.mkdir(parents=True, exist_ok=True)
    src = f"testsrc2=size=1366x768:rate=30"
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", src, "-t", str(SECS),
                    "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", str(D / "cfr30.mp4")], check=True)
    # The real thing: a GNOME-shell screencast (r_frame_rate=1000/1, VFR). Point GIF_TEST_WEBM at one
    # (the audit's harness_out/*/audit-record/run2/g2/gnome_moderate_clip.webm is one); without it a
    # synthetic irregular webm stands in, which exercises the webm path but not the 1000 fps trigger.
    real = os.environ.get("GIF_TEST_WEBM") or next(iter(sorted(OUT.parent.glob("*/audit-record/run2/g2/gnome_moderate_clip.webm"))), None)
    if real and Path(real).exists():
        subprocess.run(["cp", str(real), str(D / "vfr.webm")], check=True)
    else:
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", src, "-t", str(SECS + 2),
                        "-vf", "select='lt(mod(n,7),4)+eq(mod(n,5),0)',trim=duration=%s" % SECS, "-fps_mode", "vfr",
                        "-c:v", "libvpx", "-b:v", "2M", "-deadline", "realtime", "-cpu-used", "8", str(D / "vfr.webm")], check=True)
    r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=r_frame_rate",
                        "-of", "csv=p=0", str(D / "vfr.webm")], capture_output=True, text=True).stdout.strip()
    print(f"  (vfr.webm r_frame_rate={r}; {'a real GNOME screencast' if real and Path(real).exists() else 'synthetic stand-in'})")


def src_dur(p):
    return float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(p)],
                                capture_output=True, text=True).stdout.strip())


def gif_stats(p):
    im = Image.open(p)
    n = tot = 0
    try:
        while True:
            tot += im.info.get("duration", 0)
            n += 1
            im.seek(im.tell() + 1)
    except EOFError:
        pass
    return n, tot / 1000.0, im.size


def main():
    make_clips()
    seed = D / "seed.sh"
    seed.write_text(f"""#!/bin/sh
P="$1/Pictures/TrontSnap"
cp {D}/cfr30.mp4 "$P/TrontSnap_2030-01-01_00-00-01.mp4"
cp {D}/vfr.webm "$P/TrontSnap_2030-01-01_00-00-02.webm"
touch -d '2 minutes ago' "$P/TrontSnap_2030-01-01_00-00-01.mp4"
touch -d '1 minute ago' "$P/TrontSnap_2030-01-01_00-00-02.webm"
""")
    seed.chmod(0o755)
    env = dict(os.environ, RIG_SEED_HOOK=str(seed), HARNESS_OUT=str(OUT))
    r = subprocess.run([str(HERE / "gui_env.sh"), "start", "93", "48793"], env=env, capture_output=True, text=True)
    if "READY" not in r.stdout:
        sys.exit(r.stdout + r.stderr)
    try:
        # newest first: webm = tile 1, mp4 = tile 2. Export the mp4 first (the GIF becomes tile 1 and pushes the others right).
        jobs = [("30 fps mp4", "TrontSnap_2030-01-01_00-00-01", (310, 134), "cfr30.mp4"),
                ("VFR webm", "TrontSnap_2030-01-01_00-00-02", (310, 134), "vfr.webm")]
        for label, stem, (x, y), srcname in jobs:
            subprocess.run(XIN + ["rightclick", str(x), str(y)], capture_output=True)
            time.sleep(1.5)
            t0 = time.time()
            subprocess.run(XIN + ["click", str(x + 40), str(y + 155)], capture_output=True)  # "Export GIF"
            gif = next(iter(PIC.glob(stem + ".gif")), None)
            end = t0 + 150
            last = -1
            while time.time() < end:
                gif = PIC / (stem + ".gif")
                if gif.exists() and gif.stat().st_size == last and last > 0:
                    break
                last = gif.stat().st_size if gif.exists() else -1
                time.sleep(2)
            took = time.time() - t0
            if not check(f"{label}: a .gif appeared next to the clip", gif.exists()):
                continue
            n, tot, size = gif_stats(gif)
            sd = src_dur(D / srcname)
            check(f"{label}: GIF duration matches the source ({sd:.2f}s)", abs(tot - sd) < 0.4, f"{tot:.2f}s over {n} frames")
            check(f"{label}: about 12 frames per source second", 9.5 <= n / sd <= 13.5, f"{n / sd:.1f} fps")
            check(f"{label}: scaled to 800 wide", size[0] == 800, str(size))
            check(f"{label}: exported in under 60 s", took < 60, f"{took:.1f}s")
            # after a GIF lands it becomes the newest tile: the next clip's tile is one to the right
    finally:
        subprocess.run([str(HERE / "gui_env.sh"), "stop", "93"], env=env, capture_output=True)
    print(f"\n{'FAIL' if fails else 'PASS'}: {len(fails)} failed")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
