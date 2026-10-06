#!/usr/bin/env python3
"""rec_fake_test: the Linux recorder's kms code path against a FAKE capkit-kms helper in an
isolated rig (own Xvfb :93, own dbus + notification mock, temp HOME, port 48793). No root, no
polkit, no GPU, nothing on the real screen or speakers: safe to run any time, any number of
times (unlike rec_test.sh, which is capped to a handful of real VCE recordings).

What it proves (each check prints PASS/FAIL; exit 1 on any FAIL):
  S1  start/stop cues: "Starting recording" at the press, closed (+ a tick) only when frames flow
      , "Saving recording" at the stop, banners replace/close each other and
      never stack; the delivered clip STARTS about where the cue said and ENDS at the stop press
      (post-press tail trimmed); stage dir empty; audio sidecar is cut AFTER the stop helper
      returned (end alignment) and the muxed audio is exactly as long as the video
  S2  record pressed twice 0.2 s apart: nothing delivered, "Recording cancelled", no orphan
  S3  stop then record again 0.6 s later: the restart waits out the old kmsgrab instead of
      refusing with the "capkit?" message; both clips delivered
  S4  helper leaves a 0-byte file (a stop that beat the first frame): not delivered as a clip
  S5  region (`record x,y,w,h`): kms crop -> the clip is exactly that size, audio kept
  S6  `trontsnap record` with NO app running starts the app, then the recording
  S7  `trontsnap record region` picker (drag) -> region recording
  S8  CPU: no app thread burns CPU while a recording runs (kms_watch used to poll /proc at 20 Hz)
  S9  UI: title-bar "Starting.../REC mm:ss/Saving..." (screenshots in $OUT/fake/shots, LOOK at them)
  S10 tray dot (StatusNotifierItem): registers when frames flow with a red-dot pixmap, vanishes at the stop,
      a click (Activate) stops the recording (mock watcher on the rig bus)
  S11 opening a video from the gallery is deferred while a kmsgrab recording runs, and works after
  X1  x11 backend (ffmpeg x11grab of the rig's Xvfb): full screen + a region, audio kept and as long as the video

  rec_fake_test.py [--only S1,S3] [--no-audio] [--keep]
"""
import argparse, json, os, re, signal, socket, subprocess, sys, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
OUT = Path(os.environ.get("HARNESS_OUT") or REPO / "harness_out" / time.strftime("%Y-%m-%d"))
N, PORT = 93, 48793
FD = OUT / "fakekms"
HOME = OUT / "homes" / str(N)
PIC = HOME / "Pictures" / "TrontSnap"
CAP = HOME / "Videos" / "captures"
STAGE = HOME / ".cache" / "trontsnap" / "rec"
SHOTS = OUT / "fake" / "shots"
BIN = os.environ.get("TRONTSNAP_BIN") or str(REPO / "target" / "debug" / "trontsnap")
sys.path.insert(0, str(HERE / "fake_kms"))
import gen_frames  # noqa: E402

results = []


def say(m):
    print(m, flush=True)


def check(label, ok, detail=""):
    results.append(("PASS" if ok else "FAIL", label))
    say(f"  {'PASS' if ok else 'FAIL'}  {label}" + (f"  [{detail}]" if detail else ""))
    return ok


def header(t):
    say(f"\n=== {t} ===")


def gui(*args, **kw):
    env = dict(os.environ, HARNESS_OUT=str(OUT))
    return subprocess.run([str(HERE / "gui_env.sh"), *args], env=env, capture_output=True, text=True, **kw)


def send(cmd, port=PORT):
    s = socket.create_connection(("127.0.0.1", port), timeout=2)
    s.sendall(cmd.encode())
    s.close()
    return time.time()


def cfg(**kv):
    FD.mkdir(parents=True, exist_ok=True)
    (FD / "cfg").write_text("".join(f"{k}={v}\n" for k, v in kv.items()))


