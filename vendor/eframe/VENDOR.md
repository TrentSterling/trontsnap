# Vendored eframe

This directory contains `eframe` 0.29.1 from crates.io. It is licensed
`MIT OR Apache-2.0`; see the license declaration and upstream repository in
`Cargo.toml`.

TrontSnap carries one local Windows event-loop backport in
`src/native/run.rs`. Invisible Windows viewports do not receive
`RedrawRequested`, so due hidden-window frames are painted directly instead
of leaving winit's `ControlFlow::Poll` latched indefinitely. This follows the
current upstream eframe handling for invisible windows.
