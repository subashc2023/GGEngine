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

## Profiling on a Mac

KosmicKrisp's timestamps are good at render passes' edges alone, and a pass's fragment work lands
after its end, so the engine's own pass times blame the wrong passes. Metal System Trace times
each Metal encoder instead:

```sh
xcrun xctrace record --template 'Metal System Trace' --output target/run.trace \
  --env DYLD_FALLBACK_LIBRARY_PATH=/usr/local/lib --launch -- "$PWD/target/release/gg-showcase" \
  --package <package> --headless --size 3024x1890 --frames 200
xcrun xctrace export --input target/run.trace \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]'
```

- Encoders carry no pass names: match them to the graph's passes in order.
- KosmicKrisp runs a stretch of compute passes in one encoder. Split one by dispatching a pass
  twice; skipping a pass leaves later ones reading garbage, which changes their cost.
- The `graphics-compiler-spill-events` table gives each encoder's register spills, in bytes a
  thread.
- An M3 slows by a fifth over a long run: compare builds by turns, over the same frames.

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
