#!/usr/bin/env python3
"""rec_gnome_test: the GNOME-screencast backend (org.gnome.Shell.Screencast) on the REAL session, CPU
only (VP8 in the shell; no kmsgrab, no VCE, no video decode), through a throwaway TrontSnap instance.

The instance is isolated everywhere except the one thing under test: own Xvfb :93 for its window / toast /
clipboard, temp HOME, TRONTSNAP_TEST=1 (no keybindings/autostart), GSETTINGS_BACKEND=memory (nothing reaches
dconf), fake_bin on PATH (no shutter/tick through the speakers), port 48792 -- but it talks to the REAL
session bus, so the shell really records the real screen and its banners appear on the real desktop.

Checks: (G1) full-screen clip is webm/vp8 + opus audio, |video-audio| < 0.2 s, and the audio/video CONTENT
sync (rec_clock flash + 1 kHz beep) reads 0.14 +- 0.15 s -- the audio is cut AFTER StopScreencast returns;
(G2) `record x,y,w,h` goes through ScreencastArea and yields exactly that size.
Run at most once or twice (the beacon beeps once, and the shell burns 1-2 cores while recording).
"""
import os, signal, subprocess, sys, threading, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import rec_lat  # noqa: E402

REPO = HERE.parents[2]
OUT = Path(os.environ.get("HARNESS_OUT") or REPO / "harness_out" / time.strftime("%Y-%m-%d"))
HOME = OUT / "homes" / "93g"
PIC = HOME / "Pictures" / "TrontSnap"
PORT, DISP = 48792, ":93"
BIN = str(REPO / "target" / "debug" / "trontsnap")
fails = []


def check(label, ok, detail=""):
    print(f"  {'PASS' if ok else 'FAIL'}  {label}" + (f"  [{detail}]" if detail else ""), flush=True)
    if not ok:
        fails.append(label)
    return ok


def send(cmd):
    import socket
    s = socket.create_connection(("127.0.0.1", PORT), timeout=2)
    s.sendall(cmd.encode())
    s.close()
    return time.time()


def probe(p):
    import json
    r = subprocess.run(["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", str(p)], capture_output=True, text=True)
    j = json.loads(r.stdout or "{}")
    info = {"dur": float(j.get("format", {}).get("duration") or 0), "v": None, "a": None}
    for s in j.get("streams", []):
        k = "v" if s.get("codec_type") == "video" else "a"
        info[k] = info[k] or {"codec": s.get("codec_name"), "w": s.get("width"), "h": s.get("height"), "dur": float(s.get("duration") or 0)}
    if info["a"] and not info["a"]["dur"]:
        # webm carries no per-stream duration: the end of the last audio packet
        r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries", "packet=pts_time,duration_time",
                            "-of", "csv=p=0", str(p)], capture_output=True, text=True).stdout.strip().splitlines()
        if r:
            pts, _, dur = r[-1].partition(",")
            info["a"]["dur"] = float(pts) + float(dur or 0)
    return info


def media():
    return {p.name for p in PIC.iterdir() if p.suffix in (".mp4", ".webm")} if PIC.exists() else set()


def wait(fn, t, every=0.2):
    end = time.time() + t
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(every)
    return fn()


def deliver(before, timeout=90):
    new = wait(lambda: sorted(media() - before), timeout)
    if not new:
        return None
    p = PIC / new[-1]
    last = -1
    for _ in range(60):
        sz = p.stat().st_size
        if sz == last and sz > 0 and probe(p)["v"]:
            return p
        last = sz
        time.sleep(0.5)
    return p


