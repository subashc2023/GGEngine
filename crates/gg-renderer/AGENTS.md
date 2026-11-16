# gg-renderer

Rules for work on rendering, beside the workspace's [AGENTS.md](../../AGENTS.md).

## Profiling with Tracy

The `tracy` feature sends the engine's zones to Tracy (`cargo run -p sandbox --release --features
tracy`). Run the Tracy viewer of the release the root Cargo.toml names, and connect it to the app.

To capture a headless run instead:

1. Start `tracy-capture -o target/run.tracy`.
2. Run the app with `TRACY_NO_EXIT=1`.
3. `tracy-csvexport target/run.tracy` lists the zones' times. With `-u`, it lists each time a zone
   ran, when, and on which thread.

## Capturing a frame with RenderDoc

```sh
renderdoccmd capture -w -d . -c target/captures/sandbox target/debug/gg-sandbox --headless --frames 60 --capture 60
renderdoccmd convert -f <capture> -c xml -o <capture>.xml
```

- The first captures frame 60 and logs `Frame captured` with the file's path.
- On Windows, give the executable as `target\debug\gg-sandbox.exe`: renderdoccmd launches nothing
  from a path with forward slashes there.
- renderdoccmd exits 0 whatever the run does, so read the run's log.
- The XML holds every object's name (`vkSetDebugUtilsObjectNameEXT`) and every pass's label
  (`vkCmdBeginDebugUtilsLabelEXT`).
