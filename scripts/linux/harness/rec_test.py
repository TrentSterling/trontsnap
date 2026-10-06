#!/usr/bin/env python3
"""rec_test: end-to-end checks of TrontSnap's Linux recorder against the LIVE app.

Drives the real app over its command port (same bytes the Ctrl+Shift+Print
keybinding sends), then asserts on the files and processes that result.
Run via rec_test.sh. Prints PASS/FAIL/SKIP per check; exit 1 on any FAIL.

Cases: basic (A), capkit-first collision (B), trontsnap-first collision +
fresh restart (C), crash salvage (D, kills and relaunches the live app).
Touches the real screen/audio and pops a toast per delivered clip: that is the
point. Leaves the live app running a working build.
"""
import argparse, json, os, re, signal, socket, subprocess, sys, time
from pathlib import Path

HOME = Path.home()
REPO = Path(__file__).resolve().parents[3]
OUT = Path(os.environ.get("HARNESS_OUT") or REPO / "harness_out" / time.strftime("%Y-%m-%d"))
PIC = HOME / "Pictures" / "TrontSnap"
CAP = HOME / "Videos" / "captures"
STAGE = HOME / ".cache" / "trontsnap" / "rec"
LOG = HOME / ".cache" / "trontsnap" / "app.log"
CONF = HOME / ".config" / "trontsnap" / "settings.conf"
PORT = 48761
KMS_TOGGLE = HOME / "capkit" / "kms-toggle"
KMS_STOP = "/usr/local/bin/capkit-stop"
KMS_HELPER = "/usr/local/bin/capkit-kms"
CAP_RE = re.compile(r"^rec_\d{8}_\d{6}\.mp4$")

results = []          # (status, label)
created_pic = []      # clips THIS run caused in Pictures/TrontSnap
created_cap = []      # capkit files THIS run caused in ~/Videos/captures


# === Reporting ===
def say(msg):
    print(msg, flush=True)


def check(label, ok, detail=""):
    status = "PASS" if ok else "FAIL"
    results.append((status, label))
    say(f"  {status}  {label}" + (f"  [{detail}]" if detail else ""))
    return ok


def skip(label, why):
    results.append(("SKIP", label))
    say(f"  SKIP  {label}  [{why}]")


def header(title):
    say(f"\n=== {title} ===")


