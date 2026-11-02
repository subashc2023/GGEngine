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
  gg-engine/      what games use: main loop, window, input, scene, assets, hot reload, the tool API
  gg-macros/      the derives gg-engine re-exports
  gg-api/         the tool API, a game's shared view, projects' manifests, assets' ID files
apps/             executables
  sandbox/        the engine's own app: a swarm of up to 100,000 sprites and a grid's scene file
  editor/         the editor: a project's game, embedded, in a panel among dockable panels
  player/         reference player, with no layers; runs cooked scenes without game code (to come)
games/shards/     the first demo, a project: two players, alone or in lockstep (`--host`, `--join`)
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
cargo run -p editor               # in games/shards, the editor on Shards, its game embedded
cargo run -p sandbox -- --help    # the flags every app shares
cargo run -p sandbox -- --headless --frames 60   # does the engine run? 0 means cleanly
cargo run -p sandbox -- --record target/run.recording   # a run's seed, commands and hashes
cargo run -p sandbox -- --replay target/run.recording   # without a GPU: 0 means every hash matched
cargo run -p sandbox --release --features tracy   # with zones for Tracy
cargo build -p player --release   # shipping: the player alone, as wider builds merge in dev
cargo test -p gg-renderer --test gpu -- picks   # the GPU tests whose names contain picks
cargo gg run sandbox              # supervised, serving the tool API: `cargo gg help`
cargo gg run --peers 2            # in games/shards, its project: a lockstep host and joiner
cargo gg new <NAME>               # a new game, games/NAME, a project with an empty start scene
```

Building needs a C++ compiler, for the Vulkan Memory Allocator, and `slangc` for the shaders: the
one `SLANGC` names, else the Vulkan SDK's, else the first on the `PATH`. Games' shaders compile
with that slangc as they run, so it stays where the build found it. On Linux, gamepads and sound
need pkg-config, libudev's headers and ALSA's too: `pkgconf`, `libudev-dev` and `libasound2-dev` on
Debian or Ubuntu.

Each run writes a JSON log and logs its path as it starts; filter it on `level`, `code` or
`vuid`. `RUST_LOG` sets what's logged: `RUST_LOG=gg_gpu=trace` adds gg-gpu's trace events to the
default; a level for every target, as `RUST_LOG=info` has, replaces it. A run that doesn't end in
an error logs `engine.perf` as it ends: its frame times and where they went. Compare two builds by
theirs, from release builds run headless for the same number of frames. Headless frames draw each
tick as it ends, so theirs leave out blending between ticks, which windowed runs measure.

A replay that goes another way names the tick and the part of the hash; `--replay <path> --dump
<tick>`, run where it goes another way and where it doesn't, prints each world's content there a
field a line, and the two diff to the field where they part, or match where the layout alone parts.

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

Development builds (gg-engine's `dev`, the apps' and the games') serve the tool API, apply a
scene file a layer loaded with `Context::load_scene_file` as it's saved, and, serving the API,
reload a game's shaders, textures, models, fonts, sounds and music as their files change (`Shader
reloaded`, `Texture reloaded`, `Model reloaded`, `Font reloaded`, `Sound reloaded`, `Music
reloaded`), warning at a change that fails. Windowed and embedded runs play sound, which an agent
can't hear: each sound or music started logs `Sound started` or `Music started` at the debug level.
Run a game under `cargo gg run`, in the background: it patches in an edit inside function bodies
as it's saved, with Dioxus's `dx` of the root Cargo.toml's `subsecond` release (`gg` names its
install), and restarts it from a snapshot after any other change, a crash or a hang (without `dx`,
after a `cargo build`). `cargo gg status`, `capture` (a PNG), `set`, `undo`, `logs --follow` and
the rest answer in JSON; `cargo gg mcp` serves them over MCP. A layer's `load` makes its assets;
its `attach`, which a restart skips, builds the world. Test `dev` with `--features dev -p
gg-engine`.

A change is done when all seven pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features sandbox/tracy -- -D warnings
cargo clippy -p player --all-targets -- -D warnings
cargo clippy -p gg-engine --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo test --workspace
```

The tests include GPU tests, which need a Vulkan 1.4 driver and the Khronos validation layer.
Without a GPU, lavapipe serves: on Debian or Ubuntu, `mesa-vulkan-drivers` (Mesa 25.0 or later) and
`vulkan-validationlayers`. When a test's image differs from its reference, the test says where it
wrote its own; after an intended change, `GG_UPDATE_REFERENCES=1` rewrites the references, which you
look at before committing them. The sandbox's and the demo's world hashes, at each tick of a seeded
run, are references too, which CI checks on every platform: a change to their simulations, to what
the hash reads, or to glam, libm or rapier2d changes them, and `GG_UPDATE_REFERENCES=1 cargo test -p
sandbox -p shards` rewrites them alone, with those of the demo's live recording,
`games/shards/tests/references/live.recording`. gg-engine's `physics.rs` holds another, edited by
hand. A difference on some platforms only is a bug, not a reference to rewrite. Some changes need a
new recording, since rewriting the hashes would keep what they break: a change to the demo's layers,
to a command's type that its lines no longer read as, or to the arena's types, whose edits would
then be refused at their ticks. Make one with a windowed run of the demo and `--record` that steers
and dashes with the keys, sets a field through `cargo gg set`, then `undo` and `redo`, saves
`scenes/arena.ron` changed and puts it back, and closes.

CI ([ci.yml](.github/workflows/ci.yml)) runs the gates on every push, on Windows and Linux, x86-64
and ARM64, and on macOS ARM64, the shipping gate, gg-engine's without `dev` and the doc gate on
Linux x86-64 alone, and the layering check. Only its Linux jobs run the GPU tests, on lavapipe,
and the apps and the demo's two lockstep peers headless, grepping their logs; Windows' and macOS's
runners have no GPU, so there `GG_SKIP_GPU_TESTS` skips the GPU tests. lavapipe has no queue for
transfers alone, so CI never uploads through one: after changing uploads, run the GPU tests on a GPU
that has one, as NVIDIA's do. Its minutes are billed, a macOS minute at ten times a Linux one, so
push a branch to probe CI only for a change another platform could break.

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
  layer with synchronization validation, and a test fails on any validation message. GPU tests fail
  without synchronization validation too, which needs a layer that takes `VK_EXT_layer_settings`.
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
- Open questions are the user's to decide: bring a recommendation instead of settling one yourself.
  Once one is decided, the answer goes in ARCHITECTURE.md and the question leaves docs/PLAN.md.

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
