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
  buffers, images, descriptors, pipelines, swapchain, synchronization and submission. They stay
  safe whatever safe code does with them: each checks that what it's given was made by its device,
  or a surface by its instance, and a frame or a pass forgotten rather than dropped is caught at
  the next call.
- **`gg-renderer`:** the render graph, passes, materials, lighting and post-processing. It owns
  the GPU resources and hands out handles to them.
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
  game as an agent does, under `gg run --embedded`, which ends as the editor does, the game
  starting stopped (`--stopped`), its world the world as edited; and shows the game's view; the
  game's world in its Entities and Inspector panels, a remote Inspector's, which edit it through
  the API as the editor's own edits, as the handles in the view do, a click picking what the view
  shows under the cursor through the API; its camera, the pick's outline, its grid and the
  colliders' outlines going to the game as the API's look, which it takes up from other clients
  too; the game's events in its Logs as the game logs them; and the project's assets in its Assets
  panel, an ID file beside its asset hidden, from which a texture dropped on the view sets the
  texture of the sprite there, and a model is a root where the cursor shows the grid's plane,
  each an edit of the editor's naming the asset by its ID. Its Play and Simulate are the API's
  `play`, and Stop its `stop`, which brings back the world as edited. Its File menu's New, Open
  and Save are the API's `scene`, on the game's open scene, which its title bar names, marked
  while unsaved; and since closing it ends the game, it asks before New, Open or closing loses
  unsaved edits. Play sees through the game's camera, which gets all of the view's input. For a
  game whose ticks are all its own code, Simulate plays the game as Play does, but through the
  editor's orbit camera, the game getting no input, as Unreal's Simulate In Editor does; switching
  between them carries on, moving the camera and the input over, as Unreal's Possess and Eject do.
  The toolbar's camera switch is that switch while the game plays; stopped, it shows what the
  game's camera sees, the input staying the editor's, until the editor's camera takes some. A Stop
  turns the view to the editor's camera, which carries on from where the view was, as it does
  whenever it takes the view over. The editor has no world of its own: started elsewhere, it runs
  no game, and says it needs a project. [game.rs](apps/editor/src/game.rs) describes the rest.
- **`player`:** the reference player. It runs a cooked scene without game code and shows how a
  game builds its own player.
- **`tools/gg`:** the command-line tool, a client of the tool API, which `cargo gg` runs.

