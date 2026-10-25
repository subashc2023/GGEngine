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
        gg-engine - gg-macros  crates/: libraries
            |
       gg-renderer
            |
          gg-gpu
            |
         gg-core               every crate may use gg-core
```

- **`gg-core`:** math, handles, logging, profiling and error types.
- **`gg-gpu`:** Vulkan through `ash`. It turns ash's unsafe API into safe types: device, memory,
  buffers, images, descriptors, pipelines, swapchain, synchronization and submission.
- **`gg-renderer`:** the render graph, passes, materials, lighting and post-processing. It owns
  the GPU resources and hands out handles to them.
- **`gg-engine`:** the main loop, window and input, the job system and the render thread, scene
  and assets, plus hot reload and the tool API in development builds: what apps and games build
  on.
- **`gg-macros`:** the derives `gg-engine` re-exports beside its traits of the same names,
  `Reflect` and `Component`. Rust builds a derive as a crate of its own; only `gg-engine`
  depends on it, and games derive through `gg-engine`.
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
   (cameras, instances, lights, settings) and hands it over. So rendering runs on its own thread,
   a frame behind simulation, as [Threads](#threads) says, and renderer tests run without the
   engine.
5. **The engine reaches the GPU only through `gg-renderer`.**
6. **`main` is thin.** An app's or a game's `main` describes it and hands control to
   `gg-engine`, so there is no entry-point crate. Android will be the exception: its
   `android_main` lives in a `cdylib`, which gets a small crate of its own.

Cargo's cycle check enforces rule 1, and rule 2's ban on `gg-*` dependencies. It can't see a
skip-level edge such as `gg-engine` → `gg-gpu`, which forms no cycle, so
[deny.toml](deny.toml) names the only crates that may depend directly on `gg-gpu`,
`gg-renderer`, `gg-macros`, `ash`, `ash-window`, `winit`, `egui-winit`, `gilrs`, `gilrs-core`,
`rayon-core`, `tracy-client` and `tracy-client-sys`, and CI checks it with cargo-deny. Rule 4,
and what a crate does with the dependencies it may have, are left to review.

### Builds

`gg-engine`'s development tooling (hot reload, the tool API, debug UI) will sit
behind a `dev` feature that development builds turn on and shipping builds never do; shipping
builds run cooked data only. Cargo merges the features of every package a command selects, so
`--workspace`, a bare `cargo build` and `--bin gg-player` would all hand the player `dev`.
Shipping builds therefore select the player alone, with `-p player`, and a game's shipping
build selects the game alone the same way.

Tracy is a feature of its own, `tracy`, off by default and in no shipping build: it compiles
Tracy's C++ client, which listens on the machine's loopback and keeps what it records in memory
until a viewer connects. Apps and games turn it on through `gg-engine`'s, as the sandbox does.

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
  work run this way; rendering checks render offscreen, and simulation-only runs need no GPU. A
  headless run takes a tick of the simulation each frame, so that a run of a set number of frames
  runs as many ticks on any machine.

## Project state

A project's state is text that people and agents edit, diff and merge with ordinary tools.

- **Canonical text files.** Scenes, prefabs and project settings are saved as RON in a
  canonical form: saving what was loaded gives the same bytes, entities are ordered by ID, and
  changing a field changes only that field's lines. RON maps Rust's enums and tuples directly,
  and its trailing commas mean adding an entry never touches its neighbors' lines. A scene is
  `(entities: {...})`, each entity by its ID, as a string, with its components by name in the
  order of their names, and each one's fields in its description's order, every field written,
  and no type's name, so that a rename touches only its keys. A struct or a variant with fields
  and a list with elements are blocks, a part to a line; a value of one part, `Some(v)` or a
  one-field tuple's `(v)`, wraps it; and leaves are inline: a float as the shortest text that
  reads back to its bits, never an exponent, with `.0`, and `inf` and `NaN` keeping their sign;
  a string by escapes of the scene's own, which leave no control or bidirectional character
  bare; names ASCII identifiers; at most 64 levels deep. The engine writes the text itself, so
  that the bytes depend on no library's formatting, line endings or Rust release, and `ron`
  only reads it, through seeds that reflection's descriptions drive, never its untyped values.
  What's opaque, such as a sprite's texture or a `MeshDraw`, isn't saved: a load builds each
  component on its type's `Component::base`, which gives it that, or from the scene alone. A
  child's `Parent` is saved as its parent's ID, and a root's isn't. A save makes each component
  again as a load would, so that every scene saved loads; a load checks everything before it
  spawns anything, all or nothing, and a refusal begins with the line and column where the text
  is refused, and, but for a syntax error, names the entity, the component and the field.
- **Prefabs.** A prefab is a scene of one subtree, placed as a copy under a parent. Each of the
  copy's IDs is mixed from one drawn for the copy and the prefab's own, `mix64(local ^
  mix64(instance ^ PLACE))`, a permutation of the prefab's IDs for each copy, fixed for good, so
  that a copy's IDs are as distinct as its prefab's, and two copies' meet only by chance;
  references inside the prefab are to the copy's own entities. A scene saves the copies as
  entities like any others; linked instances are to come.
- **Stable IDs.** Every entity has a persistent ID, separate from its runtime handle, that
  files, operations, diagnostics and network messages use to name it. IDs are random rather
  than sequential, so entities added on two branches neither collide nor land on the same
  lines.
- **Operations.** Every authoring change, by a person or an agent, is an operation with an
  inverse: create or delete an entity, add or remove a component, set a field, reparent. An
  operation names entities by their IDs and carries values as reflection's. Applying one checks
  everything before anything changes, and gives what undoes it: another operation, but for a
  delete and a remove, whose inverses keep what they took out, typed, so that what's opaque
  comes back too. A delete keeps its entities' slots, so that undoing it gives each back under
  its handle, and every reference to it is whole again. The log keeps the operations applied, in
  order, with their inverses: it undoes and redoes them, merges a gesture's sets of one field,
  such as a drag's, into one edit, and drops an entry whose undo the world refuses, since the
  world has moved on from it. A running game applies operations at its ticks, in the order they
  arrive, so several clients can edit it at once, and their log gives undo, review and replay.
  Editing a scene file while the game runs is an edit too: the engine applies the difference as
  operations.

## ECS

The world lives in our own ECS, built for what the tools, hot reload and multiplayer need:

- **Reflection:** every component type, whether Rust or a script declares it, is registered at
  runtime with a description of its fields. Saving, operations, the inspector, hot reload's
  snapshots, network replication and script bindings all work from it.
- **Snapshots:** the whole world can be saved and restored fast enough to take one every
  frame, for rollback and client-side prediction as well as hot reload.
- **Commands:** nothing outside the simulation changes its state except a command applied at a
  tick: player input, a network message or an [operation](#project-state). Each layer queues its
  own, which the engine reads and fills as reflection's values at each tick through
  `Layer::commands`: a `Vec` of a reflected type, or a queue that maps its own, as the Inspector's
  does.
  A recorded run (`--record`) writes its seed, each layer's commands just before its tick applies
  them, with the entities they hold by their IDs, and the world's hash after each tick, a line at a
  time, so that a run that crashes or is killed keeps what it ran. A replay (`--replay`) runs the
  ticks again alone, without a window or a GPU, each layer's with the commands recorded for it in
  place of its own, for bug reports an agent can rerun; it refuses a recording of another format,
  app or stack of layers, and replays one from another build, naming both builds where it goes
  another way. A snapshot and the commands after it will replay from there, for rollback; so a
  command is data, never a closure.
- **A deterministic mode:** the fixed timestep, a fixed order of systems and of iteration, and
  math that gives identical results on every platform, so lockstep games and rollback netcode
  stay in sync. So glam is built without SIMD, whose SSE2 and NEON paths add in different orders
  and fuse multiplications with additions only on ARM, and with libm's functions in place of the
  platform's C math library: for the whole build, drawing too, since Cargo merges features. And
  clippy refuses std's float methods whose precision is the platform's across the workspace
  ([clippy.toml](clippy.toml)), for `gg_core::math`'s. A recorded run hashes the world once its
  layers attach and after every tick, so that its replay, on another machine or after a change,
  finds the first tick where it goes another way. `World::difference`, which walks two worlds
  field by field and tells values apart as the hash does, names the entity and field in one
  process; across two machines, `--dump` writes the world there a field a line, and the two dumps
  diff to them.

Each game picks its own network model, such as an authoritative server, lockstep or
peer-to-peer rollback; the engine supplies these pieces.

The world is a `World` of entities and their components, in safe Rust. An entity is a handle,
its slot and which use of the slot it is, so that a despawned entity's handle never names
another, unless a restore takes the world back to before it was spawned. A delete operation
leaves its entities' slots vacant rather than free, their uses as they were, until its inverse
puts them back or is dropped, so that an undone delete's entities have their handles again.
A component is a struct or an enum that derives `Component`, which describes it to reflection;
it's `Clone`, for snapshots, and `Send` and `Sync`, for the job
system. Components live in archetype tables, one for each set of component types, whose rows are
its entities and whose columns are each type's own `Vec`, which the world reaches through a trait
object, downcast once a table, rather than through raw pointers. Queries borrow the world, shared
to read it and exclusively to change it, so that the borrow checker keeps them apart, and walk
each table's columns as slices. Systems are plain functions over the world, run at its ticks in a
fixed order; one can hand a query's rows to the job system's threads with `par_for_each`, as
[Threads](#threads) describes. The order queries find entities in follows only from the changes
made: tables are kept in the order they were made, despawning an entity or changing its components
moves its table's last row into its place, the last freed slot is the first taken, and no hash map
is iterated. So it's the same in every run, as the deterministic mode needs, but means nothing a
game may rely on: anything ordered, such as what's drawn over what, orders by its components'
values, or by its entities' IDs.

Each entity has a persistent ID, an `EntityId`: a nonzero 64-bit number, written as 16 lowercase
hex digits, so that IDs sort as their text does and never read as a handle. Each world draws its
IDs from SplitMix64, whose 2^64 states each mix to a different number, seeded from the OS's
randomness through std's `RandomState`, so that a run of the cycle never gives an ID twice, and two
runs, two sessions' or a world's on either side of a reseed, meet only where they overlap; a run
that must go the same way every time seeds it, as the sandbox's reference run does, and a snapshot
copies its state. A scene's entities keep the IDs it gives them, and a load refuses one a living
entity has. A draw that meets a living entity's ID, as a stream that walks into the run of the
session that saved a loaded scene does, reseeds the stream, the same way for the same state, and
draws again. The world keeps each slot's ID in an array beside the slots' `Meta` rather than in
it, since every lookup of a handle reads `Meta`, and IDs are read only where they cross into
files, tools and diagnostics, and as drawing sorts the rows at one z: written only as an entity is
spawned into the slot and removed from it, and read through the entity's handle, checked first, so
that a stale handle never names the entity now in its slot. A map hashed with SipHash, so that IDs
from files can't be chosen to collide, finds an entity by its ID; it's looked up, never iterated.

Reflection names each component type by its identifier, unless `#[reflect(name = "...")]` gives
another, and two types of one name are an error rather than a guess. The derive writes the
description, with hints for bounds, angles, colors and fields kept opaque, and how a value is
hashed. The engine's own components register as it starts, so that their names are taken first,
and a game's the first time an entity has one; each struct or enum among their fields is described
once, so a type can hold itself. Values cross as a closed `Value`: integers of fixed sizes only, so
that a value means the same on every machine; `f32` apart from `f64`, so that bits come back as
they went; an entity as its handle, as components hold one, `Parent` among them, which scenes
write as IDs, and the tool API and replication will map to them; and `Opaque` for what reflection
keeps but doesn't read, such as a texture's handle, which no value may make. The world reads and
sets a field by its component's name and a path, such as `rect[1].y`: a set checks the whole value
against the description before anything changes, refuses a number outside its field's bounds, and
returns the old value, which sets it back unless code stored one that a set refuses. The Inspector
edits components through these calls alone, as operations it queues for its next tick and its log
undoes, which the tool API will serve.

