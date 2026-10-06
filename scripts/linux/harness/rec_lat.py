#!/usr/bin/env python3
"""Start/stop latency and A/V sync of a TrontSnap recording, measured against a
wall-clock beacon on the real screen (see rec_clock.py). kms backend via the live
app on 48761, exactly like the Ctrl+Shift+Print binding.

  rec_lat.py --secs 10 --out DIR [--flash-at 4.5]

Reports (seconds, wall clock, all relative to the key press):
  start:  kmsgrab up, audio sidecar up, notify-send seen, first frame IN THE CLIP
          (= press -> first captured instant; includes pkexec+warmup AND capkit's 1 s trim)
  stop:   kmsgrab gone, capkit file seen, delivered file seen, toast window seen,
          last frame in the clip vs the stop press
  sync:   audio onset (1 kHz beep) minus the video flash onset inside the clip
"""
import argparse, glob, json, os, shutil, subprocess, sys, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import rec_clock  # noqa: E402
import rec_perf  # noqa: E402  (snap_procs, poke, probe, PICS)

CAPKIT_DIR = os.path.expanduser("~/Videos/captures")


def proc_start_epoch(pid):
    st = open("/proc/%d/stat" % pid).read()
    ticks = int(st[st.rfind(")") + 2:].split()[19])
    btime = int([x for x in open("/proc/stat") if x.startswith("btime")][0].split()[1])
    return btime + ticks / os.sysconf("SC_CLK_TCK")


def reader(proc, sink):
    for line in proc.stdout:
        try:
            sink.append((time.time(), json.loads(line)))
        except Exception:
            pass


def first_event(sink, key):
    for t, d in sink:
        if key in d:
            return t, d
    return None, None


def frames_wall(clip, press_t, yoff=None):
    """[(clip_pts, wall_epoch, flash)] for every frame, decoded from the beacon strip.
    `yoff` = where the strip sits INSIDE the clip (default: its place on the full screen; a
    region recording shifts it by the region's top)."""
    yoff = rec_clock.YOFF if yoff is None else yoff
    tmp = os.path.join(os.path.dirname(clip), "_frames_" + os.path.basename(clip))
    shutil.rmtree(tmp, ignore_errors=True)
    os.makedirs(tmp)
    r = subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "info", "-i", clip, "-an", "-vf",
                        "crop=1344:32:0:%d,showinfo" % yoff, "-vsync", "passthrough", os.path.join(tmp, "f%05d.png")],
                       capture_output=True, text=True)
    pts = []
    for ln in r.stderr.splitlines():
        if "pts_time:" in ln and "Parsed_showinfo" in ln:
            pts.append(float(ln.split("pts_time:")[1].split()[0]))
    out = []
    for i, p in enumerate(pts):
        fp = os.path.join(tmp, "f%05d.png" % (i + 1))
        if not os.path.exists(fp):
            break
        v, fl = rec_clock.decode_frame(fp, yoff=0)  # frames are pre-cropped to the strip
        out.append((p, rec_clock.wall_from_mod(v, press_t), fl))
    shutil.rmtree(tmp, ignore_errors=True)
    return out


