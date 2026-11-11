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
  gg-gpu/         Vulkan through ash, the render graph too: the only crate that touches Vulkan
  gg-renderer/    passes, materials; gets plain render data, never the scene
  gg-engine/      what games use: main loop, window, input, scene, assets, hot reload, the tool API
  gg-macros/      the derives gg-engine re-exports
  gg-api/         the tool API, a game's shared view, projects' manifests, assets' ID files
apps/             executables
  sandbox/        the engine's own app: a swarm of up to 100,000 sprites and a grid's scene file
  editor/         the editor: a project's game, embedded, in a panel among dockable panels
  player/         reference player, with no layers; runs cooked scenes without game code (to come)
games/shards/     the first demo, a project: two players, alone, as peers or as a server's clients
games/foxball/    the 3D demo, networked as Shards is: two foxes, a ball and two goals
games/showcase/   a project of a scene for each feature: add one as a feature lands
tools/            gg/, the command-line tool `cargo gg` runs; cook/, the cooker of games' packages
docs/              PLAN.md, the roadmap, and DEFECTS.md, known defects (see Workflow); LUAU.md, what
                  Luau scripts and mods can use: every `gg` function, the limits, the diagnostics
```

Dependencies point down: apps → `gg-engine` → `gg-renderer` → `gg-gpu` → `gg-core`, and any
crate may use `gg-core`. Cargo catches only some breaches of the layering rules in
ARCHITECTURE.md, and `cargo deny check bans` (from [deny.toml](deny.toml)) catches skip-level
edges and a crate using a library another crate owns, such as `ash`, `winit` or `rapier2d`:
check a change against the rest yourself.

## Where to look

Each topic's one home. A crate's own docs are its root's `//!` comment and its items' doc comments
(`cargo doc --no-deps --open`, or the source).

- What games use, `App`, `Layer`, `Context`, `Ui`, the world, assets and input: [gg-engine's
  root](crates/gg-engine/src/lib.rs); working in it, [its AGENTS.md](crates/gg-engine/AGENTS.md).
- Rendering, each pass and the debug names of the images it makes: [ARCHITECTURE.md's
  Rendering](ARCHITECTURE.md#rendering), [gg-renderer's root](crates/gg-renderer/src/lib.rs), each
  shader's header in [its shaders](crates/gg-renderer/shaders); Vulkan, [gg-gpu's
  root](crates/gg-gpu/src/lib.rs).
