#!/usr/bin/env python3
"""Recorder CPU / fps / stutter under controlled on-screen motion, on the REAL session.

  rec_perf.py --backend kms|gnome [--secs 10] [--rate 20] [--no-motion] --out DIR

MACHINE SAFETY (HARD-WON): the motion source is ffplay on a GENERATED lavfi graph,
so nothing is ever decoded. An earlier version played a clip in mpv; with
--hwdec=vaapi that hung the radeon UVD while kmsgrab encoded on the VCE, froze
the desktop and forced a hard reset. Do not reintroduce a media player here.

The motion is testsrc2 + temporal noise over the whole 1366x768 screen (worst
case for an encoder) with a 16-bit frame counter drawn as black/white boxes at
the bottom-left. The counter is read back out of the clip, so we can tell what
the RECORDER lost apart from what the source never presented:
  - counter gaps       = source frames that never reached the clip
  - repeated counters  = the clip repeated a frame (CFR padding or a hitch)
  - first-seconds table= stutter/warmup at the start of a clip

Flow: start ffplay, settle, sample a no-recording baseline, start the recorder,
sample CPU in 1 s buckets (box, per process, the app's own threads), stop, wait
for the file, analyse. kms goes through the live app on 48761 (the same poke the
keybinding does); gnome talks to org.gnome.Shell.Screencast with the options
start_gnome() uses and holds the bus connection like the app does.

CPU = jiffies from /proc/<pid>/stat (the root-owned kmsgrab ffmpeg is readable).
Other agents share this box: load average is printed, treat a noisy run as noisy.
"""
import argparse, glob, json, os, re, shutil, signal, socket, subprocess, sys, time

HZ = os.sysconf("SC_CLK_TCK")
HOME = os.path.expanduser("~")
PICS = os.path.join(HOME, "Pictures", "TrontSnap")
PORT = 48761
NBITS = 16
BIT_X0, BIT_Y, BIT_W, BIT_PITCH = 8, 726, 24, 28


# === motion source (generated, never decoded) =====================================
def motion_graph(rate, noise=35):
    g = ["testsrc2=s=1366x768:r=%d" % rate] + (["noise=alls=%d:allf=t" % noise] if noise else []) + [
         "drawbox=x=%d:y=%d:w=%d:h=%d:color=black:t=fill" % (BIT_X0 - 4, BIT_Y - 2, NBITS * BIT_PITCH + 4, BIT_W + 4)]
    for i in range(NBITS):
        g.append("drawbox=x=%d:y=%d:w=%d:h=%d:color=white:t=fill:enable='eq(mod(floor(n/%d),2),1)'"
                 % (BIT_X0 + i * BIT_PITCH, BIT_Y, BIT_W, BIT_W, 1 << i))
    return ",".join(g)


def start_motion(rate, logpath, noise=35):
    log = open(logpath, "wb")
    return subprocess.Popen(["ffplay", "-hide_banner", "-loglevel", "warning", "-f", "lavfi", "-i", motion_graph(rate, noise),
                             "-fs", "-noborder", "-an"], stdout=log, stderr=log, preexec_fn=os.setsid)


def ffplay_drops(logpath):
    """Last 'fd=N' (frames the PLAYER dropped) from ffplay's status line."""
    try:
        txt = open(logpath, "rb").read().decode("latin1")
    except OSError:
        return None
    m = re.findall(r"fd=\s*(\d+)", txt)
    return int(m[-1]) if m else None


# === /proc sampling ================================================================
def snap_procs():
    out = {}
    for d in glob.glob("/proc/[0-9]*"):
        try:
            with open(d + "/stat") as f:
                s = f.read()
            rp = s.rfind(")")
            comm = s[s.find("(") + 1:rp]
            fl = s[rp + 2:].split()
            j = int(fl[11]) + int(fl[12])
            pid = int(os.path.basename(d))
            tag = comm
            if comm == "ffmpeg":
                with open(d + "/cmdline", "rb") as f:
                    cl = f.read()
                tag = "ffmpeg:kmsgrab" if b"kmsgrab" in cl else ("ffmpeg:audio" if b"pulse" in cl else "ffmpeg:other")
            out[pid] = (tag, j)
        except Exception:
            pass
    return out


def snap_total():
    with open("/proc/stat") as f:
        v = list(map(int, f.readline().split()[1:]))
    return sum(v), v[3] + v[4]  # total, idle+iowait