Games depend on `gg-engine` the same way the apps do; see [Game code](#game-code). The first, in
`games/shards`, is a demo of two players, alone or networked in lockstep, as
[Networking](#networking) describes, which the v0.1 loop is tested on. Executables are
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
checks `gg-engine` as they build it. A gate of its own, `cargo clippy -p gg-engine --all-targets`,
checks `gg-engine`'s tests without `dev` too, which no other command builds: kept apart from the
shipping gate, as their dev-dependencies' features would merge into the player's build.

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

- `gg-assets`: cooked asset formats, shared by the cooker and the runtime.
- `tools/cook`: the offline asset cooker.
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
  matching each changed component to its new layout by field name.
- **Luau comes after v0.1, for mods and quick scripts.** Players can install sandboxed mods,
  and quests, dialogue and level events reload in milliseconds. Scripts reach Rust components
  through reflection, without hand-written bindings, and can declare components of their own.
  Luau doesn't guard against crafted bytecode, so mods ship as source and only a game's own
  packages hold bytecode.

## Agents and tools

Agents work on a game the way people do, through the same interfaces: nothing a person can do
in the editor is out of an agent's reach.

- **The tool API.** A game's development build serves an API on the machine's loopback, at a port
  the OS picks, which it writes with a token, and its project's manifest's path, in a session file
  in `sessions` beside its executable, removed as the run ends: query the world, apply and undo
  [operations](#project-state), play and stop, pause and step, open, save and empty the [open
  scene](#project-state), capture frames, share the view as frames render it, set how it looks and
  pick what it shows, list the assets in the asset root with their [IDs](#project-state),
  hand the layers input as the window would, save the world whole as a snapshot file, as [ECS](#ecs)
  describes, which a run given `--restore` starts from, apply patches of the game's code, as
  [Game code](#game-code) describes, and read [diagnostics](#diagnostics). Requests
  and their answers are lines of JSON, as [gg-api](crates/gg-api/src/lib.rs) describes, and a
  connection's first request says hello with the token, so that only processes that can read the
  file reach the game. A thread of the API's reads each connection and answers its pings itself,
  from the frames the main loop counts, so that a tool tells a hung game from a busy one, and from
  one whose frames have paused for the user, as for a minimized window or a file dialog; and its
  reads of the log, from the events the run keeps in memory as it logs them, its last 10,000, each
  numbered, which `logs` serves as they come, waiting for one if none has, so that a client follows
  them from the number it left off at, and, by the run's own number, which a restart changes, misses
  none across a restart that the new run kept. The main loop answers the rest as each frame starts,
  before its ticks: a step once its ticks have run, a capture once the render thread has read the
  frame's view back, without the UI, which a thread of the API's writes as a PNG, and a snapshot
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
  frames too. A game run `--embedded` renders its view at the size the client asks, and runs each
  frame once the client has taken the last, rather than rendering frames it never takes. The view
  has one look, as the engine renders one view, which every client sets part by part, the last to
  set a part winning: the viewpoint it sees from, in place of the game's own; the entity it
  outlines; a grid on one of the world's axis planes; and whether the colliders' outlines show. A
  part given back is the game's own again. A look lasts until a client changes it or the run ends,
  so a restarted run starts without one; it says which client set each part, so that a client takes
  up what another set, as the editor's camera carries on from a viewpoint an agent set; and each
  frame shared carries the viewpoint it was drawn from and the revision of the look it drew with, so
  that what a client draws over a frame matches it. A capture shows the view as it's shown, the look
  among it, or, asked to, as the game shows it on its own, from a bare frame, which draws without
  the look, isn't shared and leaves the picks asked to the next frame, so that none reads a view a
  client isn't shown. A pick answers which entity the view shows at a pixel, and its name, once the
  frame that read it comes back. Each client gets only the capabilities it is granted, such as
  reading the world or editing one scene.
- **Clients.** The command-line tool, `tools/gg`, builds and runs games, supervising them: it
  runs a copy of a game's executable, so that the game builds again meanwhile, removing those runs
  it killed left; patches it as its code changes, and rebuilds it at any other change, as [Game
  code](#game-code) describes; snapshots it every few seconds from its first frame; and starts it
  again from its last snapshot after a crash or a hang, and from one taken then as it's rebuilt,
  telling of each, or waits for a rebuild if the game would only end the same way again, as one
  that ends before its first snapshot would. A game started again so comes back stopped if it was,
  or playing a copy with the world as edited held, as its snapshot keeps both. A game that comes
  to its own end, as its window closes, ends the supervision, as one that never serves does; so does
  the end of the tool a run embeds the game in (`--embedded`), as the editor ends, which closes gg's
  stdin. One `gg run` runs a game at a time; a second is refused. It runs headless checks, talks to
  running games, all with machine-readable results, and makes new games. Inside a game's
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
  data, so a build without `dev` reads none.
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
  that reflection's descriptions drive, never its untyped values. A texture, a model or a font is
  saved as its name, its asset ID or else its path relative to the app's root (`App::root`), as
  Asset IDs says below, which a load finds it by, as it finds it by a path to its file, as a scene
  written by hand may name it, loading it where no layer has; what's opaque, such as a `MeshDraw`,
  isn't saved: a load builds each component on its type's `Component::base`, which gives it that,
  or from the scene alone. A field a scene leaves out, as one saved before its type had the field
  does, takes its default, and saves again whole. A child's `Parent` is saved as its parent's ID,
  and a root's isn't. A save makes each component again as a load would, so that every scene saved
  loads; a load checks everything before it spawns anything, all or nothing, and a refusal begins
  with the line and column where the text is refused, and, but for a syntax error, names the
  entity, the component and the field.
- **Prefabs.** A prefab is a scene of one subtree, placed as a copy under a parent. Each of the
  copy's IDs is mixed from one drawn for the copy and the prefab's own, `mix64(local ^
  mix64(instance ^ PLACE))`, a permutation of the prefab's IDs for each copy, fixed for good, so
  that a copy's IDs are as distinct as its prefab's, and two copies' meet only by chance;
  references inside the prefab are to the copy's own entities. A scene saves the copies as
  entities like any others.
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
  changed, and the others are spawned again, under the same IDs. Overrides of a node's fields, as
  linked prefabs will keep them, wait for those.
- **Stable IDs.** Every entity has a persistent ID, separate from its runtime handle, that
  files, operations, diagnostics and network messages use to name it. IDs are random rather
  than sequential, so entities added on two branches neither collide nor land on the same
  lines.
- **Asset IDs.** Each texture (`.png`, `.jpg`, `.jpeg`), model (`.gltf`, `.glb`) and font (`.ttf`,
  `.otf`) under the asset root, whatever its extension's case, has an ID that outlives its file's
  name: 64 random bits, never 0, written as 16 lowercase hex digits, as an entity's are, in an ID
  file beside it, its file's name and `.meta`, such as `stone.png.meta`, matched by that exact
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
    texture, a model or a font by its name: its asset ID, or a model's image by its model's and its
    place in it, such as `5f3c9a1e07b2d846#0`, so that a file renamed with its ID file changes none
    of them; or else its path, as for an asset outside the root, or whose ID file is missing in a
    run that writes none. Each is read by a name or by a path to the asset's file. An asset is
    named as it's added, by the ID its run's database holds for its file then, and keeps that name
    for the run, but in a replay or a lockstep joiner, which go by the names they're given, as
    below.
  - A texture, a model or a font that a scene, the tool API's edit or `scene open`, or a recorded
    or a lockstep peer's command or edit names, and no layer has added, loads there, before the
    text is read, as `Context::add_texture`, `add_model` and `add_font` load a file, and is named as
    they name one, its file found by the name: in a run that follows another, a replay or a
    lockstep joiner, at the path the name was given; else by an ID, as the run's database has it,
    which a joiner has but a replay, reading no ID file, doesn't; else by a path from the asset
    root, which mustn't be absolute or lead outside it, as through `..` or a link. A model's image
    loads with its model, which a name of the image loads. A name that doesn't load refuses what
    names it where it's asked for (`asset.no_texture`, `asset.no_model` or `asset.no_font`, the
    API's `api.no_texture`, `api.no_model` or `api.no_font`), saying why: `asset.model_image` for
    an image the model hasn't, `asset.unknown_id`, `asset.outside_root`, or the add's own, as
    `engine.bad_texture`. In a run that follows another, and in a restore, a stand-in takes its
    place instead, of magenta and black squares for a texture, of no nodes for a model, and for a
    font one whose text draws in the engine's own, named as named, warning `asset.stand_in`, so
    that the run goes as the other did, its hash reading names alone, but for a model's nodes,
    which its roots spawn none of then. What's loaded so is noted among what the layers added, so
    that a snapshot's `asset` lines list it, and a run restored from one loads each texture, model
    and font its run had, in order, before it reads the world. A lockstep host loads what a scene
    file's save names before it offers it, so that their names go out first, and offers none
    naming an asset it can't load; a joiner loads for its tool API only what its host named
    (`asset.not_given`). Scene text that a layer loads into a world itself, as inside a tick, finds
    only what's loaded. Each decodes, or bakes, on the main thread as it's named, as a layer's add
    does.
  - Builds without `dev`, which read no ID file, name every asset by its path, so that a scene, a
    recording or a snapshot naming one by its ID finds none there; cooked data will give them IDs.
  - A run's names travel with what another process reads of it, so that the other goes as it did
    whatever ID files its disk holds, as a checkout without those not yet committed. A recording
    has a `named` line for each texture, model and font named otherwise than by its path, the name
    and the path, by which its replay, reading no ID file, names its assets. A snapshot file has
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
  reparent. An operation names entities by their IDs and carries values as reflection's, and
  applying one checks everything before anything changes. A delete's and a remove's inverses keep
  what they took out, typed, so that what's opaque comes back too. A component can name a `bool`
  field of its own exclusive, as a camera's `primary` is, so that one edit, a person's or an
  agent's, switches which entity has it, and one undo switches it back. A log keeps the edits
  applied, in order, each its operations with their inverses: it undoes and redoes an edit whole,
  applies one of several operations all or nothing, merges a gesture's sets of one field, such as
  a drag's, into one edit, a tool API client's too, by the number it gives each of its gestures,
  and drops an entry whose undo the world refuses. A running game applies the tool API's edits
  between ticks, as a frame starts, paused or not, in the order they arrive, so several clients
  can edit it at once; each author, a client by the name it says hello with or a scene file by
  its path, undoes and redoes its own through a log of its own, as the Inspector does through its,
  and a recording keeps each edit as it was asked for, before the tick after, where a replay
  applies it. Editing a scene file while the game runs is an edit too: one a game loaded through
  `Context::load_scene_file`, saved, applies in a development build as the operations that take
  the world's entities from what the file held to what it holds, which keep what the file doesn't
  change, such as where the game has moved an entity, and leave alone what the game has deleted;
  the entities it adds are made first, so that the rest may hold them. A snapshot keeps the files
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
a texture as its handle, which the renderer alone makes, so that no value makes an index that
shaders sample unchecked, and which scenes, snapshots, recordings and the tool API name by its name,
its asset ID or else its path, as [Project state](#project-state) says; and `Opaque` for what
reflection keeps but doesn't read, such as a `MeshDraw`, which no value may
make. The world reads and sets a field by its component's name and a path, such as
`rect[1].y`: a set checks the whole value against the description before anything changes, and
returns the old value, which sets it back unless code stored one that a set refuses. The Inspector
reads the world through these calls, its IDs and its hierarchy, and edits it through operations,
all of which the tool API serves, so that a remote Inspector shows a game's world in another
process, as the editor's does: it reads the game's entities and the picked one's components
through the API a few times a second, and the descriptions of its types once, and again as a type
it hasn't comes, with one request at a time, so that what it reads after an edit's answer shows the
edit, and edits the world as the game's client, drawing what waits for an answer as it will be.
Values cross as JSON, read and written by the types the game describes, an entity's by its ID and
a texture's by its name.

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

A snapshot file holds a world whole as text, so that a run restored from it, in another process,
after a crash or a rebuild, goes on where the world was: each slot under its generation, the free
ones in their order; each table in the order it was made, empty ones too, and its rows in theirs,
each with its entity's ID and its components' values, as a recording's text writes values, by their
fields' names; each parent's children in their order; the stream of IDs; the resources that save
themselves; the ticks the simulation had run; the assets the layers had added, the textures loaded
where text named them among them, which a run restored from it loads again before its world, and
the names its textures had, as [Project state](#project-state) says; and, of a game the tool API
stopped, that it's stopped, or of one playing a copy, the world as edited too, which it held aside,
with its ticks, its hash and its scene files. Loaded, it's the world saved, hash for hash, but for
the slots a log kept vacant, which no log in the new process gives back, so that they're freed. A
component whose type changed since is matched to the type as it is by its fields' names: a field
added takes its default, one dropped goes, a number of another kind is taken where it's exactly a
number of the field's, and a component that no longer reads, or whose type is gone, is left out,
its entity going on without it, each change warned of once, by the type's path rather than by
value (`snapshot.changed`); a rotation that rounding took off unit length is normalized. What's
opaque, such as a `MeshDraw`'s mesh and shader, holds indices, so it comes back only into a run
whose layers added the same assets in the same order, a file added again counting again, and the
textures loaded where named among them, of the same types, as the file's `asset` lines check, a
draw's bytes by its mesh's and shader's types' names and sizes, and is left out otherwise. A world no load could read, such as a value nested
deeper than a scene's may, isn't saved. [ecs/file.rs](crates/gg-engine/src/ecs/file.rs) and
[snapshot.rs](crates/gg-engine/src/snapshot.rs) have the rest.

A world holds resources beside its entities, a value of each type, for the rest of the simulation,
such as 2D physics' bodies and contacts. Snapshots copy them, so that a restored world steps as the
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

The engine supplies what each multiplayer model needs, the first of them deterministic lockstep
between two peers, on one machine or a LAN; rollback and a server that owns the world come later,
from the same snapshots, commands and hashes. A run given `--host <address>` listens there, and one
given `--join <address>` connects to it; the host is player 0 and the joiner player 1, as
`Context::player` tells a layer, which queues commands for its own player alone, naming it in
them, since each peer runs both players' world.

- **A tick runs on its inputs alone.** A tick's inputs are each layer's commands, the host's then
  the joiner's, and the edits from outside the layers that apply before it, as a recording keeps
  them: so a peer runs a tick only once it has the other's inputs for it, and frames go on drawing
  while it waits. Each tick, a peer takes what its layers queued since the last and sends it as its
  inputs for the tick [`DELAY`](crates/gg-engine/src/net.rs) ticks on, so that the inputs reach the
  other peer before it needs them; a frame that ticks drains the socket first. Inputs travel as
  text, the values written as recordings write them, entities by their IDs and textures by their
  names, which the joiner takes from its host, as [Project state](#project-state) says, read into
  values only at their tick, against the world then.
- **Edits and patches are inputs too.** The tool API's edits, on either peer, go out with the
  peer's inputs for the tick `DELAY` on, and apply on both before it, so that a request's answer
  comes once its edit has applied. A scene file's save is an edit of the file's text, which each
  peer turns into operations against its own world at that tick, as both worlds are the same; the
  host alone watches scene files. `gg run` gives both peers each patch, in the same order, and the
  host chooses the tick it applies before, `DELAY` on from its next as it gets it, which neither
  peer has run; it tells its joiner, which waits at that tick for its own patch if it hasn't it yet.
- **A joiner starts from the host's world.** As a joiner connects, the host sends its world as a
  snapshot file's text, as it stands between two ticks, with how many patches its code is on, and
  its inputs for the ticks after, up to the first the joiner can reach; the joiner restores it in
  place of its own, as a run restored from a snapshot does, applies the patches the host had, and
  goes on from that tick, its own inputs empty until the first it can reach. So a restart of
  either peer, after a crash or a rebuild, rejoins: a joiner that loses its host waits for it,
  frozen, and connects again, while a host that loses its joiner plays on alone, the joiner's
  player still.
- **Desyncs name their tick.** Each peer sends the world's hash after each of its ticks; a hash
  that isn't the other's logs `net.desync` with the tick, and the host sends its world again, as to
  a joiner, so that the peers carry on in sync.

Messages are lines of text over TCP, without Nagle's delay, which keeps them in order and whole;
[net.rs](crates/gg-engine/src/net.rs) has their format and the rest. `gg run --peers 2` runs a host
and a joiner from one build, patching both and restarting both after a rebuild.

## Diagnostics

Diagnostics are data, so agents and tools can act on them as reliably as people read them.

- **Logs** go through `tracing` with structured fields, filtered by `RUST_LOG`. Each run prints
  readable, colored lines to stderr, which keeps stdout for machine-readable output, and writes
  every event as a JSON line to a file that agents and tools read:
  `logs/<app>-<UTC time>-<process ID>.jsonl` beside the executable, whose path the run logs as it
  starts; and keeps the last of those lines in memory, which the tool API's `logs` serves as they
  come, as `gg logs --follow` and the editor's Logs follow them. A run embedded in a tool prints no
  readable lines, since the tool shows its events, which it follows through the API.
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
run no pass, egui sees no events, and the game's view fills the window.

egui, the engine's immediate-mode UI, is the top overlay: it sees every event first and keeps those
it wants, such as a click on one of its panels, but no key's or button's release, and `gg-renderer`
draws its meshes over the frame.

egui is the development UI. A game's own, its HUDs and menus, is immediate too, but apart from it:
layers lay it out as they update, each frame, on the view's canvas, `Context::canvas`, as text and
sprites placed from an anchor, one of the view's edges, its corners or its middle, which the engine
places them from as it hands the frame off, at the view's size then, so that a resize moves them in
the frame it comes in. The canvas scales with the view, `CANVAS_HEIGHT`, 1,080 of its units, to its
height, and draws into it over everything else there, so that captures and the editor's view show
it; it's of no entity, so a pick there finds none. A layer finds the cursor on the canvas as the
last frame showed it, to tell what it's over; the engine has no widgets, so a game draws its buttons
itself. Its state, such as which button has the focus, is the layer's, so that each lockstep peer
has its own, and a choice that changes the game queues a command, as input does; a Quit asks the
run to end through `Context::exit`, as closing the window does.

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
render data carries what's been added to them, or replaced in them, since, in order. After the
frame's ticks and the layers' updates, the engine draws the world's entities that have a `Transform`
and a sprite, a circle, a `Text` or a `MeshDraw`, a mesh with its shader and parameters held without
their types, after what the layers drew, children where their ancestors carry them: first the
meshes whose shaders are opaque, whose depth hides what's behind them whatever order they draw in,
by shader, then mesh, so that each mesh with each shader is one instanced draw, and their z's never
sort them; then what blends, back to front by their z's in the world, those at one z drawing
sprites, then circles, then text, under meshes; each batch and each z in the order of their IDs,
which overlapping entities keep as others come and go. World z orders what blends rightly as seen
from in front of layers parallel to XY, as 2D cameras see them; from behind or the side, what blends
draws in the wrong order among itself, though rightly against what's opaque. At one depth, an opaque
mesh goes under what blends; a layer's opaque draws hide the world's behind them, and its draws
that blend go under the world's opaque ones behind them.

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
set, and draws into the view's image once the view's pass has drawn it, so that captures and the
view shared show it too, round whatever shape the entity shows, its colliders' outlines among it,
going across what's in front of it rather than round what's hidden.

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

In a development run that serves the tool API, a thread of its own, `gg-watch`, watches the files
each game's shader, texture, model and font read as it was loaded, a shader's own and those it
includes as slangc lists them, a texture's image, a model's glTF file and the buffers and images it
names, and a font's file, by their times and lengths every tenth of a second, and reads the asset
again as they change and then hold for a look, off the main thread: compiles the shader, decodes the
image, reads the model, bakes the font. As the next frame starts, the main thread puts what it read
in place of the asset, under the same handle, so that whatever draws it draws what's new from that
frame: a shader's old pipeline and a mesh's old buffers retire until the GPU is done with them, and
a texture's new image takes its slot in the bindless set once the GPU has finished the frames
before, which may sample the old, so the frame that takes it waits for them, a frame's time at
most. A texture takes an image of any size,
but one cut into a sprite sheet, whose cells a layer has where its size put them, keeps drawing as
it did rather than take one of another (`asset.sheet_resized`). A model takes each primitive's
vertices and indices and each image, which its entities draw at once, and its nodes and materials,
which its roots' nodes take, as [Models in scenes](#project-state) says, and other numbers of
primitives and images, those past what it had added, or given back the meshes and textures of
their places, which those it has no more keep for them; but in a run
that's recorded or in a lockstep session, whose world changes only as the run it goes with changed
it, a model takes its vertices, indices and images alone: its nodes and materials stay as they were
until the run starts again (`asset.model_changed`), and a model of other numbers of primitives or
images takes nothing. The engine adds its model shader for every alpha mode with a run's first
model, so that a material read again adds none. A font takes what it baked, its field's texels at
any size, which its text is laid out by from that frame. A texture's or a font's stand-in takes its
file's once the file comes, or the file of the asset whose ID it stands for, once one has it, but
never a file outside the asset root (`asset.outside_root`); and a texture, a model or a font whose
file under the root is renamed or moved, as the asset database finds it by the ID it has, or that
the database gives its file once it has one, is watched where it is, a rename alone reading nothing
again. A change that doesn't compile, decode, read or bake, or breaks a shader's contract, warns,
and the asset keeps drawing as it did. The engine's own shaders and font don't reload, nor do the
meshes layers make themselves.

Windowed runs keep a pipeline cache, which they load from the OS's per-user directory for caches as
the renderer starts and save as it ends, so that pipelines an earlier run made are made again in a
fraction of the time. Headless runs, replays and tests use none, as they keep no layout, so that
their startup times compare.

A game's texture is a PNG or a JPEG that the engine decodes as a layer adds it, or as text names it,
as [Project state](#project-state) says, to RGBA8 with colors sRGB-encoded and alpha not multiplied
into them, as images store them, whatever color profile or gamma the file holds, as glTF requires;
the next frame that renders copies it into GPU memory, where it stays, and shaders sample its colors
decoded to linear. The engine reads a texture, a model, a
scene, a layout or the pipeline cache only from a regular file, so that a FIFO or a device can
neither hang the read nor fill memory; `--replay` alone reads its recording from whatever it's
given, so that it can come through a pipe. A texture's handle is its index in the bindless set,
which a draw's parameters carry as it is and the shader samples by, with one of the engine's
samplers, which it picks by its index too. A model's textures have their full chains of mip levels,
which the CPU makes as the frame that copies them renders, until they're cooked. A layer that adds a
file again, by whatever form of its path, as a texture or a sprite sheet, a model, or a shader of
the same vertex and parameter types and blend, gets what the first add made, the file loaded once,
as the OS resolves `.`, `..`, links and, where names ignore it, case; a texture keeps the path it
was first added by, which its other paths find it by too, and its name, as [Project
state](#project-state) says. A path is as the layer wrote it, rather than as the disk spells it,
which a file system that ignores case keeps as an editor last saved it, so that every checkout of a
game names its files alike.

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
and a projection, through `Context::set_viewpoint`, as the engine's camera controllers do, steering
it itself or with one of them. A viewpoint isn't in the world: it moves every frame, by input, where
the world changes only at ticks, so no hash, scene or recording holds it.

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
before it.

Text is drawn from fonts' fields, which `Context::add_font` bakes as a TrueType or OpenType font
loads, or the engine as text names one, until Phase 10 cooks them: a texture of data, which shaders
read as stored rather than decoded from sRGB, holding a multi-channel signed distance field of each
glyph of the characters Windows-1252 prints that the font has, and of its `.notdef`, which draws in
place of any other character but whitespace and controls, 48 texels to an em, its distances
reaching 4 texels either side of each glyph's edge, and in its alpha the true distance, which
outlines and shadows will read. The median of a texel's three distances keeps a glyph's corners
sharp at any size, where a single distance would round them off. fdsm generates each glyph's field
from the outline skrifa reads, unhinted, on the job system's threads, a fixed seed coloring its
edges so that a font bakes the same field every time, and corrects its errors. `Context::draw_text`
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
eye, which keeps the grid's precision there. It draws in the layers' order, over what's drawn
before it; the tool API's look's grid draws after the layers' draws and before the world's, so over
a backdrop a layer draws, as Shards' floor, and under the world's entities.

3D games draw models: `Context::add_model` reads a glTF 2.0 file and adds its meshes and its images,
as textures, once, all of itself or nothing, or the engine does as text names it, and a root with a
`ModelRoot` of it, as `Model::spawn` makes one and a scene loads one, has its default scene spawned
under it as entities, as often as the game likes, as [Models in scenes](#project-state) says: a node
each, with its transform in its parent's space, and a child for each primitive of a mesh with
several, since an entity holds one `MeshDraw`. The engine reads glTF itself, over serde_json,
rather than through the `gltf` crate, whose public calls panic on some malformed files. A model's
URIs can't name a file outside its folder, though a link inside the folder is followed wherever it
leads.

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

## Audio

Sounds and music are kira's, mixed on cpal's audio thread, as the simulation's state never is: what
plays follows from the world, and never feeds it. A layer loads a sound from a WAV, Ogg Vorbis, FLAC
or MP3 file, which symphonia decodes whole into memory, as a game's short sounds are. A tick queues
a sound with `World::play_sound`, from nowhere, at a point or on an entity, in a resource of the
world that the engine empties once each tick has run, playing what it held, so that no snapshot,
hash, recording or scene holds one, and each lockstep peer plays what its own ticks queue. Layers
play their UI's sounds as they update, with `Context::play_sound`.

Music streams: a layer loads it from the same formats, read whole into memory as compressed as its
file is and decoded as it plays, on a thread of kira's for each play, so that a long piece takes
little memory and a play never reads the disk, as a cooked package's audio, mapped into memory,
won't. Layers play one at a time, with `Context::play_music`, looping from its start, the music
before it fading out as it fades in. Playing the music playing goes on as it was, so that a layer
can say each frame which plays; ticks play none, since a layer that reads the world as it updates
plays what the world calls for. The decoder is the engine's, over symphonia, as kira's leaves the
codec as it was across a seek, so that a loop's start would run on from the packets before it.

What plays mixes through three buses: the music's, the sounds', and the main one over both, whose
volumes layers set with `Context::set_volume`, gliding there over a fade, as a game's settings
would, and as a menu ducks the music under it.

A sound at a point or on an entity is panned and faded by where it shows in the game's view, the
listener, as the frame handed off sees it: toward the side of the view it's on, halfway there at
the view's edge, as kira pans at constant power, all of it in one ear at a side, and as loud as it
is inside the view, fading out to silence a view's width or height past its edges; a camera with
perspective hears nothing behind it. One on an entity follows it, each frame, to where the latest
tick left it, up to a tick ahead of where it draws, too little to hear; once the entity is
despawned, it plays to its end from where the entity was last, as an impact's does when what made
it is gone.

Windowed and embedded runs play sound, the editor's game among them, opening the system's default
output device as their first sound or music loads, so that a game without either opens none;
headless runs, replays and tests play none. In development runs, music reloads as its file
changes, its play crossfading to what the file holds now from where it was, so that an edit is heard
at once, as a shader's is seen.

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
| Hot patching | `subsecond`, its patches built by Dioxus's `dx` of the same release, which `gg run` checks |
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
| Text | TrueType and OpenType fonts, whose outlines `skrifa` reads and `fdsm` bakes into multi-channel signed distance fields, as fonts load until they're cooked; text shaped by `harfrust`; `skrifa` and `harfrust` at the releases egui's own text takes |
| Audio | `kira` on `cpal`, its sounds and music decoded by `symphonia`, music through a decoder of the engine's: tracks as buses, panning and streaming, without allocating on the audio thread |

Areas not listed here are undecided.
