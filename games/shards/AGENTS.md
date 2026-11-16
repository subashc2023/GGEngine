# Shards

Rules for work on the demo, beside the workspace's [AGENTS.md](../../AGENTS.md).

## The live recording

Rewriting `tests/references/live.recording`'s hashes would keep what they break, so these changes
need a new recording instead:

- a change to the demo's layers;
- a change to a command's type, so that its recorded lines no longer read;
- a change to the arena's types, whose recorded edits would then be refused at their ticks.

To make one, start a windowed run of the demo with `--record`, then:

1. Steer and dash with the keys.
2. Set a field with `cargo gg set`, then `undo` and `redo`.
3. Save `scenes/arena.ron` changed, then put it back.
4. Close the window.