def snap_threads(pid):
    """{tid: (jiffies, syscr)} for one process (the app under test)."""
    out = {}
    for d in glob.glob("/proc/%d/task/[0-9]*" % pid):
        try:
            s = open(d + "/stat").read()
            fl = s[s.rfind(")") + 2:].split()
            sysc = 0
            for ln in open(d + "/io"):
                if ln.startswith("syscr:"):
                    sysc = int(ln.split()[1])
            out[int(os.path.basename(d))] = (int(fl[11]) + int(fl[12]), sysc)
        except Exception:
            pass
    return out


def app_pid():
    r = subprocess.run(["ss", "-ltnpH", "sport = :%d" % PORT], capture_output=True, text=True).stdout
    m = re.search(r"pid=(\d+)", r)
    return int(m.group(1)) if m else None


def cores(p0, p1, wall, floor=0.01):
    agg = {}
    for pid, (tag, j) in p1.items():
        j0 = p0.get(pid, (tag, j))[1]  # born mid-window: counts from its first sample
        agg[tag] = agg.get(tag, 0) + (j - j0)
    return {k: round(v / HZ / wall, 3) for k, v in sorted(agg.items(), key=lambda kv: -kv[1]) if v / HZ / wall >= floor}


class Sampler:
    """Take 1 s-ish buckets; keep the whole timeline."""

    def __init__(self, pid):
        self.pid = pid
        self.last = (snap_procs(), snap_total(), snap_threads(pid) if pid else {}, time.time())
        self.rows = []

    def tick(self, label=""):
        p1, (t1, i1), th1, w1 = snap_procs(), snap_total(), (snap_threads(self.pid) if self.pid else {}), time.time()
        p0, (t0, i0), th0, w0 = self.last
        wall = w1 - w0
        row = {"t": round(w1, 2), "label": label, "wall": round(wall, 2),
               "box": round((1 - (i1 - i0) / max(t1 - t0, 1)) * os.cpu_count(), 2), "procs": cores(p0, p1, wall),
               "app_threads": {str(t): {"cores": round((j - th0.get(t, (j, 0))[0]) / HZ / wall, 3),
                                        "syscalls_per_s": round((s - th0.get(t, (0, s))[1]) / wall)}
                               for t, (j, s) in th1.items() if (j - th0.get(t, (j, 0))[0]) / HZ / wall >= 0.01}}
        self.rows.append(row)
        self.last = (p1, (t1, i1), th1, w1)
        return row

    def window(self, a, b):
        """Average cores/box over rows[a:b]."""
        rows = self.rows[a:b]
        if not rows:
            return {}
        tot = sum(r["wall"] for r in rows)
        procs, thr = {}, {}
        for r in rows:
            for k, v in r["procs"].items():
                procs[k] = procs.get(k, 0) + v * r["wall"]
            for t, d in r["app_threads"].items():
                x = thr.setdefault(t, [0, 0])
                x[0] += d["cores"] * r["wall"]
                x[1] += d["syscalls_per_s"] * r["wall"]
        return {"box_cores": round(sum(r["box"] * r["wall"] for r in rows) / tot, 2),
                "procs": {k: round(v / tot, 3) for k, v in sorted(procs.items(), key=lambda kv: -kv[1])[:12]},
                "app_threads": {t: {"cores": round(c / tot, 3), "syscalls_per_s": round(s / tot)} for t, (c, s) in thr.items()}}


def poke(word, port=PORT):
    s = socket.create_connection(("127.0.0.1", port), timeout=2)
    s.sendall(word.encode())
    s.close()


def kmsgrab_alive():
    return any(v[0] == "ffmpeg:kmsgrab" for v in snap_procs().values())


