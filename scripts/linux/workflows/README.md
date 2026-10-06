# Queued Claude workflows for TrontSnap

`perf-native-dry.js` — the "make it fast, native and bug-free" pass, written 2026-10-06 and NOT yet run
(parked at the weekly usage cliff, 94% with reset Thu 2026-10-08 04:00).

Run it from a Claude Code session in ~/trontsnap with the Workflow tool:
`Workflow({scriptPath: "/home/trent/trontsnap/scripts/linux/workflows/perf-native-dry.js", args: {date: "2026-10-08"}})`

Phases: Baseline (adds profile.fastrel, measures an optimized binary) -> Perf (idle repaint incl. the
30 ms toast repaint, startup/memory, gallery/thumbs, capture latency, renderer cost) -> Native (Wayland/HiDPI/
app_id/theme-follow/portals, loupe+multimonitor+drag-out) -> Dry (loop-until-dry adversarial hunt: lifecycle,
failure injection, soak, security) -> Ship (fat-LTO release install). Roughly 15-25 Sonnet agents; budget a
few percent of a week. Contains hard-won GPU safety rules (radeon hangs): keep them.
