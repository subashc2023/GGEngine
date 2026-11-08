# Foxball

Rules for work on the 3D demo, beside the workspace's [AGENTS.md](../../AGENTS.md).

## The live recording

Some changes need a new live recording, `tests/references/live.recording`, since rewriting its
hashes would keep what they break: a change to the demo's layers, to a command's type that its lines
no longer read as, or to the arena's types, whose edits would then be refused at their ticks. Make
one with a run of the demo given `--record`, headless or windowed, that the tool API drives at once:
`cargo gg input` walks with W, grabs the cursor with a click and turns with `motion`, strafes with
D, kicks with Space and with a click, turns with E and lets go with Escape; `cargo gg set` sets a
field of the ball's, then `undo` and `redo`; and `quit` ends it, writing its end. Keep it to some
2,000 ticks, as its tests replay it three times.
