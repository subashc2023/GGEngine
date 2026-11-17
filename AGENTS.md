# GGEngine

An agent-native, cross-platform game engine in Rust on Vulkan 1.4, built by people and agents
together. The v0.1 goal: change a running game by asking. The user tells a coding agent what to
change, and the game picks up each edit with its state intact.

This file says how to work here. The design and its reasons are in
[ARCHITECTURE.md](ARCHITECTURE.md): read it before moving code between crates, adding a crate, or
adding or removing a dependency.

## Where things are

```text
crates/
  gg-core/        math, logging, profiling, errors
  gg-gpu/         Vulkan through ash, and the render graph: the only crate that touches Vulkan
  gg-renderer/    passes and materials; gets plain render data, never the scene
  gg-engine/      what games use: main loop, window, input, scene, assets, hot reload, tool API
  gg-macros/      the derives gg-engine re-exports
  gg-api/         the tool API's protocol, a game's shared view, manifests, asset ID files
  gg-relay/       the relay that sessions behind routers meet through
apps/             sandbox (up to 100,000 sprites), editor, player (a package's scenes and scripts,
                  no game code), relay (the relay's executable)
games/            shards (2D, networked), showcase (a scene per feature)
tools/            gg/ (`cargo gg`), cook/ (the cooker of games' packages)
docs/             PLAN.md (roadmap), DEFECTS.md (known defects), LUAU.md (the Luau API)
```

Dependencies point down: apps → gg-engine → gg-renderer → gg-gpu → gg-core, and any crate may use
gg-core. `cargo deny check bans` ([deny.toml](deny.toml)) catches skip-level edges and a crate using
a library another crate owns (`ash`, `winit`, `rapier2d`…). It can't catch every layering rule in
ARCHITECTURE.md: check the rest yourself.

## Where to look

A crate's docs are its root `//!` and its items' docs (`cargo doc --no-deps --open`, or the source).

- What games use (`App`, `Layer`, `Context`, `Ui`, the world, assets, input): [gg-engine's
  root](crates/gg-engine/src/lib.rs) and its `prelude`. Working in it: [its
  AGENTS.md](crates/gg-engine/AGENTS.md).