- The tool API: `cargo gg help`, each command and what it answers; the protocol, [gg-api's
  root](crates/gg-api/src/lib.rs); how a game serves it, [Agents and
  tools](ARCHITECTURE.md#agents-and-tools) and [the server](crates/gg-engine/src/api/server.rs).
- The flags every app shares: `--help`, from [args.rs](crates/gg-engine/src/args.rs).
- Scripts and mods: [docs/LUAU.md](docs/LUAU.md). Game UI files, `.ui`: [game_ui's
  root](crates/gg-engine/src/game_ui/mod.rs).
- Scene files, prefabs, assets' IDs and projects' manifests: [Project
  state](ARCHITECTURE.md#project-state); a scene for each feature in [the showcase's
  scenes](games/showcase/scenes).
- The simulation: [ECS](ARCHITECTURE.md#ecs), [Networking](ARCHITECTURE.md#networking),
  [Physics](ARCHITECTURE.md#physics), [Animation](ARCHITECTURE.md#animation),
  [Navigation](ARCHITECTURE.md#navigation), [Audio](ARCHITECTURE.md#audio); the demos' rules,
  [Shards'](games/shards/AGENTS.md) and [Foxball's](games/foxball/AGENTS.md) AGENTS.md.
- Diagnostics and their codes: [Diagnostics](ARCHITECTURE.md#diagnostics).
- The editor, each panel, menu and control: [its game layer's root](apps/editor/src/game.rs).
- Tests to imitate: images against references, [gg-renderer's GPU
  tests](crates/gg-renderer/tests/gpu.rs); frames that don't allocate,
  [frames.rs](crates/gg-engine/tests/frames.rs); a run embedded in a tool,
  [embedded.rs](crates/gg-engine/tests/embedded.rs); UI driven by egui's input, [the
  dock's](crates/gg-engine/src/dock/ui.rs).
- The roadmap and the defects kept: [docs/PLAN.md](docs/PLAN.md), [docs/DEFECTS.md](docs/DEFECTS.md).

### Debugging and measuring

- What a run did: its JSON log by `code`, and `engine.perf` as it ends (Commands, below); while it
  runs, `cargo gg logs --follow` and `cargo gg status`, whose `frames` count the game's frames.
- Frame times: the Performance panel, an overlay apps add, the main thread's parts beside the render
  thread's and the GPU's; Tracy's zones (below). In the editor the panel times the editor's frames:
  the title bar's gear has "Show frame rates", the game's beside the editor's, at the view's bottom
  right.
- Frame rate: `--max-fps N`, or the gear's frame rate limit, which the app keeps for its next runs,
  caps a window's frames, and so an editor's game; at 60 something that settles over frames is
  watched as a player sees it, at 2 to 10 frame by frame. `--vsync` caps at the display's rate.
- How a frame is made: `cargo gg capture --show` and `cargo gg probe` (below); RenderDoc (below).
- The simulation: `--record`, `--replay` and `--dump`, and `--sync-test` (below); a session over a
  slow link, `--link`.

## Commands

```sh
cargo build                       # the whole workspace
cargo run -p editor               # its hub, which opens a project; in games/shards, Shards
cargo run -p sandbox -- --help    # the flags every app shares
cargo run -p sandbox -- --headless --frames 60   # does the engine run? 0 means cleanly
cargo run -p sandbox -- --record target/run.recording   # a run's seed, commands and hashes
cargo run -p sandbox -- --replay target/run.recording   # without a GPU: 0 means every hash matched
cargo run -p sandbox --release --features tracy   # with zones for Tracy
cargo gg ship shards              # as it ships, in target/ship/shards; on macOS, Shards.app
cargo run -p cook -- games/shards target/debug/gg-shards.ggpk   # for a build --no-default-features
cargo test -p gg-renderer --test gpu -- picks   # the GPU tests whose names contain picks
cargo gg run sandbox              # supervised, serving the tool API: `cargo gg help`
cargo gg run --peers 2 [-- --rollback]   # in games/shards: a host and joiner, lockstep or predicting
cargo gg new <NAME>               # a new game, games/NAME, a project with an empty start scene
cargo gg fetch                    # in games/showcase: Sponza and Bistro, which git doesn't hold
```

Building needs a C and C++ compiler, for VMA, Luau, zstd and meshoptimizer, and `slangc` for the
shaders: the one `SLANGC` names, else the Vulkan SDK's, else the first on the `PATH`. The build
warns of one that isn't the release [CI's action](.github/actions/slangc/action.yml) pins, whose
SPIR-V can differ. Games' shaders compile with that slangc as development builds run, so it stays
where the build found it; builds without `dev` run those their package holds compiled. On Linux,
gamepads and sound need `pkgconf`, `libudev-dev` and `libasound2-dev`, as Debian names them.

Each run writes a JSON log, a build without `dev` in the user's state directory, and logs its path
as it starts; filter it on `level`, `code` or `vuid`. `RUST_LOG` sets what's logged: in debug builds
`RUST_LOG=gg_gpu=trace` adds gg-gpu's trace events to the default; a level for every target, as
`RUST_LOG=info` has, replaces it. A run that doesn't end in an error logs `engine.perf` as it ends:
its frame times and where they went, and its memory. Compare builds by theirs, from release builds
run headless for as many frames; headless frames draw each tick as it ends, without blending. A
replay that goes another way names the tick and the part of the hash; `--replay <path> --dump
<tick>`, run where it does and where it doesn't, prints each world a field a line, to diff. Peers
whose worlds part log `net.desync`, the joiner `net.desync_field`; `--sync-test` runs each tick
again on a copy, ending where they part (`sync_test.diverged`): both name the field and values.

The `tracy` feature sends the engine's zones to Tracy: run the Tracy viewer of the release the root
Cargo.toml names, and connect it to the running app. To capture a headless run instead, start
`tracy-capture -o target/run.tracy` first and give the run `TRACY_NO_EXIT=1`; then
`tracy-csvexport target/run.tracy` lists its zones' times, and with `-u` each time a zone ran, when,
and on which thread.

RenderDoc captures a frame of a run it starts: `renderdoccmd capture -w -d . -c
target/captures/sandbox target/debug/gg-sandbox --headless --frames 60 --capture 60` captures
frame 60 and logs `Frame captured` with the file's path. On Windows the executable is
`target\debug\gg-sandbox.exe`: renderdoccmd launches nothing from a path with forward slashes
there. renderdoccmd exits 0 whatever the run does, so read the run's log. `renderdoccmd convert -f
<capture> -c xml -o <capture>.xml` writes the capture as XML, every object's name
(`vkSetDebugUtilsObjectNameEXT`) and pass's label (`vkCmdBeginDebugUtilsLabelEXT`) among it.

Development builds (gg-engine's `dev`, the apps' and the games') serve the tool API, apply a scene
file or script a layer loaded (`Context::load_scene_file`, `Scripts::load`) as it's saved, and,
serving the API, reload a game's assets as their files change, logging `Shader reloaded`,
`Model reloaded` and the like for each kind, or warning at a change that fails. Windowed and
embedded runs play sound, which an agent can't hear: each sound or music started logs
`Sound started` or `Music started` at the debug level. Run a game under `cargo gg run`, in the
background: it patches in an edit inside function bodies as it's saved, with Dioxus's `dx` of the
root Cargo.toml's `subsecond` release (`gg` names its install), and restarts it from a snapshot
after any other change, a crash or a hang (without `dx`, at every change); `cargo gg wait` waits for
an edit's code. `cargo gg status`, `capture`, `set`, `undo`, `logs --follow` and the rest answer in
JSON; `cargo gg mcp` serves them over MCP. To see how a frame is made, `cargo gg capture --show
base_color,normal,roughness` captures parts of shading, or an image the frame makes by its debug
name (`--show "view depth"`), in place of the view's light, each pixel's bytes numbers as gg-api's
`Shown` says; the editor's toolbar shows the same. `cargo gg probe X Y` decodes such captures of one
pixel into numbers: its base color, normal, roughness, light, depth, distance and entity. A layer's
`load` makes its assets; its `attach`, which a restart skips, builds the world, so an edit to it
shows only in a run started afresh; a patch or a rebuild that fails, and such an edit, warn in its log (`gg.patch_failed`,
`gg.rebuild_failed`, `gg.attach_changed`). Test `dev` with `--features dev -p gg-engine`.

A change is done when all seven pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features sandbox/tracy -- -D warnings
cargo clippy -p player -p shards -p foxball -p showcase --no-default-features --all-targets -- -D warnings
cargo clippy -p gg-engine --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo test --workspace
```

The tests include GPU tests, rendering offscreen against reference images within a tolerance: they
need a Vulkan 1.4 driver and the Khronos validation layer. Without a GPU, lavapipe serves: on Debian
or Ubuntu, `mesa-vulkan-drivers` (Mesa 25.0 or later) and `vulkan-validationlayers`. When a test's
image differs from its reference, the test says where it wrote its own; after an intended change,
`GG_UPDATE_REFERENCES=1` rewrites the references, which you look at before committing them. The
sandbox's and the demos' world hashes, at each tick of a seeded run, are references too, which CI
checks on every platform: a change to their simulations, to what the hash reads, or to glam, libm,
rapier or Luau changes them, and `GG_UPDATE_REFERENCES=1 cargo test -p sandbox -p shards -p foxball`
rewrites them and the demos' live recordings' alone. gg-engine's `physics/` holds five more, its
`animation/` three and its `navigation/` one, edited by hand. A difference on some platforms alone
is a bug. Some changes need a new recording, as each demo's AGENTS.md says.

CI ([ci.yml](.github/workflows/ci.yml)) runs the gates on every push of more than docs, on Windows
and Linux, x86-64 and ARM64, and on macOS ARM64; the shipping gate, gg-engine's without `dev`, the
doc gate, the GPU tests, on lavapipe, the apps and the demo's peers headless, grepping their logs,
and the demo and the player run as they ship on Linux x86-64 alone; and the layering check. Hosted
runners have no GPU, so the other jobs set `GG_SKIP_GPU_TESTS`. lavapipe has no transfer queue,
so CI never uploads through one: after changing uploads, run the GPU tests on a GPU with one, as
NVIDIA's. Minutes are billed, a macOS one at ten Linux ones: push a branch to probe CI only for
what another OS could break, and run [ship.yml](.github/workflows/ship.yml) only as asked.

## Rust

- Stable Rust, the release `rust-toolchain.toml` pins; edition 2024, resolver 3.
- Every member manifest inherits `version`, `edition`, `license` and `publish` from
  `[workspace.package]`, but a game `gg new` makes, whose license is its own, and has `[lints]
  workspace = true`. Without that line, neither the unsafe denial nor the SAFETY-comment lint
  reaches the crate.
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
- Name and time everything: every Vulkan object gets a debug name and every pass a debug label,
  so captures and Tracy timelines read clearly. Work that takes about 10 µs or more gets a zone
  (`gg_core::zone!`), but no draw, sprite or other item of a loop over a frame's content does.
- Diagnostics are data: log through `tracing` with fields rather than printing, and give every
  error and warning a stable code and the thing it is about, as
  [ARCHITECTURE.md](ARCHITECTURE.md#diagnostics) describes.
- Steady-state frames don't allocate: per-frame data lives in frame arenas and ring buffers.
  [gg-engine's AGENTS.md](crates/gg-engine/AGENTS.md) says how it's checked.

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
- This file loads into every agent session; `CLAUDE.md` only imports it, for Claude Code. A rule
  for one crate goes in an `AGENTS.md` inside that crate, beside a `CLAUDE.md` that imports it, so
  it loads only when an agent works there.
- No session logs, ADR files or agent notes in the repo. A lesson that holds only on one
  machine goes in the agent's own memory.