def flog():
    out = []
    try:
        for ln in (FD / "log").read_text().splitlines():
            t, _, msg = ln.partition(" ")
            out.append((float(t), msg))
    except OSError:
        pass
    return out


def flog_t(prefix, after=0.0):
    for t, m in flog():
        if t >= after and m.startswith(prefix):
            return t
    return None


def notifs(after=0.0):
    out = []
    try:
        for ln in (FD / "notify.jsonl").read_text().splitlines():
            d = json.loads(ln)
            if d["t"] >= after:
                out.append(d)
    except OSError:
        pass
    return out


def wait(fn, timeout, every=0.1):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(every)
    return fn()


def live_count():
    """How many recordings have gone live (the app logs it when kmsgrab's first stats line arrives)."""
    try:
        return (OUT / "rigs" / str(N) / "app.log").read_text(errors="replace").count("kms frames flowing")
    except OSError:
        return 0


def media(d=PIC):
    try:
        return {p.name for p in d.iterdir() if p.suffix in (".mp4", ".webm")}
    except OSError:
        return set()


def probe(p):
    r = subprocess.run(["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", str(p)],
                       capture_output=True, text=True)
    try:
        j = json.loads(r.stdout)
    except ValueError:
        return None
    info = {"dur": float(j.get("format", {}).get("duration") or 0), "v": None, "a": None}
    for s in j.get("streams", []):
        rec = {"codec": s.get("codec_name"), "w": s.get("width"), "h": s.get("height"),
               "dur": float(s.get("duration") or 0)}
        k = "v" if s.get("codec_type") == "video" else "a"
        info[k] = info[k] or rec
    return info


def frame_wall(clip, which, ref):
    """Wall-clock epoch of the clip's first/last frame (decoded from the generator's strip)."""
    png = OUT / "fake" / f"_f_{which}.png"
    png.parent.mkdir(parents=True, exist_ok=True)
    args = (["-i", str(clip), "-frames:v", "1"] if which == "first"
            else ["-sseof", "-0.3", "-i", str(clip), "-update", "1"])  # -update 1: each frame overwrites, the last stays
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *args, str(png)], check=True)
    return gen_frames.decode_wall(png, ref)


def procs(match):
    out = []
    for d in Path("/proc").iterdir():
        if not d.name.isdigit():
            continue
        try:
            argv = [a.decode("utf-8", "replace") for a in (d / "cmdline").read_bytes().split(b"\0") if a]
        except OSError:
            continue
        if argv and match(argv):
            out.append(int(d.name))
    return out


def fake_ffmpeg():
    return procs(lambda a: os.path.basename(a[0]) == "ffmpeg" and "kmsgrab" in a)


def audio_ffmpeg():
    return procs(lambda a: os.path.basename(a[0]) == "ffmpeg" and "@DEFAULT_MONITOR@" in a)


def stage():
    try:
        return sorted(p.name for p in STAGE.iterdir())
    except OSError:
        return []


def app_pid():
    return int((OUT / "rigs" / str(N) / "app.pid").read_text())


def shot(name):
    SHOTS.mkdir(parents=True, exist_ok=True)
    p = SHOTS / name
    subprocess.run([str(HERE / "shot.sh"), str(N), str(p)], capture_output=True)
    return p


