#!/usr/bin/env python3
"""Stop-before-live race: press the record hotkey, then press it AGAIN a moment later
(a user mashing the key because nothing seems to happen), and see what is left running.

  rec_race.py --gap 0.2 [--watch 8] --out DIR

One REAL kms recording at most (<= ~10 s). If the second press did not stop the helper
the script ends the stray recording itself with the whitelisted capkit-stop and reports
what the app delivered. Talks to the live app on 48761 exactly like the keybinding.
"""
import argparse, glob, json, os, shutil, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import rec_perf  # noqa: E402

LOG = os.path.expanduser("~/.cache/trontsnap/app.log")


def live_state():
    procs = rec_perf.snap_procs()
    tags = [t for t, _ in procs.values()]
    cmds = []
    for pid, (tag, _) in procs.items():
        if tag in ("pkexec", "capkit-kms") or tag.startswith("ffmpeg"):
            try:
                cmds.append((tag, open("/proc/%d/cmdline" % pid, "rb").read().replace(b"\0", b" ").decode()[:90]))
            except OSError:
                pass
    return {"kmsgrab": "ffmpeg:kmsgrab" in tags, "audio_sidecar": "ffmpeg:audio" in tags,
            "pkexec": tags.count("pkexec"), "procs": cmds}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gap", type=float, default=0.2, help="seconds between the two presses")
    ap.add_argument("--watch", type=float, default=8.0)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    if rec_perf.kmsgrab_alive():
        sys.exit("a kmsgrab recording is already running; not starting another")
    before = set(glob.glob(rec_perf.PICS + "/TrontSnap_*.mp4"))
    log0 = os.path.getsize(LOG) if os.path.exists(LOG) else 0
    res = {"gap_s": a.gap, "timeline": []}
    t0 = time.time()
    rec_perf.poke("record")
    time.sleep(a.gap)
    rec_perf.poke("record")
    res["second_press_at_s"] = round(time.time() - t0, 2)
    while time.time() - t0 < a.watch:
        time.sleep(0.5)
        st = live_state()
        res["timeline"].append({"t": round(time.time() - t0, 1), "kmsgrab": st["kmsgrab"], "audio": st["audio_sidecar"], "pkexec": st["pkexec"]})
    st = live_state()
    res["state_at_watch_end"] = st
    res["orphan_recording"] = st["kmsgrab"]
    if st["kmsgrab"]:
        t_stop = time.time()
        subprocess.run(["pkexec", "/usr/local/bin/capkit-stop"], stdin=subprocess.DEVNULL)
        res["ended_with_capkit_stop"] = True
        for _ in range(60):
            if not rec_perf.kmsgrab_alive() and set(glob.glob(rec_perf.PICS + "/TrontSnap_*.mp4")) - before:
                break
            time.sleep(0.5)
        time.sleep(1.0)
        res["stray_recording_ended_after_s"] = round(time.time() - t_stop, 1)
    new = sorted(set(glob.glob(rec_perf.PICS + "/TrontSnap_*.mp4")) - before)
    if new:
        dst = os.path.join(a.out, os.path.basename(new[-1]))
        shutil.move(new[-1], dst)
        res["delivered_clip"] = rec_perf.probe(dst)
    else:
        res["delivered_clip"] = None
    with open(LOG) as fh:
        fh.seek(log0)
        res["app_log_new_lines"] = fh.read().strip().splitlines()
    res["kmsgrab_after"] = rec_perf.kmsgrab_alive()
    print(json.dumps(res, indent=1))
    json.dump(res, open(os.path.join(a.out, "race_result.json"), "w"), indent=1)


if __name__ == "__main__":
    main()
