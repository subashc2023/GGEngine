# gg-engine

Rules for work in gg-engine, beside the workspace's [AGENTS.md](../../AGENTS.md).

## Allocations

Steady-state frames don't allocate. Two tests check it:

- `ticks`: ticks on every thread, entities coming and going, animation and navigation stepping. Not
  physics, which allocates as contacts come and go.
- The GPU test `frames`: headless frames of an app without development UI (egui allocates), drawing
  a game's UI from its file.

After changing the windowed loop, run `cargo test -p gg-engine --test frames -- --windowed` without
`dev`, `tracy` or a gamepad, with the mouse over its window. gilrs allocates as it reads a pad,
winit for each key on Windows, and Tracy's client as it records.