# === Rig ===
def rig_start(audio, recorder="kms"):
    rig_stop()
    FD.mkdir(parents=True, exist_ok=True)
    for f in ("log", "notify.jsonl", "cfg", "ffmpeg.pid", "kmsgrab"):
        (FD / f).unlink(missing_ok=True)
    hook = FD / "seed_hook.sh"
    hook.write_text(f"#!/bin/sh\nsed -i 's/RecordAudio=0/RecordAudio={1 if audio else 0}/' \"$1/.config/trontsnap/settings.conf\"\n")
    hook.chmod(0o755)
    # HARD-WON: without this the rig's shutter/ticks played through the REAL speakers (pw-play) on every run.
    os.environ["PATH"] = f"{HERE / 'fake_bin'}:" + os.environ["PATH"].replace(f"{HERE / 'fake_bin'}:", "")
    os.environ.update(FAKE_NOTIFY_LOG=str(FD / "cli_notify.log"), FAKE_OPEN_LOG=str(FD / "open.log"))
    for f in ("cli_notify.log", "open.log"):
        (FD / f).unlink(missing_ok=True)
    os.environ.update(FAKE_KMS_DIR=str(FD), NOTIFY_LOG=str(FD / "notify.jsonl"), RIG_SEED_HOOK=str(hook),
                      TRONTSNAP_KMS_HELPER=str(HERE / "fake_kms" / "capkit-kms"),
                      TRONTSNAP_KMS_STOP=str(HERE / "fake_kms" / "capkit-stop"))
    r = gui("start", str(N), str(PORT), "--recorder", recorder)
    if "READY" not in r.stdout:
        sys.exit(f"rig failed: {r.stdout}{r.stderr}")
    if recorder == "kms":
        subprocess.Popen([str(HERE / "gui_env.sh"), "run", str(N), "python3", str(HERE / "notify_mock.py")],
                         stdout=open(FD / "mock.out", "w"), stderr=subprocess.STDOUT, start_new_session=True,
                         env=dict(os.environ, HARNESS_OUT=str(OUT)))
        wait(lambda: "ready" in (FD / "mock.out").read_text(), 8)
        (FD / "sni.jsonl").unlink(missing_ok=True)
        subprocess.Popen([str(HERE / "gui_env.sh"), "run", str(N), "python3", str(HERE / "sni_mock.py")],
                         stdout=open(FD / "sni_mock.out", "w"), stderr=subprocess.STDOUT, start_new_session=True,
                         env=dict(os.environ, HARNESS_OUT=str(OUT), SNI_LOG=str(FD / "sni.jsonl")))
        wait(lambda: "ready" in (FD / "sni_mock.out").read_text(), 8)


def rig_stop():
    for pid in procs(lambda a: str(HERE / "notify_mock.py") in a or str(HERE / "sni_mock.py") in a):
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            pass
    gui("stop", str(N))


def pause_between_scenarios():
    # let the previous scenario's helper/finalize/banners drain so timings don't bleed over
    wait(lambda: not fake_ffmpeg() and not stage() and not audio_ffmpeg(), 10)
    time.sleep(0.6)


# === Scenarios ===
def record_once(secs, region=None, shots=False):
    """press, wait for the 'Recording' banner, hold, press again. Returns timing dict."""
    t = {}
    before_notifs = time.time()
    lc0_before = live_count()
    t["press"] = send("record" if not region else f"record {region}")
    t["live"] = None
    # "live" = frames flowing: the Starting banner closes at that instant (that IS the cue: a
    # banner would be in the picture), and the app logs it.
    wait(lambda: live_count() > lc0_before, 15)
    closes = [n for n in notifs(before_notifs) if n["op"] == "close"]
    t["live"] = closes[0]["t"] if closes else None
    if shots:
        time.sleep(0.3)
        shot("1_recording.png")
    time.sleep(secs)
    t["stop"] = send("record")
    if shots:
        time.sleep(0.5)
        shot("2_saving.png")
    return t


def deliver_wait(before, timeout=40):
    new = wait(lambda: sorted(media() - before), timeout)
    if not new:
        return None
    p = PIC / new[-1]
    last = -1
    for _ in range(40):  # settled
        sz = p.stat().st_size
        if sz == last and probe(p):
            break
        last = sz
        time.sleep(0.4)
    return p