Entities form a hierarchy, as scenes, prefabs and the camera as a component need: a child has its
parent as its `Parent` component, and its `Transform` is relative to its parent's, so that a
parent carries its children along. The world keeps the hierarchy whole. A scene's entities, loaded
or placed as a prefab, and those an operation creates or an undo puts back, are spawned with
their `Parent`s, and after that only `World::set_parent` gives, changes or takes away a
`Parent`: it keeps the child's own transform, refuses a parent that is the child or one of its
descendants, and returns the old parent, which sets it back. The typed calls that change
components take only `Writable` ones, which the derive implements, as does a component
implemented by hand, and `Parent` doesn't, so that code changing a `Parent` any other way
doesn't compile; `set_field` and operations refuse it by name. Each slot keeps its entity's
links, by slot: its parent, as its `Parent` has it, and its children, in the order they became
its children, which means no more than the order queries find entities in; a load makes each
parent's children its own in the order of their IDs, an undone delete puts a subtree back in
its order, the root the last of its parent's children, and an undone reparent makes the child
the last of its old parent's. So despawning an entity despawns its descendants, children first,
and the links allocate nothing, though giving or taking away a `Parent` moves the child to
another table, as `insert` and `remove` do; a snapshot copies them with the slots. World
transforms aren't stored: drawing computes each child's from its ancestors, every frame, while a
table of roots draws by its transforms alone, for one check a frame; the links cost each slot 16
bytes, whether its entity has a parent or not.