# === Process / file helpers ===
def procs(match):
    """[(pid, argv)] for every process whose argv satisfies match(argv). Scans /proc
    (HARD-WON: not pgrep -f, which matches its own shell wrapper)."""
    out = []
    me = os.getpid()
    for d in Path("/proc").iterdir():
        if not d.name.isdigit() or int(d.name) == me:
            continue
        try:
            argv = (d / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        argv = [a.decode("utf-8", "replace") for a in argv if a]
        if argv and match(argv):
            out.append((int(d.name), argv))
    return out


def is_ffmpeg(argv):
    return os.path.basename(argv[0]) == "ffmpeg"


def kms_pids():
    return [p for p, a in procs(lambda a: is_ffmpeg(a) and "kmsgrab" in a)]


def audio_pids():
    return [p for p, a in procs(lambda a: is_ffmpeg(a) and "@DEFAULT_MONITOR@" in a)]


def environ_of(pid):
    try:
        raw = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
    except OSError:
        return {}
    return dict(r.decode("utf-8", "replace").split("=", 1) for r in raw if b"=" in r)


def live_app_pid():
    """The Trent-session app: trontsnap with no/--startup args, and NOT a harness
    rig instance (those carry TRONTSNAP_TEST / TRONTSNAP_PORT in their env)."""
    def m(a):
        if os.path.basename(a[0]) != "trontsnap" or a[1:] not in ([], ["--startup"], ["app"], ["tray"]):
            return False
        return True
    for pid, _ in procs(m):
        e = environ_of(pid)
        if "TRONTSNAP_TEST" not in e and "TRONTSNAP_PORT" not in e:
            return pid
    return None


def port_up():
    try:
        socket.create_connection(("127.0.0.1", PORT), timeout=1).close()
        return True
    except OSError:
        return False


def send(cmd):
    """Same path the GNOME keybinding uses."""
    subprocess.run(["bash", "-c", f"echo -n {cmd} > /dev/tcp/127.0.0.1/{PORT}"], check=True)


def media(d):
    try:
        return {p.name for p in d.iterdir() if p.suffix in (".mp4", ".webm")}
    except OSError:
        return set()


def cap_files():
    try:
        return {p.name for p in CAP.iterdir() if CAP_RE.match(p.name)}
    except OSError:
        return set()


def stage_entries():
    try:
        return sorted(p.name for p in STAGE.iterdir())
    except OSError:
        return []


def wait_until(fn, timeout, every=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(every)
    return fn()


def log_size():
    try:
        return LOG.stat().st_size
    except OSError:
        return 0


def log_since(mark):
    try:
        with open(LOG, "rb") as f:
            f.seek(mark)
            return f.read().decode("utf-8", "replace")
    except OSError:
        return ""


def probe(path):
    """Dict of what the checks need, or None if ffprobe can't read it yet."""
    r = subprocess.run(
        ["ffprobe", "-v", "error", "-count_packets", "-show_streams", "-show_format", "-of", "json", str(path)],
        capture_output=True, text=True)
    if r.returncode != 0:
        return None
    try:
        j = json.loads(r.stdout)
    except ValueError:
        return None
    info = {"v": None, "a": None, "dur": float(j.get("format", {}).get("duration") or 0)}
    for s in j.get("streams", []):
        d = float(s.get("duration") or info["dur"] or 0)
        rec = {"codec": s.get("codec_name"), "dur": d, "pk": int(s.get("nb_read_packets") or 0),
               "w": s.get("width"), "h": s.get("height")}
        if s.get("codec_type") == "video" and info["v"] is None:
            info["v"] = rec
        elif s.get("codec_type") == "audio" and info["a"] is None:
            info["a"] = rec
    return info


def settled_probe(path, timeout=30):
    """Wait for a delivered file to be complete + readable (size still, ffprobe ok)."""
    last = -1
    end = time.time() + timeout
    while time.time() < end:
        try:
            sz = path.stat().st_size
        except OSError:
            time.sleep(0.3)
            continue
        if sz == last and sz > 0:
            info = probe(path)
            if info and info["v"]:
                return info
        last = sz
        time.sleep(0.7)
    return None


def audio_on():
    try:
        for line in CONF.read_text().splitlines():
            k, _, v = line.partition("=")
            if k.strip() == "RecordAudio":
                return v.strip() != "0"
    except OSError:
        pass
    return True  # settings.rs default


def expected_backend(pid):
    forced = environ_of(pid).get("TRONTSNAP_RECORDER")
    if forced:
        return forced
    if (HOME / ".config" / "trontsnap" / "record-start.sh").exists():
        return "hook"
    if os.access(KMS_HELPER, os.X_OK) and os.access(KMS_STOP, os.X_OK):
        return "kms"
    return "gnome" if os.environ.get("XDG_SESSION_TYPE") == "wayland" else "x11"


# === Recording drivers ===
def start_recording(backend):
    """Press record; wait until the recorder is really running. Returns log mark."""
    mark = log_size()
    send("record")
    if backend == "kms":
        ok = wait_until(kms_pids, 25)
    else:
        ok = wait_until(lambda: "recording via" in log_since(mark), 25)
    if not ok:
        say(f"    (recorder never came up; log tail: {log_since(mark)[-300:]!r})")
    return mark, bool(ok)


def stop_and_collect(before, timeout=60):
    """Press record again and wait for the delivered clip in Pictures/TrontSnap."""
    send("record")
    return collect(before, timeout)


def collect(before, timeout=60):
    new = wait_until(lambda: sorted(media(PIC) - before), timeout)
    if not new:
        return None, None
    name = new[-1]
    created_pic.extend(n for n in new if n not in created_pic)
    return PIC / name, settled_probe(PIC / name)


def quiet_system(label_prefix, extra_wait=6):
    """Everything the recorder spawned is gone and the stage dir is empty."""
    wait_until(lambda: not kms_pids() and not audio_pids() and not stage_entries(), extra_wait)
    check(f"{label_prefix}: no kmsgrab ffmpeg left", not kms_pids(), str(kms_pids()))
    check(f"{label_prefix}: no @DEFAULT_MONITOR@ ffmpeg left", not audio_pids(), str(audio_pids()))
    check(f"{label_prefix}: stage dir empty", not stage_entries(), str(stage_entries()))


def check_clip(label, path, info, backend, secs, want_audio):
    if not check(f"{label}: delivered file appears + is readable", bool(path and info), str(path)):
        return
    v, a = info["v"], info["a"]
    want_codec = "vp8" if (backend == "gnome" or path.suffix == ".webm") else "h264"
    check(f"{label}: codec {want_codec}", v["codec"] == want_codec, f"got {v['codec']} {v['w']}x{v['h']}")
    if backend == "kms":
        fps = v["pk"] / v["dur"] if v["dur"] else 0
        check(f"{label}: fps >= 27", fps >= 27, f"{fps:.1f} fps ({v['pk']} packets / {v['dur']:.2f}s)")
    else:
        skip(f"{label}: fps >= 27", f"backend {backend}, only enforced for kms")
    if secs:
        # kms trims ~1s of encoder warmup; allow generous slack either side.
        check(f"{label}: duration sane for {secs}s recording", secs - 3 <= v["dur"] <= secs + 4, f"{v['dur']:.2f}s")
    if want_audio is None:
        pass
    elif want_audio:
        check(f"{label}: audio stream present (Record audio on)", a is not None, "none" if not a else a["codec"])
        if a:
            d = abs(v["dur"] - a["dur"])
            check(f"{label}: |video_dur - audio_dur| < 0.15s", d < 0.15,
                  f"video {v['dur']:.3f}s audio {a['dur']:.3f}s diff {d:.3f}s")
    else:
        check(f"{label}: no audio stream (Record audio off)", a is None)


# === Cases ===
def case_basic(backend, secs):
    header(f"A. basic {secs}s recording ({backend})")
    want_audio = audio_on()
    before = media(PIC)
    mark, up = start_recording(backend)
    if not check("A: recorder started", up):
        return
    via = re.findall(r"recording via (\w+)", log_since(mark))
    say(f"    backend per log: {via}")
    time.sleep(secs)
    path, info = stop_and_collect(before)
    check_clip("A", path, info, backend, secs, want_audio)
    quiet_system("A")


def run_kms_toggle():
    return subprocess.run([str(KMS_TOGGLE)], capture_output=True, text=True, timeout=40)


def stop_capkit_recording(cap_before):
    """kms-toggle again (pgrep sees kmsgrab -> capkit-stop), then wait out the remux
    and collect capkit's rec_*.mp4 into harness_out."""
    run_kms_toggle()
    wait_until(lambda: not kms_pids(), 15)
    new = wait_until(lambda: sorted(cap_files() - cap_before), 40)
    for n in new or []:
        if n not in created_cap:
            created_cap.append(n)
        settled_probe(CAP / n, 20)
    return new


def case_capkit_first(backend):
    header("B. capkit recording first -> TrontSnap must refuse")
    if backend != "kms":
        skip("B: collision with capkit", f"backend is {backend}, collision guard is kms-only")
        return
    cap_before, pic_before = cap_files(), media(PIC)
    run_kms_toggle()
    if not check("B: capkit recording started", bool(wait_until(kms_pids, 25))):
        return
    time.sleep(2.0)
    mark = log_size()
    send("record")
    refused = wait_until(lambda: "another kmsgrab recording is already running" in log_since(mark), 10)
    check("B: TrontSnap refused to start (log says another kmsgrab is running)", bool(refused),
          log_since(mark)[-200:].strip())
    time.sleep(1.0)
    check("B: exactly one kmsgrab ffmpeg (no stacked second recorder)", len(kms_pids()) == 1, str(kms_pids()))
    check("B: no audio sidecar started by the refused attempt", not audio_pids(), str(audio_pids()))
    check("B: nothing delivered to Pictures/TrontSnap", media(PIC) == pic_before)
    check("B: stage dir empty after refusal", not stage_entries(), str(stage_entries()))
    new = stop_capkit_recording(cap_before)
    check("B: capkit's own recording still completes", bool(new), str(new))
    quiet_system("B")


def case_trontsnap_first(backend, want_audio):
    header("C. TrontSnap recording, capkit toggle stops it, next press starts fresh")
    if backend != "kms":
        skip("C: capkit stops TrontSnap's recording", f"backend is {backend}")
        return
    cap_before, pic_before = cap_files(), media(PIC)
    mark, up = start_recording(backend)
    if not check("C: TrontSnap recording started", up):
        return
    time.sleep(4.0)
    run_kms_toggle()  # sees kmsgrab -> capkit-stop
    path, info = collect(pic_before, 70)
    check_clip("C1 (stopped by capkit)", path, info, backend, 4, want_audio)
    check("C1: capkit's rec_*.mp4 for our session moved out of ~/Videos/captures",
          not (cap_files() - cap_before), str(sorted(cap_files() - cap_before)))
    quiet_system("C1")

    # Next press must START (session slot cleared), not try to stop a ghost.
    pic_before = media(PIC)
    mark, up = start_recording(backend)
    if not check("C2: next press starts a fresh recording", up):
        return
    time.sleep(3.0)
    path, info = stop_and_collect(pic_before)
    check_clip("C2 (fresh start, normal stop)", path, info, backend, 3, want_audio)
    quiet_system("C2")


def case_crash_salvage(backend, want_audio):
    header("D. crash salvage: kill -9 the live app mid-recording, relaunch")
    pid = live_app_pid()
    if not check("D: live app found", pid is not None):
        return
    pic_before, cap_before = media(PIC), cap_files()
    mark, up = start_recording(backend)
    if not check("D: recording started", up):
        return
    time.sleep(5.0)
    marker = (STAGE / "session.marker").exists()
    check("D: session marker written (so a crash is recoverable)", marker)
    os.kill(pid, signal.SIGKILL)
    wait_until(lambda: not Path(f"/proc/{pid}").exists(), 5)
    check("D: app is dead", not Path(f"/proc/{pid}").exists())
    say(f"    orphans right after kill -9: kmsgrab={kms_pids()} audio={audio_pids()} stage={stage_entries()}")
    # Relaunch exactly as the lab does: detached, from the repo, log truncated.
    logf = open(LOG, "wb")
    subprocess.Popen(["target/debug/trontsnap", "--startup"], cwd=REPO, stdout=logf, stderr=subprocess.STDOUT,
                     stdin=subprocess.DEVNULL, start_new_session=True)
    check("D: relaunched app binds the port", bool(wait_until(port_up, 60, 0.5)))
    path, info = collect(pic_before, 90)
    check("D: crashed recording salvaged into Pictures/TrontSnap", bool(path and info), str(path))
    if info:
        v = info["v"]
        check("D: salvaged clip is playable h264 with >1s of video",
              v["codec"] == "h264" and v["dur"] > 1.0, f"{v['codec']} {v['dur']:.2f}s")
        if want_audio:
            say(f"    note: audio {'present' if info['a'] else 'absent'} in salvaged clip "
                "(salvage doesn't mux the sidecar; absent is expected)")
    check("D: salvage logged", "salvaging" in LOG.read_text(errors="replace"))
    check("D: capkit's rec_*.mp4 moved out of ~/Videos/captures", not (cap_files() - cap_before),
          str(sorted(cap_files() - cap_before)))
    quiet_system("D", extra_wait=12)
    check("D: live app alive on a working build", live_app_pid() is not None and port_up())


# === Main ===
def cleanup():
    """Only files this run created: clips -> harness_out/rec_test_clips, capkit
    leftovers -> harness_out/capkit_files. Never delete."""
    if created_pic:
        dst = OUT / "rec_test_clips"
        dst.mkdir(parents=True, exist_ok=True)
        for n in created_pic:
            try:
                os.replace(PIC / n, dst / n)
            except OSError:
                pass
        say(f"\nmoved {len(created_pic)} test clip(s) from Pictures/TrontSnap to {dst}")
    if created_cap:
        dst = OUT / "capkit_files"
        dst.mkdir(parents=True, exist_ok=True)
        for n in created_cap:
            try:
                os.replace(CAP / n, dst / n)
            except OSError:
                pass
        say(f"moved {len(created_cap)} capkit test file(s) from ~/Videos/captures to {dst}")
    # Abort safety: a still-running kmsgrab at exit is ours to stop (allowed helper).
    if kms_pids():
        say("a kmsgrab ffmpeg is still running at exit; stopping it via capkit-stop")
        subprocess.run(["pkexec", KMS_STOP], stdin=subprocess.DEVNULL)


def main():
    ap = argparse.ArgumentParser(description="TrontSnap recorder end-to-end test (LIVE app)")
    ap.add_argument("--secs", type=int, default=6, help="length of the basic recording (default 6)")
    ap.add_argument("--only", default="", help="comma list of cases to run: A,B,C,D (default all)")
    ap.add_argument("--no-crash", action="store_true", help="skip case D (it kills + relaunches the live app)")
    a = ap.parse_args()
    only = {c.strip().upper() for c in a.only.split(",") if c.strip()} or set("ABCD")
    if a.no_crash:
        only.discard("D")

    header("preflight")
    pid = live_app_pid()
    if not check("live app running", pid is not None):
        return 1
    if not check(f"live app answers on port {PORT}", port_up()):
        return 1
    backend = expected_backend(pid)
    want_audio = audio_on()
    say(f"    live pid {pid}, expected backend {backend}, Record audio {'on' if want_audio else 'OFF'}")
    clean = not kms_pids() and not audio_pids() and not stage_entries()
    if not check("system idle: no kmsgrab/@DEFAULT_MONITOR@ ffmpeg, stage dir empty", clean,
                 f"kms={kms_pids()} audio={audio_pids()} stage={stage_entries()}"):
        return 1
    if backend == "kms":
        check("capkit helpers present (kms-toggle, capkit-kms, capkit-stop)",
              KMS_TOGGLE.exists() and os.access(KMS_HELPER, os.X_OK) and os.access(KMS_STOP, os.X_OK))

    try:
        if "A" in only:
            case_basic(backend, a.secs)
        if "B" in only:
            case_capkit_first(backend)
        if "C" in only:
            case_trontsnap_first(backend, want_audio)
        if "D" in only:
            case_crash_salvage(backend, want_audio)
    finally:
        cleanup()

    header("summary")
    n = {k: sum(1 for s, _ in results if s == k) for k in ("PASS", "FAIL", "SKIP")}
    say(f"PASS {n['PASS']}  FAIL {n['FAIL']}  SKIP {n['SKIP']}")
    for s, l in results:
        if s == "FAIL":
            say(f"  FAIL  {l}")
    return 1 if n["FAIL"] else 0


if __name__ == "__main__":
    sys.exit(main())