def s1(audio):
    header(f"S1 start/stop cues, tail trim, A/V end alignment (audio={'on' if audio else 'off'})")
    cfg(INIT="1.4", STOP_DELAY="0.8", MODE="normal")
    before = media()
    mark = time.time()
    audio_gone = {}

    t = record_once(6.0, shots=True)
    # audio sidecar lifetime vs the fake stop helper's SIGINT of kmsgrab
    t0 = time.time()
    if audio:
        while audio_ffmpeg() and time.time() - t0 < 5:
            time.sleep(0.02)
        audio_gone["t"] = time.time()
    clip = deliver_wait(before)
    check("S1: a clip was delivered", clip is not None)
    if not clip:
        return
    info = probe(clip)
    ns = notifs(mark)
    sums = [n.get("summary") for n in ns]
    say(f"    banners: {sums}")
    check("S1: banner order Starting -> (closed when frames flow) -> Saving -> closed; no 'Recording' banner",
          [n.get("summary") or "close" for n in ns] == ["Starting recording", "close", "Saving recording", "close"], str(sums))
    shown, stacked = set(), False
    for n in ns:  # a banner that opens while another is still up (and does not replace it) is a pile-up
        if n["op"] == "notify":
            stacked |= bool(shown) and n["replaces"] not in shown
            shown.add(n["id"])
        else:
            shown.discard(n["id"])
    check("S1: banners replace/close each other, never stack", not stacked and not shown, f"still open: {shown}")
    first_src = flog_t("first_frame_source", mark)
    check("S1: the live cue did NOT fire at the press (clip starts ~init+1 s later)",
          t["live"] is not None and t["live"] - t["press"] > 2.0, f"+{(t['live'] or 0) - t['press']:.2f}s after the press")
    first = frame_wall(clip, "first", t["press"])
    last = frame_wall(clip, "last", t["press"])
    say(f"    clip first frame wall = press+{first - t['press']:.2f}s, last = stop-press{last - t['stop']:+.2f}s, "
        f"live cue = first frame{(t['live'] or 0) - first:+.2f}s")
    # The cue fires on the helper's first stats line. The real helper's line trails its first raw
    # frame by about the 1 s the helper trims (cue ~0.2 s before the clip, measured on the real
    # box: rec-fix/r1); this fake's line is immediate, so here it leads by ~the trim.
    check("S1: the live cue is never later than 0.6 s after the clip's first frame",
          t["live"] is not None and t["live"] - first <= 0.6 and t["live"] - first >= -1.4)
    check("S1: the clip ENDS at the stop press (post-press tail trimmed: -0.45..+0.25 s)",
          -0.45 <= last - t["stop"] <= 0.25, f"{last - t['stop']:+.2f}s")
    check("S1: h264 640x360", bool(info["v"]) and info["v"]["codec"] == "h264" and (info["v"]["w"], info["v"]["h"]) == (640, 360))
    if audio:
        check("S1: audio stream present", info["a"] is not None)
        if info["a"]:
            d = abs(info["dur"] - info["a"]["dur"])
            check("S1: |video_dur - audio_dur| < 0.15 s", d < 0.15, f"{d:.3f}s")
        sig = flog_t("stop_sigint_sent", mark)
        lag = audio_gone.get("t", 0) - (sig or 0)
        check("S1: audio sidecar cut AFTER the stop helper SIGINTed kmsgrab (0..+0.6 s)",
              sig is not None and 0.0 <= lag <= 0.6, f"{lag:+.2f}s (old code: about -0.8 s, the helper's latency)")
    else:
        check("S1: no audio stream (Record audio off)", info["a"] is None)
    check("S1: stage dir empty", wait(lambda: not stage(), 5) is not False and not stage(), str(stage()))
    check("S1: no captures leftover", not list(CAP.glob("*")), str(list(CAP.glob("*"))))


def s2():
    header("S2 record pressed twice 0.2 s apart")
    cfg(INIT="1.4", STOP_DELAY="0.8", MODE="normal")
    before = media()
    mark = time.time()
    send("record")
    time.sleep(0.2)
    send("record")
    ok = wait(lambda: any(n.get("summary") == "Recording cancelled" for n in notifs(mark)), 20)
    check("S2: 'Recording cancelled' banner", bool(ok), str([n.get("summary") for n in notifs(mark)]))
    time.sleep(1.0)
    check("S2: nothing delivered", media() == before, str(media() - before))
    check("S2: no fake kmsgrab left running", wait(lambda: not fake_ffmpeg(), 8) is not None and not fake_ffmpeg(), str(fake_ffmpeg()))
    check("S2: no stub left in captures / stage", not list(CAP.glob("*")) and not stage(), f"{list(CAP.glob('*'))} {stage()}")
    # the listener and the toggle still work afterwards
    t = record_once(3.5)
    clip = deliver_wait(before)
    check("S2: a normal recording right after still works", clip is not None and probe(clip) is not None)


