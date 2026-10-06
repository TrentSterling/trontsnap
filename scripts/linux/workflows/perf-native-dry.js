export const meta = {
  name: 'trontsnap-perf-native-dry',
  description: 'Make TrontSnap fast, natively Ubuntu and bug-free: measured baseline, one-at-a-time perf fixes, native-desktop fixes, loop-until-dry adversarial bug hunt (lifecycle, failure injection, soak, security), final verified release install',
  phases: [
    { title: 'Baseline', detail: 'fast-release profile + measured numbers on an optimized binary' },
    { title: 'Perf', detail: 'startup, idle CPU, gallery/thumbs, capture latency, memory: measure-fix-remeasure' },
    { title: 'Native', detail: 'renderer/Wayland/HiDPI/theme/app identity/portals' },
    { title: 'Dry', detail: 'rounds of adversarial finders until two consecutive rounds find nothing new' },
    { title: 'Ship', detail: 'full harness, fat-LTO release build, install, verify, docs' },
  ],
}

const DATE = (args && args.date) || 'run'
const OUT = `~/trontsnap/harness_out/${DATE}/perf-native`

const RULES = `
CONTEXT: TrontSnap (~/trontsnap, branch linux-port, Rust eframe/egui screenshot + recording tool) now runs on Trent's Ubuntu laptop (Lenovo G50-45: AMD Mullins A6-6310 4 weak cores 1.8GHz, 6 GB RAM, Radeon R5 on the legacy radeon driver, 1366x768, GNOME 46 Wayland) and on Windows (his main OS; the Windows build must never regress) and for a friend's Ubuntu box. Trent's goal, verbatim: "make trontsnap run fast and good native ubuntu rust app, make it good and fast and bug free, audit bugs, keep working". Read LINUX.md, scripts/linux/harness/README.md (the isolated GUI rig, XTest input driver, shot/clip tools, rec_test.sh) and git log first: an earlier overnight workflow already fixed many gallery/paste/recording/tray issues; do not redo them, find what is STILL wrong or slow.
Shared tree etiquette: other agents may be building or committing; check git status/log before editing, commit only your own files with the pathspec form (git commit -m ... -- <paths>), end commit messages with the Co-Authored-By line your environment's attribution reminder specifies, keep commits small and verified.
HARD RULES:
- Never cargo clean or delete target/ (builds are slow). Never run two cargo builds yourself at once. Use the fast optimized profile (profile.fastrel, created in the Baseline phase) for measurements and iteration; fat-LTO release only in the Ship phase.
- Trent's LIVE app is running from ~/trontsnap/target/debug/trontsnap (until Ship installs the release binary to ~/.local/bin/trontsnap); his GNOME hotkeys point at it. Restart it only by exact PID (pgrep -f '^target/debug/trontsnap --startu[p]' or the installed path), relaunch with setsid + logging, and leave it running on a working build. Never broad pkill -f (self-matches the harness wrapper; use [b]racket regexes).
- Test instances are ISOLATED (own Xvfb display, dbus-run-session, temp HOME, TRONTSNAP_PORT, TRONTSNAP_TEST=1, TRONTSNAP_RECORDER=x11) via scripts/linux/harness/gui_env.sh: never touch Trent's real gsettings keybindings/autostart/clipboard beyond what a task explicitly needs.
- GPU/driver safety: this radeon hung and froze the machine earlier (UVD hardware video decode + VCE encode together). NEVER play video with hardware decode (mpv --hwdec=vaapi etc.), at most 3 real kmsgrab recordings per agent each <=15 s and 60 s apart, none during other recordings. After any GPU-touching test: journalctl -k -b --since '-5 min' | grep -i -E 'uvd|vce|radeon.*error|hung'; if anything appears STOP GPU work and report.
- Machine is weak and shared (other workflows may be running): measure CPU TIME (user+sys from /proc/<pid>/stat) and best-of-N latencies rather than single wall-clock numbers, record loadavg with every measurement, and do not run heavy things in parallel.
- Windows safety: shared-code edits must stay valid for Windows. After ANY edit outside src/linux/* run the Windows cross-check that the earlier parity audit found (grep scripts/linux/harness/README.md, LINUX.md and git log for 'windows-gnu' / cross-check; the x86_64-pc-windows-gnu rustup target is installed) and report if it cannot run. Windows-only code stays cfg(windows); do not change Windows behaviour except to fix real bugs, and say so loudly.
- Don't add abstractions for hypothetical reuse. Terse dry comments that explain WHY; tag non-obvious findings with '// HARD-WON:'. Artifacts under ${OUT}/ (not /tmp). LOOK at screenshots yourself (Read the PNG) before claiming a visual result. Report honestly; never claim fixed without harness or live evidence.
`

