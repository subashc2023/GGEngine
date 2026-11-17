# Architecture

This file is GGEngine's design: how it fits together, and why. [README.md](README.md) introduces
the engine, [AGENTS.md](AGENTS.md) holds the rules the code follows, and
[docs/PLAN.md](docs/PLAN.md) the work, unbuilt parts included. Reference material (limits, codes,
formats, algorithms) lives in the code docs beside its code. A change to the design updates this
file in the same commit.

## Goals

- **v0.1: change a running game by asking.** The user tells a coding agent, Claude Code on their
  own subscription, what to change. It edits the game's code or works through the
  [tool API](#agents-and-tools), and the running game picks up each change with its state intact.
- **People and agents build together.** Many work on one project at once: project state is text
  that diffs and merges, every authoring action is an operation an agent can perform, and every
  result is machine-readable.
- **Any kind of game, at any scale, single-player or multiplayer.** Game code is native Rust, and
  the engine supplies what each multiplayer model needs.
- **Grown with games.** Small demos first, each exercising part of the engine; the v0.1 loop is
  tested on the first, a simple networked game. A flagship picked from them then sets the engine's
  priorities, and shipping it proves the engine.
- Rendering as current AAA engines do: bindless resources, a render graph, GPU-driven rendering.
- Simulation and rendering pipelined across threads.
- An offline asset pipeline that cooks content for a shipping player.
- Windows and Linux on x86-64 and ARM64, and macOS on Apple Silicon. Consoles, mobile and the web
  wait until the desktop engine is solid.

## Crates

```text
sandbox  editor   player       apps/: executables          gg  tools/: tools
       \    |    /                                         |
        gg-engine - gg-macros  crates/: libraries          |
            |  \  \_______________________________________ gg-api
            |   \_____________ gg-relay - relay
       gg-renderer
            |
          gg-gpu
            |
         gg-core               every crate may use gg-core
```

- **`gg-core`:** math, logging, profiling and error types.
- **`gg-gpu`:** safe types over `ash`, the render graph among them. Safe code can't break the CPU
  side: each type checks its inputs came from its own device, and a leaked frame or pass fails the
  next call. GPU-side inputs (SPIR-V, addresses, texture indices) are trusted, and a bad one can
  fault the device, so gg-renderer validates games' shaders and makes every address a draw reads.
- **`gg-renderer`:** the frame's passes, materials, lighting and post-processing. It owns the GPU
  resources and hands out handles to them.
- **`gg-engine`:** what apps and games build on ([its root](crates/gg-engine/src/lib.rs)), with hot
  reload and the tool API in development builds.
- **`gg-macros`:** the `Reflect` and `Component` derives, a crate of their own as Rust requires.
  Games use them through `gg-engine`'s re-exports.
- **`gg-api`:** the [tool API](#agents-and-tools)'s protocol and client, and the text of projects'
  manifests and assets' ID files ([Project state](#project-state)). It depends on no `gg-*` crate,
  so tools build without the engine. `gg-engine`'s code uses it in development builds alone, so
  only they read a manifest or an ID file: shipping builds start from cooked data. Its build script
  uses it in every build, to check Luau's C++ flags.
- **`gg-relay`:** the relay that a session's runs meet through when they can't connect directly
  ([Networking](#networking)): both ends of its protocol, and the relay the `relay` app runs, with
  a systemd unit for a server ([gg-relay.service](apps/relay/gg-relay.service)). Of the `gg-*`
  crates it uses only `gg-core`, so a relay runs on a server without Vulkan, sound or input
  libraries.
- **`sandbox`:** the engine's development app and benchmark: 100,000 sprites and a scene file.
- **`editor`:** one more client of the tool API, through `gg-api`, with no world of its own. It runs
  the project's game, wherever the project is, as an agent does (`gg run --embedded`, by the gg of
  the engine the editor is built from), stopped at the world as edited, and
  shows the game's view in a panel of its dock. Its panels and handles edit the game through the
  API. Play and Simulate are the API's `play`, and Stop its `stop`: a game's ticks are all its own
  code, so Simulate is Play seen through the editor's camera, with no input to the game.
  [game.rs](apps/editor/src/game.rs) describes each panel, menu and control.
- **`player`:** the reference player, which runs a game's package without the game's code.
- **`tools/gg`:** a client of the tool API that also runs, ships and fetches games: `cargo gg` in
  the engine's workspace, by its alias, and an installed `gg` in a game's own. It isn't installed
  as `cargo-gg`, which the alias would shadow.
- **`tools/cook`:** `gg-cook`, the offline cooker of a game's [package](#project-state). It reuses
  `gg-engine`'s code to make each asset, so a package holds what a run would make.

Games build on `gg-engine` as apps do ([Game code](#game-code)). The v0.1 loop is tested on
Shards. The showcase has a scene for each feature and grows with the engine.

### Layering rules

1. **Dependencies point down only.** Each crate depends on the crate below it, even before it uses
   it, so reaching up closes a cycle, which Cargo rejects. Cargo allows cycles through
   dev-dependencies: don't reach up through them.
2. **`gg-core` is the leaf:** no `gg-*` dependencies, and nothing GPU- or window-specific.
3. **`gg-gpu` owns Vulkan** and knows nothing of meshes, materials or scenes. So the unsafe surface
   stays small and reviewable, and rendering techniques change quickly on safe types.
4. **`gg-renderer` never sees the scene.** The engine hands it plain [render data](#rendering) each
   frame. So rendering runs on its own thread, a frame behind simulation ([Threads](#threads)), and
   renderer tests run without the engine.
5. **The engine reaches the GPU only through `gg-renderer`.**
6. **`main` is thin.** It describes the app or game and hands control to `gg-engine` (the relay's,
   to `gg-relay`), so there is no entry-point crate.

Cargo's cycle check enforces rule 1, and rule 2 but for `gg-macros` and `gg-api`, which depend on
no `gg-*` crate. Nor does it see skip-level edges, such as `gg-engine` → `gg-gpu`.
[deny.toml](deny.toml) bans those, `gg-macros` outside `gg-engine`, and a library's use outside the
crate that owns it. Review checks rule 4, that `gg-core` doesn't use `gg-api`, and how a crate uses
its dependencies.

### Builds

`gg-engine`'s development tooling, the tool API and debug UI among it, is behind its `dev` feature
([its Cargo.toml](crates/gg-engine/Cargo.toml)). Development builds turn it on. Shipping builds
never do, and run cooked data only. Cargo merges the features of every package a command selects,
so `--workspace`, a bare `cargo build` or `--bin gg-player` would hand the player `dev`: a shipping
build selects the player or one game alone. A game's own `dev`, on by default, turns on
gg-engine's, so `--no-default-features` builds it as it ships.

Without `dev`, `Ui` has no values, so the engine never calls `Layer::ui` or runs egui, and game
code, its `ui` included, builds unchanged. egui stays a dependency for its types alone, and the
linker keeps none of its code. Games draw their own UI on the [canvas](#events-and-layers).

Two gates build without `dev` ([AGENTS.md](AGENTS.md#gates)). The shipping gate builds
`gg-engine` as the player and games do, and the games' tests, which run with `dev` as they read
their projects. `gg-engine`'s own tests get a gate apart, as their dev-dependencies' features would
merge into the player's build.

Shipping builds use the `shipping` profile of [.cargo/config.toml](.cargo/config.toml), which holds
every setting a game's build takes from the engine. `cargo gg ship` cooks the package with the
cooker of the engine the game builds against, and lays both out under the game's name
([ship.rs](tools/gg/src/ship.rs)). The folder holds what a shipped build owes: `NOTICES.txt`, the
project's `CREDITS.txt` and every crate's license, build dependencies included, as Luau's C++ is
compiled in through one, which cargo-about writes, refusing a license `about.toml` doesn't accept;
on Windows, the Visual C++ runtime, which Windows lacks. The build's symbols go beside the folder
([Diagnostics](#diagnostics)). ship.yml builds Linux's in Valve's Steam Runtime 3 SDK image, so
they run on glibc 2.31 and later. Tracy is a feature, `tracy`, off by default and
in no shipping build. Vulkan validation is a runtime switch instead, on by default in debug builds,
so tests run with it whatever the features.

The docs are a gate: rustdoc's warnings, private items included. Doc comments are most of what
people and agents read of the code, so a link that breaks, or comes to name two items, fails the
change that broke it.

### Growth

A module becomes its own crate once it has its own responsibility and a second user, not before.
The package's formats stay in `gg-engine` although the cooker uses them: the cooker builds with the
engine anyway, baking fonts and compiling shaders through its hidden `cook` module.

## Game code

Game code is Rust, as fast and unrestricted as the engine's own. Each game is a crate on
`gg-engine` that builds its own player, statically linked and optimized for shipping, and a
[project](#project-state), which `gg new` makes. Game crates deny `unsafe`, so a bug in game code
panics rather than corrupting memory. The engine's demos and test beds are workspace members in
`games/`, so one change can span engine and game; the engine's code and tests depend on none of
them, as CI's layering check confirms. A game made for players, as the flagship, is a game of its own.

- **A game of its own is a workspace of its own**, which `gg new <NAME> --engine <ENGINE>` makes
  outside the engine's. It depends on the engine by a checkout's path, or by git at a revision its
  `Cargo.lock` names, so the engine's commit reaches the digest its sessions name its build by
  ([Networking](#networking)). A `[patch]` would reach the build but not the digest, as `cargo
  metadata --no-deps` lists no patches, so a game is developed against a checkout by its path.
  `gg new` copies the engine's settings: `.cargo/config.toml` but its alias, `clippy.toml`,
  `rust-toolchain.toml`, `about.toml`, `.gitattributes`, and `Cargo.lock`, so the crates beneath
  the engine resolve as its tests ran them. `gg check` warns where the clippy.toml or the
  toolchain's pin isn't the engine's (`gg.settings`). Its `gg` is installed from the engine it
  builds against, and warns of a game that speaks another revision of the tool API.

- **The game runs in its own process.** The tools launch its development build and work on it
  through the [tool API](#agents-and-tools). A crash, panic or hang stops only the game, and the
  tools restart it from its last snapshot. The same split gives dedicated servers, local
  multiplayer tests with a server and several clients, and headless runs in which agents check
  their work. The editor shows its game in its view; run alone, a game has its own window.
- **Hot reload keeps the world.** `gg run` builds the game with Dioxus's `dx` and watches its
  sources. An edit inside function bodies of the game's crate becomes a `subsecond` patch, applied
  between ticks. A patch reaches calls made through `HotFn`, through which a development build
  calls each layer method, so it reaches the systems and scripts they call; what only the game's
  own trait object or function pointer reaches waits for a rebuild. subsecond patches only the
  binary's crate, so a game's crate is a binary with no library, and its statics start over at each
  patch, so a game keeps its state in the world. Any other change rebuilds and restarts the game
  from a snapshot taken just before, matching changed components by field name: a new field (even
  of a type declared in a function body) or component, a constant, an engine edit, or an edit to
  `main`, `load` or `restored`, which ran before any patch could. An edit inside `attach` is
  patched, but restarts start from snapshots, so only a fresh run shows it (`gg.attach_changed`).
  Without `dx`, every change rebuilds and restarts the game.
- **Luau, in every build, for mods and quick scripts.** Players install sandboxed mods, and quests,
  dialogue and level events reload in milliseconds. A script file declares one component, its
  fields and a tick per entity, as a native `Script` does; adding the component attaches the script.
  Scripts reach Rust components through reflection, without hand-written bindings. What they can use
  is in [docs/LUAU.md](docs/LUAU.md), their rules on [`Scripts`](crates/gg-engine/src/luau/mod.rs).
  The rules follow from the simulation's. A tick changes the world at once, drawing IDs and slots
  from the world, so a spawn gives the same entity on every peer, in a replay and in rollback. A
  chunk runs again as a lockstep joiner or a restored run loads it, so it changes nothing
  (`script.loading`) and keeps no state (`script.state`): a script's state is its components.
  Scripts run alike everywhere: libm's math, garbage collected at set points rather than by the
  clock, and the same bytecode in every build, Luau's full debug information in each. A budget of
  interrupts per call fails a loop that never ends alike on every peer, rather than hanging the
  game, which `gg run` would restart into. Ticks don't allocate. Luau doesn't guard against crafted
  bytecode, so mods ship as source, and only a game's own packages hold bytecode.
- **Scripts are debugged where they run**, through the tool API's `debug`, with Luau's own hooks:
  breakpoints, a pause, steps. A stop holds the main thread inside the tick, so the frames and the
  main thread's requests wait while the connections' threads answer the debugger; `ping` says so,
  and `gg run` neither takes the game for hung nor snapshots it. A stop changes nothing in the
  world: a recording replays across one, and a lockstep peer waits as for a slow one.
- **Mods are scripts a player installs**, as Luau source beside a manifest, `mod.ron`. Each runs in
  a VM of its own, kept to the capabilities it asks for, which the game must grant, a memory limit,
  an interrupt budget per tick and a print rate; one refused warns, and the game runs without it.
  A game registers its component types before it loads mods, which may take any free name
  (`ecs.registered_late`). Mods go whole in snapshots and recordings, so a lockstep joiner runs its
  host's and a replay its recording's. [`Mods`](crates/gg-engine/src/luau/mods.rs) has the layout,
  limits and refusals.

## Agents and tools

Agents work on a game the way people do, through the same interfaces: nothing a person can do in
the editor is out of an agent's reach. The editor is one more client, with no ability an agent
lacks.

### The tool API

A development build serves the tool API on the loopback: lines of JSON over TCP, at a port the OS
picks. Clients query and edit the world, play, stop and step it, open and save the [open
scene](#project-state), capture and share the view, read UIs' layouts as data, send input,
snapshot, patch code, debug scripts and follow the log. [gg-api](crates/gg-api/src/lib.rs) owns
the protocol, each method's answer, limits and codes; [the
server](crates/gg-engine/src/api/server.rs) how a game answers.

- **Trust.** The run writes a session file beside its executable, with its port and a token, that
  only the user can read. A connection's `hello` gives the token, compared in constant time, so only
  the user's own processes reach the game. A `hello` may narrow its connection to some capabilities
  (`api.ungranted` beyond them). That restrains only a client that can't read the session file, such
  as a sandboxed agent: any process that reads it reaches everything.
- **Revisions.** The session file names the protocol's revision, gg-api's `PROTOCOL`, bumped by
  hand at a change an older `gg` would misread. `gg` warns of a game whose revision isn't its own
  (`gg.protocol`), as one built against another engine may be. A number, not the engine's commit,
  which would rebuild games and `gg` at every commit and warn of every game running across one.
- **Who answers.** Each connection's thread reads at most `MOST_IN_FLIGHT` requests ahead of its
  answers, so a client that sends too fast is held back by TCP, not buffered by the game. It answers
  `ping`, `logs`, `note` and `debug` itself, so a tool tells a hung game from a busy or paused one.
  The main loop answers the rest as a frame starts, before its ticks, only those that came before
  it, so clients can't stall it.
- **Play and stop.** Stopped (`stop`, or `--stopped`), no tick runs: the world is the world as
  edited, which only authors' edits, scene files' saves and the open scene's New and Open change.
  `play` runs a copy, holding the world as edited, its scene files and authors' logs aside until
  `stop`, so two plays of one world, given the same inputs, go alike. Paused frames draw the world
  as the last tick left it, so a capture shows what a query reads. Recorded runs and lockstep peers
  refuse `stop` (`api.no_stop`): it replaces the world and takes ticks back, which neither a replay
  nor the other peer could follow.
- **The view** is shared through memory the client makes, a triple buffer neither end waits on, not
  through the game's window: only Windows and X11 let a process embed another's window, which it
  can't draw over. An embedded run (`--embedded`) renders at the client's size, a frame once the
  client has taken the last. The editor takes one a display refresh (`App::embeds_game`). Clients
  share one look, part by part, the last setter of each winning:
  viewpoint, outline, grid, colliders, what shows in place of the light, passes left out. Extraction
  drops a pass left out, so the world is unchanged and the renderer has nothing to skip. Each frame
  carries its viewpoint and the look's revision, so what a client draws over it lines up, and the
  game's last frame times, for the client's Performance panel.
- **Captures** show the view as shown. Asked, they show it as the game shows it alone, or a part of
  shading in place of the light, from frames that aren't shared, so the user's view doesn't change.
  `frames` captures frames in a row along a scripted path, to see how the view settles or shimmers.

### Clients

- **`gg`** ([main.rs](tools/gg/src/main.rs)) builds and runs games, talks to them, runs headless
  checks and makes games, with machine-readable results. Inside a project it builds the project's
  game and reaches only runs whose session files name its manifest. `gg mcp` gives agents that speak
  MCP the same reach, connecting afresh at each call so a restarted game is the one it reaches.
- **`gg run`** ([run.rs](tools/gg/src/run.rs)) supervises one game: it runs a copy of the
  executable, so the game builds meanwhile; patches or rebuilds it as its code changes ([Game
  code](#game-code)); snapshots it every few seconds; and starts it again from the last snapshot
  after a crash, a hang or a rebuild. `--supervised` ends each game as gg ends, however gg ends.

### Headless runs

Every app runs without a window (`--headless`), and `--frames N` ends it after N frames with a
status: 0 for a clean run, 1 after an error or any validation message, 2 for bad arguments.
Dedicated servers, CI and agents checking their work run this way. Rendering checks render
offscreen; simulation-only runs need no GPU. A run embedded in a tool renders offscreen in real
time.

## Project state

A project's state is text that people and agents edit, diff and merge with ordinary tools. The
engine writes it canonically: the same state gives the same bytes, and an edit changes only its
own lines.

- **Projects.** A game's project is its crate's directory. Its manifest, `project.ron`, names the
  package, asset root, scenes folders and start scene, and may name the relay its sessions meet
  through and its icon ([project.rs](crates/gg-api/src/project.rs)); tools never rewrite it. A
  `CREDITS.txt` beside it heads a shipped folder's notices. Its fetch list, `fetch.ron`, pins by
  URL and SHA-256 the files its repository can't hold, as models whose licenses keep them out
  ([fetch.rs](crates/gg-api/src/fetch.rs)).
- **Packages.** Shipping builds start from cooked data: a project's game built without `dev` reads
  no manifest and nothing under its asset root, only its package, one file the
  [cooker](tools/cook/src/lib.rs) writes ([format](crates/gg-engine/src/package.rs)). It holds
  the manifest's asset root, start scene and relay, and what runs load, cooked where a load would
  make something (font fields, SPIR-V, compressed textures, optimized meshes), so a shipped run
  needs no slangc and decodes no PNG. A run finds its
  files by exact path, so a package runs alike wherever it's copied. The cooker refuses ID files a
  development run would warn of, so a package names assets alike and replays recordings to every
  hash. Cooking a project again gives the same bytes.
- **Canonical text.** Scenes, prefabs, UIs, manifests and ID files are RON, which maps Rust's enums
  and tuples directly, and whose trailing commas keep an added entry off its neighbors' lines. The
  engine writes scenes itself, so their bytes depend on no library, line ending or Rust release;
  `ron` only reads them, through reflection ([the form](crates/gg-engine/src/ecs/scene/mod.rs)). A
  scene writes every field and no type's name, so a rename touches only its keys. A load checks
  everything before it spawns anything; a refusal names its line and column, and its entity,
  component and field.
- **Stable IDs.** Every entity has a persistent ID, separate from its runtime handle, that files,
  operations, diagnostics and network messages name it by. IDs are random, not sequential, so
  entities added on two branches neither collide nor land on the same lines. A prefab instance's
  and a model's entities mix theirs from the root's, so the root's ID alone gives them wherever
  it's loaded, made or restored ([id.rs](crates/gg-engine/src/ecs/id.rs)).
- **Asset IDs.** Each texture, model, font, prefab and UI has an ID that outlives its file's name,
  in an ID file beside it (`stone.png.meta`), committed and moved with it. Everything that names an
  asset, from scenes to the world's hash, names it by ID, or by path where it has none. A
  development run serving the tool API, but a lockstep joiner, writes the ID files missing and
  moves a renamed file's old one beside it, paired by content
  ([asset.rs](crates/gg-api/src/asset.rs), [assets.rs](crates/gg-engine/src/assets.rs),
  [asset_watch.rs](crates/gg-engine/src/asset_watch.rs)); `gg fetch` writes those of what it
  fetches. A run never rewrites an ID file it can't read, which may be a merge's conflict. A run's
  names travel with what other processes read of it, so a replay or a lockstep joiner goes as it
  did whatever ID files its disk holds, a stand-in (`asset.stand_in`) for what won't load. A
  removed asset's name still finds it inside ticks, so nothing the simulation reads depends on when
  it was removed.
- **Prefabs.** A `.prefab` file is a scene of one subtree. An entity with a `PrefabRoot` is a linked
  instance, the prefab's entities spawned under it. Its overrides are what differs from the prefab,
  found by comparison, never stored apart: a scene saves only them, so a prefab's edit reaches every
  instance, live in a development run. Tools change an instance's fields and give it components and
  children, but remove nothing its prefab gives (`ecs.prefab_entity`); they unlink, link and
  revert. `save_prefab` links a subtree to a new prefab without changing an ID. Prefabs nest, but a
  cycle, or a variant (a root linking another prefab), is refused
  ([prefab.rs](crates/gg-engine/src/prefab.rs),
  [ecs/prefab.rs](crates/gg-engine/src/ecs/prefab.rs)).
- **Models in scenes.** A `ModelRoot` spawns the model's nodes under it. Scenes save the root alone
  and tools edit no node, so the model's file stays the nodes' one source; what code changes of a
  node lasts the run ([model.rs](crates/gg-engine/src/model.rs)).
- **UIs.** A `.ui` file is a tree of named nodes ([game_ui](crates/gg-engine/src/game_ui/mod.rs)),
  written by people and agents, never by the engine. Every button and widget has a name, by which
  code is told of its presses and changes and tools find it. A UI's state is the run's, not the
  world's.
- **Operations.** Every authoring change, by a person or an agent, is an operation with an inverse:
  create, duplicate, delete, add, remove, set, reparent, and unlink, link or revert an instance
  ([ops.rs](crates/gg-engine/src/ecs/ops.rs)). An operation names entities by ID, checks everything
  before anything changes, and keeps what it takes out, typed, so an undo brings back what's opaque.
  Each author, a client by its `hello` name or a scene file by its path, has its own log, so
  several edit at once. A log undoes edits whole and merges a gesture's sets
  ([log.rs](crates/gg-engine/src/ecs/log.rs), [edits.rs](crates/gg-engine/src/edits.rs)). Edits
  apply between ticks, and recordings keep them. A scene file saved while the game runs applies as
  the operations from what it held to what it holds, keeping what the game changed that the file
  doesn't ([scene_files.rs](crates/gg-engine/src/scene_files.rs)).
- **The open scene.** A project's game has one scene open, which the tool API's `scene` opens, saves
  and empties. It owns its file's entities and those authors' edits made in it, never the game's
  code's, so a save writes the scene alone. A save refuses a file changed since it last applied
  (`scene.changed`). New and Open start every author's log afresh, as the world goes on as another.
- **Scene changes.** A tick changes the scene (`World::change_scene`): every entity goes, the
  scene's take their place, and resources stay. It's the simulation's own, as a command is, so
  replays, peers, rollback and servers' clients make it at the same tick, nothing recorded. What the
  run keeps beside the world follows once the tick is final. Edits from before don't undo
  (`ops.scene_changed`), as the scene may reuse their IDs
  ([scene_change.rs](crates/gg-engine/src/scene_change.rs)).
- **Worlds of cells.** A world too large to hold at once is a scene whose `Cells` cuts space into
  cubes, each a scene file or made by code. Cells come in near a `Viewer`, decided from the world
  alone as each tick ends, so replays, rollback and peers stream alike. A cell that changed goes
  out as kept text and comes back from it. Threads of their own prepare cells, and a tick waits for
  them ([cells.rs](crates/gg-engine/src/cells.rs)). Stopped, as the editor edits, cells come in
  around the look's viewpoint and are edited as files: a save writes each streamed root to the file
  of the cell it stands in.

## ECS

The world lives in our own ECS, built for what the tools, hot reload and multiplayer need:

- **Reflection:** every component type, Rust's or a script's, registers at runtime with a
  description of its fields, which saving, operations, the Inspector, snapshots, replication and
  Luau's bindings all work from.
- **Snapshots:** the whole world saves and restores fast enough to take one every frame, for
  rollback and client-side prediction as well as hot reload.
- **Commands:** nothing outside the simulation changes its state but a command applied at a tick:
  player input, a network message or an [operation](#project-state). A command is data, never a
  closure, so a snapshot and the commands after it replay from there, for rollback, and a recording
  (`--record`) replays (`--replay`) without a window or a GPU, for bug reports an agent can rerun.
- **Determinism:** the fixed timestep, a fixed order of systems and of iteration, and math with the
  same bits on every platform ([Cargo.toml](Cargo.toml)'s glam, [clippy.toml](clippy.toml), Luau
  held to the same, its C++ built with no multiply fused into an add, which gg-engine's build
  checks), so lockstep and rollback stay in sync. Cargo.toml pins every crate the simulation's bits
  go through, and those beneath them, to the releases CI's reference hashes were made with, so
  neither `cargo update` nor a game's own lock moves one: a new release rewrites the hashes. A
  recording hashes the world after every tick, so its replay finds the first tick that goes
  another way.
- **Ticks run again:** `--sync-test` runs each tick again on a copy of the world as it started and
  ends where the two part (`sync_test.diverged`), checking as a game runs what rollback needs: that
  a tick goes by the world, its commands and its layer's setup alone, and a copy holds all it reads.

The world is a `World` of entities and their components, in safe Rust:

- An entity is a handle, its slot and the slot's generation, so a despawned entity's handle never
  names another. A delete keeps its slots vacant until its inverse puts them back or is dropped, so
  an undone delete's entities get their handles back. Files, operations and diagnostics name an
  entity by its `EntityId`, a persistent 64-bit ID from its world's own stream, seeded at random,
  so two sessions' IDs meet only by chance. A `Name` is for people: optional, and not unique.
- A component derives `Component`; it's `Clone`, for snapshots, and `Send` and `Sync`, for the job
  system. It lives in an archetype table, one per set of types, whose columns are each type's own
  `Vec` behind a trait object, not raw pointers. Queries borrow the world, shared to read and
  exclusively to change it, so the borrow checker keeps them apart.
- Systems are plain functions run at ticks in a fixed order; `par_for_each` splits a query's rows
  across [threads](#threads). Query order follows only from the changes made, and no hash map is
  iterated, so it's the same every run, but it means nothing: what's ordered sorts by values or IDs.
- A native script, a `Script`, is a component that acts. Its state is the component, so scenes,
  operations, the hash and snapshots reach it, and adding the component by name attaches the
  behavior. Scripts run one entity at a time on the tick's thread, so what a game has thousands of
  is a system. Each type's pass is a generic function, not a table of functions, so subsecond's
  patches reach it as they reach its layer.
- Each column marks when each of its rows last changed, and only its table reaches it, so nothing
  changes one unmarked. Drawing and a server's states cost what changed, not the world's size.
- A child's `Parent` names its parent, its `Transform` is relative to the parent's, and a despawn
  takes the descendants. Only `World::set_parent` changes a `Parent`, which isn't `Writable`, so the
  hierarchy stays whole. Drawing keeps world matrices outside the world, placing again only what
  changed; `World::pose` gives the same, bit for bit.
- Resources, a value of each type beside the entities, such as physics' bodies, are copied by
  snapshots but held by no scene, hash or recording: they hold only what the simulation carries on
  from or makes again from the entities.

A `Value`, what reflection reads and sets, is closed: integers of fixed sizes and `f32` apart from
`f64`, so a value means the same on every machine; an entity as its handle; a texture as the
renderer's, so no value makes an index that shaders sample unchecked; and `Opaque` for what
reflection keeps but doesn't read. The Inspector reads only through reflection and edits only
through operations, which the tool API serves too, so a remote Inspector shows a game in another
process as the editor's does.

A snapshot is the world copied (`World` is `Clone`), and `clone_from` reuses the memory it holds,
so one taken every frame allocates nothing in steady state. A restored world goes on as the one
copied did, spawning the same handles and IDs. A copy, as a world cleared, replaced or renewed, is
another world with a serial of its own (`World::serial`), which refuses what undoes an operation
applied before, so undo history doesn't outlive a restore; state kept of a world is kept by its
serial. Rollback's `World::rewind` keeps it. A snapshot file holds the world as text, for a restart
in another process ([snapshot.rs](crates/gg-engine/src/snapshot.rs)). The world's hash reads its
entities by ID, and neither slots, handles nor resources, so a scene loaded into another world keeps
its content hash ([hash.rs](crates/gg-engine/src/ecs/hash.rs)).

The simulation steps in ticks, 64 a second whatever the frame rate (`Tick::DT`). A simulation slower
than real time slows down rather than spending every frame catching up, and a headless frame runs
exactly one tick. Once the layers have attached, the world changes only at their ticks:
`Context::world_mut` panics elsewhere, and a tick gets the world but no `Context`, so it goes by the
world, its commands and its layer's setup, never by the frame's time, input or drawing. A replay
runs the ticks with nothing between them, so a layer keeps between ticks only its setup and its
queue. The Inspector's log is the exception, which a replay rebuilds from the recorded edits: its
inverses hold typed columns tied to the world they were made in, so it can't be a component.
Operations are authoring's commands: the Inspector's wait for its next tick, and the tool API's and
scene files' apply between ticks.

## Networking

Every multiplayer model runs on the same snapshots, commands and hashes, and each run simulates
every player's world. Two peers run in lockstep (`--host`, `--join`), or predict each other and
roll back (`--rollback`, on both); a server (`--serve`) owns the world its clients (`--connect`)
play. Any of them may meet through a relay (`--relay`). `Context::player` names the run's own
player, whose commands its layers queue, naming it in them. `Context::networked` says whether the
run is in a session at all, so a game leaves out of one what only a run alone has, as a bot
playing the other player. [net.rs](crates/gg-engine/src/net.rs) and its modules, and [gg-relay's
root](crates/gg-relay/src/lib.rs), have the wire formats, the limits and the rest.

A game hosts and joins sessions from its own UI too, so its players need no flags.
`Context::host` starts a lockstep session through the run's relay, `--relay`'s or else its
project's, playing on from its world as player 0; `Context::join` joins by the code a player types
or pastes, the host's world taking the run's place; `Context::leave` ends either. `Context::session`
says where it stands, with its code, for the UI to show, and `networked` and `player` follow it.
`App::predicting` has such sessions predict each other's inputs: over a 150 ms round trip,
lockstep answered a player's own character some 160 ms late, predicting in 31 ms. A recorded run
starts none, as a recording holds a run alone or a session from its start.

### Peers

- **A tick runs on its inputs alone**: each layer's commands, the host's first, and the edits from
  outside the layers that apply before it, as a recording keeps them. A lockstep peer runs a tick
  once it has the other's inputs, drawing frames while it waits.
- **Rollback predicts that the other's inputs hold nothing.** Commands set what lasts, as a disc's
  heading, so the other player goes on as its last commands had it. Where the inputs held
  something, the peer takes the very world back (`World::rewind`) and runs the ticks again. The
  layers are told nothing: a tick goes by the world, its commands and what its layer set up alone,
  which `--sync-test` checks.
- **Peers keep alike in time**: the one ahead runs its ticks slower, as GGPO's time sync has it.
  Otherwise a rollback peer ahead predicts far, and a lockstep peer ahead stalls at each jitter.
- **Edits and patches are inputs too**: the tool API's edits, scene files' and scripts' saves,
  `gg run`'s patches, and commands of a queue that edits (`Commands::edits`, the Inspector's). Each
  applies once, at a tick run on both peers' inputs, so the authors' and the Inspector's logs undo
  through a world taken back. The host alone watches scene files and scripts, and picks each
  patch's tick.
- **A joiner starts from the host's world**, as a snapshot file's text. So a restarted peer
  rejoins: a joiner that loses its host waits for it, and a host that loses its joiner plays on
  alone, the joiner's player still in the world.
- **Desyncs name their field.** The peers compare the world's hash after each tick run on both
  peers' inputs (`net.desync`). Where they part, the joiner names the first entity, component and
  field that differ, with both values (`net.desync_field`), before the difference spreads. The host
  then sends its world again, and the peers carry on in sync.

### A server and its clients

- **The server owns the world.** It runs headless, in real time. After every second tick, which
  halves a client's download, it sends each client what the client sees, against the latest state
  the client has. A thread of its own writes the states, so the main thread's time doesn't grow
  with the clients or the world. A client refuses the tool API's edits (`net.no_edit`), which the
  server's next state would take away.
- **Clients predict their own player, and correct.** A client runs ahead of the server by the way
  there and back and a margin, on its own inputs alone: its own player exactly, the others as their
  last commands had them. Where a state's hash isn't what it predicted, it takes its world back to
  the server's and runs the ticks after again, as rollback does. Commands of a queue that edits go
  to the server alone.
- **Each client sees the cells near its own viewers**, those whose `Owner` names its player. Its
  states, its hash (`World::hash_seen`) and its physics hold that view alone. A client brings no
  cell in itself.
- **The other players draw between the server's states**, about five ticks behind the latest. So a
  collision with another player happens where the client predicted it, ahead of where it's drawn.

### Trust and transport

- **A session takes runs of its own build.** A host or a server refuses a run of another app,
  engine, layers or build (`net.refused`), as builds that simulate otherwise part at every tick. A
  game names its build by the digests of its source, which its build script makes
  ([build.rs](crates/gg-api/src/build.rs)), and of the package it reads. So runs of one source and
  package join whatever OS or machine built them, CI's reference hashes holding the simulation
  alike on each. An app that names no source, as the sandbox, is named by its executable's bytes,
  which differ between machines, as rustc writes paths into them.
- **A session is kept to the runs given its code** (`--code`, 80 random bits). It keys each
  connection's Noise `NNpsk0` handshake, through `snow`, which refuses a run without the code at
  its first message, before it takes a joiner's place. The handshake's keys seal the stream and
  each packet (ChaCha20-Poly1305), so what the runs send can't be read, forged or played again.
  Each end's own keys go into them, so a code found later opens no link made before. QUIC would
  rewrite every link for congestion control and migration the engine doesn't need yet; a MAC alone
  would leave a server's states readable on the path.
- **Runs behind routers meet through a relay** that each connects out to (`gg-relay`). The host's
  room there is an HMAC of the code: the relay sees the room, but can't find the code, nor read or
  forge what it carries. A run given another code comes for a room no host holds, and one with the
  room but not the code fails the handshake. Packets go direct by UDP hole punching where the
  routers let them, and only while both ends hear each other direct, so a way that works one way
  alone is never taken. The stream stays on the relay, as TCP's hole punching gets through fewer
  routers. TURN relays UDP alone, and iroh would rewrite the links on QUIC; the engine's relay
  carries both, and builds on gg-core alone of the engine's crates.
- **A relay holds each end to rates**, token buckets at three to four times what a 3D game's
  session of two skinned characters and a ball sent at most, and caps connections, per address too
  (`relay.limited`). Datagrams past a rate drop, as on a lossy link; a stream past one is read more
  slowly, so TCP holds its sender back and nothing is lost.
- **A player's commands steer that player alone.** A command with an integer field `player` must
  name the player whose inputs carry it, or every run refuses it alike (`net.bad_command`). A
  server's own commands, which may steer any, aren't checked.
- **TCP carries what must come whole and in order, and UDP the rest**: a tick's other inputs and
  the server's states. Each packet of inputs repeats those the other hasn't said it has, so a
  packet lost loses none.

## Diagnostics

Diagnostics are data, so agents and tools can act on them as reliably as people read them.

- **Logs** go through `tracing` with structured fields, filtered by `RUST_LOG`. A run prints
  readable lines to stderr, keeping stdout for machine-readable output, and writes every event as a
  JSON line to a file whose path it logs as it starts ([log.rs](crates/gg-core/src/log.rs); where,
  `log_dir` in [gg-engine's root](crates/gg-engine/src/lib.rs)). Its last events stay in memory for
  the tool API's `logs`. An embedded run prints no lines, as its tool shows its events.
- **Errors and warnings name their subject.** Each carries a stable code, a dotted name such as
  `gpu.device_lost`; what it's about: an entity's ID (a despawned one's handle), an asset path, a
  render pass, a shader's file and line, or a scene's line and column; and, where there is one, the
  next step, such as `cargo gg fetch`. What outlives an entity, as a pick or a queued message does,
  keeps its ID, not its handle. A layer that can't go on ends the run through `Context::fail`, not
  a panic: the layers detach, and the status is 1. One that can't load returns the error from
  `Layer::load`, which ends the run before any layer attaches. A panic logs `log.panic` before the
  process ends.
- **Panics in shipped builds.** `log.panic` holds the backtrace, whatever `RUST_BACKTRACE` says, and
  its `frames`, each a module and offset, as `Shards.exe+0x12a4f0`
  ([frames.rs](crates/gg-core/src/log/frames.rs)). `Starting GGEngine` names the build's source
  digest, as the symbols' `build.txt` does. The symbols `gg ship` keeps beside the folder turn
  frames into lines offline, as a player's machine can't:
  - Windows: `llvm-symbolizer --obj <symbols>/Shards.exe --relative-address <offset>...`, which
    Visual Studio installs, finding the PDB beside it;
  - Linux: `addr2line -f -C -e <symbols>/Shards <offset>...` (binutils 2.36 or later demangles
    Rust's symbols);
  - macOS: `atos -o <symbols>/Shards.dSYM/Contents/Resources/DWARF/<executable> -l 0x100000000`
    and each offset plus `0x100000000`.
- **Validation messages** become events with their VUID and the debug names of the objects
  involved. A run that can't validate synchronization warns `gpu.no_sync_validation`, saying why.
- **Captures.** Under RenderDoc's layer, `--capture N` captures frame N, headless too, and logs the
  file's path; everything the engine makes shows by its debug name ([gg-renderer's
  AGENTS.md](crates/gg-renderer/AGENTS.md)).
- **Profiling.** Zones (`gg_core::zone!`) show in Tracy beside each pass's GPU timestamp, and
  compile to nothing without the `tracy` feature. The engine also times every frame itself, part
  by part, for the Performance panel. A run that doesn't end in an engine error logs `engine.perf`,
  its frame times and where they went, its ticks, memory, GPU and build, so agents compare builds
  by their runs. A rendering run logs `engine.loaded` once loaded, and each load its time under its
  own code ([main_loop.rs](crates/gg-engine/src/main_loop.rs)).

## Events and layers

An app is a stack of layers, then overlays above them, which its `main` describes on `App`. Each
layer loads, making what it draws with and registering its component types, then attaches,
building the world; a run restored from a snapshot calls `Layer::restored` instead, so a layer that
spawns as it loads panics. [`Layer`](crates/gg-engine/src/layer.rs) has the rest of the lifecycle.
The engine turns winit's and gilrs's events into its own, so apps and games never see their types.
Events go down from the top overlay until a layer handles one, but releases, sticks and triggers
back at rest, and the loss of focus reach every layer, so none is left holding what it saw pressed.
Ticks, updates and UI go up from the bottom. Headless runs have no window or gamepad events, but
their layers tick, update and draw UI all the same.

egui is the development UI, and in development builds the top overlay: it sees every event first
and keeps what it wants, but never a release. An app that draws none stops running it. The dock,
through `Ui`, is the engine's own, since egui's docking crates can't put a panel in a window of its
own: panels dock around a central area that stays the game's, or float, over the main window or in
windows of their own. Tools take a dockspace (`App::view_panel`), where the game's view is a panel
too but never leaves the main window, whose input alone reaches the layers, and draw their windows'
title bars (`App::custom_title_bar`). Games keep the OS's frame.

A game's own UI is apart from egui, on the view's canvas (`Context::canvas`): text and sprites
anchored to the view and scaled with its height, drawn into the view, so captures show them. A
`.ui` file is a tree of named nodes, which a layer adds (`Context::add_ui`) and drives by name
through `GameUi` ([game_ui](crates/gg-engine/src/game_ui/mod.rs)). A UI's state is the run's, not
the world's: each lockstep peer has its own, and none of it is hashed, snapshot or recorded, so a
choice that changes the game queues a command, as input does.

Input enters the simulation only as commands. A layer polls `Context::input` for what's presented,
or binds `Actions` and queues each change of an action's value. What an action holds is a component
its commands set, so a tap between two frames applies at one tick, a hold at every tick, and
snapshots and replays keep it. A recording holds actions' changes, never keys, so binding them to
others changes none. Mouse look adds the mouse's raw motion up into a command of its own: motion is
a distance moved, not a value as an action holds. Losing focus lets go of everything held, keys,
buttons, actions and pads, since not every platform reports the releases that come out of focus.
What Escape does is the game's.

## World space

World space is right-handed with +Y up, as glTF's is: a camera without rotation looks down -Z, with
+X to its right. glTF assets need no conversion, and scenes, physics and the editor share the axes.

Positions are `f64`, so a world 1,000 km across is as precise at its edges as at its origin, where
`f32` steps 6 cm: translations, world matrices (glam's `DAffine3`, rotation and scale staying
`f32`), physics (rapier's `f64` builds), cells, sounds, bounds, interpolation, the editor's handles
and the tool API. The GPU stays `f32`: each frame draws relative to an origin of its own
(`Context::origin`), which follows the view in steps of 1,024. As it moves, the engine and the
renderer move what they keep from the frame before, so the view goes on without a cut. Fog's height
and the atmosphere's altitude stay absolute. Navigation, Luau's vectors and models' local spaces
stay `f32`, as [DEFECTS.md](docs/DEFECTS.md) weighs.

## Physics

Physics is rapier's, in gg-engine's `physics` module: rapier2d for bodies in the XY plane and
rapier3d for bodies in space, their `f64` builds with `enhanced-determinism`, so a step gives the
same bits on every platform. Games reach it through components: an entity with a `Transform` and a
`RigidBody2D` or a `RigidBody3D` is a body, and its collider components are its shape, in its
entity's space. `step_physics`, a system a layer's tick runs, steps 2D, then 3D.

- **Physics' state is the world's**: rapier's bodies, colliders, joints, contacts and what sleeps,
  in the `Physics2D` and `Physics3D` resources, which snapshots copy and snapshot files hold through
  rapier's own serialization. Bodies made again from their components alone wouldn't step as the
  world did.
- **What would spread is refused.** A body under a parent isn't moved (`physics.child_body`), and a
  component that isn't finite is refused (`physics.not_finite`), so a NaN doesn't spread through
  contacts.
- **A joint is a component of its own**, a `Joint2D` or a `Joint3D` naming both bodies, on an entity
  of its own or on either body's, rather than on one body naming the other. So a body is in as many
  joints as name it, as a chain's link is in two and a ragdoll's torso in four, with no rule for
  where a second goes. A joint comes and goes, as an edit's create or delete, without changing
  either body's components, which prefab instances' overrides would otherwise hold. And an entity
  holds one joint, whatever its kind. Its frames, limits and motor are its fields, so the component
  alone says where it holds, which scenes save, the Inspector, the tool API and scripts edit, and
  the hash reads.
- **A character**, a `CharacterController3D` beside a `CapsuleCollider3D`, is moved by rapier's
  kinematic character controller at the `velocity` its game sets. Its state is its components' and
  the world's, so replays, snapshots and peers move it alike.
- **Contacts and casts read rapier's state as the last step left it**, ordered by entities' IDs, so
  every platform, peer and replay has them alike.
- **Determinism.** Joints, motors and swept bullets (`continuous_collision`) are rapier's
  arithmetic, which reference hashes of each kind of joint and a bullet, 2D and 3D, check on every
  platform. A mesh collider's triangles are ordered by where they are, not as the cooker leaves
  them, so a run from glTF and one from its package collide alike. The hash reads colliders'
  `sensor`, `membership` and `filter` only while they aren't their defaults
  (`#[reflect(hashed_when_set)]`), so a world that sets none hashes as it would without them.

## Animation

Animation is gg-engine's `animation` module. Its state is in components, `Flipbook`, `Tweens`,
`Animator` and `AnimationStates`, so snapshots, replays and peers see it alike. `step_animation`, a
system a layer's tick runs after what starts tweens and plays clips, such as scripts, and before
physics, which takes bodies in where animation put them, steps it.

- An `Animator` plays a model's glTF clips on its nodes, blended by their weights. The joints it
  moves are nodes, so snapshots, replays and peers see a pose as any transform; what they move of a
  skinned mesh is drawing's, as [Rendering](#rendering) says.
- `AnimationStates` is a state machine of the animator's clips. It drives the animator and leaves
  the sampling to it; its state and its time in it are its fields, so a machine resumes exactly.
- **Determinism.** Curves are libm's functions, rotations glam's slerp, and the rest `f32`
  arithmetic. Three reference hashes in `animation/`, of tweens and flipbooks, of clips, and of the
  Fox driven through its states, check them on every platform.

## Navigation

Navigation is gg-engine's `navigation` module, our own rather than a crate's, as none promises the
same bits on every platform. It bakes a navmesh from the world's static 3D colliders, finds paths
over it (`World::find_path`), and walks `NavAgent`s along them round each other. `step_navigation`,
a system a layer's tick runs after what sets the agents' destinations, such as scripts, and before
physics, which moves the characters the agents steer, steps it. Its state is the agents'
components and the world's `Navigation` resource, which holds the navmesh.

- **The bake follows Recast's pipeline as far as the erosion**, then cuts the walkable cells into
  rectangles, which stay correct across slopes, steps and floors over floors, as
  [bake.rs](crates/gg-engine/src/navigation/bake.rs) says.
- **The navmesh follows the world.** It bakes again as a fingerprint of what it's baked from
  changes, and every step and search brings it up to date first. The bake is deterministic, so
  snapshot files don't hold the navmesh: a world restored bakes the same one, and its agents' paths
  hold.
- **Agents steer round each other by ORCA**, as RVO2 does, among their ten nearest within reach on
  the same floor. Every pair is compared, which bounds a crowd at some hundreds.
- **Determinism.** Plain `f32` arithmetic, libm's trigonometry and order by entities' IDs, as
  [navigation's root](crates/gg-engine/src/navigation/mod.rs) says; a reference hash in
  `navigation/` checks every bit on every platform.

## Rendering

`gg-renderer` turns each frame's [`RenderData`](crates/gg-renderer/src/lib.rs), plain data the
engine extracts from the world, into images. It never sees the world, the ECS or game code, so the
render thread renders one frame while the main thread runs the next. Three choices shape it:

- **Bindless.** One descriptor set per frame in flight holds every texture, sampler and storage
  image ([`bindless.rs`](crates/gg-gpu/src/bindless.rs)). Shaders pick textures by index and read
  buffers through device addresses, so nothing is bound per draw, and a texture change is only a
  parameter change.
- **A render graph.** Each frame declares its passes in order, and how each uses each image and
  buffer. gg-gpu's `Graph` places every barrier and layout transition, aliases the graph's own
  images in shared memory, and splits passes between the graphics and compute queues. No pass
  records a barrier itself.
- **GPU-driven opaque geometry.** The world's opaque meshes stay on the GPU from frame to frame.
  Compute passes cull them and pick their levels of detail, and their meshlets draw into a
  visibility buffer that a compute pass shades. The CPU's cost per frame follows what changed, not
  what's drawn.

### The view

The view renders into images of its own, its area's size in pixels, made again as that changes.
The frame shows them pixel for pixel, under the UI or where a dock panel's
[`ViewCallback`](crates/gg-renderer/src/view.rs) paints it. So post-processing and captures read
the whole view, and nothing stretches.

- Colors are linear 16-bit floats, which games' pipelines are made for. Passes that read them take
  NaN as 0 and infinity as 65,504, since a shader's float can be infinite.
- Every draw writes its 64-bit ID beside its color; what writes depth also writes its motion.
- Depth is a 32-bit float, reversed: 1 at the near plane, 0 far away. That keeps precision where
  it's needed and lets perspective see infinitely far. The test is greater-or-equal, so at one
  depth the later draw goes over, as 2D games stack draws at one z.
- The tonemap pass maps the colors into the 8-bit sRGB `view` image, which the frame shows,
  captures read and the outline draws over.
- Lit shading takes light in physical units, scaled by the exposure (EV100), which keeps a sunny
  day's light within half floats. Emission, sprites, text and unlit materials draw their colors
  as they are, so they show as authored at any exposure.

A world's [`Environment`](crates/gg-engine/src/environment.rs) sets how the view shows light, for
every viewpoint, the editor's and tools' looks too, so they show what the game shows. A world
without one isn't tonemapped, antialiased, bloomed or graded, so 2D sprites show as colored. Ties,
of environments and of primary cameras, go to the lowest entity ID, since scenes keep no order.

### Passes in frame order

A pass runs only when its feature is on and has something to do, so a feature that's off costs
nothing. Labels are the passes' debug labels, as RenderDoc and Tracy show them. Parenthesized are
the debug names of what a pass makes of the graph's own; `cargo gg capture --show` shows those
that are images. Paths are under `crates/gg-renderer/src/`.

| Pass | What it does | Labels (images) | Code |
|---|---|---|---|
| Writes and uploads | Material table, the scene's changed records, streamed meshes and textures, recorded before any pass | | `lib.rs`, gg-gpu `frame/upload.rs` |
| Atmosphere | Its tables, every frame it shows | `atmosphere transmittance`, `… multiple scattering`, `… sky view`, `… aerial perspective` | `atmosphere.rs` |
| Sky light | Prefilters the sky into diffuse and specular cubes, and once the split sum's table, only as the sky changes | `sky cube`, `sky levels`, `sky light`, `split sum` | `sky.rs` |
| Skin | Moves skinned meshes' vertices by their joints | `skin` (`skinned vertices`) | `skin.rs` |
| Particle births | Writes the frame's births into the particle pool | `particle pool growth`, `particles moved`, `particles` | `particles.rs` |
| Cull | Gathers opaque instances, culls them for the view and each cascade, picks levels, culls meshlets; on the compute queue | `cull gather`, `cull choose`, `cull offsets`, `cull write`, `cull meshlets` | `mesh/cull.rs` |
| Shadows | Casters' depth into four cascades | `shadow cascade 0`–`3` | `shadow.rs` |
| Light clusters | Bins lights into the view's 16×9×24 clusters; on the compute queue, beside the shadows | `light clusters` | `cluster.rs` |
| View, first pass | What was unhidden last frame: into the visibility buffer, as depth alone, or shaded | `view visibility`, `opaque depth` or `view` (`view visibility`, `view ids`, `view depth`, `view colors`, `view motion`) | `lib.rs`, `mesh.rs` |
| Occlusion cull | Builds the depth pyramid from the first pass, culls the rest against it | `depth pyramid`, `cull choose second` … `cull meshlets second` | `mesh/cull.rs` |
| View, second pass | What came into view | `view visibility second`, `opaque depth second` or `view second` | `lib.rs` |
| Early depth, normals | Beside a visibility buffer, depth of what draws forward; normals for reflections | `opaque depth`, `view normals`, `opaque normals` | `lib.rs`, `mesh/visibility.rs` |
| Ambient occlusion | XeGTAO from the depth, denoised, accumulated over frames | `ambient occlusion prefilter`, `ambient occlusion`, `ambient occlusion denoise` | `ao.rs` |
| Reflections | Mirror rays traced through a nearest-depth pyramid | `reflections depth`, `reflections`, `reflections blur` (`reflections spread`) | `reflections.rs` |
| Shading | Shades the visibility buffer, then draws the rest: sky, other opaque runs, what blends, canvas. Without a buffer, the forward `view` pass | `view shading`, `view rest`, `view` | `mesh/visibility.rs`, `mesh.rs` |
| Fog | Fogs what's opaque from depth; what blends then draws, fogging itself | `fog`, `view blended` | `fog.rs` |
| Antialiasing | TAA's resolve, or SMAA | `taa resolve` (`view resolved`); `smaa edges`, `smaa weights`, `smaa blend` (`view antialiased`) | `taa.rs`, `smaa.rs` |
| Keep | Copies the antialiased colors for the next frame's reflections | `reflections keep` | `reflections.rs` |
| Exposure meter | Luminance histogram and mean, read back two frames later | `exposure clear`, `exposure histogram`, `exposure average`, `read exposure` | `exposure.rs` |
| Depth of field | Near and far fields gathered at half size | `depth of field downsample`, `… tiles`, `… dilate`, `… gather`, `depth of field` | `depth_of_field.rs` |
| Motion blur | McGuire et al.'s reconstruction filter | `motion blur velocity`, `… tiles`, `… neighbors`, `motion blur` (`view blurred`) | `motion_blur.rs` |
| Bloom and grade | Downsample chain, upsample, blend; grade in the same last pass | `bloom down 1`–`8`, `bloom up 1`–`7`, `bloom`, `bloom and grade` or `grade` (`bloom 1`–`8`) | `bloom.rs`, `grade.rs` |
| Lens | Chromatic aberration, vignette, grain | `lens` (`view lensed`) | `lens.rs` |
| Canvas | The game's UI, in the view's pixels, where a pass above follows the view's last | `canvas` | `lib.rs` |
| Show | An image, by its debug name, in place of the view's light | `show` (`view shown`) | `show.rs` |
| Picks | Reads the IDs at picked pixels | `read ids` | `pick.rs` |
| Tonemap | Into 8-bit sRGB, dithered | `tonemap` (`view`) | `tonemap.rs` |
| Outline | Row distances, then band and glow | `outline distances`, `outline` | `view.rs` |
| Frame | Shows the view under the UI; the other windows' UI | `frame`, `window` | `lib.rs`, `ui.rs` |

### The graph and the GPU

- The graph's own images last a frame and share memory where their uses don't overlap. Its buffers
  last from frame to frame by name, starting as zeros, as the culling's record of what was unhidden
  does.
- Compute-queue passes run there where the device has a compute family apart from graphics and
  uploads, as the RTX 4090 and AMD's GPUs do, else on the graphics queue, as on lavapipe. They use
  buffers alone, which every queue shares, so nothing changes queue family.
- Readbacks (picks, the culling's counts, the exposure's meter) come back as the frame two later
  begins, so no frame waits for the GPU.
- Every Vulkan object has a debug name and every pass a label; runs of draws are labeled with their
  shader's name. Per-pass timestamps feed Tracy's GPU timelines and `engine.perf`'s `gpu_ms_mean`,
  `gpu_ms_p95` and `transient_bytes`.

### Drawing

- **Batching.** Each run of consecutive draws of one mesh with one shader, drawing the same faces,
  is one instanced draw. Nothing caps a run: the frame's arenas grow, rather than a fixed buffer
  splitting it.
- **Order.** Everything opaque draws first, in the view's two culled passes. Then what blends, back
  to front by its z in the world, by entity ID within a z; then lines, over all but nearer opaque
  meshes, as debugging wants; then the canvas, at the near plane, over everything. World z orders
  blending among itself rightly only for layers parallel to XY seen from in front, as 2D cameras
  see them; against what's opaque it's always right. Of two opaque draws at one depth the later
  goes over, except in the frame the earlier comes into view, which it draws in the second pass.
- **IDs every frame.** The view renders its IDs whether or not anything picks: a pipeline is made
  for its pass's targets, so leaving them out would take a second pipeline per shader, or
  `VK_EXT_dynamic_rendering_unused_attachments`. IDs don't blend, so blending shaders discard
  what's fully transparent. A pick answers about four frames later: the main thread runs a frame
  ahead of the render thread, and the GPU finishes two frames after that.
- **The scene.** Each entity with a `Transform` and an opaque `MeshDraw` is a record in a GPU
  table, written only as it changes, so 16 Bistros cost the CPU about what one does.
- **Culling.** Instances are culled by their spheres for the view and each cascade. The view's
  first pass draws what the last frame found unhidden; a depth pyramid built from it culls the
  rest for the second. What comes into view draws in the frame it does.
- **Meshlets.** Cooked opaque and masked meshes that aren't skinned draw by meshlets: through mesh
  shaders where the device has them, else as instances of a 124-triangle vertex draw, as on AMD's
  iGPU. A view's room for meshlets is capped, so runs of many instances, as of foliage, can't ask
  more memory than the GPU gives and end the renderer; a run without room draws by its indices
  that frame. What blends keeps its triangles' order, so it draws by indices too.
- **Visibility buffer.** The model shaders' meshlet draws write only which run, meshlet and
  triangle each pixel shows; a compute pass finds the triangle again and shades by the code the
  forward model shaders share. `Settings::visibility_buffer` off shades as they draw, for tests.

### Light

- Models shade by glTF's metallic-roughness BRDF; lights are KHR_lights_punctual's, binned into
  clusters. Each point shades by the lights that light everywhere, then its cluster's, in the
  frame's order, so the sum is what every light would add.
- One directional light casts shadows, the brightest that casts, in four cascades kept steady in
  the world as the view moves.
- A sky is an image or an atmosphere, prefiltered on the GPU only as it changes. A sky's sun lights
  once: with a directional light standing for it, the sky's light leaves out the sun's disk. That
  light should shine from where the sky shows its sun.
- The atmosphere is Hillaire's. Its shaders work in kilometers above the ground, not radii, at
  which f32s are half a meter apart, so every GPU finds what's near the ground alike. The sun's
  light reaching the world is dimmed on the CPU, once a frame. A set sun casts no shadows; the
  next caster takes over.
- Height fog: a pass fogs what's opaque, sky and clear color among it, from depth. What blends
  fogs itself as it draws, since a pass over the colors would fog it as far away as what's behind
  it. A game's blending shader draws unfogged unless it fogs itself, as `shaders/fog.slang` does.
- Ambient occlusion and reflections need opaque depth before anything shades. Without a visibility
  buffer, the view's two passes draw depth alone and a third draws everything shaded, since a
  game's opaque shader may discard and two pipelines aren't bound to place a vertex alike. So
  forward opaque vertices draw twice, and the model shaders' a third time for reflections' normals.
- Reflections are traced before shading, from depth, so the visibility buffer's shading and forward
  shaders take them alike, by one sample. A composite after shading would need normal, roughness
  and specular weight in images that every pipeline, games' too, would have to draw. Rays sampled
  from GGX by roughness would need roughness before the trace, and a denoiser.

### Antialiasing and post-processing

- Order: antialiasing, the copy for reflections, the meter, depth of field, motion blur (both before
  bloom, as Unreal orders them), bloom and grade, lens, canvas, tonemap. All come before the canvas,
  so a game's UI is neither antialiased, blurred, bloomed, graded nor seen through the lens.
- TAA resolves before tonemapping, and the canvas draws after the resolve, so UI stays sharp and
  out of the history. Particles draw before it, so they sort by z among what blends; they write no
  motion, and the resolve's clip keeps them from trailing. The IDs are jittered too, so a pick at
  an edge can move a pixel.
- The exposure adapts on the CPU, from a meter read back two frames later, by the simulation's
  time, so the same frames show the same and a paused game's exposure holds. A tool's look adapts
  apart, so looking round a dark room changes neither the game's view nor its captures. The meter
  takes what isn't lit as lit ([DEFECTS.md](docs/DEFECTS.md)).
- The view can show a part of shading, or a graph image by its debug name, in place of its light
  ([`show.rs`](crates/gg-renderer/src/show.rs)), as the tool API's look, captures and the editor's
  toolbar ask.

### Shaders and assets

- Shaders are Slang, compiled by slangc as a layer adds them, so a bad one fails there. All compile
  unoptimized: drivers optimize anyway, and slangc's optimizer reads a struct through an address a
  field at a time, which AMD's driver miscompiles.
- Shaders pull vertices through addresses, and Slang lays out a pointer's target as `#[repr(C)]`
  does. Vulkan checks none of this, so the engine checks slangc's SPIR-V against `Shaders::add`'s
  contract (`renderer.shader_mismatch`): SPIR-V, because Slang's reflection names what a pointer
  points at but none of its fields. The frame's data only grows at its end, so games' shaders that
  read a prefix keep to the contract.
- glTF is read over serde_json, not the `gltf` crate, whose public calls panic on some malformed
  files.
- Meshes and textures stream within 16 MiB a frame, through a transfer queue where there's one, so
  no frame waits for copies; a texture shows a stand-in until then. A bindless set per frame in
  flight lets a slot change without waiting. Past the driver's memory budget an asset waits, but
  the oldest goes ahead alone, so a game comes up on a GPU others have filled.
- Hot reload ([`hot_reload.rs`](crates/gg-engine/src/hot_reload.rs)) swaps an asset under its
  handle: it draws as it was until the new bytes stream, and a change that fails warns.
- A rendering error ends the renderer, since a failed frame can leave copies half done. A reloaded
  shader whose pipeline can't be made only warns.

### State outside the world

What only drawing needs never enters the world: no hash, scene, snapshot or recording holds it, so
lockstep peers and replays agree whatever each draws or chooses.

- **Between ticks.** Frames draw the world between ticks, from a copy of where entities were as the
  latest tick started ([`interpolation.rs`](crates/gg-engine/src/interpolation.rs)). A child
  blends its own and its ancestors' transforms, then composes them, so it keeps to its orbit; the
  order is the tick's, so drawing never sorts again. Drawing runs up to a tick behind the
  simulation, besides the render thread's frame. Jumps call `World::reset_interpolation`. Headless
  frames, a tick each, draw each tick as it ends.
- **Viewpoints.** Where the view sees from, the primary camera between ticks or a layer's
  `Context::set_viewpoint`, moves every frame, while the world changes only at ticks.
- **Particles.** The world holds emitters, which its hash reads; particles are the frames'. Each is
  a birth record whose age gives its motion and look in closed form, its randomness a hash of its
  emitter's ID and number. So particles draw alike at any frame rate, stop while paused, and a
  rollback that runs a tick again bursts once.
- **A player's choices.** `Context::set_antialiasing` overrides the environment for the run alone:
  the environment is the world's, so a choice written into it would part lockstep peers.

## Audio

Sounds and music are kira's, mixed on cpal's audio thread, as the simulation's state never is: what
plays follows from the world, and never feeds it.

- **Ticks queue sounds in the world** (`World::play_sound`), which the engine plays and empties
  once each tick has run. So no snapshot, hash, recording or scene holds one, and each lockstep peer
  plays what its own ticks queue. Layers play their UI's sounds as they update
  (`Context::play_sound`).
- **Sounds decode whole as they load; music streams**, held in memory as compressed as its file
  is, so a long piece takes little memory and a play never reads the disk. Layers play one music at
  a time. Ticks play none: a layer that reads the world as it updates plays what the world calls
  for.
- **The decoder is the engine's, over symphonia**: kira's would hold all a crafted file decodes to,
  and leaves the codec as it was across a loop's seek.
- **What plays mixes through three buses**, the music's, the sounds' and a main one over both,
  whose volumes layers set, as a game's settings would, or a menu that ducks the music.
- **A sound at a point or on an entity is panned and faded as the game's view hears it**: by where
  it shows in a view without perspective, and by its direction and distance from a camera with
  one. The engine does this for each play itself, as [audio.rs](crates/gg-engine/src/audio.rs)
  says, rather than through kira's spatial tracks.
- **Windowed and embedded runs play sound**, the editor's game among them, opening the system's
  default output as their first sound or music loads. Headless runs, replays and tests play none.
  What plays follows how the run plays, as the tool API pauses and stops it.

## Threads

The main thread runs the OS event loop, which macOS requires, and the layers and their ticks. A
tick's systems hand rows to the job system, rayon-core's pool of `--threads` threads with the main
thread among them, and wait, so the world changes only within a system's call. A job changes only
its own rows, work splits at a fixed number of rows, and no thread changes its floating-point
environment, so results don't depend on how many threads there are or which takes what.

Rendering runs on a thread of its own, `gg-render`, a frame behind: the main thread hands the
renderer over with each frame's render data and takes it back for the next. So the main thread is
at most a frame ahead, and the GPU works on up to two frames behind that. Windows and macOS give
out a window's handle only on its own thread, and macOS closes a window there, so the main thread
makes and drops the surfaces, and the render thread never holds a window's last reference. A
frame-rate cap (`Context::set_max_fps`) sleeps before each frame, since Windows doesn't run the
event loop while a window is moved or resized. Images decode on threads of their own, `gg-load`, so
neither a layer's add nor a tick that names an asset waits for a decode.

## Platforms

- **Vulkan 1.4**, with a few features Vulkan doesn't require but every target has
  ([candidate.rs](crates/gg-gpu/src/device/candidate.rs)). Newer features are taken up as drivers
  ship them, with a fallback wherever a target lacks one.
- **Windows and Linux:** native Vulkan drivers.
- **macOS: KosmicKrisp,** LunarG's Vulkan 1.4 driver on Metal, for Apple Silicon and macOS 26 or
  later; not MoltenVK, which Khronos deprecated. It has no mesh shaders or ray tracing, so those
  stay optional paths. macOS has no Vulkan loader, so a shipped game is a bundle holding the SDK's
  loader and KosmicKrisp ([Builds](#builds)), signed ad hoc, as Apple Silicon runs nothing unsigned.
- **x86-64 and ARM64:** built and tested on both from the start; macOS is ARM64 only.
- **No GPU:** lavapipe, Mesa's CPU Vulkan driver, runs the GPU tests headless.

## Technology

The root [Cargo.toml](Cargo.toml) lists the crates used, with the reasons for their features and
pins.

| Area | Choice |
|---|---|
| Game code | Rust; Luau through `mlua`, in every build, for mods and quick scripts |
| GPU API | Vulkan through `ash`: one backend, no multi-API abstraction layer |
| GPU memory | VMA, through our own binding: budgets, aliasing, pools and defragmentation |
| UI | egui, in development builds, drawn by `gg-renderer`, with a dock of our own; text in Inter |
| Project state | RON in a canonical form ([Project state](#project-state)) |
| ECS, navigation | Our own ([ECS](#ecs), [Navigation](#navigation)) |
| Agent access | MCP and a command-line tool, both clients of the [tool API](#agents-and-tools) |
| Shaders | Slang, compiled to SPIR-V by the Vulkan SDK's `slangc`, not linked in |
| Assets | A package of our own format, read at offsets; glTF 2.0 in, through our own reader |
| Networking | Our own, over std's TCP and UDP sockets, sealed by Noise's `NNpsk0` through `snow` |

The license is `MIT OR Apache-2.0`, at the user's choice, as most Rust crates are
([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)); contributions come under the same
terms. A file beside a license of its own keeps it, as the fonts' OFL and the fox's CC BY 4.0. A
game links the engine into its own executable, so a copyleft license such as the GPL would bind
every game made with the engine, console games too, whose SDKs can't be opened.

Areas not listed here are undecided.
