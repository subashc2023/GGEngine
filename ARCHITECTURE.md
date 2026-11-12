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

The editor uses `gg-api` too, beside `gg-engine`, as a client of the tool API, as `gg` is.

- **`gg-core`:** math, logging, profiling and error types.
- **`gg-gpu`:** Vulkan through `ash`. It turns ash's unsafe API into safe types: device, memory,
  buffers, images, descriptors, pipelines, swapchain, the render graph, synchronization and
  submission. They keep the CPU's side safe whatever safe code does with them: each checks that
  what it's given was made by its device, or a surface by its instance, and a frame or a pass
  forgotten rather than dropped is caught at the next call. What runs on the GPU is trusted: a
  shader's SPIR-V, and the addresses and texture indices it's given, none of which Vulkan checks,
  so a shader that reads past what it's given, or samples by an index no filled texture holds, can
  fault the device. gg-renderer checks games' shaders against their contract, and makes every
  address a draw reads.
- **`gg-renderer`:** the frame's passes, declared in gg-gpu's render graph, materials, lighting and
  post-processing. It owns the GPU resources and hands out handles to them.
- **`gg-engine`:** the main loop, window and input, the job system and the render thread, scene
  and assets, fonts and text, a game's UI, sounds and music, plus hot reload and the tool API in
  development builds: what apps and games build on.
- **`gg-macros`:** the derives `gg-engine` re-exports beside its traits of the same names,
  `Reflect` and `Component`. Rust builds a derive as a crate of its own; only `gg-engine`
  depends on it, and games derive through `gg-engine`.
