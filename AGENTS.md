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
  gg-core/        math, handles, logging, timing, errors
  gg-gpu/         Vulkan through ash: the only crate that touches Vulkan
  gg-renderer/    render graph, passes, materials; gets plain render data, never the scene
  gg-engine/      main loop, window, input, scene, assets, hot reload, tool API: what games use
apps/             executables
  sandbox/        development playground
  player/         reference player: runs cooked scenes without game code
docs/PLAN.md      the roadmap (see Workflow)
```

Dependencies point down: apps → `gg-engine` → `gg-renderer` → `gg-gpu` → `gg-core`, and any
crate may use `gg-core`. Cargo catches only some breaches of the layering rules in
ARCHITECTURE.md, and `cargo deny check bans` (from [deny.toml](deny.toml)) catches skip-level
edges and the wrong crate using `ash` or `winit`: check a change against the rest yourself.

## Commands

```sh
cargo build                       # the whole workspace
cargo run -p sandbox
cargo run -p sandbox -- --help    # the flags every app shares
cargo run -p sandbox -- --headless --frames 60   # does the engine run? 0 means cleanly
cargo build -p player --release   # shipping: the player alone
```

Each run writes a JSON log and logs its path as it starts; filter it on `level`, `code` or
`vuid`. `RUST_LOG` sets what's logged, such as `RUST_LOG=gg_gpu=trace`.

Build and check shipping configurations with `-p player` alone, since a wider build merges the
sandbox's features into the player ([Builds](ARCHITECTURE.md#builds)).

A change is done when all three pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The tests include GPU tests, which need a Vulkan 1.4 driver and the Khronos validation layer.
Without a GPU, lavapipe serves: on Debian or Ubuntu, `mesa-vulkan-drivers` (Mesa 25.0 or later)
and `vulkan-validationlayers`.

CI ([ci.yml](.github/workflows/ci.yml)) runs the three gates on every push, on Windows, Linux
and macOS, x86-64 and ARM64, and the layering check. Its Linux jobs run the GPU tests on
lavapipe; Windows' and macOS's runners have no GPU, so there `GG_SKIP_GPU_TESTS` skips them.

## Rust

- Stable Rust, the release `rust-toolchain.toml` pins; edition 2024, resolver 3.
- Every member manifest inherits `version`, `edition` and `publish` from `[workspace.package]`
  and has `[lints] workspace = true`. Without that line, neither the unsafe denial nor the
  SAFETY-comment lint reaches the crate.
- Dependencies, internal and third-party, are declared once in the root
  `[workspace.dependencies]`; members write `name.workspace = true`.
- `unsafe_code` is denied. A crate that needs unsafe opts in at its root with
  `#![allow(unsafe_code)]` and a comment giving the reason; only `gg-gpu` does today.
  Integration tests (`tests/*.rs`) are separate crates and opt in on their own. Every unsafe
  block has a `// SAFETY:` comment (`clippy::undocumented_unsafe_blocks`), and unsafe stays
  behind safe APIs: code that calls `gg-gpu` never needs it.
- Each crate root opens with a `//!` doc saying what the crate owns.
- Anything that can fail at runtime (device loss, out of memory, missing or malformed assets)
  returns an error. Panics are for broken invariants.

## Engine rules

- Validation errors are test failures: debug builds and tests run under the Khronos validation
  layer with synchronization validation, and a test fails on any validation message.
- GPU tests run headless: they render offscreen and compare against reference images within a
  tolerance, so lavapipe can run them without a GPU.
- Name and time everything: every Vulkan object gets a debug name, and every render-graph pass
  a debug label and a GPU timestamp, so captures and Tracy timelines read clearly.
- Diagnostics are data: log through `tracing` with fields rather than printing, and give every
  error and warning a stable code and the thing it is about, as
  [ARCHITECTURE.md](ARCHITECTURE.md#diagnostics) describes.
- Steady-state frames don't allocate: per-frame data lives in frame arenas and ring buffers.

## Workflow

- Commit only when the user asks. A milestone's commit message says what was done, what was
  deferred and how it was verified: git history is the work log.
- `docs/PLAN.md` holds the current milestone, the phases after it, deferred work and open
  questions; read it before roadmap work, and update it in the same commit as the work it
  tracks. When a milestone is done, replace its section with one line under Done and move what
  was skipped to Deferred.
- Open questions are the user's to decide: bring a recommendation instead of settling one
  yourself. Once one is decided, the answer goes in ARCHITECTURE.md and the question leaves
  docs/PLAN.md.

## Keeping the docs lean

- Each fact has one home: this file for how to work here, ARCHITECTURE.md for the design,
  docs/PLAN.md for the work. Link instead of copying.
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
