# Vendored eframe

This directory contains `eframe` 0.29.1 from crates.io. It is licensed
`MIT OR Apache-2.0`; see the license declaration and upstream repository in
`Cargo.toml`.

TrontSnap carries one local Windows event-loop backport in
`src/native/run.rs`. Invisible Windows viewports do not receive
`RedrawRequested`, so due hidden-window frames are painted directly instead
of leaving winit's `ControlFlow::Poll` latched indefinitely. This follows the
current upstream eframe handling for invisible windows.

A second local backport, off Windows only (`src/native/epi_integration.rs`, `post_rendering`):
eframe always creates the root window hidden and calls `set_visible(true)` after the first
frame, which silently overrode `ViewportBuilder::with_visible(false)`. An app that asked to
start hidden (TrontSnap's Linux `--startup`) now stays unmapped on X11 until it asks to be
shown. winit-wayland's `set_visible` is a no-op, so Wayland clients minimize instead (see
`App::update`).