const FIXRESULT = {
  type: 'object',
  properties: {
    fixed: { type: 'array', items: { type: 'object', properties: { id: { type: 'string' }, commit: { type: 'string' }, before_after: { type: 'string' } }, required: ['id', 'commit', 'before_after'] } },
    not_fixed: { type: 'array', items: { type: 'object', properties: { id: { type: 'string' }, reason: { type: 'string' } }, required: ['id', 'reason'] } },
    notes: { type: 'string' },
  },
  required: ['fixed', 'not_fixed', 'notes'],
}
const FINDINGS = {
  type: 'object',
  properties: {
    findings: { type: 'array', items: { type: 'object', properties: {
      id: { type: 'string' }, title: { type: 'string' }, severity: { type: 'string', enum: ['high', 'medium', 'low'] },
      evidence: { type: 'string' }, repro: { type: 'string' }, fix_plan: { type: 'string' }, files: { type: 'array', items: { type: 'string' } } },
      required: ['id', 'title', 'severity', 'evidence', 'fix_plan', 'files'] } },
  },
  required: ['findings'],
}

// === Baseline ===
phase('Baseline')
const baseline = await agent(`${RULES}
TASK:
1. Add a build profile to the root Cargo.toml (this is the one allowed Cargo.toml edit): [profile.fastrel] inherits = "release", lto = false (or "thin" only if the build time is acceptable), codegen-units = 16, incremental = true, opt-level = 3, strip = false, panic = "abort" unchanged semantics. Measure and report build times (from warm cache, and the cost of a one-line incremental change) for fastrel vs the fat-LTO release profile (do NOT build fat release now; estimate it from CPU count or skip). Binary: target/fastrel/trontsnap.
2. Build fastrel, then MEASURE the optimized binary in the isolated rig and record numbers in ${OUT}/baseline.md: (a) cold start to first painted frame (add a TRONTSNAP_TIMING=1 env that logs monotonic timestamps at main(), window created, first frame presented, gallery scan done, first thumbnails; keep it tiny and Linux-neutral), (b) idle CPU time per minute with the window visible and unfocused, minimized, and (if possible) hidden, (c) RSS/threads/fds after startup and with a seeded 2000-image gallery (create the seed set from the existing PNGs by copy/hardlink into the rig's temp HOME; one real mp4 too), (d) gallery scroll cost: CPU time while the harness scrolls a 2000-tile gallery for 20 s, thumbnail cache fill time, (e) one-shot 'trontsnap full' wall time from process start to PNG written and to clipboard set, best of 10, and 'trontsnap region' time to first overlay frame, (f) recorder start/stop latency on the x11 backend inside Xvfb, (g) which GL renderer eframe actually gets (check GL_RENDERER via the app's own log, eglinfo/glxinfo if present, or LIBGL_DEBUG; a silent llvmpipe/software fallback on the REAL session would be a major finding; the vendored eframe is in vendor/eframe, find which backend (glow vs wgpu) and which GL context it requests). Note perf/strace/valgrind availability and use what exists.
3. Rank the top 8 suspected hot spots by measured cost with evidence, as the work list for the next phase. Commit Cargo.toml + the timing hook + the baseline doc.
Return the ranked list (id, title, measured cost, where in code).`, { label: 'baseline', phase: 'Baseline', model: 'sonnet', schema: {
  type: 'object',
  properties: {
    fastrel_build: { type: 'string' },
    numbers: { type: 'string' },
    renderer: { type: 'string' },
    hotspots: { type: 'array', items: { type: 'object', properties: { id: { type: 'string' }, title: { type: 'string' }, measured: { type: 'string' }, where: { type: 'string' } }, required: ['id', 'title', 'measured', 'where'] } },
  },
  required: ['fastrel_build', 'numbers', 'renderer', 'hotspots'],
} })
log(`Baseline: ${baseline ? baseline.hotspots.length : 0} hotspots; renderer: ${baseline ? baseline.renderer : '?'}`)

