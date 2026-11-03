# Shards

Rules for work on the demo, beside the workspace's [AGENTS.md](../../AGENTS.md).

## The live recording

Some changes need a new live recording, `tests/references/live.recording`, since rewriting its
hashes would keep what they break: a change to the demo's layers, to a command's type that its lines
no longer read as, or to the arena's types, whose edits would then be refused at their ticks. Make
one with a windowed run of the demo and `--record` that steers and dashes with the keys, sets a
field through `cargo gg set`, then `undo` and `redo`, saves `scenes/arena.ron` changed and puts it
back, and closes.