def s3():
    header("S3 stop, then record again 0.6 s later (restart inside the winding-down window)")
    cfg(INIT="1.0", STOP_DELAY="0.8", MODE="normal")
    before = media()
    mark = time.time()
    lc_before = live_count()
    t = record_once(4.0)
    time.sleep(0.6)
    t2 = send("record")
    ns = lambda: [n.get("summary") for n in notifs(mark)]
    started2 = wait(lambda: ns().count("Starting recording") >= 2, 10)
    check("S3: second recording started (not refused)", bool(started2), str(ns()))
    check("S3: no 'Another recording is running' refusal", "Another recording is running" not in ns(), str(ns()))
    wait(lambda: live_count() >= lc_before + 2, 15)
    time.sleep(3.0)
    send("record")
    wait(lambda: len(media() - before) >= 2, 40)
    new = sorted(media() - before)
    check("S3: both clips delivered", len(new) == 2, str(new))
    for n in new:
        check(f"S3: {n} readable", probe(PIC / n) is not None and probe(PIC / n)["v"] is not None)
    check("S3: stage dir drains", wait(lambda: not stage(), 8) is not None and not stage(), str(stage()))


def s4():
    header("S4 helper leaves a 0-byte file")
    cfg(INIT="1.0", STOP_DELAY="0.5", MODE="empty")
    before = media()
    mark = time.time()
    record_once(3.0)
    ok = wait(lambda: any(n.get("summary") == "Recording too short" for n in notifs(mark)), 20)
    check("S4: 'Recording too short' banner", bool(ok), str([n.get("summary") for n in notifs(mark)]))
    time.sleep(1.5)
    check("S4: nothing delivered", media() == before, str(media() - before))
    check("S4: the empty stub is cleaned up", not list(CAP.glob("*")), str(list(CAP.glob("*"))))
    cfg(INIT="1.4", STOP_DELAY="0.8", MODE="normal")


def s5(audio):
    header("S5 region recording through the kms backend (post-crop)")
    cfg(INIT="1.0", STOP_DELAY="0.8", MODE="normal")
    before = media()
    record_once(4.0, region="100,60,320,180")
    clip = deliver_wait(before)
    check("S5: delivered", clip is not None)
    if clip:
        info = probe(clip)
        check("S5: clip is exactly 320x180 h264", info["v"]["codec"] == "h264" and (info["v"]["w"], info["v"]["h"]) == (320, 180),
              f"{info['v']}")
        if audio:
            check("S5: audio survived the crop", info["a"] is not None and abs(info["dur"] - info["a"]["dur"]) < 0.15)
        check("S5: duration sane (3-6 s)", 2.5 <= info["dur"] <= 6.5, f"{info['dur']:.2f}")
    # a region press while one runs is a plain stop; a bad region is ignored
    send("record 1,2,3")
    time.sleep(0.5)
    check("S5: a malformed region does not start anything", not fake_ffmpeg())