A snapshot is the world copied: `World` is `Clone`, and `clone_from` restores a world from a
snapshot, or takes a snapshot into one kept from before, in the memory it holds. It copies all of
the world: each slot under its generation, the free ones in the order they're taken and the vacant
ones vacant; each entity under its handle and ID, in its table and row, with its links; the
tables in the order they were made, cutting away those made since; the stream the IDs come from;
and the component types. So a restored world goes on as the world the snapshot was taken from
did, spawning the same handles with the same IDs and finding entities in the same order, and gives
out again the handles and IDs given out since the snapshot. A copy is another world, with a serial
of its own: drawing sorts its rows again, and it refuses what undoes an operation applied before,
as another world's, so that undo history doesn't outlive a restore, and the slots a log kept when
the snapshot was taken stay vacant for good, since freeing them would change the handles to come.
A snapshot taken again, or restored, is copied into memory already held, so that one taken every
frame allocates nothing while the tables, the component types and the world's size stay, for
components whose `Clone` allocates nothing.

A world's hash has two parts. Its content is what it holds: a term for each component of each
entity, from the entity's ID, the type's name and the words the type's derive writes, its fields in
order, an entity it holds by its ID, or 0 if it's dead, and a float by its bits, but every NaN as
one, since arithmetic leaves a NaN's sign to the CPU; summed, so that where the world holds them
doesn't count, and leaving out what's opaque. Its layout is how the world holds them, which the
order queries find entities in follows from: its tables in the order they were made, with their
types and their rows' IDs in order, each parent's children in their order, and the state of its
stream of IDs. Neither reads slots, handles or the order types registered in, so that a scene
loaded into another world has the content it had. Undoing a log gives back the content, though not
always the layout, since what an undo puts back goes last in its table, and the IDs a create drew
stay drawn: rewinding the stream would hand an ID out twice. Hashing reads every component, without
allocating.