def probe(path):
    r = {"file": os.path.basename(path), "bytes": os.path.getsize(path)}
    j = json.loads(subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "stream=codec_name,width,height,r_frame_rate,avg_frame_rate",
         "-show_entries", "format=duration", "-of", "json", path], capture_output=True, text=True).stdout or "{}")
    v = [s for s in j.get("streams", []) if "width" in s]
    r["codec"] = v[0]["codec_name"] if v else None
    r["r_frame_rate"] = v[0].get("r_frame_rate") if v else None
    r["avg_frame_rate"] = v[0].get("avg_frame_rate") if v else None
    pk = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time",
                         "-of", "csv=p=0", path], capture_output=True, text=True).stdout.split()
    ts = sorted(float(x.strip(",")) for x in pk if x.strip(",") not in ("N/A", ""))
    if len(ts) > 2:
        dur = ts[-1] - ts[0]
        gaps = [b - a for a, b in zip(ts, ts[1:])]
        r.update(packets=len(ts), span_s=round(dur, 2), fps=round((len(ts) - 1) / dur, 1) if dur else None,
                 max_gap_ms=round(max(gaps) * 1000), median_gap_ms=round(sorted(gaps)[len(gaps) // 2] * 1000, 1))
        r["kbps"] = round(r["bytes"] * 8 / dur / 1000) if dur else None
    return r


# === clip analysis: read the frame counter back ===================================
def counter_frames(path):
    """[(pts_s, counter)] for every frame of the clip."""
    r = subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "info", "-i", path, "-an", "-vf",
                        "crop=%d:%d:%d:%d,scale=%d:1:flags=area,format=gray,showinfo"
                        % (NBITS * BIT_PITCH, BIT_W, BIT_X0, BIT_Y, NBITS),
                        "-vsync", "passthrough", "-f", "rawvideo", "-"], capture_output=True)
    pts = [float(m) for m in re.findall(rb"pts_time:([\d.]+)", r.stderr)]
    raw = r.stdout
    out = []
    for i, p in enumerate(pts):
        cell = raw[i * NBITS:(i + 1) * NBITS]
        if len(cell) < NBITS:
            break
        v = 0
        for b in range(NBITS):
            v |= (1 if cell[b] > 110 else 0) << b
        out.append((p, v))
    return out


def analyse_counter(path):
    fr = counter_frames(path)
    if len(fr) < 10:
        return {"error": "too few frames", "frames": len(fr)}
    t0 = fr[0][0]
    ctr = [c for _, c in fr]
    missing = sum(max(0, b - a - 1) for a, b in zip(ctr, ctr[1:]) if 0 < b - a < 1000)
    backwards = sum(1 for a, b in zip(ctr, ctr[1:]) if b < a)
    distinct = len(set(ctr))
    span = fr[-1][0] - t0
    # longest run of identical counters (a hitch), and where it happened
    best, run, start = (1, 0.0), 1, 0
    for i in range(1, len(fr)):
        if ctr[i] == ctr[i - 1]:
            run += 1
            if run > best[0]:
                best = (run, fr[start][0] - t0)
        else:
            run, start = 1, i
    per = {}
    for p, c in fr:
        per.setdefault(int(p - t0), []).append(c)
    table = {s: {"frames": len(v), "distinct": len(set(v)),
                 "src_frames_skipped": sum(max(0, b - a - 1) for a, b in zip(v, v[1:]) if 0 < b - a < 1000)}
             for s, v in sorted(per.items())}
    return {"frames": len(fr), "span_s": round(span, 2), "distinct_counters": distinct,
            "distinct_per_s": round(distinct / span, 1) if span else None,
            "source_frames_missing": missing, "counter_went_backwards": backwards,
            "longest_repeat_frames": best[0], "longest_repeat_at_clip_s": round(best[1], 2),
            "counter_first": ctr[0], "counter_last": ctr[-1],
            "per_clip_second": table}


# === recorders =====================================================================
def kms_record(secs, sampler):
    before = set(glob.glob(PICS + "/TrontSnap_*.mp4"))
    t0 = time.time()
    poke("record")
    live = None
    t_end = None
    while True:
        now = time.time()
        sampler.tick("rec")
        if live is None and kmsgrab_alive():
            live = now
        if live and now - t0 >= secs:
            break
        if now - t0 > 25:
            break
        time.sleep(max(0.0, 1.0 - (time.time() - now)))
    t_stop = time.time()
    poke("record")
    new = None
    deadline = time.time() + 60
    while time.time() < deadline:
        sampler.tick("stopping")
        n = set(glob.glob(PICS + "/TrontSnap_*.mp4")) - before
        if n and not kmsgrab_alive():
            new = sorted(n)[-1]
            time.sleep(0.6)
            break
        time.sleep(0.5)
    return new, {"poke_to_kmsgrab_s": None if live is None else round(live - t0, 2),
                 "recording_s": round(t_stop - t0, 1), "stop_to_file_s": round(time.time() - t_stop, 2)}


def gnome_record(secs, outdir, sampler):
    from gi.repository import Gio, GLib
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    tpl = os.path.abspath(os.path.join(outdir, "gnome_%d" % int(time.time())))  # absolute: a relative template lands in ~/Videos
    args = GLib.Variant("(sa{sv})", (tpl, {"framerate": GLib.Variant("i", 30), "draw-cursor": GLib.Variant("b", True)}))
    t_call = time.time()
    ok, f = bus.call_sync("org.gnome.Shell.Screencast", "/org/gnome/Shell/Screencast", "org.gnome.Shell.Screencast",
                          "Screencast", args, GLib.VariantType("(bs)"), Gio.DBusCallFlags.NONE, 8000, None).unpack()
    t_ready = time.time()
    t0 = time.time()
    while time.time() - t0 < secs:
        n = time.time()
        sampler.tick("rec")
        time.sleep(max(0.0, 1.0 - (time.time() - n)))
    t_stop = time.time()
    bus.call_sync("org.gnome.Shell.Screencast", "/org/gnome/Shell/Screencast", "org.gnome.Shell.Screencast",
                  "StopScreencast", None, None, Gio.DBusCallFlags.NONE, 8000, None)
    last = -1
    for _ in range(40):
        n = os.path.getsize(f) if os.path.exists(f) else 0
        if n and n == last:
            break
        last = n
        time.sleep(0.15)
    return f, {"start_call_s": round(t_ready - t_call, 2), "recording_s": round(t_stop - t_call, 1),
               "finalize_after_stop_s": round(time.time() - t_stop, 2)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--backend", required=True, choices=["kms", "gnome"])
    ap.add_argument("--secs", type=float, default=10)
    ap.add_argument("--rate", type=int, default=20, help="motion source fps (counter increments per frame)")
    ap.add_argument("--noise", type=int, default=35, help="temporal noise strength (0 = clean testsrc2, moderate motion)")
    ap.add_argument("--no-motion", action="store_true", help="idle desktop instead of the generated motion")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    if a.backend == "kms" and a.secs > 14:
        sys.exit("refusing: kms recordings are capped at ~15 s on this box (see MACHINE-SAFETY)")
    os.makedirs(a.out, exist_ok=True)
    tag = "%s_%s" % (a.backend, "idle" if a.no_motion else ("motion" if a.noise else "moderate"))

    player = None
    plog = os.path.join(a.out, "ffplay_%s.log" % tag)
    if not a.no_motion:
        player = start_motion(a.rate, plog, a.noise)
        time.sleep(5)
    pid = app_pid()
    res = {"backend": a.backend, "scenario": tag, "secs": a.secs, "motion_rate": None if a.no_motion else a.rate,
           "load_before": round(os.getloadavg()[0], 2), "app_pid": pid}
    try:
        sam = Sampler(pid)
        for _ in range(5):
            time.sleep(1)
            sam.tick("baseline")
        res["baseline"] = sam.window(0, 5)
        n_base = len(sam.rows)
        if a.backend == "kms":
            f, timing = kms_record(a.secs, sam)
        else:
            f, timing = gnome_record(a.secs, a.out, sam)
        rec_rows = [r for r in sam.rows[n_base:] if r["label"] == "rec"]
        res["timing"] = timing
        res["recording"] = sam.window(n_base, n_base + len(rec_rows))
        # steady state = after the start burst (first 4 buckets) so startup cost doesn't mask the encoder
        res["recording_steady"] = sam.window(n_base + 4, n_base + len(rec_rows))
        res["recording_first4s"] = sam.window(n_base, n_base + 4)
        res["stopping"] = sam.window(n_base + len(rec_rows), len(sam.rows))
        res["timeline"] = [{"t": r["label"], "box": r["box"], "kmsgrab": r["procs"].get("ffmpeg:kmsgrab", 0),
                            "app": r["procs"].get("trontsnap", 0), "shell": r["procs"].get("gnome-shell", 0),
                            "ffplay": r["procs"].get("ffplay", 0)} for r in sam.rows[n_base:]]
        res["load_after"] = round(os.getloadavg()[0], 2)
        if player:
            res["ffplay_frames_dropped_by_player"] = ffplay_drops(plog)
        if f and os.path.exists(f):
            if os.path.dirname(os.path.abspath(f)) != os.path.abspath(a.out):  # kms: move OUR clip out of the user's folder
                dst = os.path.join(a.out, "%s_%d%s" % (tag, int(time.time()), os.path.splitext(f)[1] or ".mp4"))
                shutil.move(f, dst)
                f = dst
            res["clip"] = probe(f)
            if not a.no_motion:
                res["counter"] = analyse_counter(f)
    finally:
        if player:
            try:
                os.killpg(player.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
    print(json.dumps(res, indent=1))
    with open(os.path.join(a.out, "perf_%s.json" % tag), "w") as fh:
        json.dump(res, fh, indent=1)


if __name__ == "__main__":
    main()