// === Perf: measure -> fix -> re-measure, one area at a time (shared tree) ===
phase('Perf')
const PERF_AREAS = [
  { key: 'idle-and-repaint', prompt: 'Idle behaviour: any CPU burn while the window is visible-but-idle, unfocused, minimized, or hidden (Trent runs this for days; a 3-4% idle CPU on the debug build was seen). Find repaint loops (request_repaint*, timers, polling threads, the 48761 listener, watcher/inotify wakeups, toast 30ms repaint). Known lead: a hover-held trontsnap toast process (it repaints every 30 ms via request_repaint_after in src/toast.rs even when nothing changes) was seen burning ~130% CPU for minutes under software GL in the rig; make a held or idle toast repaint only when something changes (hover state, countdown tick at ~10 Hz, fade). Fix so idle CPU time approaches zero with no behaviour loss; verify the UI still updates immediately on events (new capture splices in, hover feedback).' },
  { key: 'startup-and-memory', prompt: 'Cold start to first frame and steady-state memory: lazy-init what can wait, avoid blocking the first frame on gallery scan/thumbnails/settings/gsettings forks/which-forks/D-Bus, cap thumbnail cache and decoded-image memory with an explicit budget (6 GB box), check for leaks by cycling captures and gallery refreshes in the rig and tracking RSS/threads/fds over 200+ cycles.' },
  { key: 'gallery-and-thumbnails', prompt: 'Gallery with thousands of items: scroll smoothness (frame time while scrolling in the rig), thumbnail decode pipeline (worker count vs 4 weak cores, nice/priority so it never starves the UI thread, JPEG/PNG scaled decode, cache on disk?), video thumbnails via ffmpeg (spawn count/limits), directory scan cost and incremental updates, search filtering cost per keystroke on 20k items.' },
  { key: 'capture-latency', prompt: 'Capture pipeline latency, the thing Trent feels when he presses a key: Print -> file written -> clipboard ready -> toast. Earlier audit saw ~4.8 s on the DEBUG build; measure fastrel/release, profile the one-shot path (xcap vs gnome-screenshot fallback ordering and cost, PNG encode level/filter/threads, clipboard subprocess hand-off, toast process spawn cost, sound), reduce wall time and make the clipboard ready BEFORE slow work like the toast and file indexing. Region picker: time to first overlay frame and freeze-frame capture. Recording start/stop latency for the x11/gnome backends and honest numbers for the kms path (do not run more than the allowed real kmsgrab recordings).' },
  { key: 'renderer-and-frame-cost', prompt: 'Rendering cost on the Mullins GPU: confirm hardware GL is used (not llvmpipe), frame time of the gallery/editor/settings, GPU-heavy egui choices (blur/shadows/gradients/theme effects, texture uploads, MSAA, vsync/present mode, continuous repaint animations), the annotation editor responsiveness on a 1366x768 capture and on a large image, and any shader/feature that costs a lot for little visual value. Provide a reduce-effects fallback only if measured to matter.' },
]
const perfResults = []
for (const a of PERF_AREAS) {
  const r = await agent(`${RULES}
Baseline findings (trust but verify): ${JSON.stringify(baseline)}
AREA: ${a.key}. ${a.prompt}
Method: reproduce and MEASURE first with the rig on target/fastrel, find the real root cause with evidence (perf/strace/timing logs), fix, re-measure, and commit each fix separately with before/after numbers in the message. Add or extend a harness script in scripts/linux/harness/ so the metric can be re-run (and, where a regression threshold makes sense, asserts it). Make sure Trent's live app is left running on a working build. Return what you fixed with before/after and what you could not.`, { label: `perf:${a.key}`, phase: 'Perf', model: 'sonnet', schema: FIXRESULT })
  perfResults.push({ key: a.key, r })
  log(`Perf ${a.key}: ${r ? r.fixed.length + ' fixed, ' + r.not_fixed.length + ' not' : 'FAILED'}`)
}