The simulation steps in ticks, 64 a second whatever the frame rate, each 15.625 ms, exact in
`f32`, `f64` and nanoseconds. A frame runs the ticks the time since the last one owes, at most 8,
and no more once its ticks have taken longer than the time they simulate, so that a simulation
slower than real time slows down rather than spending every frame catching up, before the layers'
updates, and draws what they leave as [Rendering](#rendering) says. The whole ticks a frame
doesn't run are dropped, and counted in `engine.perf`, and the fraction of one left over carries
to the next, as how far the frame draws the world on its way from where the last tick found it. A
frame without time runs none, as the first does and the first after the window is restored, and
a headless run takes exactly one each frame. Each layer's tick applies the commands it queued
since its last, data of its own, in the order it queued them, then runs its systems, the layers
from the bottom up. Once they've attached, the world changes only there: `Context::world_mut`
panics outside attaching and detaching, and a tick is given the world but not the `Context`, so
that it goes by the world, its commands and what its layer set up, never by the frame's time,
input or drawing. A replay runs the ticks alone, with no update, UI or event between them, so a
layer keeps between ticks only what it set up as it attached and its queue, its actions being the
frame's, as its UI is, and the world holds the rest of the simulation, as a snapshot restores it:
the sandbox finds its swarm by its drifters' indexes, and which way its triangle goes in a
component. The Inspector's log is the exception, which a replay from the attach rebuilds
from the recorded edits; it can't be a component, since its inverses hold typed columns tied to
the world they were made in. Operations are the commands of authoring: the Inspector's edits,
undos and redos wait for its next tick, its panel drawing a field set as the set will make it
until then, and a refusal and the Edit menu's names show from the tick; it queues no seal that
would end no edit, so that frames that change nothing queue nothing.

## Diagnostics

Diagnostics are data, so agents and tools can act on them as reliably as people read them.

- **Logs** go through `tracing` with structured fields, filtered by `RUST_LOG`. Each run prints
  readable, colored lines to stderr, which keeps stdout for machine-readable output, and writes
  every event as a JSON line to a file that agents and tools read:
  `logs/<app>-<UTC time>-<process ID>.jsonl` beside the executable, whose path the run logs as
  it starts.
- **Errors and warnings name their subject.** Each carries a stable code, a dotted name such as
  `gpu.device_lost`, and the thing it is about: an entity's ID, or a despawned entity's handle,
  since it has none; an asset path, a render pass, a shader file and line, or a scene's line and
  column. What outlives an
  entity, such as a pick or a queued message, keeps its ID, not its handle.
- **Validation messages** become events with their VUID and the debug names of the objects
  involved, and a panic is logged with its location before the process ends.
- **Profiling.** Code marks its work with zones (`gg_core::zone!`), which Tracy lays out on a
  timeline of frames, with plots such as each frame's draws; without the `tracy` feature they
  compile to nothing. The engine also measures every frame itself, always: its time on the main
  thread, and how much of it went to the simulation's ticks, the layers' updates and drawing the
  world, the UI, and waiting for the render thread; and how long the render thread took on the
  frame before, rendering and waiting for the GPU and the display. The Performance panel, an
  overlay apps add, shows them, and a run that doesn't end in an error logs `engine.perf` as it
  ends: the frame times' percentiles, means and maxima, how long the run took to start, its first
  frame and its shutdown, the ticks it ran and dropped, with the GPU and the build, so that an
  agent can compare two builds by their runs. GPU timestamps for each pass come with the render
  graph.

## Events and layers

The engine turns the window system's events into its own: window (close, resize, focus, scale,
minimize), keyboard and mouse, and gamepads' from gilrs. winit and gilrs stay inside `gg-engine`,
so apps and games never see their types.

