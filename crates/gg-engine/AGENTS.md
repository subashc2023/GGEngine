# gg-engine

Rules for work in gg-engine, beside the workspace's [AGENTS.md](../../AGENTS.md).

## Allocations

Steady-state frames don't allocate. The `ticks` test checks ticks on every thread, entities coming
and going and animation stepping, but no physics, which allocates as contacts come and go; the GPU test `frames`, headless
frames of an app without UI, as egui allocates. After changing the windowed loop, run
`cargo test -p gg-engine --test frames -- --windowed` without `dev`, `tracy` or a gamepad, with the
mouse over its window: gilrs allocates as it reads a pad, winit for each key on Windows, and Tracy's
client as it records.