// === Native ===
phase('Native')
const NATIVE_AREAS = [
  { key: 'window-and-scaling', prompt: 'Native window behaviour on GNOME Wayland and X11: winit app_id/WM_CLASS must match the .desktop (StartupWMClass) so the dock shows the right icon and groups windows, window icon, HiDPI and fractional scaling (set GDK_SCALE-like conditions in the rig, check text sharpness at 1x and 2x with screenshots you look at), the custom title bar vs Wayland (drag/resize edges/maximize/minimize/close work natively, double-click to maximize, snap to halves with tiling-assistant), min/default window size on 1366x768, and whether the main window should run native Wayland or XWayland (the toast deliberately runs XWayland for placement): decide with evidence and apply.' },
  { key: 'theme-and-integration', prompt: 'Desktop integration: follow the GNOME light/dark preference live (org.gnome.desktop.interface color-scheme via the settings portal or gsettings monitor, like the Windows build follows Windows), accent colour if cheap, cursor theme/size, reduced-motion preference, use xdg-desktop-portal where it is the right native answer (file open/save dialogs if the app has any, OpenURI, Trash, reveal-in-file-manager via org.freedesktop.FileManager1 ShowItems without blocking the UI thread), freedesktop notifications for completion/errors, XDG dirs honoured (XDG_PICTURES_DIR via user-dirs, XDG_CONFIG_HOME/CACHE_HOME), a proper .desktop + MIME + AppStream so it behaves like a first-class Ubuntu app, and single-instance activation (second launch raises the first even on Wayland).' },
  { key: 'region-and-multimonitor', prompt: 'Capture correctness: region picker loupe/precision parity (the Windows picker has one), keyboard nudging and Esc/Enter behaviour, multi-monitor and mixed-scale setups (simulate a second output in Xvfb with xrandr-style layouts if possible; otherwise audit code paths and say what is unverified), HiDPI region coordinates, window-click selection, cursor inclusion, capture of the exact pixels (compare a captured PNG against the screen via the rig), and drag-out to other apps: evaluate implementing an XDND source or the Wayland data-offer file drag from the gallery/toast (Windows has OLE drag-out) and implement it if a robust approach exists within the existing dependencies, else document precisely why not.' },
]
const nativeResults = []
for (const a of NATIVE_AREAS) {
  const r = await agent(`${RULES}
Baseline findings: ${JSON.stringify(baseline)}
AREA: ${a.key}. ${a.prompt}
Reproduce in the isolated rig (screenshots you actually Read) or on the live session when the thing only exists there (state it), fix, prove, commit each fix separately, extend the harness. Update LINUX.md for behaviour changes. Leave Trent's live app running on a working build. Return fixed (with evidence) / not fixed (honest reason).`, { label: `native:${a.key}`, phase: 'Native', model: 'sonnet', schema: FIXRESULT })
  nativeResults.push({ key: a.key, r })
  log(`Native ${a.key}: ${r ? r.fixed.length + ' fixed, ' + r.not_fixed.length + ' not' : 'FAILED'}`)
}