def main():
    import shutil
    if not os.environ.get("WAYLAND_DISPLAY") and not os.environ.get("XDG_SESSION_TYPE"):
        sys.exit("needs the real GNOME session's environment")
    shutil.rmtree(HOME, ignore_errors=True)
    (HOME / ".config" / "trontsnap").mkdir(parents=True)
    PIC.mkdir(parents=True)
    (HOME / ".config" / "user-dirs.dirs").write_text(f'XDG_PICTURES_DIR="{HOME}/Pictures"\nXDG_VIDEOS_DIR="{HOME}/Videos"\n')
    (HOME / ".config" / "trontsnap" / "settings.conf").write_text("HasRun=1\nAutostartInit=1\nRecordAudio=1\n")
    xvfb = subprocess.Popen(["Xvfb", DISP, "-screen", "0", "1366x768x24", "-nolisten", "tcp"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    time.sleep(1.5)
    env = {k: v for k, v in os.environ.items() if k not in ("WAYLAND_DISPLAY", "XAUTHORITY", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME")}
    env.update(DISPLAY=DISP, HOME=str(HOME), TRONTSNAP_PORT=str(PORT), TRONTSNAP_TEST="1", TRONTSNAP_RECORDER="gnome",
               GSETTINGS_BACKEND="memory", LIBGL_ALWAYS_SOFTWARE="1", PATH=f"{HERE / 'fake_bin'}:{os.environ['PATH']}",
               XDG_SESSION_TYPE="wayland")
    app = subprocess.Popen([BIN], env=env, cwd=str(HOME), stdout=open(OUT / "rigs" / "gnome_app.log", "w") if (OUT / "rigs").exists() else subprocess.DEVNULL,
                           stderr=subprocess.STDOUT, start_new_session=True)
    clock = None
    try:
        if not check("instance up on 48792", wait(lambda: subprocess.run(["ss", "-ltn", f"sport = :{PORT}"], capture_output=True, text=True).stdout.count("LISTEN"), 40)):
            return
        time.sleep(3)
        clock = subprocess.Popen([sys.executable, str(HERE / "rec_clock.py")], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        ev = []
        threading.Thread(target=rec_lat.reader, args=(clock, ev), daemon=True).start()
        wait(lambda: rec_lat.first_event(ev, "ready")[0], 10)
        time.sleep(2.0)

        print("\n=== G1 full screen, audio, flash+beep ===")
        before = media()
        t_press = send("record")
        time.sleep(3.0)
        clock.stdin.write("flash\n"); clock.stdin.flush()
        time.sleep(2.0)
        t_stop = send("record")
        clip = deliver(before)
        if check("G1: a clip was delivered", clip is not None):
            info = probe(clip)
            check("G1: webm / vp8", clip.suffix == ".webm" and info["v"]["codec"] == "vp8", f"{clip.name} {info['v']}")
            check("G1: opus audio present", info["a"] is not None and info["a"]["codec"] == "opus", str(info["a"]))
            if info["a"]:
                d = abs(info["dur"] - info["a"]["dur"])
                check("G1: |video - audio| < 0.2 s", d < 0.2, f"video {info['dur']:.2f}s audio {info['a']['dur']:.2f}s")
            fr = rec_lat.frames_wall(str(clip), t_press)
            fl = [f for f in fr if f[2]]
            if check("G1: the flash is in the clip", bool(fl), f"{len(fr)} frames"):
                ao = rec_lat.audio_onset(str(clip))
                if check("G1: the beep is in the audio", ao is not None):
                    lag = ao - fl[0][0]
                    check("G1: audio-minus-video within 0.14 +- 0.15 s (gnome cuts the audio BEFORE StopScreencast; after it was -0.24 s)", abs(lag - 0.14) <= 0.15,
                          f"flash {fl[0][0]:.3f}s beep {ao:.3f}s lag {lag:+.3f}s over {len(fr)} frames")
                tail = fr[-1][1] - t_stop
                print(f"    last frame wall vs stop press: {tail:+.2f}s")

        time.sleep(6)
        print("\n=== G2 region through ScreencastArea ===")
        before = media()
        send("record 100,60,320,180")
        time.sleep(4.0)
        send("record")
        clip = deliver(before)
        if check("G2: a clip was delivered", clip is not None):
            info = probe(clip)
            check("G2: exactly 320x180", (info["v"]["w"], info["v"]["h"]) == (320, 180), str(info["v"]))
            check("G2: audio present", info["a"] is not None)
    finally:
        if clock:
            try:
                clock.stdin.write("quit\n"); clock.stdin.flush()
            except OSError:
                pass
        for p in (app, xvfb):
            try:
                os.killpg(p.pid, signal.SIGTERM)
            except OSError:
                pass
        time.sleep(1.0)
        for p in (app, xvfb):
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except OSError:
                pass
        subprocess.run(["rm", "-f", f"/tmp/.X11-unix/X{DISP[1:]}", f"/tmp/.X{DISP[1:]}-lock"])
    print(f"\n{'FAIL' if fails else 'PASS'}: {len(fails)} failed")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