def s8():
    header("S8 CPU of the app's own threads while a recording runs")
    cfg(INIT="1.0", STOP_DELAY="0.8", MODE="normal")
    pid = app_pid()
    before = media()
    lc8 = live_count()
    send("record")
    wait(lambda: live_count() > lc8, 15)
    time.sleep(1.0)

    def ticks():
        d = {}
        for t in Path(f"/proc/{pid}/task").iterdir():
            try:
                f = (t / "stat").read_text()
                rest = f[f.rfind(")") + 2:].split()
                d[t.name] = (int(rest[11]) + int(rest[12]), f[f.find("(") + 1:f.rfind(")")])
            except OSError:
                pass
        return d

    a = ticks(); t0 = time.time(); time.sleep(5.0); b = ticks(); dt = time.time() - t0
    hz = os.sysconf("SC_CLK_TCK")
    cores = sorted((((b[k][0] - a[k][0]) / hz / dt, b[k][1], k) for k in b if k in a), reverse=True)
    # The UI thread (tid == pid) and the software-GL workers (the rig has no GPU) are rendering the
    # once-a-second REC clock; what matters is that no recorder helper thread is busy.
    helpers = [c for c in cores if c[2] != str(pid) and not c[1].startswith("llvmpipe")]
    say(f"    busiest threads (cores): {[(round(c, 3), n) for c, n, _ in cores[:4]]}")
    say(f"    busiest non-UI helper thread: {[(round(c, 3), n) for c, n, _ in helpers[:2]]}")
    check("S8: no recorder helper thread above 0.02 core during a recording", helpers[0][0] < 0.02, f"{helpers[0][0]:.3f} core ({helpers[0][1]})")
    send("record")
    deliver_wait(before)


def s6(audio):
    header("S6 `trontsnap record` with NO app running")
    cfg(INIT="1.0", STOP_DELAY="0.6", MODE="normal")
    port = 48794
    fake_notify = FD / "cli_notify.log"
    fake_notify.unlink(missing_ok=True)
    before = media()
    env = dict(os.environ, HARNESS_OUT=str(OUT), PATH=f"{HERE / 'fake_bin'}:{os.environ['PATH']}", FAKE_NOTIFY_LOG=str(fake_notify))
    t0 = time.time()
    r = subprocess.run([str(HERE / "gui_env.sh"), "run", str(N), "env", f"TRONTSNAP_PORT={port}", BIN, "record"],
                       env=env, capture_output=True, text=True, timeout=60)
    say(f"    CLI exited {r.returncode} after {time.time() - t0:.1f}s {r.stderr.strip()[-200:]}")
    check("S6: the CLI exited 0", r.returncode == 0)
    check("S6: it told the user the app was starting", "TrontSnap is starting" in (fake_notify.read_text() if fake_notify.exists() else ""))
    live = wait(lambda: flog_t("helper_start", t0), 25)
    check("S6: the recording started in the freshly started app", bool(live), "no helper_start in the fake log")
    time.sleep(4.5)
    r2 = subprocess.run([str(HERE / "gui_env.sh"), "run", str(N), "env", f"TRONTSNAP_PORT={port}", BIN, "record"],
                        env=env, capture_output=True, text=True, timeout=30)
    clip = deliver_wait(before, 40)
    check("S6: the second `record` stopped it and a clip was delivered", clip is not None and probe(clip) is not None)
    # exact-pid cleanup of the extra instance
    out = subprocess.run(["ss", "-ltnp", f"sport = :{port}"], capture_output=True, text=True).stdout
    for pid in set(re.findall(r"pid=(\d+)", out)):
        os.kill(int(pid), signal.SIGTERM)
    time.sleep(1.5)


