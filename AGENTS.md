# GGEngine

An agent-native, bleeding-edge, high-performance, high-quality, cross-platform game engine in
Rust on Vulkan 1.4, built by people and agents together. The goal for v0.1 is changing a
running game by asking: the user tells a coding agent what to change, and the game picks up
each change with its state intact.

This file is what an agent needs for any task here. The design, and the reasons behind it, are
in [ARCHITECTURE.md](ARCHITECTURE.md): read it before moving code between crates, adding a
crate, or adding or removing a dependency.

## Where things are

```text
Cargo.toml        the workspace: members, shared package settings, lints, internal crates
crates/           libraries
  gg-core/        math, logging, profiling, errors
  gg-gpu/         Vulkan through ash: the only crate that touches Vulkan
  gg-renderer/    passes, materials, render graph (to come); gets plain render data, never the scene
  gg-engine/      what games use: main loop, window, input, scene, image and glTF loading,
                  hot reload, the tool API
  gg-macros/      the derives gg-engine re-exports
  gg-api/         the tool API's protocol, which gg-engine and tools share
apps/             executables
  sandbox/        development playground
  editor/         the editor: the game's view in a panel among dockable panels
  player/         reference player, with no layers; runs cooked scenes without game code (to come)
tools/gg/         the command-line tool, which `cargo gg` runs
docs/PLAN.md      the roadmap (see Workflow)
docs/DEFECTS.md   known defects of the code that exists (see Workflow)
```

Dependencies point down: apps → `gg-engine` → `gg-renderer` → `gg-gpu` → `gg-core`, and any
crate may use `gg-core`. Cargo catches only some breaches of the layering rules in
ARCHITECTURE.md, and `cargo deny check bans` (from [deny.toml](deny.toml)) catches skip-level
edges and a crate using a library another crate owns, such as `ash`, `winit` or `rapier2d`:
check a change against the rest yourself.

## Commands

```sh
cargo build                       # the whole workspace
cargo run -p sandbox
cargo run -p editor
cargo run -p sandbox -- --help    # the flags every app shares
cargo run -p sandbox -- --headless --frames 60   # does the engine run? 0 means cleanly
cargo run -p sandbox -- --record target/run.recording   # a run's seed, commands and hashes
cargo run -p sandbox -- --replay target/run.recording   # without a GPU: 0 means every hash matched
cargo run -p sandbox --release --features tracy   # with zones for Tracy
cargo build -p player --release   # shipping: the player alone, as wider builds merge in dev
cargo test -p gg-renderer --test gpu -- picks   # the GPU tests whose names contain picks
cargo gg run sandbox              # supervised, serving the tool API: `cargo gg help`
```

Building needs a C++ compiler, for the Vulkan Memory Allocator, and `slangc` for the shaders: the
one `SLANGC` names, else the Vulkan SDK's, else the first on the `PATH`. Games' shaders compile
with that slangc as they run, so it stays where the build found it. On Linux, gamepads need
pkg-config and libudev's headers too: `pkgconf` and `libudev-dev` on Debian or Ubuntu.

Each run writes a JSON log and logs its path as it starts; filter it on `level`, `code` or
`vuid`. `RUST_LOG` sets what's logged: `RUST_LOG=gg_gpu=trace` adds gg-gpu's trace events to the
default; a level for every target, as `RUST_LOG=info` has, replaces it. A run that doesn't end in
an error logs `engine.perf` as it ends: its frame times and where they went. Compare two builds by
theirs, from release builds run headless for the same number of frames. Headless frames draw each
tick as it ends, so theirs leave out blending between ticks, which windowed runs measure.

A replay that goes another way names the tick and the part of the hash; `--replay <path> --dump
<tick>`, run where it goes another way and where it doesn't, prints each world's content there a
field a line, and the two diff to the field where they part. Where the layout alone goes another
way, they're the same.

The `tracy` feature sends the engine's zones to Tracy: run the Tracy viewer of the release the
root Cargo.toml names, and connect it to the running app. To capture a headless run instead,
start `tracy-capture -o target/run.tracy` first and give the run `TRACY_NO_EXIT=1`; then
`tracy-csvexport target/run.tracy` lists its zones' times, and with `-u` each time a zone ran, when,
and on which thread, such as the render thread's `render_frame` beside the main thread's ticks.

