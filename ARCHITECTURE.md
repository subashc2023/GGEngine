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
- **Grown with games.** The engine grows alongside games of its own: small demos first, each
  built to exercise part of it, the first a simple networked game that the v0.1 loop is tested on;
  then a flagship, picked from the demos, whose needs set the engine's priorities and whose
  shipping proves the engine.
- Rendering the way current AAA engines do it: bindless resources, a render graph and GPU-driven
  rendering.
- Simulation and rendering pipelined across threads.
- An offline asset pipeline that cooks content for a shipping player.
- Windows and Linux on x86-64 and ARM64, and macOS on Apple Silicon. Consoles, mobile and the
  web wait until the desktop engine is solid.

## Crates

```text
sandbox  editor   player       apps/: executables          gg  tools/: tools
       \    |    /                                         |
        gg-engine - gg-macros  crates/: libraries          |
            |     \_______________________________________ gg-api
       gg-renderer
            |
          gg-gpu
            |
         gg-core               every crate may use gg-core
```

- **`gg-core`:** math, logging, profiling and error types.
- **`gg-gpu`:** Vulkan through `ash`. It turns ash's unsafe API into safe types: device, memory,
  buffers, images, descriptors, pipelines, swapchain, synchronization and submission. They stay
  safe whatever safe code does with them: each checks that what it's given was made by its device,
  or a surface by its instance, and a frame or a pass forgotten rather than dropped is caught at
  the next call.
- **`gg-renderer`:** the render graph, passes, materials, lighting and post-processing. It owns
  the GPU resources and hands out handles to them.
- **`gg-engine`:** the main loop, window and input, the job system and the render thread, scene
  and assets, plus hot reload and the tool API in development builds: what apps and games build
  on.
- **`gg-macros`:** the derives `gg-engine` re-exports beside its traits of the same names,
  `Reflect` and `Component`. Rust builds a derive as a crate of its own; only `gg-engine`
  depends on it, and games derive through `gg-engine`.