// === Dry: loop-until-dry adversarial bug hunt ===
phase('Dry')
const LENSES = [
  { key: 'lifecycle', prompt: 'Process and thread lifecycle: start/stop/crash the app, recorder, toast, region picker, clipboard owners and OCR in every order and rapid repeats; look for zombies, orphaned ffmpeg/xclip/notify-send children, double instances, port-in-use races on 48761, hotkey presses while another capture is in flight, signals (SIGINT/SIGTERM/SIGHUP at logout), systemd/GNOME logout while recording, lid close/suspend-resume while recording (can only be reasoned about, not triggered), session marker recovery edge cases, mutex poisoning and panics on worker threads.' },
  { key: 'failure-injection', prompt: 'Failure injection: missing or broken tools (PATH without ffmpeg/xclip/gnome-screenshot/tesseract/notify-send/pkexec), full or read-only disk and quota, unwritable Pictures dir, paths with spaces/quotes/unicode/newlines/percent signs/#, huge or corrupt or zero-byte images and truncated mp4/webm, a gallery folder that disappears or is a symlink loop or slow network mount, no D-Bus session, no DISPLAY, clock jumps backwards, SIGPIPE, ENOSPC mid-recording. Each failure must produce a visible, honest message (toast/notification/log) and never a panic, hang, or silent data loss.' },
  { key: 'soak', prompt: 'Long-uptime soak (Trent leaves apps open for days): in the Xvfb rig with TRONTSNAP_RECORDER=x11, run 300+ cycles of capture/gallery-refresh/search/delete/undo/record-start-stop and sample RSS, thread count, fd count, tmp/cache dir growth, inotify watches, and CPU time every N cycles; find any monotonic growth, leaked temp files (stage dir, .muxing/.moving leftovers, clipboard daemons piling up), thumbnail cache that never evicts, log files that grow without bound. Fix what leaks.' },
  { key: 'security', prompt: 'Red-team the Linux build: the loopback command listener on 127.0.0.1:48761 (can a web page in the browser drive it via HTTP/DNS rebinding or a crafted request, can another local user or sandboxed app trigger record/full, is the command parsing exact, any unauthenticated file/path parameters, DoS by connection flood), temp files and stage/cache/config permissions (world-readable recordings or the clipboard?), predictable temp names and symlink attacks, the pkexec kmsgrab helper invocation (argument injection, PATH issues, stale marker abuse in session.marker recovery: could a forged marker make recover_orphans signal or move arbitrary things), shell-outs built from filenames (command injection through odd file names in opener/reveal/clipboard/OCR/ffmpeg arguments: must use argv, never a shell string), GNOME keybinding installer writing commands from current_exe (path quoting), autostart entry quoting, tesseract/ffmpeg on untrusted images. Produce proof-of-concept repros inside the isolated rig only, never against anything real.' },
]
const seen = new Set()
const keyOf = (f) => (f.title || '').toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim().slice(0, 80)
const dryLog = []
let dry = 0
let round = 0
while (dry < 2 && round < 4) {
  round++
  const found = (await parallel(LENSES.map(l => () => agent(`${RULES}
Round ${round} of the adversarial bug hunt. LENS: ${l.key}. ${l.prompt}
This is a FIND-ONLY pass (do not edit src/; harness/test scripts are fine, in the isolated rig): reproduce each suspected bug with a real repro before reporting it; unreproducible suspicion goes in at most low severity with a precise code-path argument. Already found in earlier rounds (do not repeat): ${JSON.stringify([...seen])}.
Recent commits to be aware of: run git log --oneline -40.`, { label: `dry${round}:${l.key}`, phase: 'Dry', model: 'sonnet', schema: FINDINGS })))).filter(Boolean).flatMap(r => r.findings)
  const fresh = []
  for (const f of found) { const k = keyOf(f); if (!seen.has(k)) { seen.add(k); fresh.push(f) } }
  log(`Dry round ${round}: ${found.length} reported, ${fresh.length} new`)
  if (!fresh.length) { dry++; continue }
  dry = 0
  const fixes = await agent(`${RULES}
Fix round ${round}. Findings (JSON): ${JSON.stringify(fresh, null, 1)}
Fix every high and medium and cheap lows, one commit each, each re-proved by re-running its repro in the rig; add the repro to scripts/linux/harness/ as a regression check where practical. Run the whole harness suite at the end (all must still pass). Return fixed/not fixed.`, { label: `dry${round}:fix`, phase: 'Dry', model: 'sonnet', schema: FIXRESULT })
  dryLog.push({ round, found: fresh.map(f => `[${f.severity}] ${f.title}`), fixes })
}

// === Ship ===
phase('Ship')
const ship = await agent(`${RULES}
SHIP STEP.
1. Run every harness suite and the Windows cross-check; record PASS/FAIL. Fix small failures (commit) or report.
2. Build the fat-LTO release (cargo build --release; slow on this CPU, generous timeout, nothing else building), measure it against the fastrel numbers (startup, idle CPU, capture latency) and record in ${OUT}/final.md.
3. Install: copy target/release/trontsnap to ~/.local/bin/trontsnap (timestamped backup of any existing one), stop Trent's live app by exact PID, relaunch from ~/.local/bin/trontsnap --startup (setsid, log to ~/.cache/trontsnap/app.log). The app rewrites the GNOME keybindings and autostart entry from current_exe on launch: VERIFY with gsettings (the four trontsnap-* bindings) and ~/.config/autostart/trontsnap.desktop that they point at ~/.local/bin/trontsnap. Live-test one 'full' one-shot, one region, one record toggle (kms backend, <=15 s), and confirm the toast docks bottom-right, then check the kernel log for radeon errors.
4. Update LINUX.md (status, perf numbers, native behaviour, remaining gaps) and the memory file ~/.claude/projects/-home-trent/memory/project_trontsnap_linux_port.md (new facts only: install path, fastrel profile, perf numbers, what is verified, remaining gaps).
Return a concise report: harness table, before/after numbers, install state, commits, remaining gaps.`, { label: 'ship', phase: 'Ship', model: 'sonnet', schema: { type: 'object', properties: { harness: { type: 'string' }, numbers: { type: 'string' }, install: { type: 'string' }, remaining_gaps: { type: 'string' } }, required: ['harness', 'numbers', 'install', 'remaining_gaps'] } })

return { baseline, perfResults, nativeResults, dryLog, ship }