def audio_onset(clip):
    st = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries", "stream=start_time",
                         "-of", "csv=p=0", clip], capture_output=True, text=True).stdout.strip()
    if not st:
        return None
    start = float(st) if st != "N/A" else 0.0
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", clip, "-vn", "-ac", "1", "-ar", "8000", "-f", "s16le", "-"],
                         capture_output=True).stdout
    import array
    a = array.array("h")
    a.frombytes(raw[: len(raw) // 2 * 2])
    for i, s in enumerate(a):
        if abs(s) > 2500:
            return start + i / 8000.0
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--secs", type=float, default=10)
    ap.add_argument("--flash-at", type=float, default=7.5)
    ap.add_argument("--out", required=True)
    ap.add_argument("--settle", type=float, default=2.0)
    ap.add_argument("--quiet", type=float, default=6.0, help="seconds after the press with NO polling")
    ap.add_argument("--restart-probe", type=float, default=None, metavar="SECS",
                    help="press record AGAIN this many seconds after the stop press (tests the stop->start race)")
    ap.add_argument("--move-pointer", nargs=3, type=float, default=None, metavar=("X", "Y", "AT"),
                    help="XTest-move the real pointer to X,Y AT seconds after the press, then look for it in the clip")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)

    clock = subprocess.Popen([sys.executable, os.path.join(HERE, "rec_clock.py")], stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, text=True, bufsize=1)
    csink = []
    threading.Thread(target=reader, args=(clock, csink), daemon=True).start()
    for _ in range(100):
        if first_event(csink, "ready")[0]:
            break
        time.sleep(0.1)
    time.sleep(a.settle)

    dbm = subprocess.Popen(["dbus-monitor", "--session", "interface='org.freedesktop.Notifications',member='Notify'"],
                           stdout=subprocess.PIPE, text=True, bufsize=1)
    notif = []

    def mon():
        for line in dbm.stdout:
            if "string" in line or "member=Notify" in line:
                notif.append((time.time(), line.strip()))

    threading.Thread(target=mon, daemon=True).start()

    toast_t = []
    stop_toast = threading.Event()

    def toast_proc_alive():
        for d in glob.glob("/proc/[0-9]*"):
            try:
                cl = open(d + "/cmdline", "rb").read().split(b"\0")
            except Exception:
                continue
            if len(cl) > 2 and cl[0].endswith(b"trontsnap") and cl[1] == b"toast":
                return True
        return False

    toast_proc = []

    def toast_poll():
        # process spawn first (cheap /proc scan), then the mapped window (xinput find, only once the process exists)
        while not stop_toast.is_set():
            if not toast_proc and toast_proc_alive():
                toast_proc.append(time.time())
            if toast_proc:
                r = subprocess.run([sys.executable, os.path.join(HERE, "xinput.py"), "-d", ":0", "find", "TrontSnap Toast"],
                                   capture_output=True, text=True)
                if r.returncode == 0 and r.stdout.strip() and "unmapped" not in r.stdout:
                    toast_t.append((time.time(), r.stdout.strip()))
                    return
                time.sleep(0.25)
            else:
                time.sleep(0.1)

    before_pics = set(glob.glob(rec_perf.PICS + "/TrontSnap_*.mp4"))
    before_capkit = set(glob.glob(CAPKIT_DIR + "/rec_*.mp4"))
    ev = {}
    vm = subprocess.Popen(["vmstat", "-n", "1"], stdout=open(os.path.join(a.out, "vmstat.txt"), "w"))
    t_press = time.time()
    rec_perf.poke("record")
    # HARD-WON: do NOT scan /proc during the start window. On this box the start itself (pkexec +
    # ffmpeg/VAAPI init) already competes with the compositor; a 10 Hz /proc poller from this
    # script froze the beacon (and so the clip) for ~1 s and made the first run look worse.
    # Sleep, then read the kmsgrab start time back from /proc afterwards.
    time.sleep(a.quiet)
    moved = []
    seen = {"kms": None, "audio": None}
    for pid, (tag, _) in rec_perf.snap_procs().items():
        if tag in ("ffmpeg:kmsgrab", "ffmpeg:audio"):
            seen["kms" if tag == "ffmpeg:kmsgrab" else "audio"] = proc_start_epoch(pid)
    t_flash_sent = False
    while time.time() - t_press < a.secs:
        now = time.time()
        if a.move_pointer and not moved and now - t_press >= a.move_pointer[2]:
            subprocess.run([sys.executable, os.path.join(HERE, "xinput.py"), "-d", ":0", "move",
                            str(int(a.move_pointer[0])), str(int(a.move_pointer[1]))], capture_output=True)
            moved.append(time.time())
        if not t_flash_sent and now - t_press >= a.flash_at:
            clock.stdin.write("flash\n")
            clock.stdin.flush()
            t_flash_sent = True
        time.sleep(0.05)
    t_stop = time.time()
    threading.Thread(target=toast_poll, daemon=True).start()
    rec_perf.poke("record")
    restart_t = None
    gone = capkit = deliv = None
    while time.time() - t_stop < 60:
        now = time.time()
        if a.restart_probe is not None and restart_t is None and now - t_stop >= a.restart_probe:
            rec_perf.poke("record")
            restart_t = now
        alive = any(p[0] == "ffmpeg:kmsgrab" for p in rec_perf.snap_procs().values())
        if not alive and gone is None:
            gone = now
        if capkit is None and set(glob.glob(CAPKIT_DIR + "/rec_*.mp4")) - before_capkit:
            capkit = now
        new = set(glob.glob(rec_perf.PICS + "/TrontSnap_*.mp4")) - before_pics
        if new and deliv is None:
            deliv = now
        if deliv and toast_t:
            break
        time.sleep(0.1)
    restart_info = None
    if restart_t is not None:
        time.sleep(4.0)  # let a refusal notification (or a second recording) show up
        second = any(p[0] == "ffmpeg:kmsgrab" for p in rec_perf.snap_procs().values())
        restart_info = {"pressed_after_stop_s": round(restart_t - t_stop, 2), "second_kmsgrab_running_4s_later": second}
        if second:  # the guard did not refuse: end the extra recording right away, it is not part of the test
            rec_perf.poke("record")
            restart_info["stopped_extra_recording"] = True
            time.sleep(6.0)
    time.sleep(1.0)
    stop_toast.set()
    clock.stdin.write("quit\n")
    clock.stdin.flush()
    dbm.terminate()
    vm.terminate()

    new = sorted(set(glob.glob(rec_perf.PICS + "/TrontSnap_*.mp4")) - before_pics)
    res = {"secs": a.secs, "press_epoch": t_press, "stop_epoch": t_stop}
    rel = lambda t: None if t is None else round(t - t_press, 2)
    res["start"] = {"kmsgrab_process_born_after_press_s": rel(seen["kms"]), "audio_sidecar_born_after_press_s": rel(seen["audio"])}
    # dbus-monitor prints the Notify call twice (app->shell, shell->daemon) and one `string` per argument:
    # app_name, icon, summary, body. Fold that into one record per distinct notification.
    seen_n, folded, cur = set(), [], None
    for t, l in notif:
        if "member=Notify" in l:
            cur = {"t_rel_s": rel(t), "strings": []}
            folded.append(cur)
        elif cur is not None and l.startswith("string") and len(cur["strings"]) < 4:
            cur["strings"].append(l[len('string "'):-1])
    res["notifications"] = []
    for n in folded:
        key = tuple(n["strings"][2:4])
        if len(n["strings"]) >= 4 and key not in seen_n:
            seen_n.add(key)
            res["notifications"].append({"t_rel_s": n["t_rel_s"], "summary": key[0], "body": key[1]})
    res["restart_probe"] = restart_info
    ft, fd = first_event(csink, "flash_wall")
    res["beacon_stalls"] = [d["stall"] for t, d in csink if "stall" in d]
    res["ptr_events"] = [d["ptr"] for t, d in csink if "ptr" in d][:5]
    res["stop"] = {"kmsgrab_gone_after_stop_s": None if gone is None else round(gone - t_stop, 2),
                   "capkit_file_after_stop_s": None if capkit is None else round(capkit - t_stop, 2),
                   "delivered_after_stop_s": None if deliv is None else round(deliv - t_stop, 2),
                   "toast_process_after_stop_s": None if not toast_proc else round(toast_proc[0] - t_stop, 2),
                   "toast_window_mapped_after_stop_s": None if not toast_t else round(toast_t[0][0] - t_stop, 2),
                   "toast_geom": toast_t[0][1] if toast_t else None}
    # unattended-cleanup of anything the toast left behind is the app's job; we only read.
    if new:
        clip = os.path.join(a.out, "lat_" + os.path.basename(new[-1]))
        shutil.move(new[-1], clip)
        res["clip"] = rec_perf.probe(clip)
        fr = frames_wall(clip, t_press)
        if fr:
            res["start"]["first_frame_wall_after_press_s"] = round(fr[0][1] - t_press, 2)
            res["stop"]["last_frame_wall_after_stop_s"] = round(fr[-1][1] - t_stop, 2)
            res["clip_frames"] = len(fr)
            frozen = 0
            for i in range(1, len(fr)):
                if fr[i][1] - fr[0][1] < 0.02:
                    frozen = i
                else:
                    break
            res["start"]["frozen_frames_at_clip_start"] = frozen + 1
            smooth = None
            for i in range(0, len(fr) - 30):
                if all(0.93 < (fr[j + 15][1] - fr[j][1]) / max(fr[j + 15][0] - fr[j][0], 1e-6) < 1.07 for j in range(i, len(fr) - 15, 5)):
                    smooth = i
                    break
            if smooth is not None:
                res["start"]["smooth_playback_from_clip_s"] = round(fr[smooth][0], 2)
                res["start"]["smooth_content_after_press_s"] = round(fr[smooth][1] - t_press, 2)
            if moved and a.move_pointer:
                # cursor-in-clip check: frame ~1.5 s after the pointer move, 120x120 around it, vs the flat ground colour
                mw = moved[0] + 1.5
                cand = [f for f in fr if f[1] >= mw]
                if cand:
                    X, Y = int(a.move_pointer[0]), int(a.move_pointer[1])
                    png = os.path.join(a.out, "cursor_region.png")
                    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-ss", "%.3f" % cand[0][0], "-i", clip,
                                    "-frames:v", "1", "-update", "1", "-vf", "crop=120:120:%d:%d" % (X - 30, Y - 30), png])
                    from PIL import Image
                    im = Image.open(png).convert("RGB")
                    g = rec_clock.GROUND
                    odd = sum(1 for px in im.getdata() if max(abs(px[i] - g[i]) for i in range(3)) > 12)
                    res["cursor"] = {"pointer_at": [X, Y], "beacon_ptr_events": [d["ptr"] for t, d in csink if "ptr" in d][:6],
                                     "pixels_not_background_in_120x120": odd, "frame_wall_after_move_s": round(cand[0][1] - moved[0], 2)}
            fl = [f for f in fr if f[2]]
            if fl and fd:
                vflash = fl[0][0]
                ao = audio_onset(clip)
                res["sync"] = {"flash_frame_clip_pts_s": round(vflash, 3), "audio_onset_clip_s": None if ao is None else round(ao, 3),
                               "audio_minus_video_s": None if ao is None else round(ao - vflash, 3),
                               "flash_wall_rel_press_s": round(fd["flash_wall"] - t_press, 2),
                               "flash_frame_wall_vs_sent_s": round(fl[0][1] - fd["flash_wall"], 3),
                               "note": "paplay spawn adds ~0.05-0.15 s to the audible beep, so a small POSITIVE number is a perfect sync"}
    print(json.dumps(res, indent=1))
    with open(os.path.join(a.out, "lat_result.json"), "w") as fh:
        json.dump(res, fh, indent=1)


if __name__ == "__main__":
    main()
