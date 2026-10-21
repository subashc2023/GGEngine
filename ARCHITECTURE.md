# Architecture

GGEngine is an agent-native, bleeding-edge, high-performance, high-quality, cross-platform game
engine in Rust on Vulkan 1.4.

This file is the engine's design: how it fits together and why, including the parts not built
yet. A change to the design updates this file in the same commit. Plans and progress are not
recorded here. The rules the code follows, such as the unsafe policy and per-frame allocation,
are in [AGENTS.md](AGENTS.md).

## Goals

- **v0.1: change a running game by asking.** The user sees something they don't like in the
  running game and tells a coding agent; the agent edits the game's code or works on the game
  through the [tool API](#agents-and-tools), and the game picks up each change with its state
  intact. The agent is Claude Code at first; v1 builds one into the editor, modeled on the pi
  coding agent.
- **People and agents build together.** Many developers and agents work on one project at
  once: project state is text that diffs and merges, every authoring action is an operation an
  agent can perform, and every result is machine-readable.
- **Any kind of game, at any scale, single-player or multiplayer.** Game code is native Rust,
  and the engine supplies what each multiplayer model needs.
- **Grown with a flagship game.** The engine grows alongside an in-house game: the game's
  needs set the engine's priorities, and shipping the game proves the engine.
- Rendering the way current AAA engines do it: bindless resources, a render graph and
  GPU-driven rendering.
- Simulation and rendering pipelined across threads.
- An offline asset pipeline that cooks content for a shipping player.
- Windows, Linux and macOS, on x86-64 and ARM64. Consoles, mobile and the web wait until the
  desktop engine is solid.

## Crates

```text
   sandbox     player          apps/: executables
        \       /
        gg-engine              crates/: libraries
            |
       gg-renderer
            |
          gg-gpu
            |
         gg-core               every crate may use gg-core
```

- **`gg-core`:** math, handles, logging, timing and error types.
- **`gg-gpu`:** Vulkan through `ash`. It turns ash's unsafe API into safe types: device, memory,
  buffers, images, descriptors, pipelines, swapchain, synchronization and submission.
- **`gg-renderer`:** the render graph, passes, materials, lighting and post-processing. It owns
  the GPU resources and hands out handles to them.
- **`gg-engine`:** the main loop, window and input, scene and assets, plus hot reload and the
  tool API in development builds: what apps and games build on.
- **`sandbox`:** the development playground for engine features.
- **`player`:** the reference player. It runs a cooked scene without game code and shows how a
  game builds its own player.

Games depend on `gg-engine` the same way the apps do; see [Game code](#game-code).
Executables are named `gg-` and the app, such as `gg-sandbox`: GPU drivers apply per-game
settings by executable name, and a generic name can match one.

### Layering rules

1. **Dependencies point down only.** Each crate declares a dependency on the crate below it,
   even before it uses it, so that reaching up closes a cycle, which Cargo rejects. Cargo
   allows cycles through dev-dependencies, so don't use them to reach up.
2. **`gg-core` is the leaf:** no `gg-*` dependencies, and nothing GPU- or window-specific.
3. **`gg-gpu` owns Vulkan** and knows nothing about meshes, materials or scenes. A separate
   device layer keeps the unsafe surface small and reviewable, and lets rendering techniques
   change quickly on safe types.
4. **`gg-renderer` never sees the scene.** Each frame the engine extracts plain render data
   (cameras, instances, lights, settings) and hands it over. That lets rendering run on its own
   thread a frame behind simulation, and renderer tests run without the engine.
5. **The engine reaches the GPU only through `gg-renderer`.**
6. **`main` is thin.** An app's or a game's `main` describes it and hands control to
   `gg-engine`, so there is no entry-point crate. Android will be the exception: its
   `android_main` lives in a `cdylib`, which gets a small crate of its own.

Cargo's cycle check enforces rule 1, and rule 2's ban on `gg-*` dependencies. It can't see a
skip-level edge such as `gg-engine` → `gg-gpu`, which forms no cycle, so
[deny.toml](deny.toml) names the only crates that may depend directly on `gg-gpu`,
`gg-renderer`, `ash`, `ash-window` and `winit`, and CI checks it with cargo-deny. Rule 4, and
what a crate does with the dependencies it may have, are left to review.

### Builds

`gg-engine`'s development tooling (hot reload, the tool API, debug UI) will sit
behind a `dev` feature that development builds turn on and shipping builds never do; shipping
builds run cooked data only. Cargo merges the features of every package a command selects, so
`--workspace`, a bare `cargo build` and `--bin gg-player` would all hand the player `dev`.
Shipping builds therefore select the player alone, with `-p player`, and a game's shipping
build selects the game alone the same way.

Vulkan validation is a runtime switch rather than a feature, on by default in debug builds, so
tests run with it whatever the features.

### Growth

A module becomes its own crate once it has its own responsibility and a second user, not
before:

- `gg-assets`: cooked asset formats, shared by the cooker and the runtime.
- `tools/gg`: the command-line tool; `tools/*` joins the workspace members then. The tool API's
  message types move into a crate that it and `gg-engine` share.
- `tools/cook`: the offline asset cooker.
- `apps/editor`: the editor, one more client of the tool API.
- `games/`: games built alongside the engine, starting with the flagship; `games/*` joins the
  workspace members with the first one.
- `gg-scene`, `gg-physics`, `gg-audio` and the like: when each outgrows a module of
  `gg-engine`.

## Game code

Game code is Rust, as fast and as unrestricted as the engine's own. Each game is a crate that
depends on `gg-engine` and builds its own player, statically linked and optimized for
shipping. Game crates forbid `unsafe`, so a bug in game code panics rather than corrupting
memory. Games built alongside the engine, starting with the flagship, live in `games/` in this
workspace, so one change can span engine and game; anyone else's game depends on the engine
from its own repository.

- **The game runs in its own process.** The development tools launch the game's development
  build and work on it through the [tool API](#agents-and-tools). A crash, panic or hang in
  game code stops only the game, and the tools restart it from its last snapshot. The same
  split gives dedicated servers, local multiplayer tests with a server and several clients,
  and headless runs in which agents check their work. The game shows in its own window until
  the editor embeds its view.
- **Hot reload keeps the world.** `subsecond` patches an edit inside a function body into the
  running game. Any other change, such as a new field, a new component or an engine edit,
  rebuilds the game, restarts it and restores the world from a snapshot taken just before,
  matching each changed component to its new layout by field name.
- **Luau comes after v0.1, for mods and quick scripts.** Players can install sandboxed mods,
  and quests, dialogue and level events reload in milliseconds. Scripts reach Rust components
  through reflection, without hand-written bindings, and can declare components of their own.
  Luau doesn't guard against crafted bytecode, so mods ship as source and only a game's own
  packages hold bytecode.

## Agents and tools

Agents work on a game the way people do, through the same interfaces: nothing a person can do
in the editor is out of an agent's reach.

- **The tool API.** A game's development build serves an API on a local connection: query the
  world, apply and undo [operations](#project-state), run, pause and step, capture frames, and
  read [diagnostics](#diagnostics). Each client gets only the capabilities it is granted, such
  as reading the world or editing one scene.
- **Clients.** The command-line tool builds, runs and tests projects with machine-readable
  results and talks to running games; an MCP server gives agents that speak MCP the same
  reach; the editor is one more client, with no ability an agent lacks.
- **Headless runs.** Every app runs without a window (`--headless`) and can run a set number
  of frames, then exit with a status (`--frames N`): 0 for a clean run, 1 after an error or any
  validation message, and 2 for bad arguments. Dedicated servers, CI and agents checking their
  work run this way; rendering checks render offscreen, and simulation-only runs need no GPU.

## Project state

A project's state is text that people and agents edit, diff and merge with ordinary tools.

- **Canonical text files.** Scenes, prefabs and project settings are saved as RON in a
  canonical form: saving what was loaded gives the same bytes, entities are ordered by ID, and
  changing a field changes only that field's lines. RON maps Rust's enums and tuples directly,
  and its trailing commas mean adding an entry never touches its neighbors' lines.
- **Stable IDs.** Every entity has a persistent ID, separate from its runtime handle, that
  files, operations, diagnostics and network messages use to name it. IDs are random rather
  than sequential, so entities added on two branches neither collide nor land on the same
  lines.
- **Operations.** Every authoring change, by a person or an agent, is an operation with an
  inverse: create or delete an entity, add or remove a component, set a field, reparent. A
  running game applies operations in the order they arrive, so several clients can edit it at
  once, and their log gives undo, review and replay. Editing a scene file while the game runs
  is an edit too: the engine applies the difference as operations.

## ECS

The world lives in our own ECS, built for what the tools, hot reload and multiplayer need:

- **Reflection:** every component type, whether Rust or a script declares it, is registered at
  runtime with a description of its fields. Saving, operations, the inspector, hot reload's
  snapshots, network replication and script bindings all work from it.
- **Snapshots:** the whole world can be saved and restored fast enough to take one every
  frame, for rollback and client-side prediction as well as hot reload.
- **Commands:** nothing outside the simulation changes its state except a command applied at a
  tick: player input, a network message or an [operation](#project-state). A snapshot and the
  commands after it make a recording, for replays, rollback and bug reports an agent can rerun.
- **A deterministic mode:** a fixed timestep, a fixed order of systems and of iteration, and
  math that gives identical results on every platform, so lockstep games and rollback netcode
  stay in sync. That rules out the platform's C math library and any SIMD path whose order of
  operations differs by CPU. The world is hashed every tick, so two runs or two machines find
  the first tick where they diverge, and comparing their snapshots through reflection names
  the entity and field that differ.

Each game picks its own network model, such as an authoritative server, lockstep or
peer-to-peer rollback; the engine supplies these pieces.

## Diagnostics

Diagnostics are data, so agents and tools can act on them as reliably as people read them.

- **Logs** go through `tracing` with structured fields, filtered by `RUST_LOG`. Each run prints
  readable, colored lines to stderr, which keeps stdout for machine-readable output, and writes
  every event as a JSON line to a file that agents and tools read:
  `logs/<app>-<UTC time>-<process ID>.jsonl` beside the executable, whose path the run logs as
  it starts.
- **Errors and warnings name their subject.** Each carries a stable code, a dotted name such as
  `gpu.device_lost`, and the thing it is about: an entity ID, an asset path, a render pass, or
  a shader file and line.
- **Validation messages** become events with their VUID and the debug names of the objects
  involved, and a panic is logged with its location before the process ends.

## Events and layers

The engine turns the window system's events into its own: window (close, resize, focus, scale,
minimize), keyboard and mouse. winit stays inside `gg-engine`, so apps and games never see its
types.

An app is a stack of layers, which its `main` describes on `App`: layers, then overlays above
them. Each layer attaches when the app starts and detaches when it ends, updates every frame
with the time since the last, handles events and draws UI. An event goes down the stack from
the top overlay until a layer handles it; updates and UI go up from the bottom. Headless runs
have no window events, but their layers update and draw UI all the same.

egui, the engine's immediate-mode UI, is the top overlay: it sees every event first and keeps
those it wants, such as a click on one of its panels, and `gg-renderer` draws its meshes over
the frame. Key and button releases reach the layers regardless, so that none is left holding a
key or button it saw pressed. Tab reaches egui only while one of its widgets has focus, to move
between them; otherwise it goes to the layers alone, since egui would take focus with it.

Layers draw UI through `Ui`: dockable panels, and egui itself for anything else. The dock is the
engine's own, since egui's docking crates can't put a panel in a window of its own. Panels sit
in tab groups, docked in strips along the main window's edges around a central area that stays
the game's, or floating: over the main window, or torn off into a window of their own, which
holds one group. egui counts only the strips and groups as its own, so input over the central
area goes to the layers. A panel shows on the frames its layer shows it, and keeps its place
while it doesn't. The layout is saved as RON in the OS's directory for per-user state, never
beside the executable; headless runs neither read nor write it.

The torn-off windows are egui's immediate viewports: egui runs each one's pass inside the main
window's, and the main window's frame renders them all. Only the main window paces frames;
another with no image free skips the frame. Their events go to egui, never to the layers. On
Wayland, where windows can't be placed, groups float over the main window instead.

Layers add menus through `Ui` too. The OS draws the windows' frames, and the menus get a bar of
their own below its title bar, unless the app calls `App::custom_title_bar`, as tools do for the
room: then the UI draws them. The main window's title bar holds the menus, the title and the
buttons that minimize, maximize and close it, and a torn-off window's tab bar is its title bar,
with a close button. Dragging the main window's title bar has the OS move it, so snapping works
as usual, while a torn-off window moves with the dock's drag, so that it can dock; the edges have
the OS resize a window. On macOS the OS keeps its own buttons and the edges. Games keep the OS's
frame. A menu quits by sending egui's `Close` to the main window,
which, like the OS's close button, gives the layers `CloseRequested` first; egui's own Ctrl or
Cmd+Q is off, since a game may use those keys.

Layers can also poll input, through their `Context` rather than a global: which keys and mouse
buttons are held, and where the cursor is. That state follows the events the stack is sent, so
input the UI keeps doesn't count, and a press and release between two frames never shows as
held. Losing focus lets go of everything, since not every platform reports the releases that
come while the window is out of focus.

Layers run on the main thread with the event loop. Input that reaches a game's layer enters the
simulation as commands, as [ECS](#ecs) describes.

## Threads

The main thread runs the OS event loop, which macOS requires. Simulation runs on a job system.
The render thread records and submits frame N while simulation computes frame N+1, and the GPU
works on up to two frames at once.

## Platforms

- **Vulkan 1.4, bleeding edge.** The engine requires a Vulkan 1.4 device and takes up newer
  features as drivers ship them, with a fallback wherever a target lacks one.
- **Windows and Linux:** native Vulkan drivers.
- **macOS: KosmicKrisp,** LunarG's Vulkan 1.4 driver on Metal. It needs Apple Silicon and
  macOS 26 or later. Not MoltenVK, which Khronos deprecated in October 2026. KosmicKrisp has no
  mesh shaders or ray tracing, so those stay optional paths.
- **x86-64 and ARM64:** the engine builds and passes its tests on both from the start, rather
  than being ported later; macOS is ARM64 only. An ARM64 device still needs a Vulkan 1.4
  driver.
- **No GPU:** lavapipe, Mesa's CPU Vulkan driver, runs the GPU tests headless.

## Technology

| Area | Choice |
|---|---|
| Language | Rust stable, edition 2024 |
| Game code | Rust; Luau through `mlua` after v0.1, for mods and scripts |
| Hot patching | `subsecond` |
| GPU API | Vulkan through `ash`: one backend, no multi-API abstraction layer |
| GPU memory | VMA through our own binding in `gg-gpu`, for its budgets, aliasing, pools and defragmentation |
| Window and input | `winit`, with `ash-window` for surfaces and `gilrs` for gamepads |
| UI | egui, drawn by `gg-renderer`, with a dock of our own |
| Project state | RON in a canonical form; see [Project state](#project-state) |
| Math | `glam` |
| ECS | Our own; see [ECS](#ecs) |
| Logs and profiling | `tracing` for logs; Tracy for CPU zones and GPU timestamps |
| Agent access | MCP and a command-line tool, both clients of the [tool API](#agents-and-tools) |
| Shaders | Slang, compiled to SPIR-V by the Vulkan SDK's `slangc` rather than linked in |
| Assets | glTF 2.0 in; meshoptimizer for meshes and meshlets; BC-compressed textures in KTX2 |

Areas not listed here are undecided.