- **`gg-api`:** the [tool API](#agents-and-tools)'s protocol: its messages, the session file that
  says where a game serves it, and a client, which tools speak it through. It depends on no `gg-*`
  crate, so that tools build without the engine, and `gg-engine` uses it in development builds
  alone.
- **`sandbox`:** the development playground for engine features.
- **`editor`:** the editor, one more client of the tool API: a dock that fills its window, with
  the game's view in a panel among the others, and an orbit camera of its own. Its Play and
  Simulate run a copy of the world in the world's place, and Stop brings back the very world as it
  was edited, so that outside them the world changes only by edits, which undo. Play, Simulate and
  Stop are commands, which recordings keep. [main.rs](apps/editor/src/main.rs) describes the rest.
- **`player`:** the reference player. It runs a cooked scene without game code and shows how a
  game builds its own player.
- **`tools/gg`:** the command-line tool, a client of the tool API, which `cargo gg` runs.

Games depend on `gg-engine` the same way the apps do; see [Game code](#game-code). Executables are
named `gg-` and the app, such as `gg-sandbox`: GPU drivers apply per-game settings by executable
name, and a generic name can match one.

### Layering rules

1. **Dependencies point down only.** Each crate declares a dependency on the crate below it,
   even before it uses it, so that reaching up closes a cycle, which Cargo rejects. Cargo
   allows cycles through dev-dependencies, so don't use them to reach up.
2. **`gg-core` is the leaf:** no `gg-*` dependencies, and nothing GPU- or window-specific.
3. **`gg-gpu` owns Vulkan** and knows nothing about meshes, materials or scenes. A separate
   device layer keeps the unsafe surface small and reviewable, and lets rendering techniques
   change quickly on safe types.
4. **`gg-renderer` never sees the scene.** Each frame the engine extracts plain
   [render data](#rendering) and hands it over. So rendering runs on its own thread, a frame
   behind simulation, as [Threads](#threads) says, and renderer tests run without the engine.
5. **The engine reaches the GPU only through `gg-renderer`.**
6. **`main` is thin.** An app's or a game's `main` describes it and hands control to `gg-engine`, so
   there is no entry-point crate. Android is the exception: its `android_main` lives in a `cdylib`,
   which gets a small crate of its own.

Cargo's cycle check enforces rule 1, and rule 2's ban on `gg-*` dependencies, for every crate but
`gg-macros`, which depends on no `gg-*` crate, so that depending on it closes no cycle. Nor can it
see a skip-level edge such as `gg-engine` → `gg-gpu`, which closes none either. So
[deny.toml](deny.toml) names the only crates that may depend directly on each `gg-*` crate but
`gg-core` and `gg-engine`, and on each library one crate owns, such as `ash`, `winit` or
`rapier2d`, and CI checks it with cargo-deny. Rule 4, and what a crate does with the dependencies
it may have, are left to review.

### Builds

`gg-engine`'s development tooling, its debug UI (the dock, the Inspector and the panels) and the
tool API included, belongs behind its `dev` feature, which development builds turn on, as the
sandbox does, and shipping builds never do. [Its Cargo.toml](crates/gg-engine/Cargo.toml) says what
`dev` gates. Shipping builds run cooked data only. Cargo merges the features of every package a
command selects, so `--workspace`, a bare `cargo build` and `--bin gg-player` would all hand the
player `dev`. Shipping builds therefore select the player alone, with `-p player`, and a game's
shipping build selects the game alone the same way; the shipping gate, `cargo clippy -p player`,
checks `gg-engine` as they build it.

Tracy is a feature of its own, `tracy`, off by default and in no shipping build: it compiles
Tracy's C++ client, which listens on the machine's loopback and keeps what it records in memory
until a viewer connects. Apps and games turn it on through `gg-engine`'s, as the sandbox does.

Vulkan validation is a runtime switch rather than a feature, on by default in debug builds, so
tests run with it whatever the features.

### Growth

A module becomes its own crate once it has its own responsibility and a second user, not
before:

- `gg-assets`: cooked asset formats, shared by the cooker and the runtime.
- `tools/cook`: the offline asset cooker.
- `gg-scene`, `gg-physics`, `gg-audio` and the like: when each outgrows a module of `gg-engine`.

## Game code

Game code is Rust, as fast and as unrestricted as the engine's own. Each game is a crate that
depends on `gg-engine` and builds its own player, statically linked and optimized for shipping. Game
crates forbid `unsafe`, so a bug in game code panics rather than corrupting memory. Games built
alongside the engine, the demos and then the flagship, live in `games/` in this workspace, so one
change can span engine and game, and `games/*` joins the workspace members with the first one;
anyone else's game depends on the engine from its own repository.

- **The game runs in its own process.** The development tools launch the game's development
  build and work on it through the [tool API](#agents-and-tools). A crash, panic or hang in
  game code stops only the game, and the tools restart it from its last snapshot. The same
  split gives dedicated servers, local multiplayer tests with a server and several clients,
  and headless runs in which agents check their work. A game the editor runs shows in the
  editor's view, and one run without it in its own window.
- **Hot reload keeps the world.** `subsecond` patches an edit inside a function body into the
  running game, which reaches calls made through its `HotFn` but none through a vtable or a
  function pointer made before the patch. The engine calls each layer's methods through `HotFn`
  and applies patches between ticks, so that a patch reaches those methods and the systems and
  scripts they call; an edit that only a game's own trait object or function pointer reaches
  waits for the next rebuild. Any other change, such as a new field, a new component or an
  engine edit, rebuilds the game, restarts it and restores the world from a snapshot taken just
  before, matching each changed component to its new layout by field name.
- **Luau comes after v0.1, for mods and quick scripts.** Players can install sandboxed mods,
  and quests, dialogue and level events reload in milliseconds. Scripts reach Rust components
  through reflection, without hand-written bindings, and can declare components of their own.
  Luau doesn't guard against crafted bytecode, so mods ship as source and only a game's own
  packages hold bytecode.

## Agents and tools

Agents work on a game the way people do, through the same interfaces: nothing a person can do
in the editor is out of an agent's reach.

- **The tool API.** A game's development build serves an API on the machine's loopback, at a port
  the OS picks, which it writes with a token in a session file in `sessions` beside its
  executable, removed as the run ends: query the world, apply and undo
  [operations](#project-state), run, pause and step, capture frames, and read
  [diagnostics](#diagnostics). Requests and their answers are lines of JSON, as
  [gg-api](crates/gg-api/src/lib.rs) describes, and a connection's first request says hello with
  the token, so that only processes that can read the file reach the game. A thread of the API's
  reads each connection and answers its pings itself, from the frames the main loop counts, so
  that a tool tells a hung game from a busy one, and from one whose frames have paused for the
  user, as for a minimized window or a file dialog. The main loop answers the rest as each frame
  starts, before its ticks: a step once its ticks have run, and a capture once the render thread
  has read the frame's view back, without the UI, which a thread of the API's writes as a PNG.
  Paused, frames run no ticks but those a step asks for, and draw the world as the last tick left
  it, so that a capture shows what a query reads. Each client gets only the capabilities it is
  granted, such as reading the world or editing one scene.
- **Clients.** The command-line tool, `tools/gg`, builds and runs games, supervising them and
  telling when one crashes or hangs, runs headless checks, and talks to running games, all with
  machine-readable results; an MCP server gives agents that speak MCP the same reach; the editor
  is one more client, with no ability an agent lacks.
- **Headless runs.** Every app runs without a window (`--headless`) and can run a set number
  of frames, then exit with a status (`--frames N`): 0 for a clean run, 1 after an error or any
  validation message, and 2 for bad arguments. Dedicated servers, CI and agents checking their
  work run this way; rendering checks render offscreen, and simulation-only runs need no GPU.
  Under RenderDoc, a run can capture a frame ([Captures](#diagnostics)), which
  `renderdoccmd convert` writes out as XML for agents to read.

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
  reads back to its bits, never an exponent, with `.0`, and `inf` and `NaN` keeping their sign,
  though not a NaN's payload; a string by escapes of the scene's own for every character that
  draws nothing or changes how the text around it reads: Unicode's controls, format characters,
  line and paragraph separators and default-ignorable code points; names ASCII identifiers; at
  most 64 levels deep. The engine writes the text itself, so that the bytes depend on no
  library's formatting, line endings or Rust release, and `ron` only reads it, through seeds
  that reflection's descriptions drive, never its untyped values. What's opaque, such as a sprite's
  texture or a `MeshDraw`, isn't saved: a load builds each component on its type's
  `Component::base`, which gives it that, or from the scene alone. A child's `Parent` is saved as
  its parent's ID, and a root's isn't. A save makes each component again as a load would, so that
  every scene saved loads; a load checks everything before it spawns anything, all or nothing, and a
  refusal begins with the line and column where the text is refused, and, but for a syntax error,
  names the entity, the component and the field.
- **Prefabs.** A prefab is a scene of one subtree, placed as a copy under a parent. Each of the
  copy's IDs is mixed from one drawn for the copy and the prefab's own, `mix64(local ^
  mix64(instance ^ PLACE))`, a permutation of the prefab's IDs for each copy, fixed for good, so
  that a copy's IDs are as distinct as its prefab's, and two copies' meet only by chance;
  references inside the prefab are to the copy's own entities. A scene saves the copies as
  entities like any others.
- **Stable IDs.** Every entity has a persistent ID, separate from its runtime handle, that
  files, operations, diagnostics and network messages use to name it. IDs are random rather
  than sequential, so entities added on two branches neither collide nor land on the same
  lines.
- **Operations.** Every authoring change, by a person or an agent, is an operation with an
  inverse: create, duplicate or delete an entity, add or remove a component, set a field,
  reparent. An operation names entities by their IDs and carries values as reflection's, and
  applying one checks everything before anything changes. A delete's and a remove's inverses keep
  what they took out, typed, so that what's opaque comes back too. A component can name a `bool`
  field of its own exclusive, as a camera's `primary` is, so that one edit, a person's or an
  agent's, switches which entity has it, and one undo switches it back. The log keeps the
  operations applied, in order, with their inverses: it undoes and redoes them, merges a
  gesture's sets of one field, such as a drag's, into one edit, and drops an entry whose undo the
  world refuses. A running game applies operations at its ticks, in the order they arrive, so
  several clients can edit it at once, and their log gives undo, review and replay. Editing a
  scene file while the game runs is an edit too: the engine applies the difference as operations.
  [ops.rs](crates/gg-engine/src/ecs/ops.rs), `World::undo` among it, and
  [log.rs](crates/gg-engine/src/ecs/log.rs) have the rest.

## ECS

The world lives in our own ECS, built for what the tools, hot reload and multiplayer need:

- **Reflection:** every component type, whether Rust or a Luau script declares it, is registered
  at runtime with a description of its fields. Saving, operations, the inspector, hot reload's
  snapshots, network replication and Luau's bindings all work from it.
- **Snapshots:** the whole world can be saved and restored fast enough to take one every
  frame, for rollback and client-side prediction as well as hot reload.
- **Commands:** nothing outside the simulation changes its state except a command applied at a
  tick: player input, a network message or an [operation](#project-state). Each layer queues its
  own, which the engine reads and fills as reflection's values at each tick through
  `Layer::commands`. A recorded run (`--record`) writes its seed and each layer's commands just
  before its tick applies them, with the entities they hold by their IDs, a line at a time, so
  that a run that crashes or is killed keeps what it ran. A replay (`--replay`) runs the ticks
  again alone, without a window or a GPU, each layer's with the commands recorded for it in place
  of its own, for bug reports an agent can rerun; it refuses a recording of another format, app
  or stack of layers, and replays one from another build, naming both builds where it goes
  another way. A snapshot and the commands after it replay from there, for rollback; so a command
  is data, never a closure. A command its replay couldn't make again, as one holding something
  opaque, which no value makes, ends the recording before it, and the run with `record.bad_command`.
- **Determinism:** the fixed timestep, a fixed order of systems and of iteration, and math that
  gives identical results on every platform, so lockstep games and rollback netcode stay in sync.
  So glam is built without SIMD and with libm's functions, for the whole build
  ([Cargo.toml](Cargo.toml)), and clippy refuses std's float methods whose precision is the
  platform's ([clippy.toml](clippy.toml)), for `gg_core::math`'s. A recorded run hashes the world
  once its layers attach and after every tick, so that its replay, on another machine or after a
  change, finds the first tick where it goes another way. `World::difference` names the entity
  and field where two worlds part in one process; across two machines, `--dump` writes the world
  there a field a line, and the two dumps diff to them.

The world is a `World` of entities and their components, in safe Rust. An entity is a handle,
its slot and which use of the slot it is, so that a despawned entity's handle never names
another, unless a restore takes the world back to before it was spawned. A delete operation
leaves its entities' slots vacant rather than free, their uses as they were, until its inverse
puts them back or is dropped, so that an undone delete's entities have their handles again.
A component is a struct or an enum that derives `Component`, which describes it to reflection;
it's `Clone`, for snapshots, and `Send` and `Sync`, for the job system. Components live in
archetype tables, one for each set of component types, whose rows are its entities and whose
columns are each type's own `Vec`, which the world reaches through a trait object, downcast once
a table, rather than through raw pointers. Queries borrow the world, shared to read it and
exclusively to change it, so that the borrow checker keeps them apart, and walk each table's
columns as slices. Systems are plain functions over the world, run at its ticks in a fixed order;
one can hand a query's rows to the job system's threads with `par_for_each`, as [Threads](#threads)
describes. A native script is a component that acts: a type that implements `Script`, which a
layer's tick runs among its systems with `run_scripts`. Its state is its component, so that scenes
save it, the Inspector and operations edit it, the hash reads it and snapshots copy it, and adding
the component by name, as an agent's operation does, attaches the behavior; and it reads that state
through the world rather than as `self`, so that what it reads is what the world holds. Since each
gets the whole world, scripts run one entity at a time on the tick's thread, and what a game has
thousands of is a system. Each type's pass is a generic function the layer's tick calls, with no
table of functions, so that subsecond's patches reach scripts as they reach their layer, as
[Game code](#game-code) describes. The order queries find entities in follows only from the changes
made, and no hash map is iterated, so it's the same in every run, as determinism needs, but means
nothing a game may rely on: anything ordered, such as what's drawn over what, orders by its
components' values, or by its entities' IDs.

Each entity has a persistent ID, an `EntityId`: a nonzero 64-bit number. Each world draws its IDs
from a stream of its own: SplitMix64, which gives no ID twice until it's reseeded, seeded from the
OS's randomness, so that two sessions' IDs meet only by chance, as a world's on either side of a
reseed do; a run that must go the same way every time seeds it, as the sandbox's reference run
does. A map hashed with SipHash, keyed at random, so that IDs from files can't be chosen to
collide, finds an entity by its ID.

An entity can have a `Name`, a string for people, which the Entities and Inspector panels show it
by and scenes save. It's optional, so that entities that come by the thousand, as a swarm's or a
game's bullets do, hold none and spawn without allocating, and it isn't unique: files, operations
and diagnostics name an entity by its ID.

Reflection names each component type by its identifier, unless `#[reflect(name = "...")]` gives
another, and two types of one name are an error rather than a guess. The engine's own components
register as it starts, so that their names are taken first, and a game's the first time an entity
has one. Values cross as a closed `Value`: integers of fixed sizes only, so that a value means the
same on every machine; `f32` apart from `f64`, so that bits come back as they went; an entity as
its handle, as components hold one, which scenes, the tool API and replication name by its ID;
and `Opaque` for what reflection keeps but doesn't read, such as a texture's handle, which no
value may make. The world reads and sets a field by its component's name and a path, such as
`rect[1].y`: a set checks the whole value against the description before anything changes, and
returns the old value, which sets it back unless code stored one that a set refuses. The Inspector
edits the world through these calls and operations alone, which the tool API serves, so that an
editor can show it for a game in another process.

A field's default is the one `#[reflect(default = ...)]` gives, or else its type's, so that
adding a component, switching an enum to another variant and resetting a field each start from a
value no one had to give whole. Some types have none (`reflect.no_default`): an entity, since a
handle to none can't be saved or set, so a component holds one in an `Option` or a list to have a
default; a number whose bounds leave none of its type; an enum without variants; and a type whose
default holds one of its own without end, which only a description written by hand can give.

Entities form a hierarchy, as scenes, prefabs and cameras that ride entities need: a child has its
parent as its `Parent` component, and its `Transform` is relative to its parent's, so that a
parent carries its children along, and despawning an entity despawns its descendants. The world
keeps the hierarchy whole: once an entity is spawned, only `World::set_parent` gives, changes or
takes away its `Parent`, which isn't `Writable`, so that code changing it any other way doesn't
compile, and `set_field` and operations refuse it by name. A parent's children are in the order
they became its children, which means no more than the order queries find entities in. World
transforms aren't stored: drawing computes each child's from its ancestors, every frame, and
`World::pose` an entity's for code, as the latest tick left it.

A snapshot is the world copied: `World` is `Clone`, and `clone_from` restores a world from a
snapshot, or takes a snapshot into one kept from before, in the memory it holds, so that one taken
every frame allocates nothing while the world's tables, its component types and its size stay, for
components and resources whose `Clone` allocates nothing. It copies all of the world, so that a
restored world goes on exactly as the world the snapshot was taken from did, spawning the same
handles with the same IDs and finding entities in the same order. A copy is another world, with a
serial of its own, which refuses what undoes an operation applied before, so that undo history
doesn't outlive a restore.

A world holds resources beside its entities, a value of each type, for the rest of the simulation,
such as 2D physics' bodies and contacts. Snapshots copy them, so that a restored world steps as the
one it was taken from did; and no scene, hash, recording or dump holds them, so a resource holds
what the simulation carries on from, or makes again from the entities, as physics makes its bodies.

A world can be emptied, or have its entities replaced with a scene's, as the editor's New and Open
do: `World::clear` and `World::replace_scene` free every slot, the vacant ones a log keeps too,
and keep the component types, the resources and the stream of IDs, so that no ID is drawn twice.
The world is then another, as a restored one is, with a serial of its own (`World::serial`): it
refuses what undoes an operation applied before, which is what makes freeing the slots a log kept
safe. Whoever keeps state of a world keeps it by its serial, as the Inspector keeps its log, its
pick and its open rows, so that it comes back only with the very world it's of, as when the
editor's Stop brings back the world a copy played in place of. The Inspector puts aside only the
world it left last, so a layer that puts one copy in place of another brings back the world
they're of at a tick between them, as the editor's switch between Play and Simulate does.

A world's hash has two parts: its content, what it holds, each entity's components under its ID
and their types' names, summed, so that where the world holds them doesn't count; and its layout,
how it holds them, which the order queries find entities in follows from. Neither reads slots,
handles, the order types registered in or the resources, so that a scene loaded into another
world has the content it had. Undoing a log gives back the content, though not always the
layout.

The simulation steps in ticks, 64 a second whatever the frame rate, each 15.625 ms, exact in
`f32`, `f64` and nanoseconds. A frame runs the ticks the time since the last one owes before the
layers' updates: at most 8, and no more once its ticks have taken longer than the time they
simulate, so that a simulation slower than real time slows down rather than spending every frame
catching up. The whole ticks a frame doesn't run are dropped, and counted in `engine.perf`, and
the fraction of one left over carries to the next, as how far the frame [draws](#rendering) the
world on its way from where the last tick found it. A frame without time runs none, as the first
does and the first after frames pause, and a headless run takes exactly one each frame, so that a
run of a set number of frames runs as many ticks on any machine. Each layer's tick applies the
commands it queued since its last, in the order it queued them, then runs its systems, the layers
from the bottom up. Once they've attached, the world changes only there: `Context::world_mut`
panics outside attaching and detaching, and a tick is given the world but not the `Context`, so
that it goes by the world, its commands and what its layer set up, never by the frame's time,
input or drawing. A replay runs the ticks with no update, UI or event between them, so a layer
keeps between ticks only what it set up as it attached and its queue, and the world holds the
rest of the simulation, as a snapshot restores it. The Inspector's log is the exception, which a
replay from the attach rebuilds from the recorded edits; it can't be a component, since its
inverses hold typed columns tied to the world they were made in. Operations are the commands of
authoring: the Inspector's edits, undos and redos wait for its next tick, and another layer picks
and edits through its link, `Inspector::link`, as the editor's view and handles do.

## Diagnostics

Diagnostics are data, so agents and tools can act on them as reliably as people read them.

- **Logs** go through `tracing` with structured fields, filtered by `RUST_LOG`. Each run prints
  readable, colored lines to stderr, which keeps stdout for machine-readable output, and writes
  every event as a JSON line to a file that agents and tools read:
  `logs/<app>-<UTC time>-<process ID>.jsonl` beside the executable, whose path the run logs as it
  starts.
- **Errors and warnings name their subject.** Each carries a stable code, a dotted name such as
  `gpu.device_lost`, and the thing it is about: an entity's ID, or a despawned entity's handle,
  since it has none; an asset path, a render pass, a shader file and line, or a scene's line and
  column. What outlives an entity, such as a pick or a queued message, keeps its ID, not its handle.
- **Validation messages** become events with their VUID and the debug names of the objects
  involved. A run whose validation layer can't validate synchronization warns
  `gpu.no_sync_validation`, naming why: the layer takes no settings (`VK_EXT_layer_settings`), or
  RenderDoc's layer hides them. A panic is logged as `log.panic`, with its location, before the
  process ends, even under `RUST_LOG=off`.
- **Captures.** A run that RenderDoc's layer is active on, as one `renderdoccmd capture` starts,
  captures its Nth frame with `--capture N`, through RenderDoc's in-application API, headless
  too, and logs the capture's path; asked to without the layer, it doesn't start
  (`renderer.no_renderdoc`), and one that ends first warns `engine.not_captured`. Everything the
  engine makes shows in a capture by its debug name, but a torn-off window's swapchain images that
  the frame doesn't render into, which keep RenderDoc's own names, since RenderDoc records only the
  captured window's images and those the frame uses. RenderDoc reports Vulkan 1.2 for a GPU without
  `bufferDeviceAddressCaptureReplay`, whose buffer addresses it can't capture, and the engine's
  refusal of that GPU says so.
- **Profiling.** Code marks its work with zones (`gg_core::zone!`), which Tracy lays out on a
  timeline of frames, with plots such as each frame's draws; without the `tracy` feature they
  compile to nothing. Each pass of the render graph has a GPU timestamp too, which Tracy shows
  beside the zones. The engine also measures every frame itself, always, part by part, on the main
  thread and on the render thread: the Performance panel, an overlay apps add, shows the parts, and
  a run that doesn't end in an error sums them up in `engine.perf` as it ends, with how long it took
  to start and to stop, the ticks it ran and dropped, the GPU and the build, so that an agent can
  compare two builds by their runs.

## Events and layers

The engine turns the window system's events into its own: window (close, resize, focus, scale,
minimize), keyboard and mouse, and gamepads' from gilrs. winit and gilrs stay inside `gg-engine`,
so apps and games never see their types.

An app is a stack of layers, which its `main` describes on `App`: layers, then overlays above them.
Each layer attaches when the app starts and detaches when it ends, ticks with the simulation, as
[ECS](#ecs) describes, updates every frame with the time since the last, for what's presented,
handles events and draws UI. Frames pause while the main window is minimized, while a file dialog is
open, and on Windows while the window menu that the UI's title bar opens is open, which the OS runs
in a loop of its own; the clock restarts after each, so that neither the frame times nor the next
update count the pause. An event goes down the stack from the top overlay until a layer handles it,
but for a key's or a button's release, a gamepad's stick or trigger back at rest and the loss of
focus, which go to every layer, so that none is left holding what it saw pressed; a stick or a
trigger that a layer handles away from rest stays, for the layers below, as they last saw it until
it rests. Ticks, updates and UI go up from the bottom. Headless runs have no window or gamepad
events, but their layers tick, update and draw UI all the same. A layer that keeps `Layer::ui`'s
default draws none, and an app none of whose layers draw UI, whose windows the OS frames, and which
has no view panel, runs without egui from its second frame: its frames collect no input for egui and
run no pass, egui sees no events, and the game's view fills the window.

egui, the engine's immediate-mode UI, is the top overlay: it sees every event first and keeps those
it wants, such as a click on one of its panels, but no key's or button's release, and `gg-renderer`
draws its meshes over the frame.

Layers draw UI through `Ui`: dockable panels, and egui itself for anything else. The dock is the
engine's own, since egui's docking crates can't put a panel in a window of its own. Panels sit in
tab groups, docked in strips along the main window's edges around a central area that stays the
game's, or floating: over the main window, or torn off into a window of their own, which holds one
group. egui counts only the strips and groups as its own, so input over the central area goes to the
layers. Tools call `App::view_panel` instead, for a dockspace: the dock fills the main window,
without a central area, and the game's view is a panel of its own, which docks, tabs and floats over
the main window as the others do, but never goes into a window of its own, since only the main
window's input reaches the layers. What reaches the layers is then the view's to say, as
`App::view_panel` details: presses and the wheel over it, and the drags they start; and keys while
it has focus, which a press on it gives, or the cursor is over it, so that keys follow the cursor
onto the view. Layers know where the view is from `Context::view`, in the window's pixels, as the
cursor's position is.

A panel the layout doesn't hold yet floats, unless the app docks it with `App::dock`, which is how
an app makes its first layout. The layout is saved as RON in the OS's directory for per-user state,
as are the layouts saved by name, never beside the executable; headless runs neither read nor write
layouts.

The torn-off windows are egui's immediate viewports: egui runs each one's pass inside the main
window's, and the main window's frame renders them all, paced by the main window's swapchain alone.

Layers add menus through `Ui` too, and controls at the middle of the main window's title bar, in
place of its title, through `Ui::title_bar`. A button at the bar's left opens the menus in a drawer.
The OS draws the windows' frames, and the menus and controls get a bar of their own below its title
bar, unless the app calls `App::custom_title_bar`, as tools do for the room: then the UI draws the
windows' title bars, the main window's with buttons that hide and show the panels left of, below and
right of the view; and the dock adds a View menu, which opens and closes panels and saves, loads,
deletes and resets layouts. On Windows the rest of the frame stays the OS's, through a window
procedure in front of winit's, and on macOS the OS keeps its own buttons and edges. Games keep the
OS's frame.

In development builds a layer asks for a file to open or save as through the OS's own dialog,
`Context::pick_file`, through rfd, modal over the main window: it blocks the frame that asks, at a
cost [DEFECTS.md](docs/DEFECTS.md) weighs. A headless run has no dialog: it picks nothing.

Layers can also poll input, through their `Context` rather than a global, for what's presented:
which keys and mouse buttons are held, and where the cursor is, as the events the stack is sent have
it, before any layer handles them.

A game names what it does as actions, which a layer binds to keys, mouse buttons and gamepads and
feeds the events it's sent, queuing each change of an action's value as a command of its own as the
event comes, so that the next tick applies it: input enters the simulation only so, as [ECS](#ecs)
describes. What an action holds, such as which way a player is going, is a component that its
commands set, so that a tap between two frames applies at one tick, a hold at every tick until its
release, and snapshots and replays keep it. A recording holds the actions' changes, never keys or
buttons, so binding them to others changes none.

Gamepads come through gilrs, polled as each frame starts, before its ticks, which apply what they
did; a pad that disconnects lets go of what it held, and if gilrs can't start, the run logs
`engine.gamepads` and goes on without pads. Losing focus lets go of everything held, keys, mouse
buttons, actions and pads alike, since not every platform reports the releases that come while the
window is out of focus; and nothing a pad does counts while the main window is out of focus, on
every platform.

## World space

World space is right-handed with +Y up, as glTF's is: a camera without rotation looks down -Z,
with +X to its right, as glTF's cameras do. glTF assets need no conversion as they're imported,
and scenes, physics and the editor share these axes.

## Physics

2D physics is rapier2d's, in gg-engine's `physics` module, and games reach it through components: an
entity with a `Transform` and a `RigidBody2D` is a body, moving in the XY plane, and a
`BoxCollider2D` or a `CircleCollider2D` beside it, or both, is the shape it collides as, in its
entity's space, so that it turns and scales with it; `step_physics` is a system a layer's tick runs.
rapier2d is built with `enhanced-determinism`, so that a step gives the same bits on every platform.
Physics' state, rapier's bodies, colliders and contacts and what sleeps, is the world's `Physics2D`
resource, which snapshots copy, as [ECS](#ecs) describes: bodies made again from their components
alone wouldn't step as the world did. A body under a parent isn't moved, since its transform is in
its parent's space (`physics.child_body`).

`Context::set_colliders_shown` outlines the world's colliders where their entities draw, in lines,
which draw over the rest of the view but where an opaque mesh is nearer, as [Rendering](#rendering)
says, rather than nudged toward the camera: so they show from behind a sprite as from in front of
it, since sprites test depth without writing it.

## Rendering

Each frame the engine extracts that frame's render data, as plain data, which the render thread
hands `gg-renderer`: the color the view clears to, the game's view, the viewpoint it sees the world
from, and what to draw in it, in order: meshes, each with a shader for its vertex type, the
parameters that shader takes, and a transform that places it in the world. The view is the main
window's central area, which the dock leaves the game, or a dockspace's view panel. It renders into
an image of its own, that area's size in pixels, made again as that size changes, which the frame
then shows there, a pixel to a pixel, so that HDR and tonemapping, post-processing and the tool
API's captures read the view whole: under the whole UI, or, from a panel, where the UI paints a
callback that stands for it, in the UI's order and clipped as the UI clips it, so that the panels
floating over it and the menus show over it; and games' pipelines are made for that image's format,
8-bit sRGB, whatever the window presents, for the image of IDs beside it, below, and for the depth
image beside them, of 32-bit floats, which every draw into the view tests. Depth runs reversed, from
1 at the near plane to 0 far away, which keeps a float's precision where it's needed and lets
perspective see infinitely far; each frame clears it to 0, and a draw shows where it's as near as
what's drawn there or nearer, so that at one depth the later goes over, as 2D draws at one z stack.

Layers add shaders, meshes and textures and draw them through their `Context`, and the renderer's
`Shaders`, `Meshes` and `Textures` hand out their handles at once, on the main thread; each frame's
render data carries what's been added to them since, in order. After the frame's ticks and the
layers' updates, the engine draws the world's entities that have a `Transform` and a sprite, a
circle or a `MeshDraw`, a mesh with its shader and parameters held without their types, after what
the layers drew, children where their ancestors carry them: first the meshes whose shaders are
opaque, whose depth hides what's behind them whatever order they draw in, by shader, then mesh, so
that each mesh with each shader is one instanced draw, and their z's never sort them; then what
blends, back to front by their z's in the world, those at one z drawing sprites, then circles, under
meshes; each batch and each z in the order of their IDs, which overlapping entities keep as others
come and go. World z orders what blends rightly as seen from in front of layers parallel to XY, as
2D cameras see them; from behind or the side, what blends draws in the wrong order among itself,
though rightly against what's opaque. At one depth, an opaque mesh goes under what blends; a layer's
opaque draws hide the world's behind them, and its draws that blend go under the world's opaque ones
behind them.

Every draw into the view writes its ID beside its color, into an image of IDs the view's size, a
64-bit number as two 32-bit halves, which each frame clears to 0: the world's draws their entities'
IDs, and a layer's own draws 0. IDs don't blend: a draw's replaces what's under it wherever it
shows, so a shader that blends discards what's fully transparent, as the engine's do, for what's
behind it to show through to picks too. `Context::pick` asks which entity the view shows at a pixel,
and the answer comes back about four frames later, once the GPU has rendered the frame that reads
the pixel back. The view renders its IDs every frame, picks or not: a pipeline is made for the
targets its pass has, so leaving them out of frames that pick nothing would take a second pipeline
for each shader, or `VK_EXT_dynamic_rendering_unused_attachments`.

`Context::outline` outlines what the view shows of an entity for a frame, as the editor outlines
what's picked, from the same IDs: a pass samples the image of IDs, which has a slot in the bindless
set, and draws over the view, under the UI, round whatever shape the entity shows, its colliders'
outlines among it, going across what's in front of it rather than round what's hidden.

Frames draw the world between ticks, so that it moves as smoothly at any frame rate. As each tick
starts, the engine copies where the world's entities are, outside the world, so that no hash,
scene, snapshot or recording holds the copy; each frame draws every entity as far on from where its
last tick found it to where the tick left it as the frame is past the tick, its translation and
scale blended and its rotation the shorter way round. So drawing runs up to a tick behind the
simulation, besides the frame the render thread runs behind. A child blends its own transform and
each ancestor's, then composes them, so that it keeps to its orbit; and the order everything draws
in is the tick's, so that blending never sorts again. An entity's own transform draws where the tick
left it, with nothing on the way, though its ancestors still carry it on theirs, where there's
nothing to draw on from or the way would mislead: spawned in the tick, given its transform there, or
put back by an undone delete; in a slot another entity had before; under another parent; in a world
restored or replaced in the tick, all of it; from a transform that isn't finite, or with a scale
whose sign changed, as a sprite mirrored has; and reset by the tick, through
`World::reset_interpolation`, as a teleport or a wrap is, which the world keeps for each slot,
copied with snapshots and read by no hash, scene or recording. A row a despawn moved still blends.
An entity despawned in a tick vanishes up to a tick before it reaches where it stopped, and an
Inspector set glides there over a tick, as a drag does. The world's camera sees from where its
entity draws, so that one riding an entity moves with it, without juddering against it. Headless
frames, a tick each, draw their tick as it leaves the world, and see it from where the tick left the
camera.

A game's shader is a Slang file that the engine compiles with slangc as a layer adds it, so one that
doesn't compile fails there, with slangc's diagnostics naming the file and line; the renderer makes
its pipeline the next time it renders. A mesh's vertices are any `#[repr(C)]` type without padding,
with glam's vectors and matrices among its fields, and the next frame that renders copies them and
the indices into GPU memory, where they stay. Shaders read vertices through their buffer's address
rather than through vertex input, and Slang lays out what a pointer points at as `#[repr(C)]` does,
so a mesh's layout is between its vertex type and the shader's struct for it; draws take the
indices. A shader is added for a vertex type and a type of parameters, such as a color, which each
draw gives it: a mesh draws only with a shader for its own vertex type, and with parameters of that
shader's type. Each frame copies the draws' transforms, IDs and parameters into the frame's own
memory, where the shader reads them through their addresses too, so that a layer's draws take
parameters as large as their shader needs, though an entity's `MeshDraw` holds 64 bytes of them at
most; and the shader writes each draw's ID beside its color. A shader is added with how it blends
too, since Vulkan bakes blending into its pipeline: opaque, which writes its depth as well as
testing it, or by the alpha it returns, straight or premultiplied, which only tests it, so that
what's behind it shows through.

Vulkan checks nothing about a shader, so the engine checks the SPIR-V slangc writes against the
contract `Shaders::add` describes, as the shader's added and as it reloads, and refuses one that
breaks it, naming each way (`renderer.shader_mismatch`). It reads the SPIR-V because Slang's
reflection names what a pointer points at but none of its fields.

In a development build's windowed run, a thread of its own watches the files each game's shader read
as it was added, its own and those it includes as slangc lists them, and compiles the shader again
as they change, off the main thread; the next frame the main thread hands the renderer draws with
it, and the old pipeline retires until the GPU is done with it. A change that doesn't compile, or
breaks the contract, warns, and the shader keeps drawing as it did. The engine's own shaders don't
reload.

Windowed runs keep a pipeline cache, which they load from the OS's per-user directory for caches as
the renderer starts and save as it ends, so that pipelines an earlier run made are made again in a
fraction of the time. Headless runs, replays and tests use none, as they keep no layout, so that
their startup times compare.

A game's texture is a PNG or a JPEG that the engine decodes as a layer adds it, to RGBA8 with colors
sRGB-encoded and alpha not multiplied into them, as images store them, whatever color profile or
gamma the file holds, as glTF requires; the next frame that renders copies it into GPU memory, where
it stays, and shaders sample its colors decoded to linear. The engine reads a texture, a model, a
scene, a layout or the pipeline cache only from a regular file, so that a FIFO or a device can
neither hang the read nor fill memory; `--replay` alone reads its recording from whatever it's
given, so that it can come through a pipe. A texture's handle is its index in the bindless set,
which a draw's parameters carry as it is and the shader samples by, with one of the engine's
samplers, which it picks by its index too. A model's textures have their full chains of mip levels,
which the CPU makes as the frame that copies them renders, until they're cooked.

Meshes and textures reach GPU memory the best way the device has, as gg-gpu's `UploadPath` says:
beside the frames the GPU renders, through a queue apart from graphics, where the device has one.
Whichever way, the frame that takes them waits for their copies and draws them, and what staged them
is freed once that frame has finished, so that adding assets holds no memory past their own.

A camera is an entity with a `Camera` and a `Transform`. Its projection, orthographic, as 2D games
use, or perspective, fits the view's aspect ratio, so that nothing stretches as the dock resizes the
view. The view sees through the primary camera, and without one from the default viewpoint; of
several that code or a scene made primary, through the one of the lowest ID, since scenes keep no
order cameras came in. It sees from where the camera's entity draws, between ticks too, its
ancestors carrying it, facing as its own rotation and theirs turn it, their scales aside, so that a
mirrored ancestor mirrors where it is but not which way it faces. Where it sees from, and which of
several primary it sees through, are drawing's: no hash, scene or recording holds them, only the
components.

A layer sets the view's viewpoint for a frame in place of the world's camera, a position, a rotation
and a projection, through `Context::set_viewpoint`, as the editor's camera does, steering it itself
or with one of the engine's camera controllers. A viewpoint isn't in the world: it moves every
frame, by input, where the world changes only at ticks, so no hash, scene or recording holds it.

2D games draw sprites, which are draws like any other rather than a renderer of their own:
`Context::draw_sprite` draws the engine's quad mesh with its sprite shader, whose parameters pick a
rectangle of a texture, so that a sprite sheet's frames and an atlas's images draw from one texture.
Every `Meshes`, `Shaders` and `Textures` starts with the engine's own: the quad, the sprite shader
and a white texture a pixel across, which a sprite of only a color shows, so that one shader draws
sprites flat and textured. A sprite samples only the texels its rectangle covers, so that a
`SpriteSheet`'s cells, packed edge to edge, need no space between them.

Circles and lines are draws on the same quad, with shaders of the engine's own, each added the first
time something draws with it: `Context::draw_circle` draws a disc or a ring, as the world draws an
entity's `Circle`, and `Context::draw_line` a line between two points in the world, a number of the
view's pixels wide however near or far it is. Lines are quads rather than Vulkan's lines, since a
draw's lines are all one width, widths past a pixel are an optional feature, and so is smoothing
them: as quads, a run of lines of any widths is one instanced draw, as sprites are, and draws the
same on every device. Lines go in a list of draws of their own, which the frame draws after
everything else, the world's draws too, so that they show over all of it but where an opaque mesh is
nearer, as debugging wants.

3D games draw models: `Context::add_model` reads a glTF 2.0 file and adds its meshes and its images,
as textures, once, all of itself or nothing, and `Model::spawn` places its default scene in a world
as entities, as often as the game likes, under a root where it's placed: a node each, with its
transform in its parent's space, and a child for each primitive of a mesh with several, since an
entity holds one `MeshDraw`. The engine reads glTF itself, over serde_json, rather than through the
`gltf` crate, whose public calls panic on some malformed files. A model's URIs can't name a file
outside its folder, though a link inside the folder is followed wherever it leads.

Draws are batched, every mesh and not only quads: each run of consecutive draws of one mesh with one
shader, drawing the same faces, is a single instanced draw, whose instances are the draws, and the
frame copies the run's transforms and its parameters into its memory, each in an array of its own,
which shaders read by the instance's index. A change of texture is a change of parameters, which
bindless textures make free, so 100,000 sprites take a handful of draws; shaders index their
textures through `NonUniformResourceIndex`, since the instances of a draw can carry different ones,
and the engine decorates as non-uniform the sampled images slangc makes of them, as Vulkan requires.
Nothing caps a run: the frame's memory grows to hold it, where a renderer with a buffer of fixed
size would draw what it holds and start another. The renderer counts each frame's draws, instances
and triangles; the Performance panel shows them and `engine.perf` logs their peaks.

An error while rendering ends the renderer, since a failed frame can leave meshes and textures
half copied to the GPU. A game's shader compiled again whose new pipeline can't be made doesn't:
it warns, and the old pipeline goes on drawing.

## Threads

The main thread runs the OS event loop, which macOS requires, and the layers and their ticks.
Simulation runs on a job system, rayon-core's pool of `--threads` threads, the main thread among
them, which a tick's systems hand their rows to and wait for, so that the world changes only within
a system's call. A job changes only its own rows, work splits at a fixed number of rows, and no
thread changes its floating-point environment, so that results don't depend on how many threads
there are or which takes what.

Rendering runs on a thread of its own, `gg-render`, a frame behind: the main thread hands the
renderer over with each frame's render data and takes it back for the next, so that the main thread
is at most a frame ahead, and the GPU works on up to two frames behind that. Windows and macOS give
out a window's handle only on its own thread, and macOS closes a window there, so the main thread
makes and drops the renderer's surfaces, and the render thread never holds the last reference to a
window. An error while rendering comes back with the renderer and ends the run, and a panic resumes
on the main thread.

## Platforms

- **Vulkan 1.4, bleeding edge.** The engine requires a Vulkan 1.4 device, with the descriptor
  indexing bindless textures need and depth in 32-bit floats, which Vulkan doesn't require but
  every target has, and takes up newer features as drivers ship them, with a fallback wherever a
  target lacks one.
- **Windows and Linux:** native Vulkan drivers.
- **macOS: KosmicKrisp,** LunarG's Vulkan 1.4 driver on Metal. It needs Apple Silicon and macOS 26
  or later. Not MoltenVK, which Khronos deprecated. KosmicKrisp has no mesh shaders or ray tracing,
  so those stay optional paths.
- **x86-64 and ARM64:** the engine builds and passes its tests on both from the start, rather
  than being ported later; macOS is ARM64 only. An ARM64 device still needs a Vulkan 1.4
  driver.
- **No GPU:** lavapipe, Mesa's CPU Vulkan driver, runs the GPU tests headless.

## Technology

| Area | Choice |
|---|---|
| Language | Rust stable, edition 2024 |
| Game code | Rust; Luau through `mlua` after v0.1, for mods and quick scripts |
| Hot patching | `subsecond` |
| GPU API | Vulkan through `ash`: one backend, no multi-API abstraction layer |
| GPU memory | VMA through our own binding in `gg-gpu`, for its budgets, aliasing, pools and defragmentation |
| Window and input | `winit`, with `ash-window` for surfaces and `gilrs` for gamepads |
| UI | egui, drawn by `gg-renderer`, with a dock of our own: the engine's overlay and the editor alike; its text in Inter |
| File dialogs | The OS's own, through `rfd`, in development builds |
| Project state | RON in a canonical form; see [Project state](#project-state) |
| Math | `glam`, with its `scalar-math` and `libm` features; `libm` |
| Jobs | `rayon-core`'s thread pool and `join`; not rayon's parallel iterators, whose splits and reductions follow the number of threads |
| ECS | Our own; see [ECS](#ecs) |
| Physics | `rapier2d`, with `enhanced-determinism`, for 2D, its state a resource of the world; see [Physics](#physics) |
| Logs and profiling | `tracing` for logs; Tracy, through `tracy-client`, for CPU zones and GPU timestamps |
| Frame captures | RenderDoc, through its in-application API, which `gg-gpu` finds in the process with `libloading` |
| Agent access | MCP and a command-line tool, both clients of the [tool API](#agents-and-tools) |
| Shaders | Slang, compiled to SPIR-V by the Vulkan SDK's `slangc` rather than linked in: the engine's own as it builds, games' as they're added and, in development builds, as their files change, their SPIR-V checked against the shader contract |
| Assets | glTF 2.0 in, read by the engine over `serde_json`; meshoptimizer for meshes and meshlets; BC-compressed textures in KTX2; until cooking, PNGs and JPEGs that `png` and `zune-jpeg` decode as layers add them |

Areas not listed here are undecided.