def s7():
    header("S7 `trontsnap record region`: drag a rectangle")
    cfg(INIT="1.0", STOP_DELAY="0.8", MODE="normal")
    before = media()
    lc7 = live_count()
    env = dict(os.environ, HARNESS_OUT=str(OUT))
    p = subprocess.Popen([str(HERE / "gui_env.sh"), "run", str(N), BIN, "record", "region"], env=env,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    xin = [str(HERE / "xinput.py"), "-d", f":{N}"]
    up = wait(lambda: subprocess.run(xin + ["find", "TrontSnap Region"], capture_output=True, text=True).stdout.strip(), 40, 0.5)
    check("S7: the picker window appears", bool(up), str(up))
    if not up:
        p.kill()
        return
    time.sleep(2.0)
    shot("3_picker.png")
    subprocess.run(xin + ["drag", "100", "100", "420", "280"], capture_output=True)
    rc = p.wait(timeout=30)
    check("S7: the CLI exited 0 after the drag", rc == 0, f"rc={rc}")
    wait(lambda: live_count() > lc7, 20)
    time.sleep(3.0)
    send("record")
    clip = deliver_wait(before)
    check("S7: delivered", clip is not None)
    if clip:
        info = probe(clip)
        check("S7: clip is 320x180 (the dragged 100,100 -> 420,280)", (info["v"]["w"], info["v"]["h"]) == (320, 180), str(info["v"]))


def x1(audio):
    header("X1 x11 backend: full screen and region")
    before = media()
    send("record")
    time.sleep(3.0)
    send("record")
    clip = deliver_wait(before)
    check("X1: full-screen clip delivered", clip is not None)
    if clip:
        info = probe(clip)
        check("X1: 1366x768 h264", (info["v"]["w"], info["v"]["h"]) == (1366, 768) and info["v"]["codec"] == "h264", str(info["v"]))
        if audio:
            check("X1: audio present and |video - audio| < 0.2 s", info["a"] is not None and abs(info["dur"] - info["a"]["dur"]) < 0.2,
                  f"video {info['dur']:.2f}s audio {(info['a'] or {}).get('dur', 0):.2f}s")
    pause_between_scenarios()
    before = media()
    shot_path = SHOTS / "x1_screen.png"
    SHOTS.mkdir(parents=True, exist_ok=True)
    send("record 100,100,320,180")
    time.sleep(2.0)
    subprocess.run([str(HERE / "shot.sh"), str(N), str(shot_path)], capture_output=True)
    time.sleep(1.0)
    send("record")
    clip = deliver_wait(before)
    check("X1: region clip delivered", clip is not None)
    if clip:
        info = probe(clip)
        check("X1: region clip is 320x180", (info["v"]["w"], info["v"]["h"]) == (320, 180), str(info["v"]))
        # the clip's picture must BE that rectangle of the screen (a static gallery: close match)
        from PIL import Image, ImageChops, ImageStat
        fpng = OUT / "fake" / "_x1_first.png"
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-ss", "1", "-i", str(clip), "-frames:v", "1", str(fpng)], check=True)
        ref = Image.open(shot_path).convert("RGB").crop((100, 100, 420, 280))
        got = Image.open(fpng).convert("RGB")
        diff = sum(ImageStat.Stat(ImageChops.difference(ref, got)).mean) / 3
        check("X1: the clip shows that rectangle of the screen (mean abs diff < 12/255)", diff < 12, f"{diff:.1f}")
        got.save(SHOTS / "x1_clip_frame.png")


def sni_log(after=0.0):
    out = []
    try:
        for ln in (FD / "sni.jsonl").read_text().splitlines():
            d = json.loads(ln)
            if d["t"] >= after:
                out.append(d)
    except OSError:
        pass
    return out


def s10():
    header("S10 tray indicator (StatusNotifierItem) against a mock watcher")
    cfg(INIT="1.0", STOP_DELAY="0.8", MODE="normal")
    before = media()
    t0 = time.time()
    lc0 = live_count()
    send("record")
    time.sleep(0.8)
    check("S10: no dot yet during the dead time (kms clip not started)", not sni_log(t0), str(sni_log(t0)))
    wait(lambda: live_count() > lc0, 15)
    reg = wait(lambda: [d for d in sni_log(t0) if d["op"] == "item"], 15)
    check("S10: the item registers with the watcher when frames flow", bool(reg), str(sni_log(t0)))
    if reg:
        it = reg[0]
        check("S10: Id / Category / Status as a tray host expects",
              (it["id"], it["category"], it["status"]) == ("trontsnap-recording", "ApplicationStatus", "Active"), str(it))
        check("S10: red-dot pixmaps 16/22/32 px with the full ARGB payload",
              [p[:2] for p in it["pixmaps"]] == [[16, 16], [22, 22], [32, 32]] and all(p[2] == p[0] * p[1] * 4 for p in it["pixmaps"]), str(it["pixmaps"]))
        check("S10: tooltip says it is recording", it["tooltip"] and "recording" in it["tooltip"][0].lower(), str(it["tooltip"]))
    time.sleep(2.0)
    # a left click on the dot = stop (SIGUSR1 makes the mock call Activate on it)
    pid = procs(lambda a: str(HERE / "sni_mock.py") in a)
    t_click = time.time()
    for p in pid:
        os.kill(p, signal.SIGUSR1)
    clip = deliver_wait(before)
    check("S10: clicking the dot (Activate) stopped the recording and a clip was delivered", clip is not None)
    gone = wait(lambda: [d for d in sni_log(t0) if d["op"] == "gone"], 10)
    check("S10: the dot is gone from the tray after the stop (bus name released)", bool(gone))
    if gone:
        say(f"    dot vanished {gone[0]['t'] - t_click:.2f}s after the click")


def s11():
    header("S11 opening a video is deferred while a kmsgrab recording runs (UVD+VCE hung this GPU once)")
    cfg(INIT="1.0", STOP_DELAY="0.8", MODE="normal")
    open_log, nlog = FD / "open.log", FD / "cli_notify.log"
    xin = [str(HERE / "xinput.py"), "-d", f":{N}"]
    before = media()
    lc0 = live_count()
    send("record")
    wait(lambda: live_count() > lc0, 15)
    time.sleep(1.0)
    open_log.unlink(missing_ok=True)
    nlog.unlink(missing_ok=True)
    subprocess.run(xin + ["rightclick", "129", "134"], capture_output=True)
    time.sleep(1.5)
    subprocess.run(xin + ["click", "153", "221"], capture_output=True)  # "Open" in a video tile's menu
    time.sleep(2.0)
    check("S11: Open on a video did NOT launch the player during the recording",
          not (open_log.exists() and open_log.read_text().strip()), open_log.read_text() if open_log.exists() else "")
    check("S11: it said why", nlog.exists() and "Not opening the video yet" in nlog.read_text(), nlog.read_text() if nlog.exists() else "no notify-send call")
    send("record")
    deliver_wait(before)
    pause_between_scenarios()
    time.sleep(1.0)
    open_log.unlink(missing_ok=True)
    subprocess.run(xin + ["rightclick", "129", "134"], capture_output=True)
    time.sleep(1.5)
    subprocess.run(xin + ["click", "153", "221"], capture_output=True)
    ok = wait(lambda: open_log.exists() and open_log.read_text().strip(), 8)
    check("S11: with no recording running, Open launches the player", bool(ok), open_log.read_text() if open_log.exists() else "no xdg-open call")


def s9_note():
    header("S9 title-bar indicator")
    say(f"    screenshots: {SHOTS}/1_recording.png (REC pill), 2_saving.png (Saving...). READ THEM.")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", default="")
    ap.add_argument("--no-audio", action="store_true")
    ap.add_argument("--keep", action="store_true", help="leave the rig running")
    a = ap.parse_args()
    only = {x.strip().upper() for x in a.only.split(",") if x.strip()}
    want = lambda k: not only or k in only
    audio = not a.no_audio
    rig_start(audio) if (not only or only - {"X1"}) else None
    try:
        for key, fn in (("S1", lambda: s1(audio)), ("S2", s2), ("S3", s3), ("S4", s4), ("S5", lambda: s5(audio)),
                        ("S8", s8), ("S6", lambda: s6(audio)), ("S7", s7), ("S10", s10), ("S11", s11)):
            if want(key):
                pause_between_scenarios()
                fn()
        s9_note()
        if want("X1"):
            rig_start(audio, recorder="x11")
            pause_between_scenarios()
            x1(audio)
    finally:
        if not a.keep:
            rig_stop()
    fails = [l for s, l in results if s == "FAIL"]
    say(f"\n{len(results) - len(fails)} PASS / {len(fails)} FAIL")
    for l in fails:
        say(f"  FAIL: {l}")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