An app is a stack of layers, which its `main` describes on `App`: layers, then overlays above
them. Each layer attaches when the app starts and detaches when it ends, ticks with the
simulation, as [ECS](#ecs) describes, updates every frame with the time since the last, for what's
presented, handles events and draws UI. Frames pause while the main window is minimized, and the
first update after it's restored is given no time, rather than the length of the pause. An event
goes down the stack from the top overlay until a layer handles it, but for a key's or a button's
release, a gamepad's stick or trigger back at rest and the loss of focus, which go to every layer,
so that none is left holding what it saw pressed; a stick or a trigger that a layer handles away
from rest stays, for the layers below, as they last saw it until it rests. Ticks, updates and UI
go up from the bottom.
Headless runs have no window or gamepad events, but their layers tick, update and draw UI all the
same.

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
area goes to the layers. A panel shows from the frame after its layer starts showing it, since
the dock lays out a frame before its panels draw, and keeps its place while it isn't shown. The
layout is saved as RON in the OS's directory for per-user state, never
beside the executable; headless runs neither read nor write it.

The torn-off windows are egui's immediate viewports: egui runs each one's pass inside the main
window's, and the main window's frame renders them all. Only the main window's swapchain paces
the render thread, and the frames with it; another with no image free skips the frame. Their
events go to egui, never to the layers. On Wayland, where windows can't be placed, groups float
over the main window instead.

Layers add menus through `Ui` too. The OS draws the windows' frames, and the menus get a bar of
their own below its title bar, unless the app calls `App::custom_title_bar`, as tools do for the
room: then the UI draws them. The main window's title bar holds the menus, the title and the
buttons that minimize, maximize and close it, and a torn-off window's tab bar is its title bar,
with a close button. Dragging the main window's title bar has the OS move it, so snapping works
as usual, while a torn-off window moves with the dock's drag, so that it can dock. On Windows the
rest of the frame stays the OS's, through a window procedure in front of winit's: the border and
shadow, the bands just outside a window that resize it, and the main window's free space in its
title bar and its maximize button, which Windows hit-tests as its own, so that Snap Layouts shows
over the button. Elsewhere the UI's edges have the OS resize a window, and on macOS the OS keeps
its own buttons and the edges. Games keep the OS's frame. A menu quits by sending egui's `Close`
to the main window, which, like the OS's close button, gives the layers `CloseRequested` first;
egui's own Ctrl or Cmd+Q is off, since a game may use those keys.

Layers can also poll input, through their `Context` rather than a global, for what's presented:
which keys and mouse buttons are held, and where the cursor is. That state follows the events the
stack is sent, before any layer handles them, so input the UI keeps doesn't count, and a press
and release between two frames never shows as held. Losing focus lets go of everything, since not
every platform reports the releases that come while the window is out of focus, and a key
already down as it regains focus counts from its next press, or from its first repeat.

A game names what it does as actions, which a layer binds to keys, by their position, mouse
buttons and gamepads' buttons, sticks and triggers: buttons, axes, such as a key pair or a
trigger, and pairs of axes, such as a stick, which go as far diagonally as straight. Bindings add
up, and so does every gamepad; a stick or a trigger's travel can't be a button, which would press
and release as it wavered. The layer feeds its actions the events it's sent, so that a layer above
that handles a press hides it from them, and queues each change of an action's value as a command
of its own as the event comes, rather than at its update, so that the next tick applies it.
Repeats don't count, so a key already down as the window regains focus counts from its next press.
Layers run on the main thread with the event loop, and input enters the simulation only so, as
[ECS](#ecs) describes: a tick polls no input, and what an action holds, such as which way a player
is going, is a component that its commands set, so that a tap between two frames applies at one
tick, a hold at every tick until its release, and snapshots and replays keep it. A recording holds
the actions' changes, never keys or buttons, so binding them to others changes none.

Gamepads come through gilrs, in windowed runs alone. Each frame polls them as it starts, before
its ticks, which apply what they did, and gives a stick's or a trigger's moves as one event, at its
last reading, at the poll's end and before each of its pad's button events, so that a press sees
the moves that came before it. Nothing a pad does counts while the main window is out of focus,
on every platform, since Windows gives pads to the focused window alone and Linux and macOS to
any, nor for 40 ms after it regains it, while gilrs's threads read the pads again: losing focus
lets go of what the pads held, and what's away from rest as they count again counts once it has
rested, as a key counts from its next press.
Sticks and triggers have a round dead zone of a tenth of their travel, scaled so that they go from
exactly 0 at its edge to 1 at nine tenths, the same on every backend, with gilrs's own filters
off. A pad that disconnects lets go of what it held. If gilrs can't start, the run logs
`engine.gamepads` and goes on without pads.

## World space

World space is right-handed with +Y up, as glTF's is: a camera without rotation looks down -Z,
with +X to its right, as glTF's cameras do. glTF assets need no conversion as they're imported,
and scenes, physics and the editor share these axes.

## Rendering

Each frame the engine extracts that frame's render data, as plain data, which the render thread
hands `gg-renderer`: the color the view clears to, the game's view, the camera it sees the world
from, and what to draw in it, in order: meshes, each with a shader for its vertex type, the
parameters that shader takes, and a transform that places it in the world. The view is the main
window's central area, which the
dock leaves the game. It renders into an image of its own, the central area's size in pixels,
made again as that size changes, which the frame then shows there, with the UI over it, so that
depth, HDR and tonemapping, post-processing and the tool API's captures read the view whole; and
games' pipelines are made for that image's format, 8-bit sRGB, whatever the window presents.
Layers add shaders, meshes and textures, draw them and set the camera through their `Context`,
and the renderer's `Shaders`, `Meshes` and `Textures` hand out their handles at once, on the main
thread; each frame's render data carries what's been added to them since, in order. After the
frame's ticks and the layers' updates, the engine draws the world's entities that have a
`Transform` and a sprite or a `MeshDraw`, a mesh with its shader and parameters held without
their types, over what the layers drew, back to front by their z's in the world, children where
their ancestors carry them; those at one z draw sprites under meshes, each in the order of their
IDs, which overlapping entities keep as others come and go. That sorts the rows at each z, which
drawing keeps until a row or a z changes, and draws them out of their tables' order; so meshes of
different meshes or shaders at one z interleave, each change between them another draw, and
meshes meant to batch take a z of their own.

Frames draw the world between ticks, so that it moves as smoothly at any frame rate. As each tick
starts, the engine copies where the world's entities are, outside the world, so that no hash,
scene, snapshot or recording holds the copy; each frame draws every entity as far on from where its
last tick found it to where the tick left it as the frame is past the tick, its translation and
scale blended and its rotation the shorter way round. So drawing runs up to a tick behind the
simulation, besides the frame the render thread runs behind. A child blends its own transform and
each ancestor's, then composes them, so that it keeps to its orbit; and the order everything draws
in, by its z, is the tick's, so that blending never sorts again. An entity's own transform draws
where the tick left it, with nothing on the way, though its ancestors still carry it on theirs,
where there's nothing to draw on from or the way would mislead: spawned in the tick, given its
transform there, or put back by an undone delete; in a slot another entity had before; under
another parent; in a world restored or replaced in the tick, all of it; from a transform that
isn't finite, or with a scale whose sign changed, as a sprite mirrored has; and reset by the
tick, through `World::reset_interpolation`, as a teleport or a wrap is, which the world keeps for
each slot, copied with snapshots and read by no hash, scene or recording. A row a despawn moved
still blends. An entity despawned in a tick vanishes up to a tick before it reaches where it
stopped, and an Inspector set glides there over a tick, as a drag does. Headless frames, a tick
each, draw their tick as it leaves the world.

A game's shader is a Slang file that the engine compiles with slangc as a layer adds it, so one
that doesn't compile fails there, with slangc's diagnostics naming the file and line; the renderer
makes its pipeline the next time it renders. A mesh's vertices are any `#[repr(C)]` type without
padding, with glam's vectors and matrices among its fields, and the next frame that renders
copies them and the indices into GPU memory, where they stay. Shaders read vertices through their
buffer's address rather than through vertex input, and Slang lays out what a pointer points at as
`#[repr(C)]` does, so a mesh's layout is between its vertex type and the shader's struct for it;
draws take the indices. A shader is added for a vertex type and a type of parameters, such as a
color, which each draw gives it: a mesh draws only with a shader for its own vertex type, and
with parameters of that shader's type. Each frame copies the draws' transforms and parameters into
the frame's own memory, where the shader reads them through their addresses too, so parameters can
be as large as a shader needs. A shader is added with how it blends too, since Vulkan bakes
blending into its pipeline: opaque, or by the alpha it returns, not multiplied into its color.
Vulkan checks nothing about a shader, so its interface, which `Shaders::add` describes, is
trusted, and validation checks it in debug builds.

A game's texture is a PNG that the engine decodes as a layer adds it, to RGBA8 with colors
sRGB-encoded and alpha not multiplied into them, as images store them; the next frame that renders
copies it into GPU memory, where it stays, and shaders sample its colors decoded to linear. Texture
coordinates run from (0, 0) at an image's top left, as glTF's do, and Vulkan samples rows stored
top first that way, so images load as they're stored, without the flip OpenGL needs. A texture's
handle is its index in the bindless set, which a draw's parameters carry as it is and the shader
samples by. The set keeps most of its slots for textures placed at a given index, so that a
texture's handle is known before the texture reaches the GPU, and gives out the rest, such as the
UI's, wherever one is free. Shaders pick one of the engine's samplers by its index: nearest or
linear, clamped, repeated or mirrored.

A camera is a position, a rotation and a projection. An orthographic camera, as 2D games use,
sees as much of the world from bottom to top as the game sets, and as much across as the view's
aspect ratio makes, so nothing stretches as the dock resizes the view; perspective comes with
depth. Each frame the renderer pushes the camera's view and projection, column-major as glam's
matrices are, with the addresses of the vertices and of the draws' transforms and parameters: a
shader places each vertex by its draw's transform, into the world, then by the camera's. Layers
can also steer the camera with the engine's orthographic camera controller, which a layer keeps
and calls from its own update and event, not adding it to the stack, so that the game can still
place its camera itself or stop steering it.

2D games draw sprites, which are draws like any other rather than a renderer of their own:
`Context::draw_sprite` draws the engine's quad mesh with its sprite shader, whose parameters are a
color, a texture, the rectangle of it to show and the sampler to show it with, so that a sprite
sheet's frames and an atlas's images draw from one texture, and a rectangle past the texture's
edges tiles it under a repeating sampler. Every `Meshes`, `Shaders` and `Textures` starts with
the engine's own: the quad, the sprite shader, which slangc compiles as the engine builds, and a
white texture a pixel across, which a sprite of only a color shows, so that one shader draws
sprites flat and textured. They reach the GPU as games' do, and games' handles come after them.

A sprite sheet is a texture cut into cells of one size, packed edge to edge from its top left,
which `SpriteSheet` gives in pixels and turns into sprites' rectangles; with cells a pixel across,
it gives any rectangle of pixels, as an atlas holds images of different sizes. The sprite shader
samples only the texels a sprite's rectangle covers, wholly or in part, never nearer its edges
than their centers, so that neither a linear sampler nor a nearest one shows any of the cells
beside it, and sheets need no space between their cells. So a tiled sprite's outermost texels
don't blend with those they'd wrap to either, nor, filtered linearly, sprites side by side with
each other; games' own shaders sample as they choose. It holds while textures have one level and
no anisotropy, which will read past those texels.

Draws are batched, every mesh and not only quads: each run of consecutive draws of one mesh with
one shader is a single instanced draw, whose instances are the draws, and the frame copies the
run's transforms and its parameters into its memory, each in an array of its own, which shaders
read by the instance's index. A change of texture is a change of parameters, which bindless
textures make free, so 100,000 sprites take a handful of draws; shaders index their textures
through `NonUniformResourceIndex`, since the instances of a draw can carry different ones.
Nothing caps a run: the frame's memory grows to hold it, where a renderer with a buffer of fixed
size would draw what it holds and start another. The renderer counts each frame's draws and
instances, which the Performance panel shows and `engine.perf` logs the most of.

An error while rendering ends the renderer, since a failed frame can leave meshes and textures
half copied to the GPU.

## Threads

The main thread runs the OS event loop, which macOS requires, and the layers and their ticks.
Simulation runs on a job system: rayon-core's pool of `--threads` threads, by default one per CPU
but the one the render thread has, the main thread among them, which a tick's systems hand their
rows to through its `join` and wait for, so that the world changes only within a system's call. A
job changes only its own rows, work
splits at a fixed number of rows, and no thread changes its floating-point environment, so that
results don't depend on how many threads there are or which takes what; outside the pool, as in
tests and with `--threads 1`, jobs run one after another on the thread that calls. Steady-state
ticks allocate nothing on any thread. Drawing hands the job threads, too, the roots it blends
between ticks, in runs of rows, after the frame's ticks.

Rendering runs on a thread of its own, `gg-render`, a frame behind. Once a frame's UI is drawn
and tessellated, the main thread takes the renderer back from the render thread, waiting for it to
finish the frame before, and hands it over again with the frame's render data, owned: so the main
thread is at most a frame ahead, and the GPU works on up to two frames behind that. While the
renderer is back, the main thread tells it the windows' sizes, opens and closes windows'
swapchains, and at the end drops it: Windows and macOS give out a window's handle only on its own
thread, and macOS closes a window there, so the render thread never makes a surface or holds the
last reference to a window. One thread at a time calls the renderer. The frame's draws swap
between two buffers that keep their memory, and what layers added since the last frame goes with
it. The render thread rebuilds a swapchain whose window changed size as it acquires the next
image. On Windows the main thread answers what's sent to its windows while it waits, since a
swapchain's acquire, present and rebuild may send the window's thread a message and wait for the
answer.
An error while rendering comes back with the renderer and ends the run, and a panic resumes on the
main thread.

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
| Math | `glam`, with its `scalar-math` and `libm` features; `libm` |
| Jobs | `rayon-core`'s thread pool and `join`; not rayon's parallel iterators, whose splits and reductions follow the number of threads |
| ECS | Our own; see [ECS](#ecs) |
| Logs and profiling | `tracing` for logs; Tracy, through `tracy-client`, for CPU zones and GPU timestamps |
| Agent access | MCP and a command-line tool, both clients of the [tool API](#agents-and-tools) |
| Shaders | Slang, compiled to SPIR-V by the Vulkan SDK's `slangc` rather than linked in: the engine's own as it builds, games' as they're added |
| Assets | glTF 2.0 in; meshoptimizer for meshes and meshlets; BC-compressed textures in KTX2; until cooking, PNGs that the `png` crate decodes as layers add them |

Areas not listed here are undecided.
