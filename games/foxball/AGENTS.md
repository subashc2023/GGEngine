# Foxball

Rules for work on the 3D demo, beside the workspace's [AGENTS.md](../../AGENTS.md).

## The live recording

Some changes need a new live recording, `tests/references/live.recording`, since rewriting its
hashes would keep what they break: a change to the demo's layers, to a command's type that its lines
no longer read as, or to the arena's types, whose edits would then be refused at their ticks; and an
asset the arena names that the recording's list of assets, at its top, doesn't, which a replay from
source stands in for. Make one with a headless run of the demo given `--record` that the tool API
drives at once, paused and stepped between inputs, so that the run doesn't depend on the machine's
speed, reading where the fox and the ball are, so that its kicks reach the ball: `cargo gg input`
walks with W, strafes with A and D, grabs the cursor with a click and turns with `motion`, kicks
with Space and with a click until the ball goes into a goal, turns with E and lets go with Escape;
`cargo gg set` sets a field of the ball's, then `undo` and `redo`; and `quit` ends it, writing its
end. Keep it to some 2,000 ticks, as its tests replay it three times.