RenderDoc captures a frame of a run it starts: `renderdoccmd capture -w -d . -c
target/captures/sandbox target/debug/gg-sandbox --headless --frames 60 --capture 60` captures
frame 60 and logs `Frame captured` with the file's path. On Windows the executable is
`target\debug\gg-sandbox.exe`: renderdoccmd launches nothing from a path with forward slashes
there. renderdoccmd exits 0 whatever the run does, so read the run's log. `renderdoccmd convert -f
<capture> -c xml -o <capture>.xml` writes the capture as XML, every object's name
(`vkSetDebugUtilsObjectNameEXT`) and pass's label (`vkCmdBeginDebugUtilsLabelEXT`) among it.

Development builds, with gg-engine's `dev` feature, as the sandbox and the editor are, serve the
tool API, and windowed, compile a game's shader again as its files change, logging `Shader
reloaded`, or warning and drawing on as it did at a change that doesn't compile or breaks the shader
contract. Run a game under `cargo gg run` in the background and talk to it beside it: `cargo gg
status`, `capture` (its view as a PNG, in place of a screenshot), `step` and the rest each answer a
line of JSON. `dev`'s tests need `--features dev` with `-p gg-engine`; `--workspace` merges it in.

A change is done when all five pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features sandbox/tracy -- -D warnings
cargo clippy -p player --all-targets -- -D warnings
cargo test --workspace
```

The tests include GPU tests, which need a Vulkan 1.4 driver and the Khronos validation layer.
Without a GPU, lavapipe serves: on Debian or Ubuntu, `mesa-vulkan-drivers` (Mesa 25.0 or later)
and `vulkan-validationlayers`. When a test's image differs from its reference, the test says
where it wrote its own; after an intended change, `GG_UPDATE_REFERENCES=1` rewrites the
references, which you look at before committing them. The sandbox's world hashes, at each tick of
a seeded run, are a reference too, which CI checks on every platform: a change to its simulation,
to what the hash reads, or to glam or libm changes them, and `GG_UPDATE_REFERENCES=1 cargo test -p
sandbox` rewrites them alone, with those of its live recording,
`apps/sandbox/tests/references/live.recording`. gg-engine's `physics.rs` holds another, the hash
of a pile of bodies, which a change to rapier2d, to the physics code or to what the hash reads
changes, and which is edited by hand. A difference on some platforms only is a bug, not
a reference to rewrite. Some changes need a new recording, since rewriting the hashes would keep
what they break: a change to the sandbox's layers, to a command's type that its lines no longer
read as, or to what its commands name, the grid scene's types and the field the Inspector sets,
whose reload and set would then be refused at their ticks. Make one with a windowed run and
`--record` that holds the arrows and taps R, uses the panels, File > Reload the grid and the
Inspector, undoing and redoing an edit, and closes; not File > Save, since the attach reads the
grid's scene.

CI ([ci.yml](.github/workflows/ci.yml)) runs the gates on every push, on Windows and Linux, x86-64
and ARM64, and on macOS ARM64, the shipping gate on Linux x86-64 alone, and the layering check.
Only its Linux jobs run the GPU tests, on lavapipe, and the apps headless, as its steps say,
grepping the sandbox's log for `engine.perf`'s fields; Windows' and macOS's runners have no GPU, so
there `GG_SKIP_GPU_TESTS` skips the GPU tests. lavapipe has no queue for transfers alone, so CI
never uploads through one: after changing uploads, run the GPU tests on a GPU that has one, as
NVIDIA's do. Its minutes are billed, a macOS minute at ten times a Linux one, so push a branch to
probe CI only for a change another platform could break.

## Rust

- Stable Rust, the release `rust-toolchain.toml` pins; edition 2024, resolver 3.
- Every member manifest inherits `version`, `edition` and `publish` from `[workspace.package]`
  and has `[lints] workspace = true`. Without that line, neither the unsafe denial nor the
  SAFETY-comment lint reaches the crate.
