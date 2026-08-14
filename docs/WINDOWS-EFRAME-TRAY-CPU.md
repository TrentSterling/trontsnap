# Windows eframe tray CPU spin

This is a reusable diagnostic note for Windows Rust applications that use
eframe/winit and hide their root window to a notification icon.

## Symptom

The application consumes almost exactly one logical CPU core while hidden to the
system tray. Ordinary Windows minimize can remain completely idle, so a
minimized-only test does not reproduce the bug.

In TrontSnap v0.24.0, Task Manager showed roughly 4 percent total CPU on a
24-logical-processor machine. Thread sampling showed one UI thread continuously
running inside winit's Windows event dispatcher while all worker threads slept.

## Root cause

eframe 0.29.1 changes winit to `ControlFlow::Poll` when a scheduled repaint becomes
due, then calls `Window::request_redraw()`. Windows does not deliver
`RedrawRequested` to an invisible root window. The redraw callback that would
eventually restore `ControlFlow::Wait` therefore never runs, leaving the event loop
latched in `Poll`.

The hot profile is usually winit event dispatch plus repeated `Instant` or
`QueryPerformanceCounter` calls. Application paint code can be absent from the hot
stack because the application is waiting for a redraw event Windows will not send.

## Durable fix

Use an eframe native runner that handles an invisible Windows viewport by draining a
due frame directly instead of requesting a native redraw. TrontSnap pins eframe
0.29.1 locally and backports that behavior in
`vendor/eframe/src/native/run.rs`. The pin is declared in the root `Cargo.toml` under
`[patch.crates-io]` so every build, including release builds, uses the corrected
runner.

Application code should also avoid unnecessary wakeups:

- Never schedule a periodic repaint merely to service tray or IPC messages. Those
  event sources should wake the UI directly.
- Gate necessary timers, such as automatic theme polling, on native window
  visibility.
- Do not request a repaint for tray Enter, Move, or Leave events unless visible UI
  state actually changes.
- Return from the close-to-tray frame after cancelling close and hiding the window,
  so that frame cannot arm another repaint deadline.

Removing application timers can reduce the apparent CPU load, but it does not repair
an already latched `ControlFlow::Poll`. Fix the event-loop transition itself.

## Verification protocol

Use an optimized executable and test the real hidden path:

1. Show the native root window.
2. Close or hide it to the system tray with the application's real command.
3. Wait beyond every scheduled repaint deadline.
4. Move the cursor repeatedly across the application's real notification-icon
   rectangle so tray Enter, Move, and Leave traffic is exercised.
5. Sample per-process CPU for at least ten seconds.
6. Restore the window, hide it again, and repeat the hover and sample cycle.

Record CPU time, not only a rounded Task Manager percentage. TrontSnap v0.24.1 used
0 ms of process CPU in both ten-second hidden samples, including repeated real tray
icon traversals. Its v0.24.0 failure consumed approximately 96 to 98 percent of one
logical core under the same controlled test.
