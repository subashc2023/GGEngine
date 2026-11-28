# GGEngine

An agent-native, cross-platform game engine in Rust on Vulkan 1.4, built by people and agents
together.

The v0.1 goal: change a running game by asking. You tell a coding agent what to change, and the
running game picks up each edit with its state intact.

**Status:** pre-release. Phases 0 to 22 of [the roadmap](docs/PLAN.md) are done, but for a hand
check of the macOS build and a match played on other networks. Nothing is published to crates.io,
and the APIs still change.

## What it is

- **Game code is Rust.** A game is a stack of layers over an ECS world, with Luau for scripts and
  mods.
- **Agents are first-class.** Development builds serve a JSON tool API (`cargo gg`, or MCP) to
  inspect, capture, set, undo and step the game. `cargo gg run` hot-patches code edits, restarts
  the game in place for edits to its types, and rebuilds from a snapshot when neither can apply.
- **Project state is text.** Scenes, prefabs, manifests and asset IDs are RON files that diff and
  merge.
- **Modern rendering.** Bindless resources, a render graph and GPU-driven culling. Also PBR,
  shadows, AO, reflections, TAA, bloom, sky and atmosphere, particles and 2D sprites.
- **Deterministic simulation.** Seeded runs hash the world each tick, and CI checks the hashes on
  every platform. Multiplayer covers lockstep, rollback, client-server and a relay for NAT.
- **Shipping builds.** A cooker turns glTF, textures, fonts and shaders into one package, and
  `cargo gg ship` lays out a folder, or a signed `.app` on macOS, with license notices and symbols.
- **Games of their own.** `gg new --engine` makes a game in a repository of its own, against a
  checkout or a pinned revision of the engine.
- **Platforms:** Windows and Linux on x86-64 and ARM64, and macOS on Apple Silicon (through
  KosmicKrisp).

## Requirements

- Rust 1.99.1. [rust-toolchain.toml](rust-toolchain.toml) pins it, so rustup installs it for you.
- A GPU and driver with Vulkan 1.4. For debug builds and tests, also the Khronos validation layer
  (the Vulkan SDK has it).
- `slangc`, the Slang shader compiler. The build uses `SLANGC`, then the Vulkan SDK's, then the
  first on the `PATH`. [CI's action](.github/actions/slangc/action.yml) pins the release whose
  SPIR-V the reference images expect.
- A C and C++ compiler, for VMA, Luau, zstd and meshoptimizer.
- On Linux: `pkgconf`, `libudev-dev` and `libasound2-dev` (Debian names).
- For edits that patch into a running game: `dx`, dioxus-cli at the release of the workspace's
  `subsecond` (`cargo install dioxus-cli --version 0.7.2 --locked`). Without it, `cargo gg run`
  rebuilds and restarts the game for every edit.
- On macOS, for quick restarts: your terminal, or the app running your agent, under System
  Settings › Privacy & Security › Developer Tools. Otherwise macOS scans each new build as it
  first launches: about 3 s a restart for a game's 200 MB debug executable.
- For `cargo gg ship`: cargo-about 0.9.2.

## Quick start

```sh
cargo run -p editor                 # the editor's hub: open a project, such as games/shards
cargo run -p sandbox                # 100,000 sprites and a scene file
cargo run -p sandbox -- --help      # the flags every app shares
cargo gg help                       # the command-line tool and the tool API

cargo gg new mygame                 # a new game in games/mygame: a square the keys move
cargo gg run mygame                 # run it supervised: edits patch in as you save

cd games/shards && cargo gg run --peers 2   # the networked demo: a host and a joiner
cd games/showcase && cargo gg fetch         # Sponza and Bistro, which git doesn't hold
cargo gg ship shards                        # a shipping build in target/ship/shards
```

To change a running game with an agent, run it under `cargo gg run`, then ask the agent. It edits
the code and uses `cargo gg` (or `cargo gg mcp`) to see the result.

## Layout

```text
crates/   gg-core (math, logging, profiling), gg-gpu (Vulkan, render graph),
          gg-renderer (passes, materials), gg-engine (what games use), gg-macros (derives),
          gg-api (the tool API's protocol), gg-relay (the relay sessions meet through)
apps/     sandbox, editor, player, relay
games/    shards (2D, networked), showcase (a scene per feature)
tools/    gg (`cargo gg`), cook (the asset cooker)
docs/     PLAN.md (roadmap), DEFECTS.md (known defects), LUAU.md (the scripting API)
```

Dependencies point down: apps → gg-engine → gg-renderer → gg-gpu → gg-core.
[deny.toml](deny.toml) enforces the layering.

## Docs

- [ARCHITECTURE.md](ARCHITECTURE.md): the design and the reasons behind it.
- [AGENTS.md](AGENTS.md): how to work here, for people and agents. It covers commands, gates,
  debugging and the rules.
- [docs/PLAN.md](docs/PLAN.md): what's done, what's open, what's deferred.
- [docs/DEFECTS.md](docs/DEFECTS.md): known defects and when each fix pays.
- [docs/LUAU.md](docs/LUAU.md): every `gg` function scripts and mods can call.
- The API: `cargo doc --no-deps --open`. Start at `gg_engine`.

## Contributing

A change is done when the seven gates in [AGENTS.md](AGENTS.md#gates) pass: fmt, four clippy
runs, rustdoc and `cargo test --workspace`. The tests include GPU tests against reference images.
Without a GPU they run on lavapipe (Mesa 25.0 or later).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