- Rendering: [ARCHITECTURE.md's Rendering](ARCHITECTURE.md#rendering), [gg-renderer's
  root](crates/gg-renderer/src/lib.rs), and each shader's header in [its
  shaders](crates/gg-renderer/shaders). Vulkan: [gg-gpu's root](crates/gg-gpu/src/lib.rs).
  Profiling and GPU captures: [gg-renderer's AGENTS.md](crates/gg-renderer/AGENTS.md).
- The tool API: `cargo gg help`. The protocol: [gg-api's root](crates/gg-api/src/lib.rs). How a game
  serves it: [Agents and tools](ARCHITECTURE.md#agents-and-tools) and [the
  server](crates/gg-engine/src/api/server.rs).
- The flags every app shares: `--help`, from [args.rs](crates/gg-engine/src/args.rs).
- Scripts and mods: [docs/LUAU.md](docs/LUAU.md). Game UI files (`.ui`): [game_ui's
  root](crates/gg-engine/src/game_ui/mod.rs).
- Scene files, prefabs, asset IDs and manifests: [Project state](ARCHITECTURE.md#project-state). A
  scene per feature: [the showcase's scenes](games/showcase/scenes).
- The simulation: [ECS](ARCHITECTURE.md#ecs), [Networking](ARCHITECTURE.md#networking),
  [Physics](ARCHITECTURE.md#physics), [Animation](ARCHITECTURE.md#animation),
  [Navigation](ARCHITECTURE.md#navigation), [Audio](ARCHITECTURE.md#audio). The demo's rules:
  [Shards' AGENTS.md](games/shards/AGENTS.md).
- Diagnostics and their codes: [Diagnostics](ARCHITECTURE.md#diagnostics).
- A game of its own, outside the workspace, and the installed `gg`: [Game
  code](ARCHITECTURE.md#game-code). The AGENTS.md it gets: [gg new's template](tools/gg/src/new.rs).
- The editor's panels, menus and controls: [its game layer's root](apps/editor/src/game.rs).
- Tests to imitate: images against references, [gg-renderer's GPU
  tests](crates/gg-renderer/tests/gpu.rs); frames that don't allocate,
  [frames.rs](crates/gg-engine/tests/frames.rs); a run embedded in a tool,
  [embedded.rs](crates/gg-engine/tests/embedded.rs); UI driven by egui's input, [the
  dock's](crates/gg-engine/src/dock/ui.rs).
- The roadmap and defects kept: [docs/PLAN.md](docs/PLAN.md), [docs/DEFECTS.md](docs/DEFECTS.md).

## Commands

```sh
cargo run -p editor                              # the hub opens a project; in games/shards, Shards
cargo run -p sandbox -- --help                   # the flags every app shares
cargo run -p sandbox -- --headless --frames 60   # does the engine run? 0 means cleanly
cargo run -p sandbox -- --record target/run.recording   # a run's seed, commands and hashes
cargo run -p sandbox -- --replay target/run.recording   # no GPU needed: 0 means every hash matched
cargo run -p sandbox --release --features tracy  # with Tracy zones
cargo test -p gg-renderer --test gpu -- picks    # the GPU tests whose names contain "picks"
cargo gg help                                    # every command; `cargo gg help <command>` for one
cargo gg run sandbox                             # supervised, serving the tool API
cargo gg run --peers 2 [-- --rollback]           # in games/shards: a host and a joiner
cargo gg new <NAME>                              # a new game in games/NAME: a square keys move
cargo install --locked --path tools/gg           # `gg`, for games of their own
gg new <NAME> --engine <ENGINE>                  # outside: ./NAME; ENGINE a checkout or URL#REV
cargo run -p cook -- games/shards target/debug/gg-shards.ggpk   # for a build --no-default-features
```

Requirements are in [README.md](README.md#requirements). The build warns when `slangc` isn't the
release [CI's action](.github/actions/slangc/action.yml) pins, whose SPIR-V can differ. Development
builds compile games' shaders with it as they run; builds without `dev` use the package's.

### Working on a running game

Run the game under `cargo gg run`, in the background. It serves the tool API, patches in edits
inside function bodies as they're saved, and restarts from a snapshot after other changes, crashes
or hangs. A layer's `attach` builds the world, and restarts skip it: an edit there shows only in a
fresh run. `cargo gg help run` has the details. `cargo gg mcp` serves the same commands over MCP.

### Debugging and measuring

- Each run writes a JSON log and logs its path as it starts; filter on `level`, `code` or `vuid`.
  While it runs: `cargo gg logs --follow` and `cargo gg status`. `--help` covers `RUST_LOG`.
- A run that doesn't end in an error logs `engine.perf`: frame times, where they went, and memory.
  Compare release builds run headless for the same number of frames.
- Live frame times: the Performance panel; in the editor, the game's first. In the editor, the
  title bar's gear has "Show frame rates", and its limit caps the game (the look's `max_fps`).
  `--max-fps N` or `--vsync` caps frames; at 2 to 10 fps you can watch frame by frame.
- How a frame is made: `cargo gg capture --show` and `cargo gg probe`. Tracy and RenderDoc:
  [gg-renderer's AGENTS.md](crates/gg-renderer/AGENTS.md).
- The simulation: `--record`, `--replay`, `--dump` and `--sync-test`; a slow link, `--link`.
- Sound: an agent can't hear it, but each sound or music started logs `Sound started` or
  `Music started` at the debug level.

## Gates

A change is done when all seven pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features sandbox/tracy -- -D warnings
cargo clippy -p player -p shards -p showcase --no-default-features --all-targets -- -D warnings
cargo clippy -p gg-engine --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo test --workspace
```

GPU tests render offscreen against reference images, within a tolerance. They need a Vulkan 1.4
driver and the Khronos validation layer. Without a GPU, lavapipe serves (Debian or Ubuntu:
`mesa-vulkan-drivers` from Mesa 25.0, and `vulkan-validationlayers`). A test whose image differs
says where it wrote its own. After an intended change, `GG_UPDATE_REFERENCES=1` rewrites the
references: look at them before committing.

World hashes are references too: the sandbox's, the demo's and the showcase's crossing's, at each
tick of a seeded run, checked on every platform. Changing their simulations, what the hash reads, or
glam, libm, rapier or Luau changes them. `GG_UPDATE_REFERENCES=1 cargo test -p sandbox -p shards -p
showcase` rewrites them and the demo's live recording's alone. gg-engine's `physics/` (five),
`animation/` (three) and `navigation/` (one) hold more, edited by hand. A difference on some
platforms alone is a bug. Some changes need a new live recording, as the demo's AGENTS.md says.

CI ([ci.yml](.github/workflows/ci.yml)) runs the tests for every push of more than docs, on Windows
and Linux (x86-64 and ARM64) and macOS (ARM64), and the plain clippy on Windows x86-64. The other
gates, the GPU tests on lavapipe, headless runs of the apps and the demo's peers, the demo and
player as they ship, the games `gg new` makes inside the workspace and out, and the layering and
license checks run on Linux x86-64 alone; the other jobs set `GG_SKIP_GPU_TESTS`. lavapipe has no
transfer queue: after changing uploads, run the GPU tests on a GPU with one, such as NVIDIA's.
Minutes are billed, macOS at ten times Linux: push a branch to probe CI only for what another OS
could break, and run [ship.yml](.github/workflows/ship.yml) only when asked.

## Rust

- Stable Rust, the release `rust-toolchain.toml` pins; edition 2024, resolver 3.
- Every member manifest inherits `version`, `edition`, `license` and `publish` from
  `[workspace.package]` (but a game `gg new` makes has its own license) and has
  `[lints] workspace = true`. Without that line, neither the unsafe denial nor the SAFETY lint
  reaches the crate.
- Dependencies, internal and third-party, are declared once in the root `[workspace.dependencies]`;
  members write `name.workspace = true`.
- `unsafe_code` is denied. A crate that needs it opts in at its root, with the reason: for the whole
  crate, `#![allow(unsafe_code)]` (gg-gpu); for one module, `#[allow(unsafe_code)]` on its `mod`
  line (gg-engine's Windows window frame). Integration tests opt in on their own. Every unsafe block
  has a `// SAFETY:` comment, and unsafe stays behind safe APIs: code that calls gg-gpu never needs
  it.
- Each crate root opens with a `//!` doc saying what the crate owns.
- Anything that can fail at runtime (device loss, out of memory, missing or malformed assets)
  returns an error. Panics are for broken invariants.
- Test `dev` with `--features dev -p gg-engine`.

## Engine rules

- Validation errors are test failures. Debug builds and tests run under the Khronos validation layer
  with synchronization validation, and any validation message fails a test. GPU tests also fail
  without synchronization validation, which needs a layer that takes `VK_EXT_layer_settings`.
- Name and time everything. Every Vulkan object gets a debug name and every pass a debug label. Work
  of about 10 µs or more gets a zone (`gg_core::zone!`), but no draw, sprite or other item of a loop
  over a frame's content does.
- Diagnostics are data: log through `tracing` with fields, never print. Every error and warning has
  a stable code and names what it's about ([Diagnostics](ARCHITECTURE.md#diagnostics)).
- Steady-state frames don't allocate: per-frame data lives in frame arenas and ring buffers.
  [gg-engine's AGENTS.md](crates/gg-engine/AGENTS.md) says how it's checked.

## Workflow

- Commit only when the user asks. A milestone's commit message says what was done, what was deferred
  and how it was verified: git history is the work log.
- `docs/PLAN.md` holds the open work, deferred work and open questions. Read it before roadmap work,
  and update it in the same commit as the work it tracks. When a milestone is done, fold it into its
  phase's line under Done, with the date, and move what was skipped to Deferred.
- A defect left in goes in `docs/DEFECTS.md`, by area: what it costs, the fix, and when the fix
  would be worth it.
- Open questions are the user's to decide: bring a recommendation instead of settling one yourself.
  Once one is decided, the answer goes in ARCHITECTURE.md and the question leaves docs/PLAN.md.

## Keeping the docs lean

- Each fact has one home: README.md introduces the engine, this file says how to work here,
  ARCHITECTURE.md holds the design, code docs the reference, docs/PLAN.md the work and
  docs/DEFECTS.md the defects kept. Link instead of copying.
- Docs state current decisions, not history or research. Rewrite or delete old text instead of
  adding a note below it; no feature matrices, version surveys or source lists. A finished
  milestone's details go in its commit message, and a source that justifies code in a comment
  beside it.
- This file loads into every agent session; `CLAUDE.md` only imports it. A rule for one crate goes
  in that crate's `AGENTS.md`, beside a `CLAUDE.md` that imports it.
- No session logs, ADR files or agent notes in the repo. A lesson that holds only on one machine
  goes in the agent's own memory.

### Writing style, for docs and comments

- One idea per sentence, about 25 words at most, subject and verb first. Lists and tables for
  enumerations.
- Say only what code can't: intent, invariants, units and sentinels, external quirks, the why and
  the why not.
- Link to a fact's home instead of re-explaining it.
- A doc opens with one short sentence; clippy's `too_long_first_doc_paragraph` checks public ones.
- `# Panics` and `# Errors` are bullet lists of conditions.
- Private items and fields get docs only for an invariant or a why.
- Never cut SAFETY notes, invariants, drop order, diagnostic codes, wire formats or determinism
  notes.