- **`gg-api`:** the [tool API](#agents-and-tools)'s protocol: its messages, the descriptions of a
  game's types that a client reads and writes its values by, the session file that says where a
  game serves it, a client, which tools and the editor speak it through, and the memory a game's
  view is shared through as it renders; and the readers and writers of the text of a
  [project](#project-state)'s manifest and of its assets' ID files, with the scan that finds those
  under an asset root: files that tools and development builds share. It depends on no `gg-*`
  crate, so that tools build without the engine, and `gg-engine` uses it in development builds
  alone, so that only they read a manifest or an ID file: shipping builds start from cooked data.
- **`sandbox`:** the engine's own app, for its development: a swarm of up to 100,000 sprites
  drifting on the job system's threads, which measures how fast the engine simulates and draws, and
  a grid from its scene file.
- **`editor`:** the editor, one more client of the tool API: a dock that fills its window, with
  the game's view in a panel among the others. Started in a game's project, it runs the project's
  game as an agent does, under `gg run --embedded`, or optimized with `--release`, as the editor's
  own flag `--release-game` asks, which ends as the editor does, the game starting stopped
  (`--stopped`), its world the world as edited; and shows the game's view; the
  game's world in its Entities and Inspector panels, a remote Inspector's, which edit it through
  the API as the editor's own edits, as the handles in the view do, a click picking what the view
  shows under the cursor through the API; its camera, the pick's outline, its grid and the
  colliders' outlines going to the game as the API's look, which it takes up from other clients
  too; the game's events in its Logs as the game logs them; and the project's assets in its Assets
  panel, an ID file beside its asset hidden, from which a texture dropped on the view sets the
  texture of the sprite there, a model is a root where the cursor shows the grid's plane, and a
  prefab an instance's root there, linked, each an edit of the editor's naming the asset by its ID.
  Its Inspector marks each field and component where an instance's entity differs from its prefab,
  as the API's `entity` says, a field's context menu and a component's menu reverting it, and
  unlinks an outermost instance's root; its Entities panel saves an entity and its descendants as
  a prefab through the API's `save_prefab`, linked in their place, which a game's own Inspector
  doesn't, as a save writes files that neither a replay nor a lockstep peer of its commands would
  have; each an edit of the editor's. Its Play and Simulate are the API's
  `play`, and Stop its `stop`, which brings back the world as edited; while the game plays, Play
  is `pause` and `resume`, and Step is `step`, a stopped game played paused first, as other editors'
  controls are. Its File menu's New, Open
  and Save are the API's `scene`, on the game's open scene, which its title bar names, marked
  while unsaved; and since closing it ends the game, it asks before New, Open or closing loses
  unsaved edits. Play sees through the game's camera, which gets all of the view's input. For a
  game whose ticks are all its own code, Simulate plays the game as Play does, but through the
  editor's orbit camera, the game getting no input, as Unreal's Simulate In Editor does; switching
  between them carries on, moving the camera and the input over, as Unreal's Possess and Eject do.
  The toolbar's camera switch is that switch while the game plays; stopped, it shows what the
  game's camera sees, the input staying the editor's, until the editor's camera takes some. A Stop
  turns the view to the editor's camera, which carries on from where the view was, as it does
  whenever it takes the view over; a New or an Open turns it to the game's camera, as the editor
  starts, which frames the scene opened. F over the view, and a double click on a row of its
  Entities panel, that row's Frame in View or F in the panel, frame the pick: the editor's camera
  eases to look at its origin, turned as it was and as far back, as no entity's size reaches the
  editor, and the view turns to it. Its Scenes panel, below the Entities, lists the scene files in
  the project's scenes folders, a click opening one as the File menu's Open does. Its Debugger
  panel debugs the game's scripts through the API: the source of each script the game runs, a click
  on a line's number setting or clearing a breakpoint there, which the editor keeps and gives each
  game as it serves; where a script has stopped, with its stack and each frame's locals; and
  Continue, Pause and the steps. The editor has no world of its own: started outside a project, it
  shows its hub, a start screen over its panels, as Unity Hub is a launcher of its own, listing the
  projects opened lately, which it keeps for the user as the dock keeps its layout, and those in
  the workspace's `games`, opening the one picked, or the latest as it starts if the user chose to
  skip the hub; its File menu's Open Project shows the hub again, and another project opens in
  place of the one open as a close of the editor goes, asking before unsaved edits go.
  [game.rs](apps/editor/src/game.rs) describes the rest.
- **`player`:** the reference player. It runs a game's package without the game's code, its start
  scene, its Luau scripts and physics, as the showcase's package runs, and shows how a game
  builds its own player.
- **`tools/gg`:** the command-line tool, a client of the tool API, which `cargo gg` runs, and which
  builds a game as it ships, `gg ship`, as [Builds](#builds) says, and fetches a project's files
  its repository doesn't hold, `gg fetch`, as [Project state](#project-state) says.
- **`tools/cook`:** the offline asset cooker, `gg-cook`, which writes a game's
  [package](#project-state) from its project. It depends on `gg-engine`, whose code bakes fonts,
  compiles shaders, decodes images, reads models and makes mip levels as runs do and writes and
  reads the package's formats; on `gg-api`, for the manifest and the scan of the asset root; and
  on `ctt` and `meshopt`, which compress textures and optimize meshes, which runs don't.

Games depend on `gg-engine` the same way the apps do; see [Game code](#game-code). The first, in
`games/shards`, is a demo of two players, alone or networked in lockstep, as
[Networking](#networking) describes, which the v0.1 loop is tested on. The second, in
`games/showcase`, is a scene for each of what the engine draws and simulates, opened from the
editor's Scenes panel, which grows as the engine does. The third, in `games/foxball`, is the 3D
demo, networked as Shards is: two foxes, skinned characters that rapier3d walks, knocking a ball
into goals, looked about with the grabbed mouse and heard in 3D. Executables are
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
player `dev`. Shipping builds therefore select the player alone, with `-p player`, or a game alone.
A game turns `dev` on through a feature of its own of that name, on by default, as `gg new` writes
its manifest, so that its development build is a plain `cargo build`, and its shipping build leaves
it out: `cargo build -p shards --no-default-features` builds the demo as it ships, which runs from
its [package](#project-state). The shipping gate, `cargo clippy -p player -p shards -p showcase
--no-default-features`, checks `gg-engine` as they build it, and the games' tests without `dev`,
which compile there but run with it, as they read their projects. A gate of its own,
`cargo clippy -p gg-engine --all-targets`, checks `gg-engine`'s tests without `dev` too, which no
other command builds: kept apart from the shipping gate, as their dev-dependencies' features would
merge into the player's build.

A build without `dev` has no development UI, nor egui at work: `Ui` is a type no value of which can
be made, so the engine never calls `Layer::ui`, makes no egui context, runs no egui pass and hands
egui no events, which go to the layers alone. A game's code, its `ui` included, builds unchanged
either way, and draws its own UI on the [canvas](#events-and-layers): `PerfPanel` and `Inspector`
are layers that do nothing, and `App::custom_title_bar`, `App::view_panel` and `App::dock` change
nothing, the OS framing the window. egui stays a dependency for its types, which `Ui`'s methods
name, and the linker keeps none of its code; egui's winit integration, the dock, the title bars,
the Inspector's and the Performance panel's UI and the torn-off windows are behind `dev`.

Shipping builds are optimized in the `shipping` profile: release's, with the whole program
optimized as one at link time (fat LTO, one codegen unit), panics still unwinding, as the render
thread's resumes on the main thread. Release and shipping builds compile trace events out
(`tracing`'s `release_max_level_debug`), and keep debug events, which agents read of `gg run
--release`'s runs. A game built without `dev`, and the player built optimized, open no console
window on Windows, their logs recording their runs. `cargo gg ship` builds a project's game so,
cooks its package beside it and lays the two out in `target/ship/<game>`: side by side on Windows
and Linux, whose GPUs' drivers install the Vulkan loader; on macOS, in an app bundle,
`<Game>.app`, its package in `Contents/Resources`, and the Vulkan SDK's loader and KosmicKrisp in
`Contents/Frameworks`, which an executable in a bundle loads the loader from, by its path, with
KosmicKrisp's manifest in `Contents/Resources/vulkan/icd.d`, where a loader in a bundle looks
first. Its Info.plist, of identifier `com.ggengine.<game>`, has a launch through Launch Services,
as from Finder, load the bundle's driver alone (`VK_LOADER_SEARCH_ONLY_IN_BUNDLE`), which needs the
SDK 1.4.363.0 or later, the first whose KosmicKrisp is a conformant Vulkan 1.4 driver too. It's
signed ad hoc, as Apple Silicon runs nothing unsigned, inside out, without the hardened runtime,
under which it would load no library a team of its own didn't sign.

Tracy is a feature of its own, `tracy`, off by default and in no shipping build: it compiles
Tracy's C++ client, which listens on the machine's loopback and keeps what it records in memory
until a viewer connects. Apps and games turn it on through `gg-engine`'s, as the sandbox does.

Vulkan validation is a runtime switch rather than a feature, on by default in debug builds, so
tests run with it whatever the features.

The docs are a gate too: rustdoc's warnings, private items' docs included, as doc comments are
most of what people and agents read of the code, so that a link that breaks, or comes to name two
items, fails the change that broke it.

### Growth

A module becomes its own crate once it has its own responsibility and a second user, not
before:

- `gg-assets`: cooked asset formats, shared by the cooker and the runtime, once the cooker builds
  without the engine. It bakes fonts and compiles shaders with the engine's own code, through
  `gg-engine`'s hidden `cook` module, so the package's format is a module of `gg-engine` until then.
- `gg-scene`, `gg-physics`, `gg-audio` and the like: when each outgrows a module of `gg-engine`.

## Game code

Game code is Rust, as fast and as unrestricted as the engine's own. Each game is a crate that
depends on `gg-engine` and builds its own player, statically linked and optimized for shipping. Game
crates forbid `unsafe`, so a bug in game code panics rather than corrupting memory. Games built
alongside the engine, the demos and then the flagship, live in `games/` in this workspace, among its
members, so one change can span engine and game, each a [project](#project-state), which `gg new`
makes; anyone else's game depends on the engine from its own repository.

- **The game runs in its own process.** The development tools launch the game's development
  build and work on it through the [tool API](#agents-and-tools). A crash, panic or hang in
  game code stops only the game, and the tools restart it from its last snapshot. The same
  split gives dedicated servers, local multiplayer tests with a server and several clients,
  and headless runs in which agents check their work. A game the editor runs shows in the
  editor's view, and one run without it in its own window.
- **Hot reload keeps the world.** `subsecond` patches an edit inside a function body into the
  running game, which reaches calls made through its `HotFn` but none through a vtable or a
  function pointer made before the patch. `gg run` builds a game's development build with
  Dioxus's `dx`, of the release of the game's subsecond, and watches its sources: an edit inside
  function bodies of the game's crate, as it's saved, it builds as a patch, which it sends through
  the [tool API](#agents-and-tools), and the game applies between ticks. A development build calls
  each layer's methods through `HotFn`, so that a patch reaches those methods and the systems and
  scripts they call; an edit that only a game's own trait object or function pointer reaches
  waits for the next rebuild. subsecond patches the binary's crate alone, so a game's crate is a
  binary with no library beside it, and that crate's statics start over at each patch, so a game
  keeps its state in the world. Any other change, such as a new field, even of a type declared in
  a function's body, a new component, a constant, an engine edit, or an edit to code that has run
  as the game started, `main` and a layer's `load` and `restored`, which a patch would come after,
  rebuilds the game, restarts it and restores the world from a snapshot taken just before,
  matching each changed component to its new layout by field name. An edit inside a layer's
  `attach`, which builds the world as a run starts afresh, is patched, but the world a run keeps,
  as its restarts start from snapshots, never shows it: `gg run` says so, having the game log
  `gg.attach_changed`, and a run started afresh shows it. Where patches can't reach the game, as
  without `dx`, `gg run` builds it with cargo and watches its sources all the same, every change
  rebuilding and restarting it.
- **Luau, in every build, for mods and quick scripts.** Players can install sandboxed mods, and
  quests, dialogue and level events reload in milliseconds. A script file declares one component:
  its fields, with their defaults, and a tick run for each entity that has it, as a native
  `Script`'s is; adding the component attaches the script. It reaches Rust components through
  reflection, without hand-written bindings; [docs/LUAU.md](docs/LUAU.md) lists what scripts and
  mods can use. Its tick spawns entities, with components by their
  types' names and fields' values, under a parent or as a prefab's instance's root, despawns them
  with their descendants, and gives and takes away their components, at once, as Rust code does:
  each script's pass runs over the entities listed as it starts, not a query that holds the world,
  passing over one despawned, or without its component, by its turn, so nothing need wait for a
  point to apply at. IDs and slots come from the world, whose stream of IDs and free slots
  snapshots copy, so a spawn gives the same entity on every peer, in a replay and as rollback runs
  a tick again. What a VM's scripts give an entity as they run, the entity spawned or a component
  given, and what rides on it, ticks first as they next run, at the next tick, whichever pass comes
  later, though another VM's scripts that run later in the tick, a mod's, tick it as they come to
  it; a despawned entity's scripts go with it, its handles found to be of none
  (`ecs.no_entity`) wherever a script uses them. A chunk changes no entity (`script.loading`), as
  it runs again as a joiner or a restored run loads it. A layer's `Scripts` loads its scripts from
  the project's files in its `load` and runs them where its tick chooses; a development build reloads
  one as it's saved, between ticks, through an edit that a recording keeps and lockstep peers apply
  at one tick, as a scene file's save is. A reload that changes the component's fields migrates
  every one in place, by field name: a field kept keeps its value, one added takes its default, one
  dropped goes, and one whose kind changed takes its default, warning; a changed default reaches
  components added after. What comes back from before a reload, the world as edited that the tool
  API's `stop` brings back and the components an undo puts back, migrates as it comes back. A
  reload doesn't rename the component, nor does an undo take a reload back: saving the file as it
  was does, the fields it dropped coming back at their defaults. A script keeps its state in its
  components, as a game keeps its in the world, since a lockstep joiner and a run restored from a
  snapshot run its chunk afresh: once the chunk has run, every table it reaches is frozen and its
  globals are read only, and a script is refused (`script.state`) whose functions assign a local of
  its chunk, as its compiled bytecode shows, or whose chunk keeps a coroutine or a buffer, which
  hold state too; and the metatable that entities share is frozen, as Luau's sandbox freezes its
  libraries' and vectors'. Every VM collects its garbage at set points between calls, as much as
  its scripts have allocated since calls for, rather than when Luau paces it by the clock, so that
  what a weak table still holds, and the memory a VM takes, are alike everywhere. Its ticks don't
  allocate, as steady-state frames don't: the engine's bindings allocate nothing, so a table,
  string or closure a script makes at each tick breaks the rule as Rust code making one would. Luau
  doesn't guard against crafted bytecode, so mods ship as source and only a game's own packages
  hold bytecode.
- **Scripts are debugged where they run.** A development build debugs its scripts through the
  [tool API](#agents-and-tools), with Luau's own debugger hooks: breakpoints, each a script's path
  from the project's root and a line, which Luau moves to the next line with code, kept as the
  script reloads and placed again in its new functions, though never in what its chunk runs as it
  loads; a
  pause, which stops the next line any script runs, breaking into a call that's running; and, once
  one has stopped, a step into, over or out, or a continue. A step out of a tick, or over its last
  line, stops at the next line a script runs, as the next entity's tick. Luau steps only in a call
  it entered stepping, so a call into a script enters so while the debugger is armed, a breakpoint
  placed or a pause or a step asked, which runs it slower and changes nothing it does; a step from
  where a pause broke into a call entered otherwise goes on to the next call. A script stops inside
  its tick: the main thread waits there, and with it the rest of the tick, the frames and every
  request that needs the main thread, while the connections' own threads answer the debugger's,
  from what the stop gathered: each frame of the stack, its function, script and line, and its
  locals and upvalues as values. `ping` says the game waits at a breakpoint, so that `gg run`
  neither takes it for hung nor snapshots it, as for a file dialog; on Windows, the wait dispatches
  what's sent and posted to the game's windows, as a modal loop does, so that Windows doesn't mark
  them hung, winit holding back the events they make, a close among them, until it goes on. A stop
  changes nothing in the world, so a recorded run replays across one, and the other peer of a
  lockstep session waits for the stopped one's inputs, as it waits for a slow one. Every build
  compiles scripts with Luau's full debug information, their locals' names among it, so that their
  bytecode is the same everywhere; and every build gives each call into a script, a tick or a chunk
  as it loads, a budget of a million of Luau's interrupts, which come at each call and each turn of
  a loop, past which the call fails as an error does: a loop that never ends fails alike on every
  peer and in every replay, rather than hanging the game, which `gg run` would restart into it
  again.
- **Mods are scripts a player installs.** A mod is a folder holding a manifest, `mod.ron`, which
  names the mod, its version, the game it's for and the capabilities it asks for, and the Luau
  scripts it lists, as source; installed in `mods` in its game's per-user folder, where the dock
  keeps its layout, which a windowed run reads, or in the folder a run's `--mods` names in its
  place, each loads as the run starts, in the order of their names. A game takes mods by loading
  them through `Mods` in a layer's `load`, with the most it grants any, and runs them where that
  layer's tick chooses, as it runs its own scripts: a mod that asks for more, or whose manifest or
  a script doesn't load, is refused, warning, and the game runs on without it. A game registers
  its component types before it loads mods, since a mod may declare any name that's free: once a
  world has loaded mods, whatever's installed, it refuses a Rust type that registers
  (`ecs.registered_late`), so that a game that registers one late fails as it first runs, not
  where a player's mod has taken its name. A mod's scripts are scripts as a game's are, run in a
  VM of the mod's own and kept to its capabilities, `read` and `write` of the whole world or of the
  component types they name, as `read:Shard` or `write:Sprite`, which `gg.component`, an entity's
  fields and the components its scripts give and take check; its own components, which no other
  script's are, always its own. Its scripts spawn only if it may write some type, and then an
  entity of components they may write; one without components, under a parent or of a prefab,
  and a despawn, need `write` of the whole world. They run within a memory limit, past which a
  call fails as an error does, at the same allocation on every platform and in every build, for
  runs that went alike; with one budget of interrupts for all of the mod's calls in a tick, past
  which they fail until the next, so that a mod slows a tick by no more than one call can; and
  with its prints held to a rate. A script, a game's or a
  mod's, reaches the game's entities through a component of the game's that its own rides on, as
  `gg.declare`'s `on` names it, which gives each entity with that component its own, at its
  defaults, as its scripts run. A run's mods, their manifests and scripts, go whole in its
  snapshots and its recording, so that a lockstep joiner runs its host's in place of its own and a
  replay its recording's, whatever is installed, each granted no more than its own game grants; a
  run restored from a snapshot loads those installed, as it reads its scripts' files.

## Agents and tools

Agents work on a game the way people do, through the same interfaces: nothing a person can do
in the editor is out of an agent's reach.

- **The tool API.** A game's development build serves an API on the machine's loopback, at a port
  the OS picks, which it writes with a token, and its project's manifest's path, in a session file
  in `sessions` beside its executable, removed as the run ends: query the world, apply and undo
  [operations](#project-state), play and stop, pause and step, open, save and empty the [open
  scene](#project-state), capture frames, share the view as frames render it, set how it looks and
  pick what it shows, list the assets in the asset root with their [IDs](#project-state), list each
  UI the layers added from its file, its nodes' rectangles in the canvas's units and the view's
  pixels, what they say, what its widgets hold and the focus, so that an agent checks a layout as
  data (`ui`), as [Events
  and layers](#events-and-layers) says, hand the layers input as the window would, save the world
  whole as a snapshot file, as [ECS](#ecs)
  describes, which a run given `--restore` starts from, apply patches of the game's code and debug
  its scripts, as [Game code](#game-code) describes, read [diagnostics](#diagnostics), and log a
  client's notes among them, warnings or facts it gives with stable codes, as `gg run` tells a game
  of a patch or a rebuild that failed. Requests and their answers are lines of JSON, as
  [gg-api](crates/gg-api/src/lib.rs) describes, and a connection's first request says hello with
  the token, which the game compares in constant time, so that only processes that can read the
  file reach the game: the user's own, as the file is the user's alone from before it holds the
  token: on Unix of mode 0600, and on Windows of a protected DACL that allows its owner alone. It
  says of an instance's entity what differs from its prefab, and saves a subtree as a prefab
  linked in its place, as [Project state](#project-state)'s Prefabs says. A
  thread of the API's reads each connection, at most 16 requests ahead of the answers written to it,
  so that a client that sends faster than it reads or than the game answers is held back by the
  connection rather than filling the game's memory, and answers its pings itself,
  from the frames the main loop counts, so that a tool tells a hung game from a busy one, and from
  one whose frames have paused for the user, as for a minimized window or a file dialog; and its
  reads of the log, from the events the run keeps in memory as it logs them, its last 10,000, each
  numbered, which `logs` serves as they come, waiting for one if none has, so that a client follows
  them from the number it left off at, and, by the run's own number, which a restart changes, misses
  none across a restart that the new run kept; its notes, so that one comes through while frames
  pause; and its requests to debug scripts, through what the debugger shares with the main thread,
  so that they're answered while a script has stopped and the main thread waits there. The main
  loop answers the rest as each frame starts, those that came before it did, so that clients that
  keep sending can't keep it from going on, before its ticks: a step once its ticks have run, a
  capture once the render thread has read the
  frame's view back, without the UI, which a thread of the API's writes as a PNG, from the first
  frame with no image decoding and nothing streaming, so that an agent sees no stand-ins, or after
  10 s, saying it still streamed (`streaming`), and a snapshot
  once a thread of the API's has written the copy of the world the main loop took, but none before
  the first tick of a game playing from its start, which applies what the layers queued as they
  started. Paused, frames run no ticks but those a step asks for, and draw the world as the last
  tick left it, so that a capture shows what a query reads. A game plays or is stopped, as `play`
  and `stop` set it, any client's request the last word, and a pause lasts across both. Stopped,
  frames run no ticks, and draw the world as edits leave it: the world as edited, which only edits,
  scene files' saves and the open scene's New and Open change. `play` has a copy of it play in its
  place, from its ticks, the engine holding the world as edited aside, with its scene files as last
  applied and the authors' logs, until `stop` brings them back, the copy and all done to it going
  but a scene file's save, which applies to the world as edited too as it comes back; and the ticks
  go back to that world's, so that two plays of one world as edited, given the same inputs, go
  alike. The IDs the copy drew are drawn again, so a client keeps none it learned across a `stop`. A
  game playing from its start, as every run but one given `--stopped` starts, holds no world as
  edited: `stop` stops it where it is, and that world is the world as edited from then on. Recorded
  runs and lockstep peers refuse `stop` (`api.no_stop`), since it replaces the world between ticks
  and takes its ticks back, which neither a replay nor the other peer could follow. The view is
  shared through memory the client makes and a `view` request names, rather than through the game's
  window, which other processes can embed on Windows and X11 alone, and couldn't draw over: three
  slots of frames, a triple buffer that neither end waits on, which the render thread writes each
  frame's view into once the GPU has finished it, and the client reads, as agents can read live
  frames too; and, after them in the header, whether the game's layers want the cursor grabbed,
  which the editor, as the client, grabs its own window's for in Play while the view has the keys,
  the mouse's raw motion going to the game through `input`, letting go at Escape, which the game
  gets too, as a game may not let go itself, until a press in the view; a click of Play grabs it
  for any game, giving the view the keys, until Escape. A game run `--embedded`
  renders its view at the size the client asks, and runs each
  frame once the client has taken the last, rather than rendering frames it never takes: so a
  client sends what the next frame is to draw with, its look and input, before it takes the last,
  or that frame draws without them. The view
  has one look, as the engine renders one view, which every client sets part by part, the last to
  set a part winning: the viewpoint it sees from, in place of the game's own; the entity it
  outlines; a grid on one of the world's axis planes; whether the colliders' outlines show; and
  what the view shows in place of its light, to see how it's made, as [Rendering](#rendering)
  says. A
  part given back is the game's own again. A look lasts until a client changes it or the run ends,
  so a restarted run starts without one; it says which client set each part, so that a client takes
  up what another set, as the editor's camera carries on from a viewpoint an agent set; and each
  frame shared carries the viewpoint it was drawn from and the revision of the look it drew with, so
  that what a client draws over a frame matches it. A capture shows the view as it's shown, the look
  among it, or, asked to, as the game shows it on its own, from a bare frame, which draws without
  the look, isn't shared and leaves the picks asked to the next frame, so that none reads a view a
  client isn't shown; and, asked to, shows something in place of the view's light, from a frame of
  its own that isn't shared either, so that an agent captures a set of them without the user's view
  changing, from one world while the game is paused, as `gg capture --show` does, and reads the
  numbers at a pixel from them, as `gg probe` does. A pick answers which entity the view shows at a
  pixel, and its name to a
  connection granted to read names, once the frame that read it comes back. A connection is kept
  to the capabilities it's granted: `read` and `write` of the world, whole or of the component
  types they name, as a mod's scripts are kept;
  and the API's own, `play`, which plays, stops, pauses and steps, `input`, `view`, which captures,
  shares the view, picks and sets the look, `scene`, `snapshot`, `debug`, `patch`, `files`, which
  writes a capture or a snapshot where a request names, `note` and `quit`. The session file's
  token grants them all, `patch`, which runs code the client gives, among them, unless the `hello`
  names fewer, as `gg --grant` and `gg mcp --grant` ask for an agent kept to them; a request beyond
  them fails with `api.ungranted`. Any process that reads the session file reaches everything, so a
  grant keeps to it only a client that can't, such as an agent sandboxed away from that file.
- **Clients.** The command-line tool, `tools/gg`, builds and runs games, supervising them: it
  runs a copy of a game's executable, so that the game builds again meanwhile, removing those runs
  it killed left; patches it as its code changes, and rebuilds it at any other change, as [Game
  code](#game-code) describes, having the game log a patch or a rebuild that failed, so that a
  client following its logs learns of it; snapshots it every few seconds from its first frame;
  and starts it again from its last snapshot after a crash or a hang, and from one taken then as
  it's rebuilt, in the request that has it quit, so that no edit lands between the two, telling of
  each, or waits for a rebuild if the game would only end the same way again, as one that ends
  before its first snapshot would. A game that waits where it can't answer, at a script's
  breakpoint or in a dialog, is killed at once instead, starting again from the snapshot before. A
  game started again so comes back paused if it was, and stopped if it was, or playing a copy with
  the world as edited held, as its snapshot keeps them.
  A game that comes to its own end, as its window closes, ends the supervision, as one that never
  serves does; so does the end of the tool a run embeds the game in (`--embedded`), as the editor
  ends, which closes gg's stdin. Its games end as gg does, however gg ends, killed too: each is
  given `--supervised`, which ends a run as its stdin, which gg holds, closes. One `gg run` runs a
  game at a time; a second is refused. It tells what it's doing, as building or running the game,
  whether the game's code holds every edit, and the last build's errors, in a file beside its
  lock, which the commands that talk to a game read while none answers, failing with a code that
  says so (`gg.rebuilding`, `gg.build_failed`), and `wait` waits on until an edit's code runs. It
  keeps its snapshots in a directory of its own, removing those of runs that were killed; a game
  keeps the newest of the captures and snapshots it writes without a path. It runs headless
  checks, talks to running games, all with
  machine-readable results, and makes new games. Inside a game's
  [project](#project-state), found from where it runs or named by `--project`, it runs cargo in the
  project's directory, builds the project's game, and reaches the project's games alone, those whose
  session files name its manifest; `gg mcp` gives agents that speak MCP the same reach over stdio, a
  tool for each of gg's commands that talk to a game but `quit` and `call`, telling them, as gg's
  usage does, that the game, but a lockstep session's joiner, whose host does, writes each
  texture's and model's ID file, to commit with it and move with it, connecting at each call to the
  game it chooses, so that a restarted game is the one it reaches, as the agent, by its MCP client's
  name; the editor is one more client, with no ability an agent lacks.
- **Headless runs.** Every app runs without a window (`--headless`) and can run a set number
  of frames, then exit with a status (`--frames N`): 0 for a clean run, 1 after an error or any
  validation message, and 2 for bad arguments. Dedicated servers, CI and agents checking their
  work run this way; rendering checks render offscreen, and simulation-only runs need no GPU. A
  run embedded in a tool (`--embedded`), as the editor runs a project's game, renders offscreen in
  real time, its view shared with the tool as the [tool API](#agents-and-tools) says.
  Under RenderDoc, a run can capture a frame ([Captures](#diagnostics)), which
  `renderdoccmd convert` writes out as XML for agents to read.

## Project state

A project's state is text that people and agents edit, diff and merge with ordinary tools.

- **Projects.** A game's project is its crate's directory, which holds its manifest,
  `project.ron`, beside its `Cargo.toml`, in canonical RON: the game's package (`game`); its asset
  root (`assets`), which is `App::root`; the folders in it that hold its scenes (`scenes`); and the
  scene it starts in (`start`), which `Context::load_start_scene` loads; each path relative to the
  manifest's directory. Tools read a manifest and never rewrite it, and `gg new` writes one. A
  development build reads its game's, through `App::project`; shipping builds start from cooked
  data, so a build without `dev` reads none, and runs from its game's package.
- **Fetched files.** What a project uses that its repository mustn't hold, as a model whose license
  keeps it out, as Sponza's does the showcase's, its fetch list, `fetch.ron` beside its manifest,
  names: each file under a folder at a URL pinned to what it holds, as a repository's at a commit,
  with its SHA-256, and the asset ID its scenes name it by, if they do. `gg fetch` fetches them,
  with curl, into a folder of the project its `.gitignore` leaves out, each put in its place once
  its SHA-256 is the list's, and writes each asset's ID file, with the list's ID or a fresh one, as
  a development run would, so that every checkout that fetches gives it the ID its scenes name. A
  checkout that hasn't fetched them, as CI's, can't load a scene that names them, which the
  showcase's tests leave unloaded; its `asset.unknown_id` names the file the list gives the ID and
  `cargo gg fetch`.
- **Packages.** A game's package is one file, which the cooker, `gg-cook`, writes from its project,
  and a build of the game without `dev` runs from: `<executable>.ggpk` beside its executable, in a
  macOS app's bundle in its `Contents/Resources`, or the one `--package` names, a run of a project
  without one failing as it starts (`engine.no_package`). So does an app that `App::from_package`
  makes, in any build, as the reference player is, which loads the Luau scripts the package holds,
  as `Context::package_files` lists them, and its start scene. It holds the files under the asset
  root that runs load, by their paths from the root, found through the folders the scan of ID files
  below goes through: a font baked into its field and a shader compiled to SPIR-V, which are what
  the engine makes of them as they load, so that a shipping run needs no slangc; a texture compressed to BC7, of the one level
  a layer's texture has, in KTX2, supercompressed by zstd; a model as the engine reads it, with
  the files it names, each primitive's triangles optimized by meshoptimizer for the GPU's vertex
  cache, overdraw and fetches, but those of a material that blends, which cover each other in
  their order and keep it, with levels of detail it simplified, each about half the triangles
  of the one before, its vertices packed as a run packs them, and each level cut by meshoptimizer
  into meshlets but where its material blends, as [Rendering](#rendering) says, and each image
  compressed as what its materials sample it as, BC7 of colors
  or of a map's values, or BC5 of a normal map's X and Y, with the full chain of mip levels a run
  makes for it, made as a run makes them, so that a run neither decodes PNGs nor makes levels, but
  a DDS image's BC7 blocks and levels kept as they are; and the
  rest as they are, sounds and music, scenes, prefabs, UIs and other RON, and scripts. It leaves out
  the manifest and the fetch list, mods' folders, whose scripts a player installs, a module that
  shaders include rather than one that compiles alone, and what no run loads, as code and docs, by
  its extension, a
  model's buffers among them, which its model holds. Which of
  the rest a game's code loads the cooker can't know, but it refuses a project whose package
  wouldn't hold its start scene (`cook.no_start`), and, as it takes every PNG and JPEG under the
  root for a texture, one of them that doesn't load as a texture (`engine.bad_texture`), and every
  `.ui` file for a UI, one a run would refuse, with its code, as one naming a font or a texture the
  package doesn't hold where a run from it would look (`asset.no_font`, `asset.no_texture`). It
  gives each asset the ID its ID file holds, and the cooker refuses a project whose ID files a
  development run would warn of (`asset.bad_meta`, `asset.meta_case`, `asset.duplicate_id`), so that
  a run from the package names its assets as a development run does, and replays what one recorded
  to every hash. A run finds a file there by its path from the root, worked out from the path it's
  given as the disk would, a relative path from the working directory, but without reading the disk,
  and with its case, so that a package runs alike wherever it's copied. A run from a package reads
  nothing of the disk's under its root: it scans for no ID file, watches nothing, and the API's
  `scene` refuses (`api.no_scene`). A
  development build runs from one too, given `--package`, as the demo's test that replays its live
  recording from its package does. Cooking the same project gives the same bytes. The format, in
  [package.rs](crates/gg-engine/src/package.rs), has a version, which a change to what a kind of
  file holds there changes too, as a baked font's layout.
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
  that reflection's descriptions drive, never its untyped values. A texture, a model, a font or a
  prefab is saved as its name, its asset ID or else its path relative to the app's root
  (`App::root`), as Asset IDs says below, which a load finds it by, as it finds it by a path to its
  file, as a scene written by hand may name it, loading it where no layer has; what's opaque, such
  as a `MeshDraw`, isn't saved: a load builds each component on its type's `Component::base`,
  which gives it that, or from the scene alone. A field a scene leaves out, as one saved before
  its type had the field does, takes its default, and saves again whole. A child's `Parent` is
  saved as its parent's ID, and a root's isn't. A prefab's instance, as Prefabs says below, is
  saved as what differs from its prefab: its root as its `PrefabRoot`, its `Transform`, and each
  other component that differs from the prefab's root's; each entity its prefab gives under it
  where something differs, under its mixed ID, and the rest not at all; a component the prefab
  gives as a struct of named fields, as the fields that differ alone, in their description's
  order, a field left out taking the prefab's rather than its default, and a component of another
  type, or one the prefab doesn't give, whole; and a `Parent` that's the prefab's, not at all. A
  save makes each component again as a load would, so that every scene saved loads; a load checks
  everything before it spawns anything, all or nothing, and a refusal begins with the line and
  column where the text is refused, and, but for a syntax error, names the entity, the component
  and the field.
- **Prefabs.** A prefab is a `.prefab` file under the asset root holding a scene of one subtree, in
  canonical text, an asset as a texture is, with its ID file, named by its asset ID or else its
  path, and cooked into the package as it is. An entity with a `PrefabRoot` naming one is an
  instance of it, linked: the world spawns the prefab's entities under it as it comes with one,
  whether a scene loads it, a tool's create or add gives it, or code spawns it through
  `Prefab::spawn`, and spawns another's in their place as it's given another prefab. The root takes
  the prefab's root's components but its `Transform`, which is its own and places the instance;
  each other entity's ID is mixed from the root's and the prefab's own for it, `mix64(local ^
  mix64(root ^ LINK))`, by a constant of its own, so that the root's ID alone gives them wherever
  it's loaded, made or restored, and they meet neither a copy's nor the root's model's nodes';
  references inside the prefab are to the instance's own entities. An entity of a prefab may link
  another, its entities' IDs mixing again from its own, as nested in the outer prefab's text, which
  saves what differs from the inner prefab as a scene does; a prefab that links itself, through
  others or not, is refused (`scene.prefab_cycle`), as is one whose root links another
  (`scene.prefab_variant`); a model's root in a prefab spawns its nodes as anywhere. The world
  keeps `PrefabRoot` as it keeps `ModelRoot`: code reads it but changes it no other way, a set by
  name refused (`reflect.read_only`). An instance's overrides are what differs from its prefab,
  found by comparison, never kept apart: a scene saves them, as Canonical text files says above,
  and a load takes the prefab's for the rest, the outermost instance's prefab giving an entity of a
  prefab nested in it as it has it. A save refuses an instance missing an entity or a component its
  prefab gives, as one code took away (`scene.prefab_missing`), or with an entity code made a root
  or put under one the scene doesn't save, which a load would put back under its prefab's parent
  (`scene.prefab_parent`); and a load refuses an instance's entity given a `PrefabRoot` of its own
  (`scene.prefab_entity`). Tools' operations change an instance's entities' fields, give them
  components and children, and create, copy and delete the root, its entities following, as one
  edit, a copy's mixed from the copy's ID with what the original changed; change the root's prefab
  or take it away, its entities and the components its prefab gave it with it, an undo giving
  back each as it was, under its handle; unlink it (`unlink`), its entities staying under their IDs,
  its own from then on; and set a field back to its prefab's (`revert`); but refuse to delete or
  move an entity its prefab gives, remove a component the prefab gives one, or give one a link, or
  change or take away a nested instance's (`ecs.prefab_entity`), and to revert what the prefab
  doesn't give (`ecs.no_prefab`). A link (`Operation::Link`) is an unlink's inverse: it gives a
  root a `PrefabRoot` and spawns nothing, its subtree holding each entity its prefab gives under
  the ID the instance mixes, under its prefab's parent, with each component the prefab gives it,
  and is refused otherwise (`ecs.prefab_missing`, `ecs.prefab_parent`), for one that links a
  prefab its prefab doesn't link it to (`ecs.prefab_entity`), and for a root linked already
  (`reflect.has_component`); an unlink's undo, and a link's redo, are checked and refused alike,
  as another author may have changed the subtree since. The tool API's `entity` says of an
  instance's entity what differs from its prefab, by the comparison a save makes: the outermost
  instance's root and prefab, each component its prefab gives that differs, with the fields that
  differ, a struct of named fields' by name and another type's whole, and the components it has
  that its prefab doesn't give, but a root's `Transform` and `PrefabRoot` and a `Parent`, the
  instance's own; a read parses the prefab's text and changes nothing. Its `save_prefab` saves an
  entity and its descendants as a new prefab and links the entity to it, as one edit of the
  client's, which an undo unlinks, leaving the file: its text written whole over nothing, at the
  path given or in `prefabs/` under the root, named after the entity's `Name`, cut to 64
  characters, numbered while that or its ID file is taken, with its ID file, as the game writes
  an asset's, and the asset database taking it at once, a new prefab of the text written whatever
  a file there gave before; everything checked before anything is written, the link too. Each of
  the prefab's own entities' local IDs is `unmix64(id) ^ mix64(root ^ LINK)`, where `unmix64`
  undoes `mix64`'s steps, its xorshifts and its multiplications by odd constants, so that the
  instance gives each the ID it has, the root's own among them, and the link changes no ID,
  references from elsewhere staying whole; a nested instance's entities' IDs mix again from its
  root's local ID, as a prefab's text has them; and the prefab's root's `Transform` is the
  identity, the instance's own being the entity's. A save is refused for an entity an instance gives (`ecs.prefab_entity`), an
  instance's root, as a prefab whose root links another is (`scene.prefab_variant`), a model's
  node (`ecs.model_node`), a file that's there, or the ID file of one deleted, whose ID scenes may
  name (`asset.exists`), a path outside the root, whose folder isn't there, or that the asset scan
  skips, as gg-api's `reached` says, so that no run would find it nor a package hold it
  (`asset.outside_root`), a text of more than 16 MiB, which no load reads (`engine.bad_prefab`),
  and as a scene of the subtree is, as one that refers outside it (`scene.outside`); and in a run
  from a package or a networked one, or while a copy of the world as edited plays, which `stop`
  takes away with the link (`api.no_scene`).
  In a development run that serves the tool API, neither recorded
  nor in a lockstep session, as the prefab's file is saved, every instance takes what changed, in
  the world and the world as edited alike: each field it had as the prefab did takes what the
  prefab has now, and each that differed is kept; a component the prefab gives now is given, and
  one it gives no more taken away where it was the prefab's; an entity the prefab gives now
  spawns, and one it gives no more goes, with what's under it but the prefab's entities, which go
  first where it has them now, under their stable IDs; and an instance an undo or a redo puts back
  takes what changed while it was out. It logs `Prefab reloaded`, or warns with the code of a text
  that either world refuses, the prefab and its instances staying as they were. Elsewhere the change
  waits for the next run (`asset.prefab_changed`). `World::place_prefab` places a prefab's text as
  a copy instead, unlinked, each of its IDs mixed from one drawn for the copy and the prefab's
  own, `mix64(local ^ mix64(instance ^ PLACE))`, a permutation of the prefab's IDs for each copy,
  so that two copies' meet only by chance. [prefab.rs](crates/gg-engine/src/ecs/prefab.rs) and
  [template.rs](crates/gg-engine/src/ecs/scene/template.rs) have the rest.
- **UIs.** A game's UI, as [Events and layers](#events-and-layers) says, is a `.ui` file under the
  asset root, an asset as a texture is, with its ID file, which a layer adds by its path or its
  asset ID (`Context::add_ui`), each add a UI of its own of the file read once, found by its ID
  wherever its file has been renamed or moved, and which is cooked into the package as it is. It's
  RON as the canonical files are written, a part to a line: `(focus: "play", nodes: [...])`, the
  button or widget focused as it's added, by its name, if any, and the nodes at its top, each its
  kind, `Panel`, `Text`, `Image`, `Button`, `Slider`, `Toggle`, `Dropdown`, `Field`, `Row`,
  `Column`, `Stack` or `List`, and its fields inside its kind's parentheses, as `Button(name:
  "play", text: "PLAY")`, a field it leaves out taking its kind's default and one that may be left
  out written as its value. Every node has its `name`, `visible`, `width` and `height`, each
  `Fixed(units)`, `Fit` or `Fill(share)`, `margin` and `anchor`; a container its `children`,
  `padding` and `align`, a row, a column or a list its `gap`, and a panel, a row, a column or a
  stack its `clip`, which clips what's under it to it, as a list always does; a panel its `color`,
  `texture`, `radius` and `slice`, and a list its `color` and `radius`; a text its `text`, `font`,
  `size`, `color` and `align`; an image its `texture`, fit to its pixels, and `color`; a button its
  `text`, `font`, `size`, `padding`, `radius`, `slice`, `disabled` and `looks`, each of its states'
  `color`, `text_color` and `texture`, a look's texture the normal one's unless it gives its own;
  and each widget its `padding`, `radius`, `disabled`, `looks`, as a button's, and `accent`, the
  color of what's on, chosen or typed in it, which a list's scroll bar takes too: a slider its
  `min`, `max`, `value` and `step`, 0 for none, 400 units by 48 by default; a toggle its `text`,
  `font`, `size` and `on`; a drop-down its `options`, 1 to 256, its `choice`, by its place among
  them, its `font`, `size` and `color`, behind its options as they open; and a text field its
  `text`, `font`, `size`, `hint`, what it says while it's empty, and `most`, the characters it
  holds, 1 to 4,096, 64 by default. Colors are linear RGBA, as a scene's, and a font or a texture is
  named by its asset ID or its path from the root, loading with the UI. A name is 1 to 64 ASCII
  letters, digits, `_` and `-`, its UI's alone, and every button and widget has one, which its
  presses and changes are told by. A load checks the whole file, refusing it at the line and column
  where its text is refused with `ui.syntax` for text that isn't a UI's, as of a field no node has,
  `ui.bad_name`, `ui.duplicate_name`, `ui.bad_field` for a field its kind doesn't take, an image
  without a texture or a drop-down without options, `ui.unnamed_button`, `ui.unnamed_widget`,
  `ui.bad_value` for a number outside its range, a size, a margin, padding, a gap, a radius, a
  slice's width, a text's size, a fill's share and a slider's step each 0 to 1,000,000, and a
  slider's bounds and value -1,000,000 to 1,000,000, at the number, a slider whose value is outside
  its bounds, a choice none of its options, or a text field's text past what it holds, or
  `ui.too_many_nodes` past 10,000 nodes, so that what a file comes to, and each UI's copy of its
  nodes, stays bounded; and with `ui.no_node` for a `focus` no button or widget has. The engine
  writes none, as people and agents write them. In a development run that serves the tool API, as a
  UI's file is saved, every UI of it takes its nodes, each keeping what code set of it, its text,
  whether it shows and whether it's disabled, and what code or input set a widget to, by its name,
  where its kind still takes it, a slider's value within its bounds, a list its scroll, and the
  focus staying on the node of its name, or else going to the file's, logging `UI reloaded`; or it
  warns with the code of a file it refuses, or of a font or a texture that doesn't load, and the UI
  stays as it was. A recorded run and a lockstep peer take it too, as a UI is the run's alone.
  [game_ui](crates/gg-engine/src/game_ui/mod.rs) has the rest.
- **Models in scenes.** A model is placed as an entity with a `ModelRoot`, which names it as a
  texture is named, by its asset ID or else its path; the world spawns the model's nodes under it
  as it comes with one, whether a scene loads it, a tool's create or add gives it, or code spawns it
  through `Model::spawn`, and spawns another's in their place as it's given another model, as a set
  of it does. The nodes are the model's: each node of its default scene an entity, with a
  `ModelNode` saying which part of the model it is, and a child for each primitive of a mesh with
  several. Their IDs are mixed from the root's and their parts', as a prefab's copy's are, so that
  the root's ID alone gives them wherever it's loaded, made or restored, and a scene saves the root
  alone, refusing an entity code put under a node (`scene.model_node`). The world keeps
  `ModelRoot` and `ModelNode`, as it keeps `Parent`: code reads them but changes neither; tools'
  operations refuse to change a node, delete it, copy it, put anything under it or give an entity
  a `ModelNode` (`ecs.model_node`, `reflect.read_only`), as a scene's text can't give one either,
  and change a root's model, take it away or copy the root, its nodes following, as one edit, an
  operation naming the root alone, never its nodes, which go with it however many its model has
  by its undo; a scene file's save edits roots alone; and the tool API's `pick` gives a node's
  root. Code may change nodes as the game runs, as snapshots keep them, and a world restored from
  one has them as it was, but for draws that a run of other assets left out, which come back from
  the model; as the model's nodes change, as its file's read again, nodes that keep their places
  in its hierarchy stay, as code left them but for their draws and the transforms the model
  changed, and the others are spawned again, under the same IDs. Unlike a prefab's instance's, a
  node's fields keep no overrides: what code changes of them lasts the run.
- **Stable IDs.** Every entity has a persistent ID, separate from its runtime handle, that
  files, operations, diagnostics and network messages use to name it. IDs are random rather
  than sequential, so entities added on two branches neither collide nor land on the same
  lines.
- **Asset IDs.** Each texture (`.png`, `.jpg`, `.jpeg`, `.hdr`), model (`.gltf`, `.glb`), font
  (`.ttf`, `.otf`), prefab (`.prefab`) and UI (`.ui`) under the asset root, whatever its extension's
  case, has an ID that outlives
  its file's name: 64 random bits, never 0, written as 16 lowercase hex digits, as an entity's
  are, in an ID file beside it, its file's name and `.meta`, such as `stone.png.meta`, matched by
  that exact
  name, a name that matches only when case is ignored being a problem (`asset.meta_case`), so that
  a file system that ignores case gives an asset the ID one that doesn't gives it. An ID file is
  canonical RON, a part to a line, `(`, `    id: "5f3c9a1e07b2d846",`, `)`; a part it hasn't is
  refused, and one added since, as import settings will be, takes its default where a file leaves
  it out. It's committed with its
  asset, and moves with it. A development run that serves the tool API writes the ID file of each
  asset that has none as it starts, before its layers load, and from then on as a thread of its
  own, `gg-assets`, finds them: it scans the root every second, or at once as the API's `assets`
  asks, which it answers once a scan begun after it has taken what it found, or, while frames
  pause, from what it last took, and takes a change once it holds from one scan to the next, 100 ms
  on. That is unless the run is a lockstep session's joiner,
  whose host alone writes them, as it alone watches scene files, and whose thread only reads them.
  Each is written only where nothing is there, whole or not at all, linked into place, so that two
  runs never give one asset two IDs, warning of one it can't write (`asset.unwritten`); each ID drawn
  until it holds a letter but `e`, so that its text never reads as a JSON number. Another
  development run reads them and warns of each asset that has none (`asset.no_id`), as `gg check`
  reports; builds without `dev` and replays read none; and tools never write one: gg-api writes
  none, and the editor's own app names no root. The scan goes through every folder under the root
  but those whose names start with `.`, every `target`, and at its top `tests`, `benches` and
  `examples`, as a package's root holds them, never through a link; it skips hidden files too, and
  warns of what it can't take (`asset.unlisted`), such as a name that isn't Unicode or a folder it
  can't list. A run warns once of each problem, and rewrites nothing, since an ID file that doesn't
  read may be a merge's conflict or someone's edit: one that doesn't read (`asset.bad_meta`), its
  asset going without an ID; one without its asset beside it as a file of its name
  (`asset.orphaned`); and one holding the ID another asset has this run (`asset.duplicate_id`), as a
  copy's does, a run's first scan giving each ID to the first asset by path whose ID file holds it,
  the others going without until their ID files are removed, after which a run writes each a fresh
  one. `assets` gives each asset's problem, and the orphans, as data. An asset's ID, once a run has
  it, stays its for the run, wherever it's renamed or moved: an ID file changed since, as by a
  branch's checkout, warns (`asset.id_changed`) and counts from the next run, and a run writes its
  ID back only while no other ID file its scan found holds it.
  - An asset renamed or moved keeps its ID. With its ID file, as `git mv` of both moves it, its ID
    goes with it at the next scan, the asset at its old path gone. Without it, as a plain `mv`,
    `git mv` or Explorer's rename of the file alone leaves the ID file behind, a run that writes ID
    files moves that ID file beside the file, linked there and then removed, if the file is new to
    the run, has none, and holds what the asset of that ID last held, as no other new file does:
    by a print of its length and its bytes, mixed 8 at a time by SplitMix64's mix. It logs
    `asset.renamed`, as its thread takes the rename, or, for one made while no run watched, as the
    next run starts, by the prints the run keeps in its clone's cache, a file of the asset root's
    own in the app's directory for caches, which every build of the clone shares; its thread reads
    each asset's file for its print as it changes, and every one in a clone's first run. An asset
    renamed and edited at once, or one of two new files alike, as a copy's twin, gets a fresh ID,
    and the ID file left behind is warned of (`asset.orphaned`), naming its ID: moved beside the
    file in place of the fresh one, it gives it that ID from the next run. A fresh clone's first
    run has no cache, so a rename made before it gets a fresh ID too. A file moved over one the
    run has, its ID file with it, takes that one's place, keeping its ID for the run, as any ID file
    changed does. A texture loaded from an asset's file before it was renamed is the one its new
    path gives, loaded once.
  - Scenes, the world's hash, recordings, snapshots, lockstep inputs and the tool API name a
    texture, a model, a font or a prefab by its name: its asset ID, or a model's image by its
    model's and its
    place in it, such as `5f3c9a1e07b2d846#0`, so that a file renamed with its ID file changes none
    of them; or else its path, as for an asset outside the root, or whose ID file is missing in a
    run that writes none. Each is read by a name or by a path to the asset's file. An asset is
    named as it's added, by the ID its run's database holds for its file then, and keeps that name
    for the run, but in a replay or a lockstep joiner, which go by the names they're given, as
    below.
  - A texture, a model, a font or a prefab that a scene, the tool API's edit or `scene open`, or a
    recorded or a lockstep peer's command or edit names, and no layer has added, loads there,
    before the text is read, as `Context::add_texture`, `add_model`, `add_font` and `add_prefab`
    load a file, a prefab with what its text names, in turn, and is named as
    they name one, its file found by the name: in a run that follows another, a replay or a
    lockstep joiner, at the path the name was given; else by an ID, as the run's package or
    database has it, which a joiner has but a replay without a package, reading no ID file,
    doesn't; else by a path from the asset
    root, which mustn't be absolute or lead outside it, as through `..` or a link. A model's image
    loads with its model, which a name of the image loads. A name that doesn't load refuses what
    names it where it's asked for (`asset.no_texture`, `asset.no_model`, `asset.no_font` or
    `asset.no_prefab`, the API's `api.no_texture`, `api.no_model`, `api.no_font` or
    `api.no_prefab`), saying why: `asset.model_image` for
    an image the model hasn't, `asset.unknown_id`, `asset.outside_root`, or the add's own, as
    `engine.bad_texture`. In a run that follows another, and in a restore, a stand-in takes its
    place instead, of magenta and black squares for a texture, of no nodes for a model, for a font
    one whose text draws in the engine's own, and for a prefab one whose instances are their roots
    alone, named as named, warning `asset.stand_in`, so that the run goes as the other did, its
    hash reading names alone, but for a model's nodes and a prefab's entities, which its roots
    spawn none of then. What's loaded so is noted among what the layers added, so that a snapshot's
    `asset` lines list it, and a run restored from one loads each texture, model, font and prefab
    its run had, in order, before it reads the world. A lockstep host loads what a scene
    file's save names before it offers it, so that their names go out first, and offers none
    naming an asset it can't load; a joiner loads for its tool API only what its host named
    (`asset.not_given`). Scene text that a layer loads into a world itself, as inside a tick, finds
    only what's loaded, but a scene a tick changes to loads what it names, as Scene changes says
    below. Each is read, and a font baked, on the main thread as it's named, as a
    layer's add does, a texture's pixels decoding after, as [Rendering](#rendering) says.
  - A layer removes a texture, a mesh or a model it added (`Context::remove_texture`,
    `remove_mesh`, `remove_model`, a model's meshes and images with it), freeing its memory, but
    changes nothing the simulation reads: the asset keeps its name, which still finds it inside
    ticks, and a model its nodes, so that a world's hash, scenes and recordings don't depend on when
    a layer removed it, and a stale handle draws a stand-in or nothing. What loads an asset, a
    layer's add or text that names it, takes a removed one for absent, and loads its file anew under
    a new handle, which its name then finds; a watched file is watched no more. A snapshot notes the
    removals among the assets its run added, in order, so that a run restored from it, or a lockstep
    joiner, adds and removes what that run did, and its handles, and the `MeshDraw`s holding them,
    line up. Shaders, fonts and the engine's own assets aren't removed.
  - A run from a package names each asset by the ID the package gives it, as a development run
    names it by its ID file. A build without `dev` that runs from no package, as an app that is
    no project's game does, reads no ID file, and names every asset by its path, so that a scene,
    a recording or a snapshot naming one by its ID finds none there.
  - A run's names travel with what another process reads of it, so that the other goes as it did
    whatever ID files its disk holds, as a checkout without those not yet committed. A recording
    has a `named` line for each texture, model, font and prefab named otherwise than by its path,
    the name and the path, by which its replay, reading no ID file, names its assets. A snapshot
    file has
    them too, by which a run restored from it reads its values, an asset whose ID changed since
    coming back by its path, though the run names it its own way, and one whose file was renamed
    since, its ID file moved with it, from where the run's database has that ID now, its `asset`
    line following it, so that what's opaque comes back. A lockstep joiner names every asset as its
    host does, by its path until the host's world comes with its names, which rename those it has
    added, as each world the host sends again does, and by the names the host sends of those it
    adds after, before the inputs that may name them; so a joiner's names may be IDs its own disk
    holds no ID file for, which its `assets` doesn't list.
  - A test's context reads the ID files under the root it's given, writing none, as a run that
    serves no tool API does, so that a game's reference hashes name its assets as its runs do.
  [asset.rs](crates/gg-api/src/asset.rs), [assets.rs](crates/gg-engine/src/assets.rs) and
  [asset_watch.rs](crates/gg-engine/src/asset_watch.rs) have the rest.
- **Operations.** Every authoring change, by a person or an agent, is an operation with an
  inverse: create, duplicate or delete an entity, add or remove a component, set a field,
  reparent, and unlink a prefab's instance, link one back or revert a field of one to its
  prefab's, as Prefabs says above. An operation names entities by their IDs and carries values as reflection's, and
  applying one checks everything before anything changes. A delete's and a remove's inverses keep
  what they took out, typed, so that what's opaque comes back too; a set's keeps the component it
  changed, whole, until nothing can roll its edit back, so that an edit refused partway rolls back
  exactly what the world held, though code had stored in it what a set refuses, such as a rotation
  no longer a unit; and, where the set may take something opaque out, as a list's element or an
  enum's variant, for good, so that an undo the world refuses for making what's opaque swaps the
  field back from it, what's opaque in it too, the rest of the component staying as it is. A set of a list of another length keeps what's
  opaque of the elements alike at its start and end, by their values, so that an element taken out
  takes its own. A component can name a `bool`
  field of its own exclusive, as a camera's `primary` is, so that one edit, a person's or an
  agent's, switches which entity has it, and one undo switches it back. A log keeps the edits
  applied, in order, each its operations with their inverses: it undoes and redoes an edit whole,
  applies one of several operations all or nothing, merges a gesture's sets of one field, such as
  a drag's, but an instance's link, into one edit, a tool API client's too, by the number it gives
  each of its gestures,
  and drops an entry whose undo the world refuses, as where a set's old value is one that code
  stored and a set refuses: an undo sets each field back, checked, keeping what has changed the
  component's other fields since. A running game applies the tool API's edits
  between ticks, as a frame starts, paused or not, in the order they arrive, so several clients
  can edit it at once; each author, a client by the name it says hello with or a scene file by
  its path, undoes and redoes its own through a log of its own, as the Inspector does through its,
  and a recording keeps each edit as it was asked for, before the tick after, where a replay
  applies it. Editing a scene file while the game runs is an edit too: one a game loaded through
  `Context::load_scene_file`, saved, applies in a development build as the operations that take
  the world's entities from what the file held to what it holds, which keep what the file doesn't
  change, such as where the game has moved an entity, and leave alone what the game has deleted;
  the entities it adds are made first, so that the rest may hold them, and the links of instances
  it changes are changed first: one it takes away unlinks its instance, and one it gives deletes
  the world's own entities that hold the IDs its prefab gives, as an unlink leaves them, each such
  instance then taken for what the file holds. A snapshot keeps the files
  watched, as last applied, so that a restarted run watches them too.
  [ops.rs](crates/gg-engine/src/ecs/ops.rs), `World::undo` among it,
  [log.rs](crates/gg-engine/src/ecs/log.rs), [edits.rs](crates/gg-engine/src/edits.rs) and
  [scene_files.rs](crates/gg-engine/src/scene_files.rs) have the rest.
- **The open scene.** A project's game has one scene open: the scene file it started in, or one
  opened since. The tool API's `scene` opens, saves and empties it, and so does the editor's File
  menu through it.
  - What a scene owns: the entities its file held as last applied, and those edits have made into
    it while it's open, a client's creates and duplicates. Never those the game's code made, so a
    game whose attach builds entities beside its scene, as Shards' players are, saves none of them
    into it.
  - Save writes exactly the open scene's entities, of the world as edited even while a copy plays,
    to its file or to another, which then becomes its file. The game's watch then applies nothing,
    since the file holds what the watch last applied. Save refuses a file changed since then
    (`scene.changed`), so an edit to it isn't lost.
  - New deletes the open scene's entities, and Open puts a file's in their place. Each stops a copy
    first and keeps the game's own entities, including a child of a scene entity.
  - Neither New nor Open can be undone: the world goes on as another (`World::renew`), so every
    author's log, and the Inspector's, starts afresh, as after a restore.
  - The open scene is unsaved when its entities, as Save would write them, differ from its file. A
    snapshot keeps which scene is open and what edits made into it.
  - Recorded runs and lockstep peers refuse New and Open, and lockstep peers refuse Save too
    (`api.no_scene`): neither a replay nor the other peer could follow them.
- **Scene changes.** A game changes its scene as it plays, from its menu to a level, from a level
  to the next or to its own start again, from a tick: `World::change_scene` names a scene file by
  its path from the asset root, `/` between its parts, as `scenes/level2.ron`, and as the tick of
  the layer that asked returns, before the next layer's, the scene's entities take the place of
  every entity of the world's, the last it asked for in its tick going in; a layer above may ask
  again in the same tick. One asked outside a tick, as in an update, is dropped as the next tick
  starts (`scene.not_in_tick`). The change is the simulation's own, as a command is, rather than an
  edit from outside the layers: a replay, each lockstep peer, a tick run again by rollback or a
  SyncTest, and a server and its clients predicting it, make it at the same tick, so it needs no
  line of a recording's nor an input of a peer's. The world stays the world, its `World::scene`
  saying which scene its entities came from.
  - A run reads a scene's file once, from the game's package in a run from one and else from the
    disk, and keeps its text for the changes after: a development build every file in the project's
    scenes folders as it starts, after its layers load, and any other as a change first names it.
    Its saves reach the text kept as edits (`Edit::Scene`), as a scene file's saves do, so that a
    recording replays them where they were made and lockstep peers take them at one tick, and a
    snapshot keeps the texts, which a run restored from it and a lockstep joiner, which starts from
    its host's world, take in place of their own. A replay reads them from its disk as it starts,
    as the run did, so that a file changed since parts its hashes from the recording's.
  - What it names that no layer has added loads first, as for `Context::load_scene_file`: its
    models and prefabs, none standing in, a model or a prefab that doesn't load refusing the change
    (`asset.no_model`, `asset.no_prefab`) before anything stands in; then its textures and fonts, a
    stand-in taking the place of one that can't be loaded, warning `asset.stand_in`, as in a run
    that follows another. A recording's `named` lines carry what it loaded by ID to the replay, a
    lockstep host's names to its joiner, and a snapshot's `asset` lines to a run restored from it,
    which comes back in the scene. A server's session, server and clients alike, loads nothing as it
    plays, as a client reads its server's states only of the assets its layers loaded, and refuses
    a scene that names another (`net.no_scene_change`): a game served loads what its scenes name as
    its layers load.
  - Every entity goes, the game's own too, as despawns take them, so that no handle from before
    names one, and its resources stay, as what a game carries from scene to scene. A layer that
    keeps handles finds its entities again at its next tick. The authors' edits, the tool API's
    clients' and the Inspector's, stay in their logs, but each entry notes how many scene changes
    the world had been through as it applied, and one applied before a change refuses to undo or
    redo (`ops.scene_changed`), as the scene may have put another entity under an ID it names, as a
    level started again does; it stays where it is, keeping its slots, since clearing or dropping
    it would free slots on one peer and not the other, and those applied after undo as ever.
  - What the run keeps of the scene beside the world follows once the tick is final, so that a tick
    rollback takes back leaves none of it: at once in a run alone, a replay, lockstep and a server;
    in a rollback session once the tick's inputs are both peers', whether it ran again or its
    prediction held; on a client never, its server's the world that's final. Then the run logs
    `scene.replaced`, and the scene's file is the one the run applies the saves of to the world, in
    place of those its layers loaded, whose entities went, and in a project's game the open scene.
    A world held aside while a copy plays brings its own back at `stop`.
  - A scene that isn't there (`scene.not_found`, naming it), a name that isn't a path inside the
    root (`scene.bad_path`), one not named exactly as its file is (`scene.case`), which a package or
    Linux's disk wouldn't find, a link out of the root (`asset.outside_root`), a file of more than
    16 MiB, a run without a root (`scene.no_root`), or a scene that doesn't load, with its own code,
    changes nothing, alike everywhere: the world notes the refusal, which the run logs at the error
    level once the tick is final, as it logs a change that went in, with how many were refused
    since.
    [scene_change.rs](crates/gg-engine/src/scene_change.rs) has the rest.

## ECS

The world lives in our own ECS, built for what the tools, hot reload and multiplayer need:

- **Reflection:** every component type, whether Rust or a Luau script declares it, is registered
  at runtime with a description of its fields. Saving, operations, the inspector, hot reload's
  snapshots, network replication and Luau's bindings all work from it. A type a script declares
  is stored flat in its tables' columns, as a Rust type is, so that a snapshot copies it without
  allocating for each row.
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
  A patch of the run's code that the tool API applies is recorded before the tick after it, which a
  replay, running its own code, doesn't run, ending at it with `replay.patched` once every tick
  before it has replayed to its hash.
- **Determinism:** the fixed timestep, a fixed order of systems and of iteration, and math that
  gives identical results on every platform, so lockstep games and rollback netcode stay in sync.
  So glam is built without SIMD and with libm's functions, for the whole build
  ([Cargo.toml](Cargo.toml)), and clippy refuses std's float methods whose precision is the
  platform's ([clippy.toml](clippy.toml)), for `gg_core::math`'s. Luau's math is held to the same:
  its functions whose precision is the platform's are libm's, a script that uses `^`, which
  calls the platform's `pow` past any hook, is refused, and its libraries' functions that make an
  integer of a number, by a C cast that platforms make otherwise past the integer's range, fail at
  such a number, as `string.format` does at a float it would write as NaN or infinity, which C
  libraries write each their own way. A recorded run hashes the world once its
  layers attach and after every tick, so that its replay, on another machine or after a change,
  finds the first tick where it goes another way. `World::difference` names the entity and field
  where two worlds part in one process; across two machines, `--dump` writes the world there a
  field a line, and the two dumps diff to them.
- **Ticks run again:** a SyncTest (`--sync-test`) runs each tick again once it has run, a replay's
  too, on a copy of the world taken as it started, each layer given back the commands its tick
  applied, and ends the run where the copy and the world then part (`sync_test.diverged`), naming
  the first entity and field there with both values, as `World::difference` finds them: so that
  what rollback needs of every tick, that it go by the world, its commands and what its layer set up
  alone, and that a copy of the world hold all it reads, is checked as a game runs. The world the
  tick ran on goes on, its serial its own, so that what's kept of it by its serial holds, as the
  authors' logs and interpolation keep it; the copy is put in its place while the tick runs again,
  each layer told as it's put there and as the world comes back (`Layer::swapped`), so that what a
  layer keeps of a world by its serial goes with it, as the Inspector's log does, a copy of it as
  the tick started running the tick's undos and redos again on the copy. The sounds the tick
  queues there stay in the copy, which nothing plays, and what the tick logs, it logs twice. A
  command its queue wouldn't queue again, as one holding something opaque, keeps the tick from
  running again, and ends the run (`sync_test.bad_command`).

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

Each table keeps, for each of its columns, the change tick at which each of its rows last changed,
and when each row joined it, as the world reaches them to change them: a row spawned or moved into a
table is new in each of its columns; `get_mut`, `insert`, a set and an operation mark the row's
component; a query's `&mut` term marks its column's rows all, without visiting them; and a rewind
marks every row, as rows came back as they were. A table keeps its columns to itself, so that
nothing reaches one to change it but through what marks it. The world's change tick moves on as
each tick starts and once each frame has drawn the world, so that drawing finds what changed since
it last looked, and costs what changed rather than what's drawn: a column's latest mark tells at
once whether any of its rows did. No hash, scene, snapshot file or recording holds the ticks.

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
a texture as its handle, which the renderer alone makes, so that no value makes an index that
shaders sample unchecked, and which scenes, snapshots, recordings and the tool API name by its name,
its asset ID or else its path, as [Project state](#project-state) says; and `Opaque` for what
reflection keeps but doesn't read, such as a `MeshDraw`, which no value may
make. The world reads and sets a field by its component's name and a path, such as
`rect[1].y`: a set checks the whole value against the description before anything changes, and
returns the old value, which sets it back unless code stored one that a set refuses; so an edit
rolled back puts the component back whole instead, and a duplicate points its copies' references
at each other in place, through `Reflect::entities_mut`, rather than setting them again. The
Inspector reads the world through these calls, its IDs and its hierarchy, and edits it through
operations, all of which the tool API serves, so that a remote Inspector shows a game's world in
another process, as the editor's does: it reads the game's entities and the picked one's
components through the API a few times a second, and the descriptions of its types once, and
again as a type it hasn't comes or the types' revision moves on, as a script's reload changes
them, with one request at a time, so that what it reads after an edit's
answer shows the edit, and edits the world as the game's client, drawing what waits for an answer
as it will be. Values cross as JSON, read and written by the types the game describes, an entity's
by its ID and a texture's by its name.

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
compile, and `set_field` and operations refuse it by name. A reparent keeps the entity's own
transform, so that it moves as its new parent is placed; the editor's panels, and the tool API's
`reparent` given `in_place`, keep it where it is in the world instead, a set of its transform
joining the reparent in one edit, as `World::reparent_in_place` finds it from the two world
matrices, decomposed, which loses any shear they give it between them. A parent's children are in
the order they became its children, which means no more than the order queries find entities in.
Drawing
keeps each entity's world matrix, its parent's times its own transform's from its root down, and
its rotation likewise, outside the world, as the latest tick left it, and places again, after the
frame's ticks, the entities whose transforms or parents changed, or that joined a table, and what
they carry, each once, from the topmost that changed; `World::pose` computes an entity's for code,
bit for bit as drawing places it.

A snapshot is the world copied: `World` is `Clone`, and `clone_from` restores a world from a
snapshot, or takes a snapshot into one kept from before, in the memory it holds, so that one taken
every frame allocates nothing while the world's tables, its component types and its size stay, for
components and resources whose `Clone` allocates nothing. It copies all of the world, so that a
restored world goes on exactly as the world the snapshot was taken from did, spawning the same
handles with the same IDs and finding entities in the same order. A copy is another world, with a
serial of its own, which refuses what undoes an operation applied before, so that undo history
doesn't outlive a restore; but rollback takes the very world back to a copy of it taken since a
log last changed it (`World::rewind`), its serial its own, which its logs undo through as before.

A snapshot file holds a world whole as text, so that a run restored from it, in another process,
after a crash or a rebuild, goes on where the world was: each slot under its generation, the free
ones in their order and the rest that hold no entity, so that every slot is named and a load makes
no more than its lines name; each table in the order it was made, empty ones too, and its rows in
theirs, each with its entity's ID and its components' values, as a recording's text writes values,
by their fields' names; each parent's children in their order; the stream of IDs; the resources that
save themselves; the ticks the simulation had run; the assets the layers had added, the textures
loaded where text named them among them, which a run restored from it loads again before its world,
and the names its textures had, as [Project state](#project-state) says; and, of a game the tool API
stopped, that it's stopped, or of one playing a copy, the world as edited too, which it held aside,
with its ticks, its hash and its scene files. Loaded, it's the world saved, hash for hash, but for
the slots a log kept vacant, which no log in the new process gives back, so that they're freed. A
component whose type changed since is matched to the type as it is by its fields' names: a field
added takes its default, one dropped goes, a number of another kind is taken where it's exactly a
number of the field's, and a component that no longer reads, or whose type is gone, is left out, its
entity going on without it, each change warned of once, by the type's path rather than by value
(`snapshot.changed`); a rotation that rounding took off unit length is normalized. What's opaque,
such as a `MeshDraw`'s mesh and shader, holds indices, so it comes back only into a run whose layers
added the same assets in the same order, a file added again counting again, and the textures loaded
where named among them, of the same types, as the file's `asset` lines check, a draw's bytes by its
mesh's and shader's types' names and sizes, and is left out otherwise. A world no load could read,
such as a value nested deeper than a scene's may, isn't saved.
[ecs/file.rs](crates/gg-engine/src/ecs/file.rs) and [snapshot.rs](crates/gg-engine/src/snapshot.rs)
have the rest.

A world holds resources beside its entities, a value of each type, for the rest of the simulation,
such as physics' bodies and contacts. Snapshots copy them, so that a restored world steps as the
one it was taken from did, and snapshot files hold those whose types implement `Saved`, as physics'
does through rapier's own serialization, while a restored run keeps those its layers made as they
loaded of the other types; no scene, hash, recording or dump holds them, so a resource holds what
the simulation carries on from, or makes again from the entities, as physics makes its bodies
where a world has none.

A world can be emptied, or have its entities replaced with a scene's: `World::clear` and
`World::replace_scene` free every slot, the vacant ones a log keeps too, and keep the component
types, the resources and the stream of IDs, so that no ID is drawn twice. The world is then
another, as a restored one is, with a serial of its own (`World::serial`): it refuses what undoes
an operation applied before, which is what makes freeing the slots a log kept safe. Whoever keeps
state of a world keeps it by its serial, as the Inspector keeps its log, its pick and its open
rows, so that it comes back only with the very world it's of, as when the tool API's `stop` brings
back the world a copy played in place of, with the authors' logs held beside it, the Inspector
taking back its own as `stop` calls `restored`. A world can also go on as another with its entities
kept (`World::renew`), as a scene's New and Open leave it, so that no edit from before them undoes:
it frees the slots logs kept, as `clear` does.

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
authoring: the Inspector's edits, undos and redos wait for its next tick, and another layer picks,
edits and creates through its link, `Inspector::link`, as the editor's handles and drops do. Edits
from outside the layers, the tool API's and the scene files', are the engine's own, which apply
between ticks, and the authors' logs the context's, rebuilt by a replay as the Inspector's is.

## Networking

The engine supplies what each multiplayer model needs, from the same snapshots, commands and
hashes: two peers, on one machine or a LAN, in deterministic lockstep, or predicting each other's
inputs and rolling back where a prediction missed (`--rollback`, of both peers); and a server that
owns the world (`--serve <address>`), which any number of clients (`--connect <address>`) play,
each predicting its own player. A run given `--host <address>` listens there, and one given
`--join <address>` connects to it; the host is player 0 and the joiner player 1, and a server
gives each client the least player no other client has, as `Context::player` tells a layer, which
queues commands for its own player alone, naming it in them, since each run runs every player's
world. `Context::networked` tells a layer whether its run takes part in a session at all, a
server's too, whose player is none as a run alone's is, so that what a game sets up for a run
alone, as Foxball's bot, a session's runs leave out.

- **A tick runs on its inputs alone.** A tick's inputs are each layer's commands, the host's then
  the joiner's, and the edits from outside the layers that apply before it, as a recording keeps
  them: so a lockstep peer runs a tick only once it has the other's inputs for it, and frames go on
  drawing while it waits. Each tick, a peer takes what its layers queued since the last and sends it
  as its inputs for the tick its delay on, [`DELAY`](crates/gg-engine/src/net.rs) ticks in
  lockstep, 62.5 ms, so that the inputs reach the other peer before it needs them, and
  `ROLLBACK_DELAY`, 31.25 ms, in a rollback session; a frame takes what came first. Inputs travel
  as text, the values written as recordings write them, entities by their IDs and textures by their
  names, which the joiner takes from its host, as [Project state](#project-state) says, read into
  values only at their tick, against the world then.
- **Rollback predicts the other's inputs.** A rollback peer runs as far as `PREDICTED` ticks, 125
  ms, past the first it hasn't the other's inputs for, on them predicted to hold nothing: since
  commands set what lasts in the world, as a disc's heading, that predicts the other player goes on
  as its last commands had it. It keeps a copy of the world as each such tick starts. As the
  other's inputs come, between frames, a tick they held nothing for ran as it would have on them,
  and ends as a tick run on them does, its hash sent; at the first they held something for, the
  peer takes the world back to the copy, the very world still (`World::rewind`), its serial its
  own, so that what's kept of it holds, and runs that tick and those after again, each layer's
  queue as it was after, and the frames draw on from where the last started. The layers are told
  nothing, as a tick goes by the world, its commands and what its layer set up alone, which a
  SyncTest checks. A sound that a tick run again queues and its runs before played isn't played
  again; one they played that it doesn't queue has played. Each peer logs how often it rolled back
  as the run ends (`net.rollback`).
- **Edits and patches are inputs too.** The tool API's edits, on either peer, go out with the
  peer's inputs for the tick its delay on, and apply on both before it, so that a request's answer
  comes once its edit has applied. A scene file's save is an edit of the file's text, which each
  peer turns into operations against its own world at that tick, as both worlds are the same, and
  a script's save is an edit of its text, which each peer reloads; the host alone watches scene
  files and scripts. `gg run` gives both peers each patch, in the same order, and the
  host chooses the tick it applies before, its delay on from its next as it gets it, which neither
  peer has run, among its inputs for it, which tell its joiner, which waits at that tick for its
  own patch if it hasn't it yet. These, and the commands of a queue that edits
  (`Commands::edits`), as the Inspector's, apply only at a tick run on both peers' inputs, once:
  a rollback peer runs a tick that applies its own only once it has the other's inputs, as a
  lockstep peer runs each, and rolls back to one that applies the other's, as its inputs come. So
  no edit applies in a tick run again, and the authors' and the Inspector's logs undo through a
  world taken back as before.
- **A joiner starts from the host's world.** As a joiner connects, the host sends its world as a
  snapshot file's text, as it stands between two ticks, with how many patches its code is on, the
  scripts and mods it runs, and its inputs for the ticks after, up to the first the joiner can
  reach; the joiner runs those in place of its own, restores the world in place of its own, as
  a run restored from a snapshot does, applies the patches the host had, and
  goes on from that tick, its own inputs empty until the first it can reach. So a restart of
  either peer, after a crash or a rebuild, rejoins: a joiner that loses its host waits for it,
  frozen, and connects again, while a host that loses its joiner plays on alone, the joiner's
  player still.
- **Desyncs name their field.** Each peer sends the world's hash after each tick run on both peers'
  inputs, and keeps a copy of its world after the tick until the other's hash of it has come; a
  hash that isn't the other's logs `net.desync` with the tick, the host sends its joiner its world
  after that tick, and the joiner logs the first entity, component and field where its own differs
  there, with both values (`net.desync_field`), as `World::difference` finds it: at the tick the
  worlds went apart, before what went apart has spread. Then the host sends its world again, as to
  a joiner, so that the peers carry on in sync, from a world that in a rollback session went by the
  joiner's inputs predicted, whatever they were.
- **A server owns the world.** It runs without a window or a GPU, its layers' ticks alone, as
  their updates draw, in real time, until Ctrl+C, which ends it cleanly, as it does a headless run.
  A client starts from its world, as a joiner from its host's, and sends its inputs, its layers'
  commands, each tick's tagged with the tick; the server applies them in their order, each tick's
  at the first tick it runs that's theirs or past it, its own layers' first and then each client's
  by its player. After each tick it sends each client its state: the world's text, as a snapshot
  writes it, compressed by zstd against the text of the latest state the client has said it has,
  which is its dictionary, about 600 bytes in the demo against 34 KB whole, with the world's hash.
  The server's own world is the only one edited: a client refuses the tool API's edits
  (`net.no_edit`), as the server's next state would take them away.
- **Clients predict their own player, and correct.** A client runs as far ahead of the server as
  the way there and back and two ticks more, on its own inputs alone, which predicts its own player
  exactly and the others as going on as their last commands had them, as rollback predicts; its
  ticks run up to a tenth faster or slower than real time, so that its inputs come two ticks before
  the server runs their ticks, and it waits at half a second past the latest state. It reads each
  state into a world of its own, of its types and assets, logging `net.desync` where that doesn't
  hash as the server's world did, as when a state's text leaves out what the hash reads; and where
  the world it predicted after that tick hashes otherwise, takes its world back to the server's,
  the very world (`World::rewind`), and runs the ticks after again, as rollback does, its inputs
  the server hadn't applied applying in them, those whose ticks have gone at the first, as the
  server would, the sounds played before not played again. A queue that edits, as the Inspector's,
  goes to the server alone. A scene a tick changes to, as [Project state](#project-state) says,
  goes in on the server and on each client predicting it at the same tick, from the assets the
  layers loaded alone. As the run ends a client logs how often it corrected
  (`net.corrected`), and the server what it sent (`net.served`).
- **The other players draw between the server's states.** An entity whose `Owner` component names
  another player, and its descendants, draw on a client between the two states about three ticks
  behind the latest, where the time since the latest came puts it, eased so that the states'
  jitter doesn't show, rather than where the client's ticks left them; the rest draw as every
  run's do. So a collision with another player happens where the client predicted it, ahead of
  where it's drawn: over a 100 ms link, about 1.5 units at the demo's speed.

The session's stream is lines of text over TCP, without Nagle's delay, which keeps them in order
and whole: the hello, the host's world, the hashes, and a tick's inputs that hold an edit or a
patch, name a texture the host has just named, or are too long for a packet. A tick's other
inputs go over UDP, in packets sent at each tick and again a tick's time after the last, each
repeating this peer's inputs for every tick the other hasn't said it has, so that a packet lost
loses none, and naming the tick the peers' worlds were last made the same at, so that one sent
before a resync is dropped. A server's link to each client is the same: a client's inputs go over
UDP, each packet repeating those the server hasn't said it applied, or on the stream if a tick's
are too long for a packet; and the server's states over UDP, in as many packets as each takes, a
state lost being lost, as the next is sent against the latest the client has. `--link` has a
run's links hold each packet that comes, and up to more, losing some, and the stream's lines, as a
slow link would. [net.rs](crates/gg-engine/src/net.rs) and
[server.rs](crates/gg-engine/src/net/server.rs) have their formats and the rest. `gg run --peers 2`
runs a host and a joiner from one build, patching both and restarting both after a rebuild, its
arguments after `--`, as `--rollback`, given both.

## Diagnostics

Diagnostics are data, so agents and tools can act on them as reliably as people read them.

- **Logs** go through `tracing` with structured fields, filtered by `RUST_LOG`. Each run prints
  readable, colored lines to stderr, which keeps stdout for machine-readable output, and writes
  every event as a JSON line to a file that agents and tools read:
  `logs/<app>-<UTC time>-<process ID>.jsonl` beside the executable in a development build, and in a
  build without `dev`, which ships where its executable's directory may not be written, as Program
  Files or a signed .app, in the app's own directory of what it keeps for the user, as the dock's
  layout is, or beside the executable without one; the run logs its path as it starts. It keeps the
  last of those lines in memory, which the tool API's `logs` serves as they come, as `gg logs
  --follow` and the editor's Logs follow them. A run embedded in a tool prints no
  readable lines, since the tool shows its events, which it follows through the API.
- **Errors and warnings name their subject.** Each carries a stable code, a dotted name such as
  `gpu.device_lost`, and the thing it is about: an entity's ID, or a despawned entity's handle,
  since it has none; an asset path, a render pass, a shader file and line, or a scene's line and
  column. What outlives an entity, such as a pick or a queued message, keeps its ID, not its handle.
  A layer that can't go on, as the player can't without its scripts, ends the run with its error
  through `Context::fail`, rather than a panic: the layers detach, and the run's status is 1.
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
  a run that doesn't end in an error of the engine's sums them up in `engine.perf` as it ends, with
  how long it took to start and to stop, the ticks it ran and dropped, the GPU and the build, the
  most of the process's memory that was resident, the GPU's own memory in use, and what the frames'
  arenas hold beside the most a frame took of them, so that an agent can compare two builds by their
  runs. A run that renders logs `engine.loaded` once, at the first frame back after which nothing is
  decoding or streaming once its layers have attached, which counts what they added as they loaded
  and attached, and may miss what they add after: how long it took to load from the engine's
  start, the process's resident memory and its peak, and the GPU's own memory in use and its
  budget. The OS's counts are read then and as the run ends alone. What that time went to logs as
  it's done, each with how long it took, `ms`, what each loads among it: at the info level, each
  scene file a layer loads (`scene.loaded`), the tool API opens (`scene.opened`) or a tick changes
  to (`scene.replaced`), and each
  model, prefab, font, UI file and music added (`asset.model_loaded`, `asset.prefab_loaded`,
  `asset.font_loaded`, `asset.ui_loaded`, `asset.music_loaded`); at the debug level, as there may
  be many, each texture and sound added from its file (`asset.texture_loaded`,
  `asset.sound_loaded`), each font baked (`asset.font_baked`, `cached` if the process had baked
  its bytes before, whose field it takes), the sound's output opened
  (`audio.opened`), each image decoded, a model's among them, from the thread that decoded it as
  it's done (`asset.texture_decoded`, with the time its mip levels took, `levels_ms`), and each
  texture and mesh streamed to the GPU, from when the render thread queued it to when it's drawable
  (`renderer.texture_streamed`, `renderer.mesh_streamed`).

## Events and layers

The engine turns the window system's events into its own: window (close, resize, focus, scale,
minimize), keyboard and mouse, and gamepads' from gilrs. winit and gilrs stay inside `gg-engine`,
so apps and games never see their types.

An app is a stack of layers, which its `main` describes on `App`: layers, then overlays above them.
As the app starts, each layer loads, making what it draws with and registering its component types,
then attaches, building the world; in a run restored from a snapshot, whose world comes from the
file, each loads, then picks up from the world in place of attaching (`Layer::restored`), such as
the values its panels show, as each does again as the tool API's `stop` brings back the world as
edited, its queue emptied first, as a lockstep joiner's world gives way to its host's, and as a
scene's New and Open renew the world, its queue kept. A layer that spawns as it loads panics, since
a restored run would spawn it again. Each detaches when the app ends, ticks with the simulation, as
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
run no pass, egui sees no events, and the game's view fills the window; a build without `dev` runs
without it from its first, as [Builds](#builds) says.

egui, the engine's immediate-mode UI, is the top overlay in development builds: it sees every event
first and keeps those it wants, such as a click on one of its panels, but no key's or button's
release, and `gg-renderer` draws its meshes over the frame.

egui is the development UI. A game's own, its HUDs and menus, is apart from it, on the view's
canvas, `Context::canvas`: text and sprites placed from an anchor, one of the view's edges, its
corners or its middle, which the engine places them from as it hands the frame off, at the view's
size then, so that a resize moves them in the frame it comes in. The canvas scales with the view,
`CANVAS_HEIGHT`, 1,080 of its units, to its height, and draws into it over everything else there,
so that captures and the editor's view show it; it's of no entity, so a pick there finds none. A
layer finds the cursor on the canvas as the last frame showed it, to tell what it's over, and may
draw there itself.

A game's UI is a file, as [Project state](#project-state)'s UIs says: a tree of named nodes, panels,
texts, images, buttons and widgets in rows, columns, stacks and lists, which a layer adds
(`Context::add_ui`), getting a handle, `GameUi`, and drives each frame, through what its file names
and nothing else. The engine lays it out, flexbox-lite, in the canvas's units: each node sized
fixed, fit to what it holds, a text's text measured by its font (`Fonts::measure`), or filling its
share of its parent, none below its fit; a row's or a column's children along it, with a gap between
each two, what's left without a fill to take it before, around or after them as the parent's
alignment says, and across it, as a stack's or a panel's both ways, each where its anchor or else
the parent's alignment puts it; padding inside a node and a margin round it; a hidden node taking no
room. A list lays its children out as a column from its top, scrolled up as far as its scroll says,
within what they reach past it, and one that fills its parent takes the room it's given, however
much it holds; it and a container that clips clip what's under them to their rectangles less their
padding, each node drawn within each such rectangle above it, and nothing of one clipped out whole,
nor a click on what's clipped. Each node at the top is placed in the view from the point its anchor
names, the view's middle by default, and drawn from it through the canvas, so that a resize keeps it
there in the frame it comes in. Layout runs as the UI's drawn only once its tree, its texts, a
list's scroll, the view's size, or a font or a texture it names, reloaded, changed, in the memory
the last layout had, and drawing an unchanged UI allocates nothing, as no input that changes nothing
does. A panel is a color or a texture, its corners rounded by quarters of a disc the engine makes
with the first UI, or its texture cut into nine; a button a panel holding its text, in a look for
each of its states, normal, focused, pressed and disabled; and a widget a box in such looks, which
holds a value: a slider a number, as a track filled as far as its thumb, in its accent; a toggle
whether it's on, as its text beside a switch; a drop-down a choice of its options, which open below
it, or above where they'd pass the view's bottom, over the rest of the UI, unclipped; and a text
field a line of text, with a caret and a selection while it's focused, scrolled as far as shows its
caret. The layer hands the UI the events it's sent (`GameUi::event`), which give back what happened,
at most one thing an event: a button pressed, by its name, a widget changed, by its name, with its
new value, the focus moved to one, or back asked for, by Escape or a pad's East. The arrows, a pad's
D-pad and its left stick move the focus to the nearest button or widget that takes it the way they
go, one level with the focused one first, the stick once a push until it comes back near rest; Tab
and Shift+Tab through them in the file's order, coming round; either scrolling what it focuses into
view in the lists it's in. Enter, Space and a pad's South press the focused button, flip a toggle or
open a drop-down, whose options take every event until it's closed, the up and down arrows lighting
them and Enter choosing; left and right step a slider's value to the next point of the grid of its
steps from its `min`, or to its `max` past the grid's last, and a drop-down's choice. The mouse
focuses what it's over, but a text field, which takes the focus by a click and keeps it as the
cursor passes over the rest; it presses what it's let go over after going down on it, drags a
slider and a text field's selection, while they show and are enabled, and its wheel scrolls the
list it's over, which keeps its scroll while it's hidden. The mouse's buttons and wheel over a
button, a widget or a list, or anywhere while a drop-down is open, count as handled by the layer
that sent the UI the event, so that a click on a button isn't a click on the world below, and
`GameUi::pointer_over` and `Context::pointer_over_ui` say whether the cursor is over one, for a
game that reads the mouse itself. A focused text field takes what's
typed (`Event::Text`, winit's text, not keys' positions) at its caret, as many characters as it
holds, its caret moved by the arrows, Home and End, Shift selecting and Ctrl+A selecting it all, and
Backspace and Delete cutting; the OS's composition of what's typed, IME, doesn't reach it. A hidden
or disabled button or widget takes neither the focus nor a press. Code sets a node's text, whether
it shows, a button's or a widget's whether it's disabled, a widget's value and the focus, and reads
a widget's value, by its name, kept as the file reloads, and turns the UI's input off while the game
plays under it; only input gives a change. Its state, the focus, the widgets' values and what code
set, is the run's, not the world's: each lockstep peer has its own, and none of it is hashed,
snapshot or recorded, as Shards' layer keeps its menu's, so that a change that's the game's, as
Foxball's bot's difficulty, queues a command, as a press does. A choice that changes the game
queues a command, as input does, so that the next tick applies it on every peer, and a replay too; a
Quit asks the run to end through `Context::exit`, as closing the window does. The tool API's `ui`
lists each UI's nodes where they lie, as [Agents and tools](#agents-and-tools) says.

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
it has focus, which a press on it gives, or a layer's own control through `Ui::focus_view`, as the
editor's Play does, and `Ui::view_focused` reads, or the cursor is over it, so that keys follow the
cursor onto the view. Layers know where the view is from `Context::view`, in the window's pixels,
as the cursor's position is.

A panel the layout doesn't hold yet floats, unless the app docks it with `App::dock`, beside a
panel, taking the share of its place the app gives, or among its tabs, which is how an app makes
its first layout, for the window's size as it opens, which `App::window_size` gives. The layout is
saved as RON in the OS's directory for per-user state, as are the layouts saved by name, never
beside the executable; headless runs neither read nor write layouts.

The torn-off windows are egui's immediate viewports: egui runs each one's pass inside the main
window's, and the main window's frame renders them all, paced by the main window's swapchain alone.

Layers add menus through `Ui` too, and controls at the middle of the main window's title bar, in
place of its title, through `Ui::title_bar`. A button at the bar's left opens the menus in a drawer.
The OS draws the windows' frames, and the menus and controls get a bar of their own below its title
bar, unless the app calls `App::custom_title_bar`, as tools do for the room: then the UI draws the
windows' title bars, the main window's with a gear that opens the settings, which layers add to
through `Ui::settings` after the engine's frame rate limit, and buttons that hide and show the
panels left of, below and right of the view; and the dock adds a View menu, which opens and closes
panels and saves, loads, deletes and resets layouts. On Windows the rest of the frame stays the
OS's, through a window procedure in front of winit's, and on macOS the OS keeps its own buttons and
edges. Games keep the OS's frame.

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

Looking about with the mouse, as first-person games do, takes its raw motion, winit's device
events, which go on past the window's and the screen's edges, rather than the cursor's moves: each
motion is an event, which a layer adds up into a command of its own, as it does an action's change,
so that each tick turns by all the motion since the tick before. Motion isn't an action, as an
action holds a value, as a key or a stick does, and motion is a distance moved. A layer asks for the
cursor grabbed through its `Context`, and the windowed loop applies it as the next frame starts:
hidden, egui told so too, and locked where the platform locks it, Windows, macOS and Wayland, or
else kept inside the window, X11, which reports the raw motion all the same; held while the window
has focus, and in a dockspace while the view has the keys, let go as either goes and grabbed again
as it comes back. In a dockspace, a grab that starts with the cursor off the view, over a panel or
the title bar, moves it to the view's center first, where it's locked, so that its clicks go to the
view rather than to egui; Wayland moves no cursor it hasn't locked. What to do with Escape is the
game's, as a game may pause on it. The editor's game, which renders offscreen, says in its view's
memory whether it wants the cursor grabbed, and the editor grabs its own window's in Play, as
[Agents and tools](#agents-and-tools) says.

## World space

World space is right-handed with +Y up, as glTF's is: a camera without rotation looks down -Z,
with +X to its right, as glTF's cameras do. glTF assets need no conversion as they're imported,
and scenes, physics and the editor share these axes.

## Physics

Physics is rapier's, rapier2d's in 2D and rapier3d's in 3D, in gg-engine's `physics` module, and
games reach it through components. An entity with a `Transform` and a `RigidBody2D` is a body
moving in the XY plane, and a `BoxCollider2D` or a `CircleCollider2D` beside it, or both, is the
shape it collides as; one with a `RigidBody3D` moves in space, its translation and rotation whole,
and collides as whichever of a `BoxCollider3D`, a `SphereCollider3D`, a `CapsuleCollider3D` and a
`MeshCollider3D` it has. Colliders are in their entity's space, so that they turn and scale with
it: a box by its scale along each axis, a circle or a sphere by the largest, and a capsule, upright
along its Y as a character stands, by its scale along Y for its height and the larger along X and
Z for its radius. A mesh collider is the triangles of the model its entity's `ModelRoot` names,
each node's where the node is in the model, scaled along each axis, as a level's floors and walls
are, for static and kinematic bodies, as triangles hold no volume to give a body mass. Its
triangles are the model's meshes as packed, which a run from glTF and one from its package unpack
alike, their vertices those their triangles use alone on both sides, ordered by where they are
rather than as the cooker leaves them, so that both runs collide alike; every model keeps its
meshes' surfaces so for physics, and a model read again, as its file changes, has its colliders
made again. `step_physics` is a system a layer's tick runs, stepping 2D physics, then 3D. rapier is
built with `enhanced-determinism`, so that a step gives the same bits on every platform. Physics'
state, rapier's bodies, colliders, joints and contacts and what sleeps, is the world's
`Physics2D` and `Physics3D` resources, each given to a world as it first steps with a body of its
kind, which snapshots copy, as [ECS](#ecs) describes: bodies made again from their components
alone wouldn't step as the world did. A snapshot file holds them through rapier's own
serialization, which writes a mesh collider's triangles whole, at a cost
[DEFECTS.md](docs/DEFECTS.md) weighs. A body under a parent isn't moved, since its transform is
in its parent's space (`physics.child_body`). A body's component that isn't finite, holding a
NaN or an infinity, as its transform, its velocities or a collider's size, is refused, warned of
once while it stays so (`physics.not_finite`), so that it doesn't spread through contacts: the
body keeps the last it took that was, and an entity without one gets none until it's finite. A
body that moves goes on from its last pose, which it writes over a transform refused as it moves.

A joint is a `Joint2D` or a `Joint3D`, a component naming the two bodies it holds together,
`body_a` and `body_b`, by their entities, which rapier's impulse joints hold: fixed; revolute, a
hinge about its axis; prismatic, a slider along it; spherical, a ball joint, in 3D; a rope, which
keeps its anchors no further apart than its length; and a spring, which pulls them toward it. Its
anchors are in each body's space, scaled with it as a collider's offset is, and its frames are its
anchors and its axis there, the axis turned with each body but not scaled, so that bodies turned
alike stay alike whatever their scales, a mirrored body's too; so the component alone says where it
holds, which scenes save, the Inspector and the tool API edit and the hash reads. A revolute's or a
prismatic's limits and motor are its fields too, each a field of its own, so that a script sets a
motor's target as it sets any field. A joint is on an entity of its own or on either body's,
naming both, rather than on one body naming the other: so that a body is in as many joints as name
it, as a chain's link is in two and a ragdoll's torso in four, with no rule for where a second
goes; so that a joint comes and goes, as an edit's create or delete, without changing either
body's components, as between two prefabs' instances, whose overrides would otherwise hold it; and
so that an entity holds one joint, whatever its kind. Physics' joints are rapier's, in its joint sets, which snapshots copy and
snapshot files hold with the rest of its state, each with its entity, its component as the last
step made it and why it was last warned of; rapier's pipeline, its workspace, keeps the joints it
last solved by counts the joint set copied brings back, so a copy taken into physics' memory, as a
rollback's is, makes it anew, as a copy has it, at the cost of its memory made again. Each step
takes the joints in after the bodies, making, changing or taking out each as its component or a
body it names changed: a body made again, as one put under a parent and back, is another of
rapier's, which takes its joints with it, and its joints are made again. A joint whose bodies
don't collide, as made or changed, has rapier find their colliders' contacts afresh, as it
recycles a touching pair's while it barely moves, past its check of the pair's joints; and a
character's sweep passes through what it's joined to so, as rapier's contacts do. One naming an
entity that isn't a body that physics moves joins nothing, warned of once for each reason
(`physics.joint_no_body`), as does one naming one body twice, by its handle
(`physics.joint_one_body`), until it joins two; and its joint goes as it does, or its entity, or
either body. An edit's delete of a body clears the field of each joint naming it, as
`Component::cleared` names fields a delete clears, so that the scene still saves; and names the
entity the edit makes again under its ID, as a scene file's relink of an instance does.

A body's `continuous_collision` makes it a bullet, as rapier calls it: at each step it goes fast
in, rapier sweeps its colliders along its way against every other body but bullets, stopping it at
the first it would otherwise pass through between the step's start and its end, as a small fast
ball passes through a thin wall. rapier sweeps each fast dynamic body against what's static anyway,
so it's for what's fired at what moves, kinematic or dynamic; and it's off by default, as each sweep
costs. Joints, motors and sweeps are rapier's arithmetic, which a reference hash of each kind of
joint, 2D and 3D, with a bullet among them, checks on every platform.

A character is an entity with a `CharacterController3D` and a `CapsuleCollider3D`, which rapier's
kinematic character controller moves at each step as fast as the controller's `velocity` says,
which its game sets, as from its player's input: sliding along what it moves into, climbing slopes
up to a limit and steps up to a height, kept to the ground going down, and pushing what's dynamic,
against the colliders as the last step left them; gravity pulls its velocity down while it's off
the ground, and the ground and what's above it stop it, `grounded` saying whether it stands. A
kinematic body it stands on, as a moving platform, a lift or a carousel, carries it as the body's
pose goes in the step, turning its facing as the body turns about up, its own move made as though
that body stood still; and one moving into it pushes it before it, each swept against the rest, so
that a lift rising under it lifts it rather than passing through. Its
body is kinematic, moved to where the controller took it as rapier steps, so that the rest of
physics meets it there. Its state is the controller's and the world's, as a body's is, so that
replays, snapshots and peers move it alike. An entity with a `RigidBody3D` is a body, its
controller doing nothing, as is one without a capsule, each warned of once
(`physics.not_character`).

Each collider's component says whether it's a `sensor`, which finds what overlaps it but pushes
nothing and is pushed by nothing, as a trigger zone, and which groups it's in and meets, its
`membership` and `filter`, a bit for each of 32, as rapier's collision groups: two colliders touch,
or overlap, only where each is in a group the other's filter has. A sensor finds kinematic bodies
and characters too, and static bodies but from a static body's sensor. A character moves through
sensors and through what its capsule's groups don't meet, and one whose capsule is a sensor pushes
nothing; casts pass through sensors. The world's hash reads these fields only while they aren't
their defaults (`#[reflect(hashed_when_set)]`), so that a world that sets none hashes as it would
without them. At each step, physics finds which entities' colliders touch, or overlap where either
is a sensor, from rapier's contact and intersection pairs rather than its events, which name
colliders, some gone by then: each pair of entities once for each kind, sorted by their IDs, and
compared with the last step's for the contacts that started and stopped at it, in that order.
`World::contacts` and `World::contact_events` read them, 2D's then 3D's, and scripts read an
entity's as `gg.contacts(entity)` and `gg.contact_events(entity)`, at the tick after, as scripts run
before physics. Both are the physics resources' state, which snapshots copy and snapshot files hold
by IDs, so that a world restored, as a rollback restores it, steps on to the events the world it was
taken from had; and a collider made again, as its component changes, keeps its contacts going.
Contacts are rapier's, where a dynamic body is one of two, so that a character finds what's static
or kinematic only as a sensor's overlap.

`World::cast_ray` and `World::cast_shape` cast a ray, or a box, a sphere or a capsule swept along a
line, through the 3D colliders but sensors, from code and from scripts, as `gg.cast_ray` and the
like: what's hit first, its entity, where, the surface's normal there and how far, passing through
an entity's own colliders if it's asked to. They see the colliders where the last step left them, as each
body's rapier holds its entity, which a snapshot file checks.

`Context::set_colliders_shown` outlines the world's colliders where their entities draw, in lines,
a sphere by its rings about its three axes and a capsule by its rings, its side's lines and its
caps' arcs, and draws each joint, in orange, as lines from its first body to its anchors and on to
its second, which draw over the rest of the view but where an opaque mesh is nearer, as
[Rendering](#rendering) says, rather than nudged toward the camera: so they show from behind a
sprite as from in front of it, since sprites test depth without writing it, and a joint inside the
meshes of the bodies it joins shows only where they don't hide it.

## Animation

Animation is gg-engine's `animation` module, reached through components that hold its state, so
that snapshots, replays and peers see it alike, and stepped by `step_animation`, a system a layer's
tick runs after what starts tweens, such as scripts, and before physics, which takes bodies in where
tweens put them. A `Flipbook` beside a `Sprite` plays the sprite through cells of its texture, cut
evenly into columns and rows, as many a second as its rate says, looping, back and forth or once,
a flipped sprite kept flipped; its cells are fractions of the texture, as the world doesn't know a
texture's size, which is the renderer's. An entity's `Tweens` ease fields of its components, each
tween one, named by its component's type and its own name, or one of a vector field's components,
from one value to another over seconds, along one of Penner's curves eased at either end or both,
played once, looped or back and forth. A tween finds its field by name at each step and reads and
writes it through reflection by its index, as scripts' handles do, allocating nothing, and writes
only a value that changed, so that the renderer's scene isn't handed rows at rest. Its curves are
libm's functions and its rotations glam's slerp, which a reference hash in `animation/` checks on
every platform. Scenes list tweens; code starts one with `World::tween` and scripts with
`gg.tween`, each in place of a tween of the same field.

A model's root's `Animator` plays its model's clips, glTF's animations, on its nodes: each play a
clip by its name, its time, speed and playback, and a weight, which eases toward a target over a
fade, a play eased to 0 leaving. Each tick samples each play's channels at its time, linear,
stepped or along a cubic spline as glTF says, a rotation by slerp, and each node a play moves takes
the plays' poses blended by their weights, the first play's alone where it's the only one, so that
a lone play is exact; a node no play moves keeps its transform, and the transform is written only
where it changed. The nodes are found by the IDs mixed from the root's, without a map, so that a
tick allocates nothing. `World::play` cross-fades to a clip, each other play easing to 0 over the
fade, and scripts call it as `gg.play`. The clips' samples are arithmetic on f32s and glam's slerp,
which a second reference hash in `animation/` checks. The joints that animation moves are nodes,
so snapshots, replays and peers see a pose as they see any transform; what they move of a skinned
mesh is drawing's, as [Rendering](#rendering) says.

A root's `AnimationStates`, beside its animator, is a state machine of its clips, which drives the
animator and leaves the sampling to it, stepped before the animators: parameters, each a float, a
bool or a trigger, which code sets with `World::set_param`, scripts with `gg.set_param` and tools
through the component's fields; states, each a clip, or clips blended along a float parameter, the
two whose points are nearest its value either side weighted between them, at a speed and a
playback; and transitions, from a state or from any but the one they enter, checked in order, the
first whose conditions all hold, a float above or below a value, a bool true or false, a trigger
set, after its exit time if it has one, firing and consuming the triggers it tests. Entering a
state, the animator cross-fades as `World::play` has it, but for a state played once, which starts
its clips afresh, from their ends if it plays backward; then, at each tick, the state's plays take
their motion's weights and every other play eases to 0, so that once the fade is done a lone clip
plays exactly. A blend keeps its clips in step, each at the fraction of its length that its
heaviest held clip is at, played at the speed that brings it round in the blend's length, the
lengths of the clips of some length weighted as the blend weights them, over their weights alone,
so that a walk and a run blend without their feet fighting, and a still pose among them speeds
neither up. The state and the seconds in it are the component's fields, and
the plays' times the animator's, so that snapshots, replays and peers resume a machine exactly; its
arithmetic is f32's alone, which a third reference hash in `animation/`, of the Fox driven through
its states, checks. A state, parameter or clip a machine names and hasn't warns once a world, which
keeps what it has warned of as a resource. A tick that enters no state, nor brings back a blend's
clip that left at a weight of 0, allocates nothing; one that does allocates the names of the plays
it adds, but where a play that leaves at the tick, as each a cut leaves does, gives up its room.

## Navigation

Navigation is gg-engine's `navigation` module, our own rather than a crate's, as none promises the
same bits on every platform: a navmesh baked from the world's static 3D colliders, paths over it,
and agents that walk them round each other, stepped by `step_navigation`, a system a layer's tick
runs after what sets the agents' destinations, such as scripts, and before physics, which moves
the characters the agents steer. Its state is the agents' components and the world's `Navigation`
resource, which holds the navmesh.

An entity's `NavMeshVolume` says what bakes and for what agent: the static bodies' colliders, as
physics has them, inside a box in its entity's space, the bake taking the box that bounds it along
the world's axes; the agent's radius, height, climb and slope; and the heightfield's cells. The
bake follows Recast's pipeline as far as the erosion: each collider's triangles are clipped to the
heightfield's columns, a box's, a sphere's and a capsule's as convex solids, a span from their
lowest point in a column to their highest, so that a wall standing in a floor leaves no floor
inside it, and a mesh collider's triangle by triangle, as surfaces; spans that touch merge, their
top walkable as the higher one's, or as either's where the tops are within a step; the walkable
tops with too little headroom go, and those a step above a walkable one join it; each open span is
joined to the one beside it an agent steps onto with room to stand; and the walkable area is eroded
by the agent's radius, by a chamfer's distance from where a span lacks a neighbor. Recast's
regions, contours and polygons give way to rectangles of cells, each the best anchored at the first
cell left in order by Z, then X, then floor, by its area times its shorter side, holding only cells
joined to their neighbors, each keeping its own floor, which keeps them correct across slopes,
steps and floors over floors; where they meet are portals. That costs more polygons than Recast
makes where walls cross the grid's axes, as their staircases of cells cut strips, and paths that
bend at a rectangle's corner where a larger polygon would let them run straight. The world bakes
one navmesh, from the volume of the least ID (`navigation.volumes`); a volume of more than 2^22
columns or 65,533 levels isn't baked (`navigation.too_large`), nor one without a size
(`navigation.bad_volume`). It bakes deterministically, each body's colliders in the order of their
entities' IDs, so that the same entities and assets bake the same navmesh, and bakes again as a
fingerprint of what it's baked from changes: the volume's entity, where it is and its fields, and
each static body's transform and colliders, a mesh collider by the hash of its model's triangles,
summed by entity, so that the order queries find them in counts for nothing. Each bake logs
`Navmesh baked`, with its polygons and milliseconds. Every step and every search brings the
navmesh up to date first, so that what a path is planned on is what the world's static colliders
give then, whatever the world was loaded from: a world copied shares its navmesh; snapshot files
don't hold it, so a world restored from one bakes it again as it first steps or searches, under
the same fingerprint, so that its agents' paths, planned on it, hold, and one loaded whole over a
world that kept another's navmesh bakes its own, which tests step and search both ways to the
same bits. A client's copy of the server's world brings its own up to date as it reads each state,
so that the world taken back to it as the client corrects keeps its navmesh rather than baking it
again.

`World::find_path` finds the shortest path over the navmesh between two points, each on it where
it is, as an agent's middle is above its floor, or snapped to its nearest point within the agent's
height: A* over the polygons, each entered at the point of its portal that makes the way from where
its parent was entered to the end shortest, rather than Detour's portal middle, whose costs stray
across long rectangles; then the funnel algorithm pulls the path taut to the corners where it turns,
points on the navmesh. It takes the world mutably, to bring the navmesh up to date, at the cost of
a fingerprint of the static bodies a search. Scripts find paths with `gg.find_path`, an array of
vectors or nil.

A `NavAgent` walks to its destination, which code, scripts and tools set, none to stand: it plans
its path as its destination or the navmesh's fingerprint changes, which `planned` mixes, so that a
re-bake plans every path again and a restored world none, and again each tick while it has a
destination and no path, as one that started off the navmesh, falling, does until it lands; heads
for its next corner, past each it comes within its radius of, standing on its floor, or from which
the next is in sight over the navmesh, a walk along the heightfield's columns from floor to floor
that ends on the corner's own, so that one pushed aside doesn't go back for a corner it no longer
needs and none takes a floor over or under its way for its own; plans again where it comes over a
corner on another floor; and slows to stop within its stopping distance of its destination, on
its floor, coming back to it where it's pushed off. Its corners, the corner it heads for, whether
it's arrived and the velocity it chose are its component's, so that snapshots, replays, peers and
the hash see them. The velocity goes, across, to its entity's `CharacterController3D`, which slides
it along walls and up steps and slopes, or else its `RigidBody3D`'s, or else to its transform,
which walks along the navmesh, kept to it, sliding along its edge to the mesh's nearest point.
Agents steer round each other by ORCA (van den Berg, Guy, Lin and Manocha, 2011), as RVO2 does,
among their ten nearest within reach on the same floor, by their entities' IDs, every pair
compared, which bounds a crowd at some hundreds: each neighbor's velocity obstacle bounds a
half-plane, each agent taking half of what a pair must change, and a small linear program picks the
velocity nearest the one preferred; walls are the navmesh's and physics' business. Where ORCA's
perfect symmetry would leave two agents face to face, every agent nudges what it prefers a degree
to its right, as RVO2's examples perturb at random, which peers couldn't agree on, and agents in
one place part, each the way the order of their IDs gives. A tick computes in plain `f32`
arithmetic and square roots, libm's cosine and sine where it needs them, comparisons rather than
`min` and `max`, whose zeros' signs are the platform's, and orders by entities' IDs, so that a
reference hash in `navigation/` checks every bit on every platform. A path search and avoidance
work in what the resource keeps, and corners in their vector, which holds room for 16, so that
steady ticks allocate nothing, which the `ticks` test checks with paths round a wall.

`Context::set_navmesh_shown` draws the navmesh's polygons, outlined along their cells' floors, and
each agent's path from where it draws through its corners left, in lines, as colliders' outlines
draw, picked as the volume's entity and the agent's.

## Rendering

Each frame the engine extracts that frame's render data, as plain data, which the render thread
hands `gg-renderer`: the color the view clears to, the game's view, the viewpoint it sees the world
from, and what to draw in it, in order: meshes, each with a shader for its vertex type, the
parameters that shader takes, and a transform that places it in the world. The view is the main
window's central area, which the dock leaves the game, or a dockspace's view panel. It renders into
images of its own, that area's size in pixels, made again as that size changes, which the frame
then shows there, a pixel to a pixel, so that tonemapping, post-processing and the tool API's
captures read the view whole: under the whole UI, or, from a panel, where the UI paints a callback
that stands for it, in the UI's order and clipped as the UI clips it, so that the panels floating
over it and the menus show over it. The view draws in colors of high dynamic range, 16-bit floats
in linear RGBA, which games' pipelines are made for, with the image of IDs beside them, below, and
the depth image, of 32-bit floats, which every draw into the view tests; a tonemap pass then maps
the colors into the view's 8-bit sRGB image, which the frame shows, captures read and the outline
draws over, whatever the window presents. What reads the colors, the tonemap, antialiasing and
bloom, takes one that's no number as none, and an infinite one as a half float's largest, 65,504: a
shader's float can be infinite, and a GPU that rounds to the nearest writes a color past that
largest as infinite. Depth runs reversed, from 1 at the near plane to 0 far
away, which keeps a float's precision where it's needed and lets perspective see infinitely far;
each frame clears it to 0, and a draw shows where it's as near as what's drawn there or nearer, so
that at one depth the later goes over, as 2D draws at one z stack.

A world's `Environment`, a component on an entity of its own, says how the view shows the world's
light, and the sky around it, as below, whatever viewpoint it sees from, the editor's and agents'
looks too, so that they show what the game does; of several, the lowest ID's, with a warning. Its
tonemapper maps the view's colors into the range it shows: Khronos PBR Neutral by default, which
shows base colors up to about 0.76 as they are, less the 0.04 a dielectric reflects whatever its
color, and rolls brighter light off toward white; ACES, as Stephen Hill fitted it; AgX; or none,
each channel clamped, which a world without an environment gets, so that a 2D game's sprites show
as they're colored. Its antialiasing is temporal, as below, by default, or SMAA, from each frame
alone, as below, or none, which a world without an environment gets too, each pixel what's at its
center. It blooms the view, subtly by
default, and grades its light, by the identity by default, as below, and a world without one does
neither. Its ambient occlusion, on by default, darkens the sky's light where what's near a point
hides it, and its reflections, off by default, show in what's opaque what the view shows, as
below. The clear color, which a sky takes the place of, is tonemapped with what's
drawn, as the sky is. Its exposure, in EV100, scales
light: lit shading multiplies light, in lux and candela, by 1 / (1.2 × 2^EV100) as it shades,
which keeps a sunny day's light within 16-bit floats, and a world without an environment is
exposed as the default's, EV100 15; what isn't lit, sprites, text, lines, the editor's grid and
unlit materials, draws its colors as they are. Its exposure may adapt to what the view sees
instead, as below, off by default. Every draw's shader can read the frame's data
through an address among its push constants: where the view sees from, the time, the simulation's
as the world draws, which stops with it and draws alike in a replay, and the exposure; the frame's
lights, how many and where; the address of the table of models' materials; where shading sees
from, the camera's position, or for an orthographic projection, which sees along one way, the way
back toward it; the sky's texture, its prefiltered light, the split sum's table and its luminance;
the shadows, their light and their cascades' shadow maps, which shaders sample by comparison alone,
through the bindless set's sampler that compares; the clusters the lights are binned into; the
view's ambient occlusion; the fog the view is in; an atmosphere's haze, its froxels and where a
point falls among them; and the view's reflections. Fields are added at its end, so that games'
shaders that read the fields
before them keep to the contract.

A world's lights are its entities with a `Light` and a `Transform`, as KHR_lights_punctual
describes them: directional, in lux, all along its entity's -Z; point, in candela, from where its
entity draws, falling off with the square of the distance, eased to nothing at its range if it has
one, as the extension recommends; and spot, a point light within a cone round its -Z, wholly within
its inner angle and fading to nothing at its outer. Each frame hands the renderer every light, from
where its entity draws and facing as it does, ancestors carrying it, as a camera sees from where its
entity draws, found again only as a light, its transform, its parent or its table changes, what
carries it moves, or it draws on its way between ticks. Each lit point shades with the
lights that light everywhere, directional lights and those without a range, which shaders read
first, then with those whose reach meets its cluster of the game's view, in the frame's order, so
that what it adds up is what every light would add. A compute pass bins them each frame, before the
view draws, into clusters: 16 tiles across, 9 down and 24 slices along the depth, which grow with
it under a perspective, as far as the farthest light binned reaches, and which are even under an
orthographic projection; each cluster's lights are bits of its words, with the first word that holds
one and the word past its last. The pass copies the lights to the front of the clusters' buffer,
where the view's shaders read them, and the clusters through their address, as AMD's driver drops
the address of a second pointer read from memory in such loops. What falls outside every cluster,
as what the canvas draws under its own projection may, shades with every light. A model's node that
KHR_lights_punctual gives a light has it as a component, which fits as the model's nodes do. Scenes
save lights, and the Inspector and tools edit them, as any component.

Models shade as glTF's metallic-roughness materials say, by Appendix B's BRDF, as Khronos's sample
viewer has it: Lambert's diffuse and GGX's specular, with Smith's height-correlated visibility,
Schlick's Fresnel and 0.04 reflected head-on by every dielectric, from a material's base color,
metallic and roughness, each a factor that its texture multiplies, and its normal map. A material
of KHR_materials_pbrSpecularGlossiness's, which takes its metallic-roughness's place, shades by the
same lobes, as the extension's reference has it: it reflects its specular color head-on, diffuses
its diffuse color times 1 less that color's strongest channel, and is as rough as 1 less its
glossiness, its map's specular color sRGB-encoded and its glossiness, in alpha, as stored. Light times
the exposure, then the material's emission, as the view shows it rather than exposed, make the
color, so that an emissive white shows as white under any light. Occlusion, a material's map's and
the view's ambient occlusion, as below, darkens only light from all around, the sky's, and the
view's reflections, as below, take the place of only its specular light. A material
that KHR_materials_unlit makes unlit, and every material in a
frame without lights or a sky, draws its base color alone, as models drew before lights. A primitive without normals draws flat, its triangles'
corners apart, as glTF says, but one whose material is unlit, which reads none; a lit material's
normal map without tangents, or whose tangents aren't all finite, as a tool can leave them where
texture coordinates have no area, gets MikkTSpace's, as glTF asks, through `bevy_mikktspace`, its square
roots and arccosines libm's, so that every platform makes the same, its vertices apart where their
corners' tangents differ. A material's maps of data, metallic-roughness, occlusion and normals, are
textures of data, sampled as stored, their levels averaging their values. A texture whose
MSFT_texture_dds names a DDS image samples that in place of its source: BC7 with its levels, of
colors or of a map's values as what samples it says, whatever the file's own format says, as glTF
takes a PNG's colors as sRGB whatever its profile, its blocks kept as they are, by development runs
and by the cooker alike. One image sampled as
colors by one texture and as data by another is the first's, the other left out, and a material's
textures read one set of texture coordinates, the first's, others left out alike, as the model's
`engine.model_ignored` lists them. Materials live in a table on the GPU, which a model's draw's
parameters index: each written into it as it's added and again as it's replaced, in the frame's own
commands before its passes, the table made again twice the size as it fills; a handle is never
given out again, as a texture's isn't.

An environment's sky, if it has one, may be an image of the world around, equirectangular, its
middle column the way toward +X, a quarter of the way across the way toward -Z, and its top row
straight up, as Khronos's glTF Sample Viewer and three.js take one, a texture of colors, which a
Radiance HDR's colors of high dynamic range suit;
its colors times its luminance, in candela per square meter, 30,000 by default, as Filament's
environments' intensity is, are light, which the exposure scales as it does lux and candela. The
view draws it before anything else, at depth 0, writing no ID, in place of the clear color, each
pixel the way it sees, from the image's first level; an orthographic view sees one way, so one
color. It lights models from all around through the split sum, with Fdez-Agüera's
roughness-dependent Fresnel and his scattering many times over, as Khronos's sample viewer has it,
so that a white surface under a sky of one color shows that color, whatever its roughness and
metal. The first frame to render the view once the sky's texture shows an image of its own, or
another, prefilters it on the GPU in compute passes before the view's: into a cube of a side a
quarter of the image's width, to the next power of two, at most 512, made for that frame alone,
whose levels each average the one before's; from it, importance sampling its levels by the solid
angle a sample covers (Křivánek and Colbert), a cube of 32 texels a side of the light Lambert's
diffuse takes, cosine-weighted, and a cube of the cube's side, at most 256, of the light GGX's
specular takes, a level for each step of roughness, from none at its first to 1 at its last, of 8
texels; and, once, the split sum's table, of 128 by 128, of GGX's reflectance as F0's scale and bias
by the cosine of the view's angle and the roughness. Those images keep that light until the sky
shows another image, as a reload does, or a directional light comes to stand for its sun or ceases
to, or, while one stands for it, the sky's luminance changes what the cube keeps its points to, as
below. A view whose sky's texture shows its stand-in shows no sky, and is lit by none; a frame that
renders no view prefilters nothing.

A sky's sun lights once. In a frame with a directional light that shines, which stands for the sun,
casting shadows where the sky's light can't, the cube keeps each point of the sky to 10⁷ candela
per square meter, or as little as half that, about a hundredth of the noon sun's disk and past any
cloud, in its own hue, so that the sun's disk lights through the light alone; the view still draws
the sky whole. In the texture's own units, that's the power of two at or below 10⁷ over the sky's
luminance, so that a luminance that changes, as a day's does, has the sky prefiltered again only
as it crosses one, rather than in every frame it changes. A frame
without one lights by the whole sky. The light should shine from where the sky shows its sun, its
way and illuminance found from the sky's texels past that, as the showcase's Sponza in fog has
them; a sun low in its sky, dimmed through more air, can fall short of it, and then lights twice.

An environment's sky may be an atmosphere instead, made from its sun, as Hillaire's "A Scalable and
Production Ready Sky and Atmosphere Rendering Technique" (2020) makes it, after his reference
implementation: Rayleigh's and Mie's scattering and ozone's absorption over a planet of Earth's
radius, 6,360 km, up to a top 100 km above it, Earth's by default, as the paper's, the world's
units taken as meters, its +Y up and its origin as high above the ground as the atmosphere says,
the viewer as high as that and its own Y, kept 10 m above the ground and below the top. Its shaders
work in kilometers from heights above the ground, not radii from the planet's center, at which an
f32 is half a meter apart: what's meters above the ground, as which points the planet shadows, is
then found alike on every GPU, rather than by how each rounds. Its sun is
the frame's brightest directional light that shines, the first of those as bright, whatever its
shadows: where the light shines from is where the sky's sun is, so that turning the light, as a
time of day does, moves the sun and its disk, which the view draws there, as wide as the
atmosphere says, of the light's illuminance over the solid angle it spans. That illuminance is
taken outside the atmosphere, and the light that reaches the world, its lit shading's, is the
light's dimmed by the transmittance from the viewer toward the sun, once a frame, on the CPU, by
the sums the GPU's table of it makes, and by the share of the sun's disk above the horizon, so that
a low sun reddens and dims what it lights, a setting one fades out over its disk's height, and a
set one lights nothing: at night the sky darkens through twilight to black, lighting nothing, and
only the world's other lights and what glows show. Which light casts shadows is found by the
lights' illuminance outside the air, so that the sun casts them while it's up, however dim, but
for a set sun, which casts none, the next of those that cast them taking its place. In
every frame that renders the view, compute passes before the view's, on the graphics queue, make
its tables, each an image of colors of high dynamic range of the graph's own, each of the light of
a sun of an illuminance of 1, its color in, which the sun's illuminance multiplies: the
transmittance, 256 by 64, by the height and the cosine of the way's zenith angle, by Bruneton's
map; the light scattered many times over, 32 by 32, by the sun's zenith angle and the height, a
workgroup a texel, from 64 ways round it; the sky as the viewer sees it, 192 by 108, by the zenith
angle, crowded toward the horizon, and the way round from the sun's, the planet's ground below the
horizon lit by the sun at its albedo; and the aerial perspective, froxels of the view, 32 across
and 32 down, in 32 slices along its depth crowded toward the viewer, the last reaching 128 km, the
slices side by side in one image, each froxel the light scattered toward the viewer from as far as
its slice and the transmittance from there. The view draws the sky from its table behind
everything, as it draws an image sky, and the sun's disk through the transmittance, unless the
ground is in the way; an orthographic view sees one way, so one color. Lit shading, the visibility
buffer's and as models draw, hazes what it lights by how far it is from the camera: dimmed by the
transmittance of the froxels round it and with their light added, between the two slices round
it, fading to none at the camera within the first half slice, 2 km, as the paper's does. The sky's
light from all around is prefiltered as an image sky's is, from a cube of 128 texels a side that
its table makes without the sun's disk, kept for the next, by 64 samples a texel, which the smooth
sky needs no more than: again only as the sun turns past an angle, the viewer rises or falls past
100 m from what the light holds, or the atmosphere or the sun's color changes, but not as the
sun's illuminance does, which multiplies that light as a sky's luminance does. The angle is a
hundredth of a degree with the sun at the horizon or below, where the sky's light falls by half or
more each degree the sun sinks, and grows by a factor of e every 5° up, as the light changes more
slowly, to 1° from 23° up, and is 1° again past 18° below, once the sky is dark, so that the light
changes in steps of 2% at most: a sun that turns 3° a second, through a day of two minutes, has
it prefiltered in every frame at 60 a second while it's low, and about three times a second while
it's high. In fog, as below, what's lit is hazed as it's shaded, and fogged after, as the sky drawn
from the atmosphere's table is: the fog lights its light by the atmosphere's light from all around
and the dimmed sun, so that a far wall in fog under an atmosphere shows its own light dimmed by the
air's transmittance and the fog's, the air's light between it and the eye dimmed by the fog's, and
the fog's light over both, the fog taken as nearer the eye than the air's haze.

A directional light casts shadows, unless its `Light` says it doesn't, as far ahead of the camera as
its shadow distance, 100 units by default, fading out over the last tenth: of a world's lights that
cast them, the brightest, the first of those as bright, as one light's shadows are drawn. Point and
spot lights cast none yet. The shadows are in four cascades, each a shadow map of 2,048 texels a
side, of 32-bit depth, the graph's own images, drawn before the view, which samples them. The
view's depth, from its near plane to the shadows' reach, splits into four slices, the nearest the
shortest, by the practical split scheme, three quarters logarithmic; each cascade is the box, seen
from the light, round the smallest sphere that holds its slice, so that its size changes only with
the view's projection, and its center moves only by whole texels of its map in the light's space,
so that its texels stay put in the world as the view turns and moves, and shadows' edges don't
shimmer. What a shader that's opaque draws casts shadows, through a pipeline of the shader's vertex
shader alone, drawing depth alone, into each cascade; the masked model shader's, through a fragment
shader of its own too, which cuts them out as it cuts out what it draws; what blends casts none, nor
what a layer draws on its canvas. Each cascade draws the casters whose spheres reach into its box,
as culling on the GPU, below, keeps them, whether or not the view sees them. Casters between the
light and a cascade's near side cast from it, their depth clamped there, and are kept for it; a
cascade's depths sit back from its casters by a texel's worth of
their slope. A lit fragment takes the nearest cascade whose map holds it and the texels the filter
reads round it, moves a texel toward the light, and off its surface along its normal by three
texels times the sine of the light's angle to it, so that a surface doesn't shadow itself, and
filters six by six of the map's texels' comparisons, in nine samples of four, by a kernel that falls
from the point outward, so that a shadow's edge moves smoothly. A frame whose lights cast no shadows
makes no shadow maps and draws no shadows.

An environment's fog, none by default, is height fog, as Unreal's exponential height fog is: a
medium filling the world, whose density, its extinction, the share of light it takes out over a
unit, is its density at its height, thinning exponentially above it by its falloff and thickening
as fast below. Along the way from the eye to what the view shows, past the fog's start, the
integral of that density, its optical depth, is in closed form, from the lower of the way's ends,
where the fog's densest, so that it stays finite however far below the eye the way falls; toward
the sky, infinitely far, a way that rises takes a finite depth of fog, and one that doesn't takes
all of the light. The fog lets through e to the minus that depth of the light from behind it, never
less than its opacity leaves, and scatters toward the eye, by as much as it takes out, its color of
the light that reaches it: from all around, the sky's diffuse light the way the eye looks, or in a
world without a sky a luminance of its own, in candela per square meter; and the sun's, the
brightest directional light's, by Henyey-Greenstein's phase function, normalized, in a lobe round
the way toward the sun as sharp as its sharpness says, times how much of the sun's light it says
reaches it, as it takes no shadows. That's light, exposed as light is, so that the fog suits any
exposure, and any sky that fills the sky's diffuse light. A compute pass of its own fogs what's
opaque, the sky and the clear color among it, from the view's depth, once the view has drawn it: of
the visibility buffer, after its shading and the pass that draws the rest that's opaque, and
without one after the passes that draw, so that both fog alike; then what blends draws over that,
in a pass of its own, each fragment fogging itself as it's drawn, by the frame's data, so that what
it covers of what's behind it gives way to the fog in front of it, where a pass over the colors
alone would fog what blends as though it were as far away as what's behind it. The engine's
shaders that blend fog themselves: models' materials that blend, a fragment at a time; and what's
drawn on the engine's quad, at its corners, between which the fog changes too little to tell for
quads small beside the fog's scale, so that a swarm of sprites out of fog pays the check a corner
rather than a fragment: particles, which are unlit but fade into the fog by their alpha, one that
adds its light adding less, sprites, circles and text. Lines and grids, which are drawn to see by,
and the canvas aren't fogged. A game's shader that blends draws unfogged unless it fogs itself by
the fog in the frame's data, as `shaders/fog.slang` does; one that's opaque is fogged by the pass.
A view without fog, or in fog of no density, draws as it does without, through neither pass.

Each frame's GPU work is a render graph, gg-gpu's `Graph`: its passes, in the order the renderer
adds them, each saying how it uses the images it touches, drawing into them, writing them from
compute shaders, sampling them or copying from them, and the buffers, writing and reading them from
compute shaders, reading them from draws' shaders, vertex, mesh or fragment, taking indirect draws'
commands from them, which the draws' shaders may read too, or reading them back, a use of each
buffer a pass, from which the frame places every barrier and layout transition as each pass
begins, each use waiting for the uses before it that it must and no more, so that no pass records
one itself. The passes run in that order, on the graphics queue, but those that compute added to
run on the compute queue: they run on a queue of their own where the device has a family for compute
apart from graphics and from uploads, as the RTX 4090 and AMD's GPUs do, and on the graphics queue
elsewhere, as on lavapipe. Such a pass uses buffers alone, which every queue shares, so that nothing
is handed between families. A frame plans as it begins the batches each queue runs, from the passes'
uses of buffers and what the frames before left of each: a pass that must follow a use on the other
queue, a write following any use and a read a write, begins a batch that waits by a semaphore for the
other queue's batch that holds that use, which ends there. So a pass on the compute queue runs beside
the graphics passes added after the last it waits for and before the first that waits for it: the
first pass of the culling runs there, and the binning of the lights after it, beside the shadows,
which draw what the culling keeps. What a frame records outside its
passes, its writes and uploads, comes before them, in its first batch, which the compute queue's
first waits for if it wrote a buffer; the graphics queue's last batch waits for all the compute
queue ran, and the frame's timeline is signaled after it. A
pass that computes dispatches compute shaders, which write storage images, 16-bit floats in linear
RGBA, of a 2D image's one layer or a cube's six faces, a mip level at a time through slots of the
bindless set's own for storage images, in the general layout, as their last use left them, or
discarding it, for a pass that writes every texel. A storage image is among a frame's targets, as
an offscreen image is, and keeps the layout its last frame left it in, so that once a frame whose
last use of it sampled it is submitted, frames after sample it through its texture slot without
declaring it; the graph's own colors of high dynamic range are written so too, through a slot of
their own, as the view's shading writes the view's, and so are its images of a 32-bit float a
pixel, as the view's ambient occlusion is. A pass may draw depth alone, as a shadow map's
does, and may draw over the colors, IDs and depth an earlier pass left, as the view's second pass
does; its color target may be a visibility buffer of the graph's own, two 32-bit integers a pixel,
cleared to 0, beside its IDs and depth, which later passes sample as integers. Beside its IDs, a
pass may draw motion, 16-bit floats in RGBA, a third color target, which the pipelines that write
depth write and the rest leave as it is, and compute passes write as storage. Later passes sample
depth by comparison, as shadow maps, or a texel at a time, as the view's depth pyramid reads the
view's. The view's images, its colors of high dynamic range, IDs, motion, visibility buffer, depth
and tonemapped colors, and the shadows' maps, are the graph's own: each lasts for a frame,
undefined until a pass there writes it, and is made from what the passes say of it, in a block of
memory that the graph's own images whose uses in a frame don't overlap share, kept for the frames
after that declare it alike, by its name, kind and size; a pass that's an image's last use doesn't
store it, and an image's next use waits for what its own last use left as well as for what the
block's did, as the view's tonemapped colors follow its depth there. The graph's own buffers last
from frame to frame, each in the GPU's memory of its own, which shaders reach through its address,
kept for the frames after that declare one of its name no larger, holding what the last of them
left, and made again at the next power of two bytes past that, holding nothing defined; a frame's
first use of one waits for the frame before's last, by a barrier on one queue and by a semaphore
across the two. A pass reads back up to 256 bytes of one, as the frame two later finds them. Each
pass is a debug label, and
the draws inside it have labels of their own, each run of draws its shader's name, and the
canvas's, and the frame pass's view and UI, apart. Each pass writes a timestamp as it begins and as
it ends, which come back as the frame two later begins: with the `tracy` feature, Tracy has a GPU
timeline for each queue, which shows each pass there, placed against the CPU's timeline by the
GPU's clock, read at once through `VK_KHR_calibrated_timestamps` as the run starts and each second
after, where the driver can; and every run's `engine.perf` logs the frames' time on the GPU, from a
frame's first pass on either queue to its last (`gpu_ms_mean`, `gpu_ms_p95`), and the memory of the
graph's own images and buffers (`transient_bytes`).

The view may show something in place of its light, to see how it's made, as the tool API's look or
a capture asks and the editor's toolbar sets: a part of what lit shading takes or makes at each
point of what models draw, which shading.slang's `lit` returns in place of its light, by a number
in the frame's data, both where the visibility buffer shades and where models shade as they draw:
the base color, the normal before and after its normal map, how metallic and how rough it is, its
occlusion, its emission, or the light the frame's lights, the sky and the view's reflections each
give it; or an image of the graph's own, by its debug name, which a compute pass reads by its kind
into colors of the view's own, shown in place of the view's. The frame shows it as drawn: without
antialiasing's offset or anything after the view's passes, no antialiasing, fog, depth of field,
motion blur, bloom, grade or lens, nor the exposure metered, which adapts on as it did; without the
sky; and through the tonemapper only where it's light, so that the view's 8-bit sRGB image stores a
color as a texture holds it and a value as its byte over 255, as gg-api's `Shown` says, and a
capture's bytes read as numbers. It keeps nothing for the next frame's reflections, which trace
against what the last frame that showed the view's light kept, and the frames after it antialias
afresh. The frame's targets, as TAA's history and the reflections, have no names in its graph, and
show by none.

Layers add shaders, meshes and textures and draw them through their `Context`, and the renderer's
`Shaders`, `Meshes` and `Textures` hand out their handles at once, on the main thread; each frame's
render data carries what's been added to them, replaced in them or removed from them since, in
order. After the
frame's ticks and the layers' updates, the engine draws the world's entities that have a `Transform`
and a sprite, a circle, a `Text`, a `MeshDraw`, a mesh with its shader and parameters held without
their types, or a `ParticleEmitter`, after what the layers drew, children where their ancestors
carry them: first the meshes whose shaders are opaque, whose depth hides what's behind them whatever
order they draw in, which the renderer's scene holds, as below; then what blends, back to front by
their z's in the world, those at one z drawing sprites, then circles, then text, then meshes, under
emitters' particles, each emitter's as a whole, as below; each z in the order of their IDs, which
overlapping entities keep as others come and go. World z orders what blends rightly as seen
from in front of layers parallel to XY, as 2D cameras see them; from behind or the side, what blends
draws in the wrong order among itself, though rightly against what's opaque. The view draws
everything opaque, the layers' and the world's, before anything that blends, as culling, below,
draws it in two passes: what blends, in its order, goes over what's opaque behind it, whatever
order the two were drawn in, and at one depth, an opaque mesh goes under what blends. Of two opaque
draws at one depth, the later goes over, but in the frame the earlier comes into view.

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
set, and draws into the view's image once the view's passes have drawn it, so that captures and the
view shared show it too, round whatever shape the entity shows, its colliders' outlines among it,
going across what's in front of it rather than round what's hidden.

Frames draw the world between ticks, so that it moves as smoothly at any frame rate. As each tick
starts, the engine copies where the world's entities are, outside the world, so that no hash,
scene, snapshot or recording holds the copy, each table's copy taking again only the rows whose
transforms or parents changed, while no row joined or left it; each frame draws what the last tick
moved, and what it carries, as far on from where the tick found it to where the tick left it as the
frame is past the tick, its translation and
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

An entity with a `ParticleEmitter` and a `Transform` emits particles where it draws, ancestors
carrying it, which belong to the frames rather than the world: the world holds the emitter, which
scenes save, the Inspector and tools edit and the world's hash reads, but no hash, scene, snapshot,
recording or rollback holds a particle, as none holds the frames' copies of where entities were.
Each frame emits what the world's time as it draws, its ticks and the fraction of the latest it
draws, owes each emitter since the frame before: births at its rate, birth n born n over the rate
seconds after the end of the latest tick as a frame first drew the emitter, each where the emitter
was then, between where the frame before drew it and where this one does, so that one that moves
leaves a trail rather than clumps; and a burst for each step its count of bursts went up by
since the frame before saw it, born as the latest tick ends, where that tick left the emitter, so
that a rollback or a server's correction that runs the tick again bursts once, and one that takes
the count back down bursts no more. The world's time stops while the simulation is paused or
stopped, and so do its particles, and headless frames, a tick each, emit and draw the same every
run. A rate that changes goes on from where its next birth was due; time that goes back a tick, as
a pause's end draws it, holds births back until it's past them, and further back, as the tool API's
`stop` takes it, starts the emitter's births again. What the frames emit an emitter from is kept by
its entity's ID, outside the world, and let go of the frame after one that doesn't draw it. A
particle's motion is a closed form of its age, its velocity at birth carried on under its emitter's
constant acceleration and linear drag, and its size, color, turn and cell of its texture's sheet go
by its age over its lifetime, so that a particle is its birth record, and draws alike whatever the
frames between its birth and now; its randomness is a hash of its emitter's ID and its number, or
its burst's and its own within that, so that it's born alike whatever the frames too. Its time is
the world's ticks: a record holds the low 32 bits of the tick it was born in and the fraction into
it, which the shader subtracts as integers first, so that its age stays exact however long the
world has run.

The renderer keeps the birth records in a pool on the GPU, a buffer of the graph's own of 32 bytes
a particle, each emitter's in a ring of its own, kept by the emitter's ID while each frame draws
it: a block of a power of two slots, at least as many as the emitter has alive at once, its rate
times its longest lifetime and a burst, given by a buddy allocator, a block let go of free from the
next frame on. The pool doubles as rings need more room, a frame that grows it copying the old into
the new in a compute pass before the rest, the two taking the pool's two names by turns, up to 2^20
particles, 32 MiB, past which an emitter that doesn't fit draws nothing, warned of once
(`renderer.particles_full`). A ring made or grown is written whole, the particles of the ring it had
first, so that drawing never reads a slot no frame wrote. A compute pass on the graphics queue,
before the culling, writes each frame's births, a thread a birth, from a few numbers an emitter:
when and where between the frames the first is born, and how many follow how far apart; births
past what a ring holds drop the oldest. Each emitter's particles draw in its place among what
blends, an instance of the engine's quad for each slot of its ring, from its oldest particle's,
facing the view along its right and up, each placed, sized, colored and turned in the vertex shader
from its record and the world's time, a slot whose particle isn't born yet or has died drawing
nothing: a soft disc or a texture's cell, over what's behind it by premultiplied alpha, or adding
its light by writing no alpha, through one pipeline, testing depth without writing it or motion,
and writing its emitter's ID where it adds more than next to nothing. Within an emitter, particles
draw in their ring's order, oldest first. They draw before temporal antialiasing resolves the view,
with everything else that blends, so that an emitter sorts by its z among the world's other blended
draws, which the canvas, after the resolve, couldn't; they write no motion, so the resolve takes
what's behind them, and its clip to what the frame drew round each pixel keeps a moving particle
from trailing, while their soft edges shimmer little under the jitter. A frame that renders no view,
or renders nothing, lets go of every ring, and so of the particles before it.

A game's shader is a Slang file that the engine compiles with slangc as a layer adds it, so one that
doesn't compile fails there, with slangc's diagnostics naming the file and line; the renderer makes
its pipeline the next time it renders. slangc compiles every shader, the engine's too, unoptimized:
drivers optimize what they compile, and slangc's optimizer reads a struct through an address a field
at a time, which AMD's driver miscompiles. A mesh's vertices are any `#[repr(C)]` type without padding,
with glam's vectors and matrices among its fields, which stream into GPU memory with the indices,
as below, where they stay until the mesh is removed. Shaders read vertices through their buffer's address
rather than through vertex input, and Slang lays out what a pointer points at as `#[repr(C)]` does,
so a mesh's layout is between its vertex type and the shader's struct for it; draws take the
indices. A shader is added for a vertex type and a type of parameters, such as a color, which each
draw gives it: a mesh draws only with a shader for its own vertex type, and with parameters of that
shader's type. Each frame copies the draws' transforms, IDs and parameters into the frame's own
memory, where the shader reads them through their addresses too, so that a layer's draws take
parameters as large as their shader needs, though an entity's `MeshDraw` holds 64 bytes of them at
most; and the shader writes each draw's ID beside its color, and, if it likes, how far what it
draws moved in the world since the frame before, as antialiasing, below, takes it. A shader is
added with how it blends
too, since Vulkan bakes blending into its pipeline: opaque, which writes its depth as well as
testing it, or by the alpha it returns, straight or premultiplied, which only tests it, so that
what's behind it shows through.

A model's mesh holds its vertices packed, 24 bytes each where the floats of a `ModelVertex`, as
glTF is read, take 64: packed by the cooker, and by a development run as it adds a model, so that
the model shaders read one layout. A vertex's position is 16-bit steps from an offset, the step the
least power of two that spans the mesh's longest side in 65,534 and the offset a multiple of it,
so that meshes of sizes within a factor of two put their vertices on one grid, and an edge two of
them share keeps its ends together; its normal and tangent are each folded onto an octahedron as
two signed 16-bit numbers, a tangent along its normal, which leaves shading no tangent space,
square to it instead, and its bitangent's sign sits beside its position; its texture
coordinates are 16-bit fractions of the mesh's box of them, and its color bytes, sRGB-encoded but
for its alpha. The mesh's buffer starts with what unpacks them, which the shaders read as they read
a vertex, and the sphere round a model's mesh, which views cull it by, holds its vertices as they
unpack.

Vulkan checks nothing about a shader, so the engine checks the SPIR-V slangc writes against the
contract `Shaders::add` describes, as the shader's added and as it reloads, and refuses one that
breaks it, naming each way (`renderer.shader_mismatch`). It reads the SPIR-V because Slang's
reflection names what a pointer points at but none of its fields.

In a development run that serves the tool API, a thread of its own, `gg-watch`, watches the files
each game's shader, texture, model, font, prefab and UI read as it was loaded, a shader's own and
those it includes as slangc lists them, a texture's image, a model's glTF file and the buffers and
images it names, a font's file, a prefab's and a UI's, by their times and lengths every tenth of a
second, and reads the asset again as they change and then hold for a look, off the main thread:
compiles the shader, decodes the image, reads the model, bakes the font, reads the prefab's text,
reads and checks the UI's tree. As the next
frame starts, the main thread puts what it read
in place of the asset, under the same handle: a shader draws with its new pipeline from that frame,
the old retiring until the GPU is done with it, and a mesh's or a texture's new bytes stream as any
asset's do, below, whatever draws it drawing what it had until they can draw, and then what's new,
without waiting for the frames before. A texture takes an image of any size,
but one cut into a sprite sheet, whose cells a layer has where its size put them, keeps drawing as
it did rather than take one of another (`asset.sheet_resized`). A model takes each primitive's
vertices and indices and each image, which its entities draw at once, and its nodes and materials,
which its roots' nodes take, as [Models in scenes](#project-state) says, and other numbers of
primitives and images, those past what it had added, or given back the meshes and textures of
their places, which those it has no more keep for them; but in a run
that's recorded or in a lockstep session, whose world changes only as the run it goes with changed
it, a model takes its vertices, indices and images alone: its nodes and materials stay as they were
until the run starts again (`asset.model_changed`), and a model of other numbers of primitives or
images takes nothing. In any run, a model whose image holds something else now, as colors where it
held a normal map, takes nothing (`asset.model_changed`), as a texture keeps what it holds, until
the run starts again. The engine adds its model shader for every alpha mode with a run's first
model, so that a material read again adds none. A font takes what it baked, its field's texels at
any size, which its text is laid out by from that frame. A prefab takes its text, which its
instances take, as [Prefabs](#project-state) says, but in a run that's recorded or in a lockstep
session, which keeps it until the run starts again (`asset.prefab_changed`). A UI takes its tree,
as [UIs](#project-state) says, in any run, as it's the run's alone. A texture's or a font's
stand-in takes its
file's once the file comes, or the file of the asset whose ID it stands for, once one has it, but
never a file outside the asset root (`asset.outside_root`); and a texture, a model, a font, a
prefab or a UI whose file under the root is renamed or moved, as the asset database finds it by the
ID it has, or that
the database gives its file once it has one, is watched where it is, a rename alone reading nothing
again. A change that doesn't compile, decode, read or bake, or breaks a shader's contract, warns,
and the asset keeps drawing as it did. The engine's own shaders and font don't reload, nor do the
meshes layers make themselves.

Windowed and embedded runs keep a pipeline cache, which they load from the OS's per-user directory
for caches as the renderer starts and save as it ends, so that pipelines an earlier run made are
made again in a fraction of the time: an embedded run, the game the editor plays, starts afresh at
each change `gg run` can't patch in. Other headless runs and replays, as most tests run, use none,
so that their startup times compare.

A game's texture is a PNG or a JPEG that the engine decodes to RGBA8 with colors sRGB-encoded and
alpha not multiplied into them, as images store them, whatever color profile or gamma the file
holds, as glTF requires; or a Radiance HDR, `.hdr`, of colors of high dynamic range, as a sky's,
which it decodes to RGBA of 16-bit floats, linear, alpha 1, a color past the most a half float
holds clamped to it, of one level; a model's images are PNGs and JPEGs, and DDS files of BC7 as
MSFT_texture_dds names them, which stay compressed. It streams into GPU
memory, as below, where it stays until it's removed, and shaders sample its colors decoded to
linear, or a Radiance HDR's as they are. A run from a game's package reads it as the cooker
compressed it instead, BC7 of those colors, or BC6H of a Radiance HDR's, which stays compressed in
GPU memory, a quarter of RGBA8's size, and which the GPU decodes as shaders sample it; a model's
maps of data are BC7 of their values, and its normal maps BC5 of their X and Y, which shaders make
Z from, as they do of an uncompressed normal map's. What the simulation reads of a texture
is its handle and its size, as a sprite sheet's cells are cut by it, so a run reads its file and its
header on the main thread as a layer adds it, or as text names it, as [Project
state](#project-state) says, which hands out the handle at once, and a file that's missing or isn't
an image of a size the engine takes fails there; its pixels, PNG's, JPEG's and Radiance HDR's or
the package's levels supercompressed by zstd, decode on threads of their own, `gg-load`, each
taking the next in the order they came, and fill each texture as the frame after its image is done
starts, which shows its stand-in until then. Pixels
that don't decode warn (`engine.bad_texture`), and the texture keeps its stand-in until its file,
still watched, reads again; the cooker refuses them, so a package holds none. A test's context and a
replay's decode on the main thread as they add, as nothing draws them, failures warning alike, so
that a replay goes as its run did. The engine reads a texture, a model, a
scene, a layout or the pipeline cache only from a regular file, so that a FIFO or a device can
neither hang the read nor fill memory; `--replay` alone reads its recording from whatever it's
given, so that it can come through a pipe. A texture's handle is its index in the bindless set,
which a draw's parameters carry as it is and the shader samples by, with one of the engine's
samplers, which it picks by its index too. A model's textures have their full chains of mip levels,
which `gg-load` makes as it decodes them, the render thread for a model's images read again or in
a test's context, or, from a package, the cooker made the same way. A layer that adds a
file again, by whatever form of its path, as a texture or a sprite sheet, a model, or a shader of
the same vertex and parameter types and blend, gets what the first add made, the file loaded once,
as the OS resolves `.`, `..`, links and, where names ignore it, case; a texture keeps the path it
was first added by, which its other paths find it by too, and its name, as [Project
state](#project-state) says. A path is as the layer wrote it, rather than as the disk spells it,
which a file system that ignores case keeps as an editor last saved it, so that every checkout of a
game names its files alike.

Meshes and textures stream into GPU memory: each frame copies at most 16 MiB of them, in the order
they came, and leaves the rest to the frames after. One larger than what's left of a frame's budget
goes in parts, a buffer by ranges of bytes and a texture by whole rows of one level, or its smallest
levels together, so that every frame copies something and what stages them holds about a frame's
budget. They go the best way the device has, as gg-gpu's `UploadPath` says: through a queue apart
from graphics, where the device has one, whose copies no frame waits for, an asset drawing from the
first frame that begins once its last part has been copied, which waits for that queue and takes a
texture over from it, and a frame that begins with two frames' copies still running there copying nothing, so that what's
in flight stays bounded however fast frames come;
otherwise in the frame's own commands, or from the CPU, drawing in the frame that copies its last
part. Until then a texture's slot in the bindless set shows a stand-in, a white texel for colors and
for a surface's map, which leaves a material's factors as they are, a texel of zeros for data, so
that text draws nothing, and a normal pointing straight out for a normal map, and a mesh draws
nothing; one replaced, as a
reload replaces it, draws what it had until what's new can draw. There's a bindless set for each
frame in flight, so that a slot that changes, as a texture shows its own image or another, changes
at once in the set of the frame being recorded and in the other as its next frame begins, once the
frame before has finished with it: no frame waits for those before it. What staged an asset, and
what's retired, is freed once the frames and the copies that may use it have finished. An asset
waits too while making it would take the GPU's memory past the budget its driver gives
(`VK_EXT_memory_budget`, where the device has it), warning once (`renderer.over_budget`), while those
after it go on, and goes on itself once removals make room; but while nothing else is copying, the
oldest waiting goes ahead alone, so that a game still comes up on a GPU that other processes have
taken past its budget. One whose memory can't be had at all waits likewise
(`renderer.out_of_memory`), rather than ending the renderer. A mesh, texture or shader removed is
freed once the GPU is done with it, and its handle is never given out again: a removed texture's
slot shows its stand-in, and a removed mesh or shader draws nothing.

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
and a projection, through `Context::set_viewpoint`, as the engine's camera controllers do, steering
it itself or with one of them. A viewpoint isn't in the world: it moves every frame, by input, where
the world changes only at ticks, so no hash, scene or recording holds it.

A layer sets how the view is antialiased in place of its environment's, through
`Context::set_antialiasing`, as Foxball's options do, a world without an environment antialiased by
it too, until it's unset. It's the run's own, as the buses' volumes are, and for the same reason:
the environment is the world's, which the hash reads, so a player's choice written into it would
part lockstep peers whose players chose otherwise, and their replays and recordings with them. No
hash, scene, snapshot or recording holds it.

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
nearer, as debugging wants. The canvas, a game's UI, as [Events and layers](#events-and-layers)
says, is a list after those, in the view's pixels rather than through its viewpoint, all at the near
plane, where depth is 1, which every depth test passes, so that each of its draws goes over all
before it. Draws are clipped to a rectangle by a scissor, `Draws::clip`: each clip starts a run of
its own, which the mesh pass draws within the box round where the view shows it, to the nearest
pixel, so that a canvas's clip is placed with its draws as the frame's handed off, a list's or a
text field's of a UI drawing as one scissored instanced draw of each mesh and shader, rather than a
rectangle in each instance tested in the shaders, which every shader that draws on the canvas would
have to take; the view's culling doesn't know a clip, and culls a clipped draw as if it weren't.

Text is drawn from fonts' fields, which `Context::add_font` bakes as a TrueType or OpenType font
loads, or the engine as text names one, or the cooker into a game's package: a texture of data,
which shaders read as stored rather than decoded from sRGB, holding a multi-channel signed distance
field of each
glyph of the characters Windows-1252 prints that the font has, and of its `.notdef`, which draws in
place of any other character but whitespace and controls, 48 texels to an em, its distances
reaching 4 texels either side of each glyph's edge, and in its alpha the true distance, which
outlines and shadows will read. The median of a texel's three distances keeps a glyph's corners
sharp at any size, where a single distance would round them off. fdsm generates each glyph's field
from the outline skrifa reads, unhinted, on the job system's threads, a fixed seed coloring its
edges so that a font bakes the same field every time, and corrects its errors. A process bakes a
font's bytes once, keeping their field as long as it runs, as tests build a game's context again
and again, baking on their own thread alone, where a debug build takes about a second a font. A
font whose glyphs' cells, alone or together, wouldn't fit a field of the largest texture's size is
refused before any is made, as a crafted font's would take gigabytes. `Context::draw_text`
shapes each line of a text with harfrust, for the font's kerning and its marks' places, but without
the features that put glyphs in place of others, as ligatures and contextual alternates, whose
glyphs the field may not hold; and draws each glyph on the engine's quad with the text shader, an
instanced draw for a run of them, as sprites are, which smooths a glyph's edge over a pixel by how
many of the view's pixels the field's range spans there, so that text is as crisp drawn large as
small. Text that names no font is in the engine's own, Inter, as the UI is, which bakes the first
time text is drawn in it, into a texture every context keeps for its field from the start, so that
no texture's index depends on when; a font's stand-in draws in it too. The world draws an entity's
`Text`, its text in a font, an em of a size, a color and an alignment, where its entity draws, as
`Context::draw_text` draws text, each glyph its entity's draw, so that a pick of it finds the
entity; scenes save it, naming its font as [Project state](#project-state) says, and the Inspector
and tools edit it, as any component.

`Context::draw_grid` draws a grid on one of the world's axis planes as a viewpoint sees it, as the
editor's view shows one: lines a power of ten apart, fading as they near each other and toward
grazing angles, and the plane's two axes in their colors, with a shader and a mesh of the engine's
own, each added the first time something draws with it, the mesh a fan round the point under the
eye, which keeps the grid's precision there. It blends in the layers' order, over what's drawn
before it and what's opaque behind it; the tool API's look's grid draws after the layers' draws and
before the world's, so over a backdrop a layer draws, as Shards' floor, and the world's opaque
meshes behind its plane, and under those in front of it and what of the world blends.

3D games draw models: `Context::add_model` reads a glTF 2.0 file and adds its meshes and its images,
as textures, once, all of itself or nothing, or the engine does as text names it, and a root with a
`ModelRoot` of it, as `Model::spawn` makes one and a scene loads one, has its default scene spawned
under it as entities, as often as the game likes, as [Models in scenes](#project-state) says: a node
each, with its transform in its parent's space, and a child for each primitive of a mesh with
several, since an entity holds one `MeshDraw`; its skins, the joints that move its skinned
primitives, and its clips are its assets, shared by every root, which the world reaches each tick.
The engine reads glTF itself, over serde_json,
rather than through the `gltf` crate, whose public calls panic on some malformed files. A model's
URIs can't name a file outside its folder, though a link inside the folder is followed wherever it
leads.

Draws are batched, every mesh and not only quads: each run of consecutive draws of one mesh with one
shader, drawing the same faces, is a single instanced draw, whose instances are the draws, and the
frame copies the draws' transforms, IDs and parameters into its memory once, each in an array of
its own, which shaders read by the instance's index. A change of texture is a change of parameters,
which bindless textures make free, so 100,000 sprites take a handful of draws; shaders index their
textures through `NonUniformResourceIndex`, since the instances of a draw can carry different ones,
and the engine decorates as non-uniform the sampled images slangc makes of them, as Vulkan requires.
Nothing caps a run: the frame's memory grows to hold it, where a renderer with a buffer of fixed
size would draw what it holds and start another.

The world's opaque meshes stay on the GPU from frame to frame, in the renderer's scene, which the
engine changes as the world does, rather than drawing them every frame: each entity with a
`Transform` and a `MeshDraw` whose shader writes depth is an instance in the scene's slot of its
entity's own, its transform, where the frame before drew it, its ID and parameters a record in a
table in the GPU's own memory, set as
the entity joins a table, its mesh changes, drawing places it again, or it draws on its way between
ticks, and once more as it stops, and taken away as it goes, which, as rows changed, the engine
looks over the instances held for. A frame writes the records that changed into the table, each
where its slot puts it, in one copy, after earlier frames' reads, and the instances' order whole as
it changes: by shader, then mesh, then the faces they draw, then ID, as many instanced draws for
each batch as its mesh has levels, culled as below, whatever its instances. The table is made again
twice the size as slots past it are used, from the records the renderer keeps, and the scene's
changes go to the renderer with the uploads, which a frame that doesn't render leaves for the next.
So a frame costs the CPU what changed of the world, rather than what it draws: as much for 16
Bistros as for one, but for the lights, which a frame finds again only as one changes, or what
carries it, or one moves between ticks.

A mesh may have levels of detail, as a cooked model's primitives do: up to eight ranges of its one
index buffer, finest first, each with how far its surface strays from the finest's, in the mesh's
own units, and a sphere round its vertices, which a model's primitive of one level has too. Each
draw of it draws the coarsest level whose error, grown by its transform's longest axis, spans no
more than a pixel of the view where the sphere comes nearest the camera, which the clip space's w
there gives, a rigid view keeping the pixels a unit spans; a draw whose sphere reaches the camera's
plane draws the finest. A mesh a layer adds without levels has no sphere, as the engine can't read
its vertices' positions.

The scene's runs and the layers' opaque ones are culled on the GPU, in compute passes before the
shadows' and the view's, which first gather their instances, the scene's from its table in its
order and then the frame's from its arrays, into arrays of the graph's own, for the view and for
each of the shadows' cascades: an instance is kept where its mesh's sphere, as its transform places
it and grown by the most it stretches, reaches inside every plane of what that view sees, its far
plane at infinity under perspective, and, for a cascade, but its near side, which casters past cast
from; a mesh without a sphere is always kept. The same passes pick each kept instance's level, as
above, and write each view's, each level's in the order they came, into buffers of the graph's own,
with the commands of indirect draws, whose instance counts and first instances they write: the CPU
records a draw for each run and level of its mesh, whatever its instances, and a run's shader reads
its instances as any does, from where the commands' first instance says.

The view culls what's hidden too, in two passes. Its first pass draws what the last frame found
unhidden, of what's in view, and then compute passes build a depth pyramid from its depth: half the
view each way, each texel the farthest of the 2 by 2 under it, and each level after half the one
before, down to a texel. The culling's second pass tests what the first didn't draw against it: the
rectangle a sphere covers on screen, which its tangents through the camera bound under perspective,
falls within 2 by 2 texels of the level whose texels are as wide, and an instance whose sphere's
nearest point is farther than the farthest of those is hidden. The
view's second pass draws what isn't, over the first's, so that what comes into view draws in the
frame it does, and then what blends and the canvas, from the frame's arrays on the CPU's levels,
once for each stretch of draws in a row at one level, keeping their order: in fog, what blends and
the canvas draw in the pass after the fog's instead, as above. The second pass finds
again whether each instance is unhidden, for the next frame's first, a word of a buffer of the
graph's own that lasts from frame to frame, by where the instance is gathered: a frame in which
instances before one came or went takes another's word for it, which draws in its first pass what
may be hidden, and in its second what the first missed, for a frame. The shadows' cascades cull by
their boxes alone. The renderer counts the view's draws that have instances in either pass, their
instances and their triangles, the culled runs' on the GPU, which come back as the frame two later
begins; the Performance panel shows them and `engine.perf` logs their peaks.

A cooked model's opaque and masked meshes are cut into meshlets, each level's apart, each of up
to 64 vertices and 124 triangles, with the sphere round them and the cone their triangles face
within, which the mesh's buffer holds after its vertices, and the renderer's table of meshlets'
bounds, in the GPU's own memory, holds for the culling, each mesh's added as it comes to be
drawable, the table made again twice the size as it fills. A culled run of such a mesh with a model
shader is drawn by its meshlets: after the culling's passes keep its instances, as above, a compute
pass in each keeps each kept instance's meshlets of its level where the meshlet's sphere, as the
instance's transform places it, reaches inside every plane of the view; where the view sees its
triangles' fronts, as its cone says, for a run that draws fronts alone, of a transform that keeps
angles, which a cone turns with; and, in the view's second pass, where the depth pyramid doesn't
hide it. The meshlets of what the view's first pass draws aren't tested against a pyramid, which
isn't built yet. Each meshlet kept goes into the run's draw in that view, as its instance's place
and its index among the mesh's, after the draw's command, in the culling's buffer of commands, with
room for every instance's meshlets of the mesh's largest level, to 2^22, past which they're left out;
the draw is one for the run, of all its levels' meshlets. A device that draws through mesh shaders
(`VK_EXT_mesh_shader`), as the RTX 4090 and lavapipe do, draws a workgroup of the model's mesh
shader for each meshlet kept; any other, as the AMD iGPU here and KosmicKrisp, an instance of a
vertex shader's draw for each, of 124 triangles, the corners of those past the meshlet's own its
first vertex, which draw nothing; their fragments shade alike. A frame plans each view room for
32 MiB of meshlets at most, one run's at its largest, so that a few runs of many instances, as of
foliage, can't ask more of the GPU than it can make, which would end the renderer. A run whose
meshlets the frame planned no room for, as of a mesh that came to be drawable after its graph was
planned, or past that most, is drawn by its levels' indices for that frame; so is what blends, which
keeps its triangles' order, and a model read as a development run reads it, which has no meshlets.

The culled runs drawn by their meshlets with the opaque and masked model shaders draw into a
visibility buffer rather than shading as they draw, in both of the view's passes: at each pixel, the
run and the pass whose draw drew it, and the meshlet among those that draw kept and its triangle,
with the instance's ID and depth beside it, a masked material cut out as it would be. Through a mesh
shader, which writes each triangle as its primitive's ID, read behind it through SPIR-V's Geometry
capability, so that a device draws through mesh shaders only with `geometryShader` too, which the
engine enables with them; through vertex shaders, whose triangles' corners are each their own, the
corners carry it. A compute pass then shades every pixel of the view's colors, a pixel a thread,
the clear color where nothing drew: it finds the triangle again through the culling's commands and
its run, unpacks and places its corners as the draw did, weighs them at the pixel's center, and a
pixel across and down, by the edge functions of the corners in clip space, homogeneous in X, Y and
W, which divide by no W, so that a corner behind the camera, which the draw clipped, weighs right
too; and so interpolates what the draw's fragments took, picking the textures' levels by the change
of their coordinates, and shades as they would, by the code both share. The view's pass after it
draws the sky behind everything, then the culled runs drawn otherwise, as games' opaque shaders and
models without meshlets, or whose frame planned no room for them, as their shaders shade them, then
what blends and the canvas, against the buffer's depth, or in fog, after the fog's pass, in the pass
of their own that follows it. The depth pyramid is the first pass's depth,
of the buffer's draws alone. `Settings::visibility_buffer` off has them shade as they draw instead,
for tests and measurements of each way.

A view that its environment gives ambient occlusion, as by default, seen through a perspective
and lit by a sky, darkens the light from all around, the sky's diffuse and specular light, at each
point it shows where what's near the point hides that light from it: ground-truth ambient
occlusion (Jimenez, Wu, Pesce and Jarabo, 2016), as Intel's XeGTAO finds it from the view's depth,
with its defaults and its high quality, in compute passes once the depth of everything opaque in
the view is drawn and before any of it shades. The first takes the depth to view-space depth, the
distance ahead of the view, in five levels, each half the one before, in a buffer of the graph's
own, each texel the mean of the 2 by 2 under it weighted toward the farthest, so that a coarser
level keeps to what's behind a thin occluder; the second searches each pixel's horizons along
three slices of the hemisphere round its normal, which the depth round it gives, three steps each
way, out as far as the environment's radius, half a unit by default, reaching farther steps through
coarser levels, the slices turned by noise that runs along a Hilbert curve from pixel to pixel and
stays still from frame to frame, however the view is antialiased, as the temporal resolve clips its
history to each frame's pixels, and so keeps noise that changes with the frame rather than averaging
it; and the denoise blurs each pixel with the eight round it, but across where the depth breaks,
three times, into an
image of the graph's own of a float a pixel, the fraction of the light from all around that
reaches it, raised to the environment's strength times XeGTAO's 2.2. Shading darkens the sky's
light, never the lights', by the less of that and the material's occlusion map: both estimate how
much of the same light reaches the point, the map at its texels' scale, so that their product would
darken twice where both see one occluder, as Filament takes them. That darkens the sky's diffuse
light; its specular light is darkened by the specular occlusion Lagarde and de Rousiers derive from
it, of the way to the eye and GGX's alpha, as Filament takes it ("Moving Frostbite to Physically
Based Rendering", 2014): a rough lobe, which gathers from all around as the diffuse does, as much
as the diffuse, and a smooth one less head-on and more where it looks along the surface, toward
what hides its light; so too where there's no ambient occlusion and a material's map alone darkens
it. What blends shows over what's
opaque, whose occlusion this is, and takes none. Games' shaders can read it, through the frame's
data, a texture loaded at a fragment's pixel, in the view's own passes; the canvas's frame data
names none.

Beside a visibility buffer, which holds the depth of what it draws, a pass then draws the depth
alone of the culled runs drawn otherwise, where any may be, games' opaque shaders, skinned models
and those without meshlets, into a depth image of its own, through a pipeline of each shader's
vertex shader alone,
as its shadows are drawn but neither clamped nor set back, the masked model shader's cut out as
what it draws is, and the occlusion, and the reflections, below, which need the depth too, take
the nearer of the two depths at each pixel. Without one,
the view's first and second passes draw that depth alone of every culled run, the depth pyramid
built from the first's, and the view then draws all of it again, shaded, in one pass that clears
its depth, in fog all but what blends, which draws after the fog's pass, rather than over the depth
alone: a game's opaque shader may discard, which its depth
alone doesn't, and two pipelines aren't bound to place a vertex alike. So what's drawn forward,
as every model of a development run is, has its opaque vertices drawn twice. A frame without a
sky, or under an orthographic projection, or whose environment has none, or a radius or a strength
that isn't positive, runs none of it and takes none of its memory.

A view that its environment gives reflections, which it doesn't by default, seen through a
perspective and lit by a sky, shows in what's opaque what the view shows, each point's mirror ray
traced through the view's depth, in place of the sky's specular light where the ray finds
something: screen-space reflections, in compute passes once the depth of what's opaque is drawn,
as the ambient occlusion's is, and before any of it shades. The first build a pyramid of the
nearer of the two depths, the nearest of the 2 by 2 texels under each texel, its first level the
view's size and each after half the one before, rounded up, in a buffer of the graph's own. Beside
a visibility buffer, a compute pass first finds each pixel's normal from it, where it drew, as the
view's shading turns it, its normal map's bumps among it, the same way the shading finds its
triangle's corners, into an image of the graph's own. Where the depth alone of what's drawn
otherwise was drawn, a pass after it draws the opaque and masked model shaders' normals, as they
shade them, into another, tested against that depth without writing it, so that what's nearest
there alone draws, and games' shaders, which have no way to draw one, draw none: what's drawn
forward has its model shaders' opaque vertices drawn a third time. That test takes the normals'
pipelines to place a vertex as the depth's do, as the drivers the tests run on do; a pixel one that
doesn't leaves takes its depth's normal, below. The trace takes the normal of what drew the nearer
depth at the pixel, where that has one, and elsewhere finds it from the depth round the pixel,
which is a curved mesh's facets', reflects the way from the eye about it,
and follows the ray through the pyramid as AMD's FidelityFX SSSR does, climbing a level each time
it crosses a texel wholly in front of what that holds and stepping down where it meets it, until
it's behind what the first level holds, as far as the environment's distance, 100 units by default,
short of the near plane. It trusts what it found less the farther behind it the ray passed, as
thick as the environment's thickness takes what the depth shows to be, 0.25 units by default,
toward the view's edges and the ray's reach, and as the ray turns back toward the view, which shows
nothing behind it; nor does it find the sky, which the sky's own light stands for, or what it meets
from behind. What it found is the colors the frame before showed, antialiased, temporally or by
SMAA, before depth of field, motion blur, bloom, the grade, the lens and the canvas, where it showed
then, as that frame's camera saw it: each frame that reflects
keeps them, by a pass of its own, in an image of the view's size, and a frame after one that kept
none, or kept them at another size, traces nothing, as the first that reflects. The trace writes,
into the first level of the view's reflections, an image of the view's size of up to seven levels,
the last's texels 64 pixels a side, what
it found times its trust, and its trust, and how far a reflection of it spreads on screen, for a
lobe of each width, into an image of the graph's own; each level after averages the 2 by 2 under
it. Shading, which knows each point's roughness, samples the level as wide as its lobe's footprint
on screen, the cone Uludag gives Blinn-Phong's lobe of the power GGX's alpha matches, takes its
trust off the sky's specular light, and adds the reflection by its trust, times what the specular
light's reflectance reflects of it, as light exposed already, as the colors it was found in were,
so that neither the sky's luminance nor the exposure scales it, and a sky that gives no light, as
an atmosphere's after its sun has set, still shows what lamps light; the trust faded out toward the
environment's roughness, 0.6 by default, past which a surface takes the sky's light alone. What blends takes
none, as it takes no occlusion; the canvas none either, and a canvas that would draw in the view's
last pass draws in a pass of its own after the colors are kept, so that the reflections hold no UI.
The two images last while the view reflects, and a frame that renders it without reflections lets
them go.

So that both the visibility buffer's shading and the forward model shaders take reflections alike,
by one sample at a pixel, as they take the occlusion, the trace runs before shading, from the depth
alone: rays follow the depth's normal, not a normal map's, and blur by roughness on screen round the
point that reflects, which bleeds a little across where surfaces meet. A composite after shading
would have the shading's normal and roughness, but needs them, and the specular light's weight, in
images that every pipeline drawing into the view, games' among them, would draw; rays sampled from
GGX by roughness (Stachowiak, "Stochastic Screen-Space Reflections", 2015) need the roughness before
the trace, and a resolve to denoise them. The kept colors are reprojected by the camera alone: what
moves shows in a reflection where it was a frame before. The trace scales them by this frame's
exposure over theirs, so that what they reflect is of this frame's light as an adapting exposure
changes.

A view that its environment antialiases, as by default, is antialiased temporally: each frame draws
it a fraction of a pixel off, by the next of eight offsets, the Halton sequence of bases 2 and 3,
everything in it but the canvas, culled as it's drawn; and a compute pass then resolves each pixel
of its colors, before they're tonemapped, against what the frame before resolved where the pixel's
point showed then, clipped to what this frame drew round it, so that over the frames an edge
averages what's drawn across its pixels, and what's uncovered or lit otherwise doesn't linger.
Beside its IDs, each frame draws how far what's opaque in the view moved in the world since the
frame before, its motion, every frame, antialiased or not, as the IDs are: the pipelines that write
depth write it, as the motion belongs to what owns the depth, and what blends takes that of what's
behind it. A model's motion is by its instance's transform the frame before: the scene's records
keep, beside each transform, the one the first change of the frame it changed in found, which the
gather takes in that frame alone, and the culling keeps it beside the instances the view's passes
keep, which their draws reach `previous` transforms on, as their push constants say, and the
visibility buffer's shading likewise. A game's shader returns its own if it likes, as
`Shaders::add` says; the engine has one that writes depth and returns none return zeros, which a
layer's own draws, made each frame, whose transforms the frame before it doesn't know, take too, so
that what they draw moves with the camera alone. The resolve finds each pixel's point from the
view's depth, and where it was, less its motion, as the frame before's camera saw it, from the
nearest of the 3 by 3 pixels round it, so that an edge that moves over what's behind it takes its
own motion; samples the history there by a Catmull-Rom filter; clips it to a box of this frame's
pixels round it, their mean give or take their deviation within their least and most, in YCoCg,
weighing colors by 1 over 1 plus their luminance, so that a lone bright pixel doesn't flicker; and
blends this frame's in, the 3 by 3 pixels round it each weighed by how far from the pixel's center
the frame's offset drew it, a Gaussian of a deviation of 0.35 pixels: a 25th of it where the pixel
stays where it was, so that a still view settles rather than following each frame's offset by the
newest frame's share, up to a tenth where it moved a pixel or more, and a share equal to each
frame's that the history holds while those are fewer; the history first scaled by this frame's
exposure over the frame before's, so that a change of exposure, as an adapting one's, leaves no ghost where the clipping
lets much of the history through. The clip throws out what's thinner than a pixel, or cut out by a
mask near its cutoff, at each offset that misses it, so that in a still view such a pixel would
show each offset's own color in turn; so each pixel keeps, in two more images of the view's size,
the luminance each of the eight offsets drew of it the last time it came round, and whether that
matched the time before, which the resolve reads and writes in place. A pixel that stood still while
every offset drew it as it had a cycle before is changing only as the offsets move what's drawn: its
history isn't clipped, its luminance settles on the mean of the eight, by a still pixel's share of
the way a frame, not at once, from where the clip left it, and its color blends in a hundredth
of this frame's. Anything else that changes it, something passing over it, a shadow too, or light
that changes, breaks the match for two cycles, which the clip resolves as it otherwise does, so
that it leaves no trail; and a pixel that moves keeps none. FSR 2's luma instability and Unreal's
TSR keep the history unclipped where a pixel's luminance flickers; this asks the same luminance at
the same offset, so that no change but the offsets' is kept. A view that moves has no cycle to
match, and there what's thinner than a pixel shows as dashes, where each frame's pixels land on it,
which the clip throws out the frame after. So the resolve tries, beside its own, one of each
pixel's luminance alone, kept in two more images that it writes and reads in turn, as it does the
history's, taken where the history is: clamped to this frame's pixels round it, then moved a
quarter of the way to this frame's. A pixel whose clamp keeps taking back what the frames' own
colors put in flickers: it keeps its history's luminance past the pixels round it, on the side the
clamp pushed it from and as far as the frames drew, and takes a 33rd of each frame, so that it
shows the mean of what the frames draw, as TSR does where luminance flickers. A clamp that pushes
otherwise, as where something arrives, leaves or is lit otherwise, ends that, and so does a pixel
that stood still and was drawn by an offset otherwise than a cycle before: what crosses such
pixels leaves no more of a trail than it did, though they flicker behind it for some tens of
frames, and what blinks before a still view shows as it's drawn; what's drawn and gone again
faster than every 20 frames, while it or the view moves, shows its mean. These four images are 32
bytes a pixel, twice the two of the history. Its history is one of two images of the view's size,
each frame resolving into the one the frame before didn't, against the other, which a frame at
another size,
or after one that resolved no view, starts afresh. The canvas draws after the resolve, in a pass of
its own over the view's resolved colors, which the resolve then writes apart from its history, so
that a game's UI stays sharp and out of the history. The view's IDs are drawn off by the frame's
offset too, so that a pick at an edge, and the editor's outline of the pick, can move a pixel from
frame to frame.

A view that its environment antialiases by SMAA is antialiased from each frame alone, without
jitter, a history or motion, in the resolve's place, before bloom, the grade and the canvas: SMAA
1x (Jimenez, Echevarria, Sousa and Gutierrez, 2012), as the reference's shader computes it at its
HIGH preset, threshold 0.1, 16 search steps across and down, 8 along diagonals, corner rounding
25, in three compute passes, each writing every pixel, rather than the reference's draws behind a
stencil that keeps the second to the pixels with edges. The first finds each pixel's edges with its
neighbors to the left and above, with local contrast adaptation; the second measures the line of
edges each pixel with one is on, along diagonals first, by bilinear fetches that read two pixels'
edges at once and a search texture that says how far the last went, and takes its length and the
edges crossing its ends through an area texture to the share of the pixel and of its neighbor
across the edge that the edge the line stands for cuts off; the third blends each pixel with its
neighbors by those shares, into an image of the graph's own, which bloom, the grade and the canvas
then go over. The view's colors are of high dynamic range, and the reference's as a display shows
them, so SMAA sees them about as the grade and the tonemapper that follow it will show them: times
the grade's slope, each channel's, and rolled off toward 1 above a shoulder by the curve of Khronos
PBR Neutral's, invertibly, its toe and desaturation left out. The shoulder is PBR Neutral's, at
0.76, under it and under ACES and AgX, which roll light off sooner and more gently; and under no
tonemapper, which clamps each channel at 1, the last 256th below 1, all but the clamp. The edges
are found on the luma of the colors so shown and encoded by sRGB's curve, which the reference's
threshold is for, and the colors so shown are blended, linear as the reference blends, and taken
back through how far each channel is below 1, which keeps light's precision up to a half float's
largest, so that an edge below the shoulder blends exactly as it would after the grade's slope and
the tonemapper, and a pixel half covered by a bright sky shows half of it, where blended in the
view's light it would show as bright as the sky. Weighing colors by 1 over 1 plus their luminance,
as the resolve does, would darken every edge's blend and compress the contrast of bright ones
under the threshold. The lookup textures are made as the renderer first antialiases a
view by SMAA, by the algorithm the reference's generator scripts describe, for subsample offset 0
alone: the area texture's 160 by 80 texels of each pattern's areas, from the revectorized line's
exact coverage of each pixel, and the search texture's 64 by 16, and written in that frame.

An environment's exposure may adapt to what the view sees, as eyes adapt between sunlight and
shade, by its adaptation, none by default. Each frame meters the view's light after antialiasing,
temporal or SMAA, and after its colors are kept for reflections, and before depth of field, motion
blur, bloom, the lens and the canvas, as Unreal's histogram exposure does, in three compute passes: one empties a histogram of 256 bins, eighths of a stop of luminance from 2^-12
candela per square meter to 2^20; one counts each pixel into it, its color taken back through the
exposure the frame shaded at to the luminance lit shading exposed, a workgroup's own bins first and
then the histogram's by an atomic add each, what's darker than the lowest bin, as a black sky,
left out; and one ranks the pixels and takes the mean log2 of their luminance between the
adaptation's two percentiles, a bin a percentile cuts counting for its share inside. The frame two
later reads the mean back as it begins, as it reads the culling's counts, so that no frame waits
for it, and the exposure a frame shades at is the frame before's moved toward what a
reflected-light meter of ISO 100 calibrated to K = 12.5 calls for, log2(luminance × 100 / 12.5),
less the adaptation's compensation and kept between its bounds, by at most its speed up or down in
stops a second over how far the time the view shows moved on, the simulation's, so that the same
frames show the same and a paused game's exposure holds. The frames before the first metering
comes back shade at the environment's exposure, within the bounds, and the first metering is
taken at once rather than moved toward. What adapts is the exposure the meter calls for, before
the compensation and from the mean last metered, which the bounds, the compensation and the speeds
are applied to in each frame, so that a change to them, as a script easing the compensation each
tick makes, shows at once and leaves the adapting going; only a change to the percentiles, which
the meter's passes take, leaves out what frames before it metered, until a frame metered by them
comes back, which is taken at once. The tool API's look adapts apart from the game's own view,
each metering into a buffer of its own, so that a look round a dark room exposes neither the game's
view nor a capture of it, a bare frame, for the room. Kept on the CPU, the exposure reaches every
reader as a fixed one does, lit shading, the sky, the fog and games' shaders through the frame's
data, so that what isn't lit still shows its colors at any exposure; antialiasing's history and
the colors reflections trace against, kept from the frame before, are scaled by the exposure's
change. But the meter can't tell what isn't lit from the view's pixels, and meters it as though it
were, as DEFECTS.md says.

A view that its environment blooms, as by default, spreads the light of what's bright round it, as
a camera's lens does: after antialiasing, temporal or SMAA, or after its last pass without, and
before the canvas, so that a game's UI doesn't bloom, by a chain of compute passes, as Call of
Duty: Advanced Warfare's (Jimenez, "Next Generation Post Processing in Call of Duty: Advanced
Warfare", 2014). Each of the chain's levels, images of the graph's own, is half the one before each
way, rounded up, from half the view down to between 16 and 32 texels on its shorter side, five at
1,280 by 720 and at most eight; each is downsampled from the one before by 13 bilinear taps over the
6 by 6 texels round the 2 by 2 under each texel, as five boxes of four, the middle one a half and
those at the corners an eighth each, so that what's small and bright doesn't shimmer as it moves
across texels; the first from the view's colors themselves, a texel at a time, taken as light as
the tonemap takes them, each box weighed by 1 over 1 plus its luminance, Karis's average, so that a
lone pixel far brighter than the rest doesn't bloom into a blot. Then, from the smallest up, each
level lerps 0.7 of the way toward the one below it, taken by a 3 by 3 tent of bilinear taps a texel
of that level apart, so that each holds a weighted mean of itself and every level below, the weights
adding up to 1; and the view's colors lerp toward the first level, by the same tent, by the
environment's bloom, 0.04 by default, so that bloom spreads light without adding any, and a view of
one color blooms to itself. Light blooms from everything, without a threshold, as in a lens, the
lerp leaving what's no brighter than what's round it as it was. That last pass grades the view's
colors too, as below, and runs to grade them alone in a view that doesn't bloom. Antialiased
temporally, the resolve then writes the view's resolved colors apart from its history, as it does
for a canvas, and that pass writes into those, as it does into SMAA's; without antialiasing, the
canvas draws in a pass of its own after it.
A view under 32 pixels on a side doesn't bloom.

An environment's grade changes the view's light in the view's linear sRGB, after bloom and before
the canvas, so that a game's UI isn't graded, and before the tonemapper maps it, as Unreal's and
HDRP's do: white balance, a von Kries adaptation in CAT02's LMS to D65's white from that of daylight
of a color temperature, along CIE's daylight locus, 4,000 K at -1, and tinted off it toward green or
magenta; contrast, each channel raised to a power round mid gray, 0.18; ASC CDL's slope, offset and
power, each channel's; and saturation, by Rec. 709's luminance, as ASC CDL's, each step's light
kept within a half float's largest, so that no grade makes a color infinite or no number. Grading
before the tonemapper keeps the tonemapper the last to map light into the range the view shows, so
that what a grade brightens or saturates rolls off into its shoulder rather than clipping, and a
warmer balance warms what's lit as a camera's would, before the tonemapper whitens what's brightest.
The pass that blooms the view computes the grade at each pixel, exactly, from the grade's numbers
among its push constants, its white balance a matrix the CPU makes; the identity, the default,
isn't computed at all. A view tonemapped by any tonemapper but none is dithered as it's mapped: triangular noise, of two
uniform noises of up to half a step either way of the view's 8 bits, from a hash of the pixel, the
same every frame, so that frames of the same colors write the same, is added to the color encoded
as sRGB, as the view's image encodes it, which is decoded again for the image to encode as it's
written, breaking the bands a smooth gradient would show while keeping its mean; within a step of
black or white the noise shrinks to reach no further, so that they stay as they are.

An environment's depth of field, motion blur and lens are the camera's, each off by default and
costing nothing then, after the view is antialiased, temporally or by SMAA, its colors kept for
reflections and metered for the exposure, and before the canvas, so that a game's UI is neither
blurred nor seen through the lens: depth of field, then motion blur, before bloom, as Unreal orders
them, and the lens after the grade. Depth of field is a thin lens's, of the focal
length the view's field of view takes on a full-frame camera's sensor, 24 mm high, and of a focus
distance and an f-number, in the world's units taken as meters, so that a wider aperture, a nearer
focus or a narrower field of view blurs more, as a camera's do; under the view's reversed infinite
perspective each pixel's circle of confusion is linear in its depth, and without perspective there's
no lens and none of it runs. A circle's radius is at most 2% of the view's height. As
Call of Duty: Advanced Warfare's (Jimenez, "Next Generation Post Processing in Call of Duty: Advanced
Warfare", 2014), the view is taken to half its size, each texel the mean of its 2 by 2 pixels and the
nearest one's circle, so that what's thin and near isn't lost; tiles of 8 texels find their largest
circle before the focus, dilated over the tiles it reaches, since what's near spreads past its own
edges; then a gather from rings of texels, as many as keep them 1.25 texels apart, up to five, round
each texel as far as the largest circle that can reach it, splits it into a near field, what's before
the focus scattered toward it by each circle that covers it, each weighed by 1 over its circle's area,
premultiplied by how much of the texel it covers, and a far field, what's at or past the focus, each
circle kept within the texel's own, so that what's behind doesn't spread over what's nearer and
sharper; a texel of the near field takes as its far field what's round it that isn't, what its blurred
edge lets show. Each of the view's pixels then lerps toward the far field by its own circle, from 1
pixel to 2, so that what's in focus keeps the view's own resolution, and takes the near field over
that. Motion blur is McGuire, Hennessy, Bukowski and Osman's reconstruction filter ("A Reconstruction
Filter for Plausible Motion Blur", 2012): each pixel's velocity found from the view's depth and motion
as temporal antialiasing finds it, against the camera of the frame before that blurred the view, as
though the camera stood still after one that didn't, or where the view is cut to its viewpoint
from elsewhere, half of it times the shutter's share of the frame, a half by default, how far the
pixel moves either way while it's open, within a tile's side,
a 36th of the view's height, 20 pixels at 720; each tile's largest velocity and their neighbors'
largest; and 15 samples along that either way, jittered by noise of the pixel's that stays from frame
to frame, weighed by whether each is in front of the pixel or behind it, within a hundredth of the
nearer's depth, and how far each moves. A headless frame draws a tick, so its motion is a tick's.
The engine says a frame is cut to its viewpoint where it comes from elsewhere than the frame
before's, rather than as a camera moves: a tool's look set, changed or cleared, a bare frame among
the look's and the look's frame after it, the primary camera switched, or the projection turned to
the other kind, which would otherwise blur every pixel by the jump, a capture of the look's first
frame among them. A game's teleport of its camera isn't, as nothing says it's one. The
lens is a compute pass of its own, into colors of its own, as its chromatic aberration reads the
pixels round each: red spread out from the view's middle and blue drawn in, by a hundredth of the way
at 1; a vignette by the cos⁴ law of a lens's light off its axis, as Unreal's natural vignetting, the
view's corners at an angle whose tangent is the vignette's; and grain, each pixel's light scaled by
triangular noise of a hash of the pixel and the frame's number, alike in every channel, so that the
same frames of a run take the same noise on any GPU, kept within a half float's largest, as bloom
and the grade keep theirs. It runs before the canvas, rather than in the tonemap pass, which samples
the view with its canvas drawn over it. Antialiased, the resolve writes its colors apart from its
history only where what follows writes them in place: depth of field, or bloom, the grade or the
canvas without motion blur or the lens, which write colors of their own, before them.

A skinned model's primitive, of glTF's JOINTS_0 and WEIGHTS_0, packs as any model's mesh does, its
vertices as they were bound, with each vertex's four joints and their weights after them, as
16-bit indices and fractions, on a grid that holds where its joints carry it: the box its vertices
reach as each clip poses the model, from rest, at each of its keys' times, or at 64 evenly, grown by
a tenth each way, which views cull it by too; a vertex carried past it clamps to it. Each entity of
a skinned part draws a mesh of its own, a copy of its model's that the engine adds as it first draws
it and removes as it goes, so that each root poses apart; the scene draws the entity by it. Each
frame hands the renderer the copy's joints' matrices, each from the mesh's space as it was bound to
its own as the joint draws, between ticks too, so that a pose moves as smoothly as anything drawn;
and a compute pass before the culling and the shadows, on the graphics queue, moves each copy's
vertices by them into a part of a buffer of the graph's own, its header first, which every pass that
draws reads, the frame's draws of the copy reading its vertices there rather than from its own
buffer. A skinned mesh isn't cut into meshlets, whose bounds its joints would leave behind, so it
draws by its indices, outside the visibility buffer; its motion is its instance's alone.

An error while rendering ends the renderer, since a failed frame can leave meshes and textures
half copied to the GPU. A game's shader compiled again whose new pipeline can't be made doesn't:
it warns, and the old pipeline goes on drawing.

## Audio

Sounds and music are kira's, mixed on cpal's audio thread, as the simulation's state never is: what
plays follows from the world, and never feeds it. A layer loads a sound from a WAV, Ogg Vorbis, FLAC
or MP3 file, which symphonia decodes whole into memory, as a game's short sounds are, through a
decoder of the engine's that refuses one as it passes 2²⁴ frames, some 6 minutes at 48 kHz, where
kira's would hold all a crafted file decodes to. A tick queues
a sound with `World::play_sound`, from nowhere, at a point or on an entity, in a resource of the
world that the engine empties once each tick has run, playing what it held, so that no snapshot,
hash, recording or scene holds one, and each lockstep peer plays what its own ticks queue. Layers
play their UI's sounds as they update, with `Context::play_sound`.

Music streams: a layer loads it from the same formats, read whole into memory as compressed as its
file is and decoded as it plays, on a thread of kira's for each play, so that a long piece takes
little memory and a play never reads the disk, whether the file is on it or in a game's package.
Layers play one at a time, with `Context::play_music`, looping from its start, the music
before it fading out as it fades in. Playing the music playing goes on as it was, so that a layer
can say each frame which plays; ticks play none, since a layer that reads the world as it updates
plays what the world calls for. The decoder is the engine's, over symphonia, as kira's leaves the
codec as it was across a seek, so that a loop's start would run on from the packets before it.

What plays mixes through three buses: the music's, the sounds', and the main one over both, whose
volumes layers set with `Context::set_volume`, gliding there over a fade, as a game's settings
would, and as a menu ducks the music under it.

A sound at a point or on an entity is panned and faded as the game's view, the listener, hears it,
as the frame handed off sees it. A view without perspective, as a 2D game's, hears it by where it
shows in the view: toward the side of the view it's on, halfway there at the view's edge, as kira
pans at constant power, all of it in one ear at a side, and as loud as it is inside the view,
fading out to silence a view's width or height past its edges. A camera with perspective hears it
by its direction and distance, before or behind it, as kira's spatial tracks would with their
defaults: three quarters of the way toward the side it's straight out to, and as loud as it is
within a unit, fading linearly in decibels to silence 100 units away. The engine pans and fades
each play itself, as a frame goes, rather than giving each place a spatial track of kira's, so that
a sound starts and follows its entity without a track made or dropped, allocating nothing. One on
an entity follows it, each frame, to where the latest
tick left it, up to a tick ahead of where it draws, too little to hear; once the entity is
despawned, it plays to its end from where the entity was last, as an impact's does when what made
it is gone.

Windowed and embedded runs play sound, the editor's game among them, opening the system's default
output device as their first sound or music loads, so that a game without either opens none;
headless runs, replays and tests play none. What plays follows how the run plays, as the tool API
sets it: paused, the music's and the sounds' tracks hold what's on them, and no sound starts;
stopped, as `--stopped` starts a run, what plays stops and nothing starts, the music the layers last
had play waiting for the run to play, which starts it from its start, as a game's load did; so the
editor's Stop silences the game, and each Play starts its music again. In development runs, music
reloads as its file changes, its play crossfading to what the file holds now from where it was, so
that an edit is heard at once, as a shader's is seen.

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

A windowed run's frames go as fast as presentation lets them: mailbox's, as fast as the GPU
renders, or with `--vsync` FIFO's, at the display's rate. `Context::set_max_fps` caps them, as
`--max-fps` does as the run starts and, in development builds, the frame rate limit among the title
bar's settings, which the app keeps for the user: the main thread sleeps before each frame until
it's due, a share of a second after the last was due, as Unreal's `t.MaxFPS` and Godot's `max_fps`
do, rather than waiting in the event loop, which Windows doesn't run while the user moves or resizes
the window. The Performance panel and `engine.perf` count the sleep apart (`limit`). An embedded run
renders a frame once its tool has taken the last, or 100 ms after, so the editor capped caps its
game too.

A run decodes images on threads of its own, `gg-load`, which the main thread hands each texture's
bytes as its handle goes out and takes the pixels back from as each frame starts, so that neither a
layer's add nor a tick that names an asset waits for a decode. There are half as many as the
hardware runs at once, leaving the rest to the main and render threads and the game's, and 8 at
most, as each holds the image it's on whole, its levels among it. They start with the first image,
and each ends once the run drops them, after the decode it's on; a panic there resumes on the main
thread, and threads that can't start leave the main thread to decode, warning (`engine.loader`).

## Platforms

- **Vulkan 1.4, bleeding edge.** The engine requires a Vulkan 1.4 device, with the descriptor
  indexing bindless textures need, depth in 32-bit floats, sampled by comparison too, depth clamp,
  which shadow maps need, and BC texture compression, which games' cooked textures are in, none of
  which Vulkan requires but every target has, and takes up
  newer features as drivers ship them, with a fallback wherever a target lacks one. Phones, which
  have ASTC rather than BC, would take a cook of their own.
- **Windows and Linux:** native Vulkan drivers.
- **macOS: KosmicKrisp,** LunarG's Vulkan 1.4 driver on Metal. It needs Apple Silicon and macOS 26
  or later. Not MoltenVK, which Khronos deprecated. KosmicKrisp has no mesh shaders or ray tracing,
  so those stay optional paths. macOS has no Vulkan loader of its own, so a shipped game's bundle
  holds the SDK's, with KosmicKrisp, as [Builds](#builds) says.
- **x86-64 and ARM64:** the engine builds and passes its tests on both from the start, rather
  than being ported later; macOS is ARM64 only. An ARM64 device still needs a Vulkan 1.4
  driver.
- **No GPU:** lavapipe, Mesa's CPU Vulkan driver, runs the GPU tests headless.

## Technology

| Area | Choice |
|---|---|
| Language | Rust stable, edition 2024 |
| Game code | Rust; Luau through `mlua`, in every build, for mods and quick scripts |
| Hot patching | `subsecond`, its patches built by Dioxus's `dx` of the same release, which `gg run` checks |
| GPU API | Vulkan through `ash`: one backend, no multi-API abstraction layer |
| GPU memory | VMA through our own binding in `gg-gpu`, for its budgets, aliasing, pools and defragmentation |
| Window and input | `winit`, with `ash-window` for surfaces and `gilrs` for gamepads |
| UI | egui, drawn by `gg-renderer`, with a dock of our own, in development builds: the engine's overlay and the editor alike; its text in Inter |
| File dialogs | The OS's own, through `rfd`, in development builds |
| Project state | RON in a canonical form; see [Project state](#project-state) |
| Math | `glam`, with its `scalar-math` and `libm` features; `libm` |
| Jobs | `rayon-core`'s thread pool and `join`; not rayon's parallel iterators, whose splits and reductions follow the number of threads |
| ECS | Our own; see [ECS](#ecs) |
| Physics | `rapier2d` and `rapier3d`, with `enhanced-determinism`, for 2D and 3D, their state resources of the world; see [Physics](#physics) |
| Navigation | Our own; see [Navigation](#navigation) |
| Logs and profiling | `tracing` for logs; Tracy, through `tracy-client`, for CPU zones and GPU timestamps |
| Frame captures | RenderDoc, through its in-application API, which `gg-gpu` finds in the process with `libloading` |
| Agent access | MCP and a command-line tool, both clients of the [tool API](#agents-and-tools) |
| Shaders | Slang, compiled to SPIR-V by the Vulkan SDK's `slangc` rather than linked in: the engine's own as it builds, games' as development builds add them and as their files change, and by the cooker into a game's package, their SPIR-V checked against the shader contract |
| Assets | A game's package, a file of the engine's own format that its shipping build reads at offsets; glTF 2.0 in, read by the engine over `serde_json`, MikkTSpace's tangents made for it through `bevy_mikktspace`; PNGs and JPEGs that `png` and `zune-jpeg` decode as development runs add them, and Radiance HDRs that `image`'s decoder of them decodes, to 16-bit floats through `half`; textures cooked in KTX2, its levels supercompressed by zstd, through `ctt`: colors and maps' values to BC7 by bc7enc_rdo, perceptual for colors alone, and normal maps to BC5 and colors of high dynamic range to BC6H by Intel's ISPC Texture Compressor; read by runs through `ktx2` and `zstd`; meshoptimizer, through `meshopt`, for the cooker's meshes and their levels of detail, and for meshlets |
| Text | TrueType and OpenType fonts, whose outlines `skrifa` reads and `fdsm` bakes into multi-channel signed distance fields, as development builds load fonts and as the cooker cooks them; text shaped by `harfrust`; `skrifa` and `harfrust` at the releases egui's own text takes |
| Audio | `kira` on `cpal`, its sounds and music decoded by `symphonia` through decoders of the engine's: tracks as buses, panning and streaming, without allocating on the audio thread |
| Networking | Our own, over std's TCP and UDP sockets; a server's states compressed by `zstd` against the latest a client has; Ctrl+C taken through the OS's own handler, by `windows-sys` and `libc` |
| License | `MIT OR Apache-2.0`, at the user's choice, as most of Rust's crates are, [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE), contributions under the same terms; a file beside a license of its own keeps it, as the fonts' OFL and the fox's CC BY 4.0. A game links the engine into its own executable, so a copyleft license, as the GPL's, would bind every game made with it, those for consoles too, whose SDKs can't be opened |

Areas not listed here are undecided.