- Dependencies, internal and third-party, are declared once in the root
  `[workspace.dependencies]`; members write `name.workspace = true`.
- `unsafe_code` is denied. A crate that needs unsafe opts in at its root with a comment giving
  the reason: the whole crate with `#![allow(unsafe_code)]`, as `gg-gpu` does, or one module
  with `#[allow(unsafe_code)]` on its `mod` line, as `gg-engine` does for Windows' window frame.
  Integration tests (`tests/*.rs`) are separate crates and opt in on their own. Every unsafe
  block has a `// SAFETY:` comment (`clippy::undocumented_unsafe_blocks`), and unsafe stays
  behind safe APIs: code that calls `gg-gpu` never needs it.
- Each crate root opens with a `//!` doc saying what the crate owns.
- Anything that can fail at runtime (device loss, out of memory, missing or malformed assets)
  returns an error. Panics are for broken invariants.

## Engine rules

- Validation errors are test failures: debug builds and tests run under the Khronos validation
  layer with synchronization validation, and a test fails on any validation message. GPU tests
  fail without synchronization validation too, which needs a layer that takes
  `VK_EXT_layer_settings`.
- GPU tests run headless: they render offscreen and compare against reference images within a
  tolerance, so lavapipe can run them without a GPU.
- Name and time everything: every Vulkan object gets a debug name and every pass a debug label,
  so captures and Tracy timelines read clearly. Work that takes about 10 µs or more gets a zone
  (`gg_core::zone!`), and nothing gets one for each draw, sprite or other item of a loop over a
  frame's content.
- Diagnostics are data: log through `tracing` with fields rather than printing, and give every
  error and warning a stable code and the thing it is about, as
  [ARCHITECTURE.md](ARCHITECTURE.md#diagnostics) describes.
- Steady-state frames don't allocate: per-frame data lives in frame arenas and ring buffers.
  gg-engine's `ticks` test checks ticks on every thread, entities coming and going, but no physics,
  which allocates as contacts come and go; its GPU test `frames`, headless frames of an app without
  UI, as egui allocates. After changing the windowed loop, run `cargo test -p gg-engine --test
  frames -- --windowed` without `dev`, `tracy` or a gamepad, with the mouse over its window: gilrs
  allocates as it reads a pad, winit for each key on Windows, and Tracy's client as it records.

## Workflow

- Commit only when the user asks. A milestone's commit message says what was done, what was
  deferred and how it was verified: git history is the work log.
- `docs/PLAN.md` holds the current milestone, the phases after it, deferred work and open
  questions; read it before roadmap work, and update it in the same commit as the work it
  tracks. When a milestone is done, replace its section with a line under Done, the title its
  commit's subject begins with, or a phase and its name, and the date, and move what was skipped to
  Deferred. A defect left in goes in `docs/DEFECTS.md`, by area: what it costs, the fix, and when
  the fix would be worth it.
- Open questions are the user's to decide: bring a recommendation instead of settling one
  yourself. Once one is decided, the answer goes in ARCHITECTURE.md and the question leaves
  docs/PLAN.md.

## Keeping the docs lean

- Each fact has one home: this file for how to work here, ARCHITECTURE.md for the design,
  docs/PLAN.md for the work, docs/DEFECTS.md for the defects kept. Link instead of copying.
- They state current decisions, not their history. When something changes, rewrite or delete
  the old text instead of adding a note below it. Git keeps the history, so a finished
  milestone's details go in its commit message.
- Docs record decisions, not the research behind them: no feature matrices, version surveys or
  source lists. A source that justifies some code goes in a comment beside that code.
- This file loads into every agent session, so keep it under 200 lines, the size Anthropic
  recommends. `CLAUDE.md` only imports it, for Claude Code. A rule for one crate goes in an
  `AGENTS.md` inside that crate, beside a `CLAUDE.md` that imports it, so it loads only when an
  agent works there.
- No session logs, ADR files or agent notes in the repo. A lesson that holds only on one
  machine goes in the agent's own memory.
