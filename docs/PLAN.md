# Plan

The roadmap. [AGENTS.md](../AGENTS.md) says how to keep it, under Workflow; the design is in
[ARCHITECTURE.md](../ARCHITECTURE.md).

## Done

- **Phase 0, workspace** (2026-10-20, the repository's first commit): the crates and apps
  build, the gates pass, and the unsafe policy is checked.
- **Phase 1, entry point, logging and Vulkan bring-up** (2026-10-20): on Windows the sandbox
  clears the screen, windowed and headless, with validation reporting nothing; CI runs the
  gates on all six platforms, with the GPU tests on lavapipe on Linux.
- **Phase 2, events, layers and the UI overlay** (2026-10-20): the engine's own events, a layer
  stack with overlays, and an egui panel over the frame, drawn with VMA memory, bindless
  descriptors, vertex pulling and a Slang pipeline; a GPU test checks the UI against a
  reference image.
- **Docking and viewports** (2026-10-21): layers show panels, which dock in tabs and splits
  around the game's central area, float, or tear off into windows of their own; the layout is
  saved per user; one frame renders every window, paced by the main one.
- **Title bars the UI draws** (2026-10-21): with `App::custom_title_bar`, the UI draws the
  windows' frames; the main window's title bar holds the menus layers add, and a torn-off
  window's tab bar is its title bar. The sandbox opts in, with a File menu that quits.
- **Windows' own frame around the UI's title bars** (2026-10-21): on Windows, those windows keep
  DWM's border and shadow and the bands just outside them that resize them; the main window's
  title bar moves it, maximizes it and opens the system menu as the OS's own does, and Snap
  Layouts shows over its maximize button.
- **First triangle** (2026-10-21): layers add meshes and draw them in the game's view, the dock's
  central area; their vertices and indices live in GPU memory, copied there by the next frame
  after they're added, and a Slang shader reads the vertices through their address. The sandbox
  draws a triangle in red, green and blue, and a GPU test checks one against a reference image.
- **Shaders and vertex types** (2026-10-21): layers bring their own Slang shaders, which the
  engine compiles with slangc as they're added, and meshes of their own `#[repr(C)]` vertex
  types; a shader is added for a vertex type, and each draw pairs a mesh with a shader for its
  own. The sandbox draws a flat blue square behind its triangle, with two shaders and two vertex
  types, and a GPU test checks the pair against a reference image.
- **Cameras and transforms** (2026-10-21): layers set the camera the game's view sees the world
  from, orthographic and keeping the view's aspect ratio, and draw each mesh with a transform, in
  a world that is right-handed with +Y up, as glTF's is. The sandbox draws a grid of squares and
  a turning triangle, and moves and turns its camera with W, A, S, D, Q and E, by the frame time;
  a unit test checks the camera's matrix, and a GPU test a camera and transforms against a
  reference image.
- **Shader parameters** (2026-10-21): a shader is added for a vertex type and a type of
  parameters, any `#[repr(C)]` type, which each draw gives it; the renderer copies them into the
  frame, and the shader reads them through their address. glam's vectors and matrices can be the
  fields of vertices and parameters. The sandbox's grid alternates two colors, picked in a panel,
  and a GPU test gives two squares colors of their own.
- **Textures and blending** (2026-10-21): layers add textures from PNGs, which their shaders
  sample as linear color through handles that are the textures' slots in the bindless set, and
  add each shader with how it blends, opaque or by straight alpha. The sandbox draws a
  checkerboard over its grid and a translucent logo over that, with the turning triangle between
  them, and a GPU test checks textures, samplers and blending against a reference image.
- **Camera controller** (2026-10-21): the engine's orthographic camera controller, which a layer
  keeps and calls from its own update and event, moves the camera with W, A, S and D at half the
  view's height a second, turns it with Q and E, and zooms it about the view's center with the
  mouse wheel; the sandbox steers its camera with it. The first update after the window is
  restored from being minimized is given no time, rather than the pause. The 2D renderer's design
  is in ARCHITECTURE.md.
- **Sprites** (2026-10-21): layers draw sprites with `Context::draw_sprite`, the engine's quad
  drawn with its sprite shader in a color, a texture, a rectangle of it and a sampler, the
  rectangle tiling the texture past its edges under a repeating sampler and flipping it when
  reversed; a sprite of only a color shows the engine's white texture, so that one shader draws
  sprites flat and textured. The sandbox's grid, its checkerboard, tiled and tinted, and its logo
  are sprites, around its triangle, and a GPU test checks sprites against a reference image.
- **Profiling** (2026-10-21): the engine marks its startup, frames and shutdown with zones, which
  Tracy shows when the `tracy` feature is on and which compile to nothing otherwise; the engine
  also measures every frame itself, always, which the Performance panel shows and each run that
  doesn't end in an error logs as `engine.perf`: frame-time percentiles, and where the time went,
  for agents to compare builds by. The sandbox shows the panel, and CI checks that its headless run logs the summary.
- **Batching** (2026-10-21): each run of consecutive draws of one mesh with one shader is a single
  instanced draw, and shaders read each instance's transform and parameters by its index, so a
  change of texture, bindless, doesn't split a run; the renderer counts each frame's draws and
  instances, which the Performance panel shows and `engine.perf` logs. The sandbox's swarm draws
  100,000 moving sprites from a sprite sheet in one instanced draw, at a p99 of 4.7 ms in a release
  build on the RTX 4090, and a GPU test checks three runs, one of them past the frame's first
  megabyte, against a reference image.
- **Sprite sheets** (2026-10-21): layers load sprite sheets, whose cells `SpriteSheet` gives in
  pixels, a cell or several, and the sprite shader samples only the texels a sprite's rectangle
  covers, so that cells packed edge to edge show none of their neighbors under a linear or a
  nearest sampler, at any zoom. The sandbox draws a map of tiles from a tile set, and its swarm
  from a sheet; a GPU test checks cells packed edge to edge, side by side, flipped, turned and cut
  across, against a reference image.
- **The game's view offscreen** (2026-10-22): the game's view renders into an image of its own,
  the size of the dock's central area in pixels, made again in the frame that changes that size,
  which the frame shows there under the UI; games' pipelines are made for the image's format
  rather than the window's. A GPU test checks a view that changes size, shuts and opens again,
  against a reference image.
- **The ECS core** (2026-10-22): the engine's own ECS, in safe Rust: entities, whose handles stay
  their own as others come and go, components of any type that implements `Component`, in
  archetype tables, and queries over them; the engine draws the entities with a transform and a
  sprite or a mesh back to front by z. The sandbox's sprites and its triangle are entities, which
  its systems move and color, in the same 3 draws; a unit test checks that entities' handles stay
  theirs as others are removed, and another checks the world against a model of it through random
  changes.
- **Reflection** (2026-10-22): component types derive `Component`, which describes their fields
  to the engine under their own names, with hints for bounds, angles and colors; the world lists
  its component types and entities, and gets and sets any field by name and path, checked against
  the description, a set refused changing nothing and one made returning the old value. The
  sandbox's Entities and Inspector panels show any entity's components and edit their fields while
  it runs; unit tests check descriptions, paths, values coming back bit for bit and values refused,
  and the model test sets fields by name too.
- **The transform hierarchy** (2026-10-22): entities have parents, which carry their children
  along: a child's `Parent`, which only `World::set_parent` changes, refusing cycles, which typed
  code changing any other way doesn't compile, and which `set_field` refuses by name; despawning an
  entity takes its descendants with it; and the engine draws children by their matrices and z's in
  the world, computed from their ancestors. The sandbox's triangle carries two moons around as it
  turns, one with a moonlet of its own, and the Inspector shows a child's parent and a parent's
  children; unit tests check the links through table moves, despawns and slots taken again, and
  the model test sets parents at random.
- **Persistent entity IDs** (2026-10-23): every entity has a persistent ID, a random 64-bit
  number, written as 16 hex digits, that its world draws as it spawns and never draws again; the
  world finds each entity by its ID; errors, the Inspector and its log name entities by their IDs,
  and a handle whose entity is gone by that handle; and at one z, entities draw sprites under
  meshes, each in the order of their IDs, which they keep as others come and go. The sandbox's
  panels show IDs; unit tests check IDs through table moves, subtree despawns, and slots taken
  again and retired, and that drawing kept from frame to frame draws as drawing afresh does, and
  the model test checks every ID after every step.
- **Scenes and prefabs in canonical text** (2026-10-23): the world saves entities and their
  descendants as RON in the canonical form, which the engine writes itself and `ron` reads
  through seeds that reflection's descriptions drive, and loads a scene all or nothing, a refusal
  saying the line and column and, but for a syntax error, the entity, the component and the field;
  a prefab is placed as a copy under a parent, its IDs mixed from its own and one drawn for the
  copy; a drawn ID that a loaded entity has reseeds the stream; and the Entities panel finds an
  entity by an ID pasted in. The sandbox loads its grid from a scene and places its moons from a
  prefab, and its File menu saves and reloads the grid; unit tests check the text byte for byte,
  every kind of value coming back bit for bit, a field set changing only its lines, refusals at
  their lines and columns changing nothing, and placements' IDs, and the model test saves and
  loads its world every 100 steps.
- **Operations with inverses and their log** (2026-10-23): creating and deleting an entity, adding
  and removing a component by name, setting a field and reparenting are operations, which name
  entities by their IDs; applied, each checks everything before it changes anything, and gives
  its inverse; a delete keeps its entities' slots and components, typed, so that undoing it gives
  each back under its handle, with what's opaque; and the log applies them in order, undoes and
  redoes them, and merges a gesture's sets of one field into one edit. The Inspector's edits go
  through it, with an Edit menu, Ctrl+Z, Ctrl+Shift+Z and Ctrl+Y; unit tests check each operation
  undone and redone by handle, refusals changing nothing, undos the world has moved past refused
  whole, and 1,000 random steps, of operations and of undos and redos, undone back to the first
  scene, byte for byte.
- **Whole-world snapshots and restores** (2026-10-23): the world copies whole, as `World` is
  `Clone`, and `clone_from` restores a world from a snapshot, or takes a snapshot into one kept
  from before, in the memory it holds; a restored world is the one its snapshot was taken from in
  every slot, free, vacant or retired, the free ones in their order, in every entity's handle, ID,
  table, row and links, in the component types and in the stream of IDs to come, so that it goes
  on as that world did; and it's another world, with a serial of its own, which refuses the
  inverses from before, the slots a log kept staying vacant, and whose drawing sorts again. Unit
  tests check worlds restored after random branches against twins that stopped at their
  snapshots, field by field and through the same steps after, restores across worlds of other
  types and tables, and memory kept; in release, a snapshot of the swarm's 100,450 entities takes
  2.2 ms cold, and 0.63 ms taken again or restored, allocating nothing.
- **A fixed timestep, with commands** (2026-10-23): the simulation steps 64 ticks a second
  whatever the frame rate, each frame running the ticks its time owes before the layers' updates,
  at most 8, and no more once they run slower than real time, the rest dropped and counted, and
  headless runs one a frame; a layer's tick applies the commands it queued since its last, plain
  data, then runs its systems, and once the layers have attached the world changes only there;
  the Inspector's edits, undos and redos are its commands, made through its log at its next tick,
  its panel drawing the sets waiting as they'll be.
  The sandbox moves its swarm and turns its triangle and moons at ticks, its panels and File menu
  queue commands, and `engine.perf` counts the ticks run and dropped; unit tests check ticks
  against frames of many lengths, the cap and the stop when slow, lockstep, ticks before the
  updates, the world refused outside ticks, edits made at the next tick on the entity picked as
  they were made, and the sandbox's commands applied in order.
- **Deterministic math and the world hash** (2026-10-24): the simulation's math gives the same
  bits on every platform, glam built without SIMD and with libm's functions, and clippy refusing
  std's float methods whose precision is the platform's; and the world hashes in two parts, its
  content, each entity's components under its ID, which undoing a log gives back, and its layout,
  how it holds them, which the order queries find entities in follows from. The sandbox's
  simulation, run without a GPU from a seed through a script of commands, gives at every tick the
  hashes its reference holds, which CI checks on all six platforms; unit tests check that every
  field reflection reads reaches the content and nothing opaque does, that where the world holds
  its entities reaches only the layout, and that undoing a log gives back the content.
- **Recordings and replays** (2026-10-24): each layer queues its commands, which the engine reads
  at each tick as reflection's values through `Layer::commands`; `--record` writes a run's seed,
  each layer's commands just before its tick applies them, with entities by their IDs, and the
  world's hash after each tick, a line at a time; `--replay` runs the ticks again without a window
  or a GPU, each layer's with its recorded commands, and names the first hash that goes another
  way, its tick, the part of it and both builds; `World::difference` names the entity, component
  and field where two worlds part, and `--dump` writes a world a field a line, for two machines'
  to diff. The Inspector's edits, seals, undos and redos are its commands, and the sandbox's swarm
  is in the world, its drifters found by their index. The sandbox's live recording, of a windowed
  run that used its panels, File menu and Inspector, replays to its hashes in debug and release;
  a copy with a grid color changed names its tick, the first square of that color and
  `Sprite.color.x`; CI records a headless run and replays it; unit tests check values' text bit
  for bit, a recorded run against command and hash changes and a cut-off copy, the difference and
  the dump, the Inspector's commands, and a restored world simulating the swarm as before.
- **Input actions** (2026-10-24): a layer's actions, of a type of its own, bind keys, by their
  position, and mouse buttons as buttons, axes and pairs of axes, a key pair an axis; the layer
  feeds them the events it's sent, so that one above that handles a press hides it from them, and
  queues each change of their values as a command of its own as the event comes, which the next
  tick applies; and what an action holds is a component, so that a tap between two frames
  applies at one tick, a hold at every tick until its release, and snapshots and replays keep it.
  The camera controller moves and turns by its actions, which bind to other keys from code. The
  sandbox's arrows steer its triangle and R reverses its spin, at ticks, in a new live recording
  that replays to its hashes; unit tests check buttons of several sources, axes adding up and
  stopping at 1, focus, repeats and refusals, each change applying at the first tick after its
  event at frame times from 1 to 130 ms, the sandbox's keys queuing as they come, and the camera's
  keys bound again.
- **Gamepads** (2026-10-24): gamepads, through gilrs in windowed runs, are the engine's own
  events, their buttons by position, each stick as one value and each trigger's travel, and the
  sources of actions beside keys, every pad adding up; each frame polls them before its ticks,
  giving a stick's or a trigger's moves as one event, through a round dead zone scaled from its
  edge, the same on every backend; nothing a pad does counts out of focus, what's held as focus
  returns counts once it has rested, and a pad that disconnects lets go. The camera controller
  moves by the right stick and turns by the triggers, and the sandbox's triangle steers by the left
  stick and the D-pad and reverses its spin by South; unit tests check sticks and triggers adding
  up with keys and across pads, a stick held as it's bound, refusals, the dead zone, a poll's
  order, focus, disconnects and gilrs's names, and pads' releases going past a layer that handles
  them. Checked by hand with an Xbox pad on Windows.
- **A job system** (2026-10-24): a tick's systems hand a query's rows to rayon-core's pool of
  `--threads` threads, the main thread among them, through `par_for_each`, in runs of 1,024 rows
  rather than a share of the threads, so that a tick goes the same way on any number of them; the
  sandbox's drifters move on it. gg-engine's `ticks` test counts every thread's allocations over
  1,000 ticks, before the pool starts and after, and finds none; the reference simulation hashes
  as its reference says on four threads, and CI replays a run recorded on every thread on one.
- **Phase 3, runtime core** (2026-10-24): each item of its done-when has a test that CI runs on
  all six platforms. A scene saved, loaded and saved again is byte-identical; undoing a log of
  random operations gives back the content hash; a restored snapshot matches the world it was
  taken from; steady-state ticks allocate nothing on any thread; the live recording replays to its
  hashes, and the reference simulation hashes as its reference says; and a forced divergence names
  its tick, and its entity and field.
- **The render thread** (2026-10-25): frames render on a thread of their own, `gg-render`, a frame
  behind: the renderer goes there with each frame's render data, owned, and back once the next
  frame's UI is drawn, so that windows' swapchains open, resize and close, and the renderer drops,
  on the main thread, which on Windows answers what's sent to its windows while it waits; what
  layers add goes with the frame. Ticks are a part of the frame times of their own, the render
  thread's sit beside the main thread's, and job threads default to one fewer than the CPUs. Unit
  tests with a stand-in renderer check frames rendering in order, a frame behind and beside the
  next, the renderer back between them, errors and panics coming back, the last frame rendering as
  a run ends, and no texture change dropped unapplied; a Tracy capture of the release sandbox shows
  rendering beside the ticks.
- **Rendering interpolated between fixed steps** (2026-10-25): each frame draws the world's
  entities on their way from where its last tick found them to where it left them, as far as the
  frame is past the tick, from a copy of their transforms taken outside the world as each tick
  starts, which no hash, scene, snapshot or recording holds; children blend their own transforms
  and their ancestors', then compose, and the order is the tick's. What jumps draws its own
  transform where its tick left it: an entity spawned, given its transform or put back, in
  another's slot, under another parent, in a world restored or replaced, mirrored, or reset, as the
  sandbox's drifters are as they wrap. Unit tests drive frames a quarter of a tick apart, and two
  and a half, through the main loop and check exactly where each draws, each way a tick breaks an
  entity's way, children on their orbits, the tick's order kept, and no memory taken again;
  headless frames draw their tick.

## Now: Phase 4's third milestone

[Phase 4](#next)'s third milestone is picked from what the phase has left as it starts, and its
plan says when it's done.

## Next

Each phase ends in something you can run and check.

4. **First mesh.** Uploads of whole assets through the transfer queue, or by host image copy
   where a device has no separate transfer queue; depth; Slang reflection; an on-disk pipeline
   cache; shader hot reload. Render extraction, with the render thread a frame behind
   simulation and rendering interpolated between fixed steps. *Done when* the sandbox draws a
   textured, depth-tested glTF mesh and thousands of moving entities, Tracy shows simulation
   and rendering overlapping, steady-state frames don't allocate, and a RenderDoc capture shows
   named resources and labeled passes.
5. **Tool API and hot reload: v0.1.** The tool API in development builds, as
   [ARCHITECTURE.md](../ARCHITECTURE.md#agents-and-tools) describes. `tools/gg`, the
   command-line tool: build, run and test with machine-readable results; launch and supervise a
   game's development build, restarting it from its last snapshot after a crash or hang; talk to
   it through the API. An MCP server with the same reach. Scene files edited while the game runs
   apply as operations. `subsecond` patches edits inside function bodies into the running game;
   it patches only the game's main crate and needs Dioxus's `dx` tool to build patches. Any
   other change rebuilds the game and restarts it from a snapshot, matching each changed
   component to its new layout by field name. *Done when* the v0.1 loop works: watching the
   running game, the user asks Claude Code for a change, Claude Code edits the game's Rust code
   or applies operations through the API, and the game shows the change with its state intact;
   and an agent runs a headless check through `tools/gg` and acts on its result.
6. **Render graph and PBR.** Passes declare the resources they read and write; the graph
   places barriers and layout transitions, aliases transient memory and schedules async
   compute. Physically based shading (glTF metallic-roughness), image-based lighting, cascaded
   shadow maps, HDR and tonemapping; reference-image tests on lavapipe. *Done when* Sponza
   renders lit and shadowed, synchronization validation is clean, and per-pass GPU timings
   show in Tracy.
7. **Assets and player.** `tools/cook` turns glTF into engine meshes (meshoptimizer: vertex and
   index optimization, meshlets, LODs) and textures into BC-compressed KTX2, packed into a
   memory-mappable package; an async streaming loader with a per-frame upload budget; hot
   reload in the sandbox; the player runs a package under a shipping profile (LTO, one codegen
   unit). *Done when* the player runs a cooked Amazon Lumberyard Bistro on all three OSes,
   with load time and memory measured.
8. **GPU-driven rendering.** GPU culling (frustum, then two-pass occlusion against a depth
   pyramid); indirect draws; meshlets, through mesh shaders where supported and a compute path
   elsewhere; a visibility buffer; clustered lighting; TAA. *Done when* CPU frame time stays
   flat as the instance count grows, and Bistro with many instances meets a frame-time target
   set in this phase on the RTX 4090 and an Apple Silicon Mac.
9. **Editor.** `apps/editor`: scene tree, inspector, play and stop, scene saving and loading,
   logs and errors, and the game's view embedded, all through the tool API. *Done when* the
   editor has no path into the game but the tool API, and the v0.1 loop works from inside it.

Later: Luau scripting and mods; networking (replication, lockstep, rollback); physics, audio,
animation, UI; upscalers (DLSS, FSR, XeSS); ray-traced effects where the driver exposes ray
tracing (no macOS driver does today); and the built-in coding agent for v1.

## Deferred

| Skipped | Add when |
|---|---|
| The engine as a dynamic library in development builds, to shorten relinks | Rebuilding a game gets slow |
| Optimized dependencies in dev builds (`[profile.dev.package."*"] opt-level = 3`) | A heavy dependency (image decoding, physics) makes debug builds slow |
| Shipping profile (LTO, `codegen-units = 1`) | Phase 7, with the player |
| Compiling trace and debug events out of shipping builds, through `tracing`'s `release_max_level_*` features | Phase 7, with the shipping profile |
| No console window for shipping builds on Windows (`windows_subsystem = "windows"` on the player); the JSON log still records the run | Phase 7, with the shipping profile |
| Checking the sandbox by hand on Linux, and on macOS with KosmicKrisp (Vulkan SDK 1.4.363.0 or later, the first conformant build), its gamepads too, and the render thread there: acquiring, presenting and rebuilding swapchains as windows resize off the main thread under KosmicKrisp, and on Wayland a frame in flight presenting at its old size after a maximize | Phase 4, once the sandbox draws a glTF mesh |
| Deleting old JSON logs; every run's log is kept | Logs pile up enough to matter |
| A clean shutdown on Ctrl+C, which now ends the process at once; the JSON log keeps every line written until then, and a recording every whole line but its `end` | A headless run that goes on until it's stopped, such as a dedicated server |
| GPU tests in Windows and macOS CI, which skip them because hosted runners there have no GPU | A self-hosted runner with a GPU, or a Vulkan driver that runs on those runners without one |
| A gate that compiles the shipping configuration: `cargo clippy -p player --all-targets -- -D warnings` | When `gg-engine` gets its `dev` feature |
| egui, the dock and the Inspector behind `dev`: the player links them and runs an empty UI pass every frame; `Ui`, which `Layer::ui` takes, is the type to turn into a no-op | When `gg-engine` gets its `dev` feature |
| The main thread takes the renderer back once a frame's UI is drawn, so a frame's input is read about a frame before it renders: a refresh under `--vsync`; and drawing between ticks draws the world up to a tick behind the simulation. Starting each frame once the render thread has acquired the image it renders into would win the first back, and extrapolating from the latest ticks the second | A game's input latency needs it |
| Answering at once what a driver sends the main thread's windows from the render thread: the main thread's wait answers each time a millisecond's wait times out, which Windows' timer tick stretches to about 16 ms, so a sent message can hold a frame that long. A Win32 event the render thread sets, waited on with `MsgWaitForMultipleObjectsEx`, would wake for both | A driver's sent messages show as stalls, such as while a window resizes |
| Pushing and popping layers while the app runs; `App` fixes the stack before it starts | A tool or game needs a layer for only part of a run |
| Composed text from an input method (winit's `Ime` events), for languages typed through one | A game takes text input, such as chat |
| Steady-state frames that draw UI allocate: in egui's layout and tessellation, in the dock's own bookkeeping, which copies each frame's panel and menu names and torn-off windows' titles, and makes its arrangement and its list of groups left to draw anew, and in the Inspector, which reads its rows' names, the picked entity's components as values and its children's IDs anew | Phase 4, with the allocation check; the overlay is for development |
| Keeping within the GPU's memory budget (`VK_EXT_memory_budget` through VMA) | Phase 7, with streaming |
| Removing meshes, shaders and textures; each lasts until the app ends | Something adds and removes them as it runs, such as asset hot reload in Phase 4 |
| Perspective cameras, and a controller for them; a camera is orthographic, and so is the engine's controller | Phase 4, with depth |
| Per-frame data in a buffer: shaders get the camera's view and projection as one matrix, and see the world through each draw's transform, but not the camera's position, the time or the lights | Phase 6, with lighting |
| Materials: parameters kept on the GPU and written when they change, rather than copied every frame with each instance | Phase 6, with materials |
| Counts of vertices, indices or triangles, which follow from the instances; the renderer counts draws and instances | Phase 4, with glTF meshes |
| Textures of data rather than color, such as normal maps, which shaders read as stored rather than decoded from sRGB | Phase 6, with PBR's maps |
| Premultiplied alpha for games' textures: with straight alpha, as PNGs store it, a linear filter at an edge mixes in the color of the transparent pixels beside it | A game's art shows dark or colored fringes |
| Sorting what blends back to front by depth, and what doesn't by its mesh and shader, so that it batches; the world's entities draw back to front by their z's in the world as the latest tick left them, at one z sprites under meshes, each by ID, so that meshes of different meshes or shaders at one z don't batch, and what layers draw themselves in the order given | Phase 4, with depth |
| Mipmaps, and anisotropic filtering: a texture drawn smaller than it is samples its one level, and shimmers. A sprite sheet's cells, packed edge to edge, will then mix together at the smaller levels and under anisotropy, which reads past the texels the sprite shader keeps to, and need space around each cell or an array layer each | Phase 4, with the textured glTF mesh |
| Sprite sheets with space between or around their cells, as some tile sets come; `SpriteSheet` takes cells packed edge to edge from the top left | A game's art comes that way |
| Tiling one cell of a sheet: a repeating sampler repeats the whole texture | A game tiles part of an atlas, such as a background |
| A sprite that samples past its rectangle, so that a seamless texture tiled across sprites side by side blends across their edges under a linear sampler | A game tiles one texture across sprites |
| A tile map in the engine, which scenes save; the sandbox's map is code of its own | Phase 4, which names assets |
| Images other than PNG, such as the JPEGs glTF allows; and PNGs' own color profiles and gamma, which are ignored, every PNG taken as sRGB | Phase 4, with glTF; Phase 7 cooks KTX2 |
| Decoding images off the main thread; `Context::add_texture` decodes before it returns | Phase 7, with the streaming loader |
| Layers making textures from their own pixels; `Textures::add` takes pixels, but `Context` adds only PNGs | A game makes its own textures |
| Pixels, vertices and instances' transforms and parameters reach the GPU through the frame's arena, which merges the buffers it grew into, holding two to three times the largest frame for each frame in flight: by its growth rule, about 20 MB each for 100,000 sprites, which use 10.4 MB a frame; and a frame that adds many textures leaves that much host memory held | Phase 4, with uploads through the transfer queue |
| More than one camera or view, such as split screen or the editor's scene view | A game or the editor draws several |
| The camera as a component of an entity, which a scene saves and a parent carries along; layers set the camera through `Context` | Phase 4, with perspective cameras |
| The game's view in a panel that docks and tabs, and keys reaching the layers only while it has focus, as editors' views do; the view is the central area, and keys reach the layers wherever the pointer is unless one of egui's widgets has focus | A tool in the game's process shows the view among its panels |
| Converting between the view's pixels and the world, to pick what's under the cursor or zoom toward it; the camera controller zooms about the view's center | Something picks with the mouse, or a tool zooms toward the cursor |
| Zooming from keys or a gamepad, as a rate beside the wheel's lines; the camera controller zooms by the wheel alone, in its own event handling | A game binds zoom elsewhere |
| Per-user bindings, kept as RON in the per-user directory beside the dock's layout, and a UI that binds them; a game binds its actions in code | A game ships its settings |
| Chords, modifiers, holds and double taps, such as Ctrl and a key: there's no modifier state, and Ctrl+Z reaches layers as `ControlLeft` and `KeyZ` | A game binds a combination |
| Keys' and gamepad buttons' labels, for prompts that show a binding: both are positions, and the engine drops what a keyboard's layout makes of a key, and whether a pad is an Xbox or a PlayStation one | A game shows its bindings |
| A hold of an axis shorter than a tick, which moves nothing: its two changes apply at one tick | A game needs one, as a flag that the tick clears |
| An engine type for what an action holds in the world, such as a button's held flag and its presses this tick; each game writes its own component | A second game writes the same |
| Pressing and setting actions from code, ahead of the commands, and listing a game's actions by name, for the tool API and agents | Phase 5, the tool API |
| Gamepads in `Input`, for layers to poll: `Input` holds keys and mouse buttons alone, and actions read the pads | A layer polls a pad |
| Gamepads connecting and disconnecting as events, and their names, which only the log has | A game prompts as a pad connects |
| Binding a source to one gamepad, or the keyboard to one player: every pad drives every layer's actions | Local multiplayer |
| Dead zones a game sets, for a pad, a stick or a binding: every stick and trigger has the engine's, from a tenth to nine tenths of its travel | A game ships its settings, with per-user bindings |
| A stick's way or a trigger's travel as a button, pressed past a point and released before it, as menus take a stick; `bind` refuses one, and a trigger's press is a button of its own | A game's menus take a stick |
| Rumble: gilrs's force feedback is off, and with it its thread | A game rumbles |
| Fewer commands from a moving stick, by quantizing its value or folding a layer's queue: a frame gives one for each stick and trigger that moved, and one more for each of its pad's button events between its moves, up to the pad's report rate, about 125 a second on Windows.Gaming.Input | Recordings grow too large, or input goes over a network |
| gilrs's allocations: on Windows.Gaming.Input, its polling thread's, which reads every pad every 8 ms, and the string of a pad's ID that each event the frame takes asks Windows for; and the channel's blocks, which the frame that takes their last event frees. The engine's own feeding of pads to actions allocates only as a pad first reports, and as a second pad first holds more than one does | Phase 4's allocation check of frames, which counts every thread, as gg-engine's `ticks` test does ticks |
| Letting go, for the layers below one that handles a stick or a trigger away from rest, of the ways it left: they keep what they last saw until it rests, so that a stick rolled from right to up in a menu leaves the game going right | A game's menus take a stick |
| Controls the engine doesn't name, C, Z and paddles, and triggers a Linux driver reports as Z axes ([gilrs #200](https://gitlab.com/gilrs-project/gilrs/-/issues/200)), which are dropped | A pad that needs them, or the check by hand on Linux |
| Codes for dependencies' own warnings, which reach the log as they give them: as a pad connects on Windows.Gaming.Input, gilrs warns, without a code, that it has no mapping for it, since that backend gives every pad a nil UUID, and goes on with its default mapping, the right one there | Phase 5, as the tool API serves diagnostics |
| Checking a game's shader against the contract on `Shaders::add` through Slang reflection: its push constants, its resources, and the sizes of its vertex and parameter structs against their types'; until then it's trusted, and validation checks it in debug builds | Phase 4, with Slang reflection |
| Games' shaders compile with slangc as they're added, so running a game needs the Vulkan SDK | Phase 7, which cooks shaders |
| A library of assets: shaders and textures load from the paths the game gives, the sandbox giving its source's, and nothing finds one by path or name, so a file added twice loads twice; scenes and the tool API will name them | Phase 4, with assets |
| `Layer::attach` can't fail, so a layer whose shader doesn't compile, or whose texture doesn't load, panics, as the sandbox does | A game loads assets as it starts |
| 8- and 16-bit vertex fields, whose features Vulkan 1.4 guarantees but the device doesn't enable | A mesh packs its vertices, as Phase 7's cooker will |
| A swapchain without an sRGB format falls back to the first one offered, where colors come out too dark | A device offers no sRGB swapchain format |
| Splits in a torn-off window, which holds one tab group | A tool wants panels side by side outside the main window |
| Closing a panel from its tab; a panel shows while its layer shows it | Panels the user opens and closes, such as the editor's |
| Reordering tabs by dragging them along their bar; a drop among the tabs adds one at the end | Phase 9, the editor |
| On Windows, an inner size asked of winit once a window whose title bar the UI draws is open (egui's `InnerSize`, `MinInnerSize` or `MaxInnerSize`) leaves its inside a frame smaller, since winit sizes it as though it had no frame | Something resizes such a window from code |
| Leaving a taskbar that hides itself a pixel or two beside a maximized window whose title bar the UI draws, as Windows Terminal and Chromium do; the window covers the whole monitor, which may keep the taskbar from showing | Someone hides their taskbar |
| Checking Windows' frame on Windows 10, whose DWM may draw no border along the top of a window whose inside reaches it | Someone runs the tools on Windows 10 |
| Checking the drawn title bars on macOS and X11: the menus' inset clear of the traffic lights (72 points), and moving a torn-off window by its tab bar where the OS moves windows asynchronously | With the sandbox's check by hand on Linux and macOS |
| On macOS, the menus in the system's menu bar, and a double-click on the title bar doing what the user's setting says | A tool ships on macOS |
| Alt and access keys to reach the menus, and layers adding items to one another's menus | Phase 9, the editor |
| Windows of their own on Wayland, where groups float over the main window | winit can place windows on Wayland |
| egui's deferred viewports, which don't open (`engine.viewport_deferred`); immediate ones do | A tool needs a window that repaints apart from the main one |
| A saved window position off every monitor, which should be moved onto one | Someone loses a window after their monitors change |
| Torn-off windows across monitors of different scales: one that opens on another monitor is placed at the main window's scale, away from the cursor. Nor is Windows' frame checked around them as their scale changes | Someone docks across such monitors |
| In a frame egui runs twice, the torn-off windows' texture changes apply before all of the main window's, so glyphs added then to a font atlas that grows can go missing | It shows, or egui hands over each pass's changes apart |
| Present fences (`VK_KHR_swapchain_maintenance1`) before a swapchain's semaphores are destroyed, which closing torn-off windows makes more frequent | Validation reports it, or windows open and close often |
| egui's own scopes in Tracy, which egui marks for the `profiling` crate: `profiling` 1.0.18 takes tracy-client 0.18, and two Tracy clients don't link together | `profiling` takes tracy-client 0.19; deny.toml's wrappers for tracy-client then add it |
| Changing the world's tables, or resetting an entity's interpolation, while a query runs, through a buffer applied after it: Bevy's `Commands`, to be called `Deferred` here, since commands are the simulation's inputs; from `par_for_each`'s jobs too, each run of rows with a buffer of its own, applied in the order of the rows, never as the jobs finish. The sandbox's drift collects the drifters that wrap under a lock, and resets them after | The first system that spawns or despawns as it iterates |
| Reductions over a query's rows on the job system's threads, such as a sum, each run of rows giving its own, combined in the order of the rows; `par_for_each` on queries that only read; a size of a run of rows for each call; and splitting tables of fewer rows than a run among the threads, which each run on the calling thread, one table after another | Each with its first user |
| Two systems at once, on components they don't share: a tick's systems run one after another, each splitting its own rows | A profile shows two heavy systems |
| Query filters: `With`, `Without`, `Option` and `Changed` | Each with its first query |
| Queries that keep the tables they match, and tables that keep where adding or removing each component leads; a query looks over every table, and a change of components finds its table by its types | Tables number in the hundreds, or changes of components show in a profile |
| Sparse storage for components added and removed often, which move their entity's row between tables | A profile shows those moves |
| A layer reading the world and drawing in one loop: `Context` lends one at a time | A layer draws from the world itself, such as debug shapes |
| Spawning many entities at once, faster than one by one: 100,000 sprites take about 16 ms in release, 5.5 ms of it their IDs, and the swarm spawns its 100,000 in one tick, longer than a tick is. A load spawns its entities one by one too, building each component from a value into a column of its own: 100,000 sprites, 45 MB of text, load in about 650 ms in release, and save in about 170 ms | A spawn hitch matters, or a scene's load time |
| `MeshDraw` parameters of more than 64 bytes, which draw through `Context::draw` | Phase 6, with materials |
| Micro-benchmarks, with criterion | A CPU hot path gets a target of its own; batching's is a frame time, which `engine.perf` measures |
| Gates on time in CI: hosted runners' times vary too much to gate on | A self-hosted runner, as for the GPU tests |
| A component type's default as a value, to add one without giving every field; `Create` and `Add` take whole values | The Inspector or the tool API adds components |
| Moving values to a type that changed: matching fields by name and by alias, defaults for fields added, and fields dropped; a scene with a field its type hasn't, or without one it has, is refused | Phase 4, when `Sprite` gains its texture's name, or Phase 5's hot reload |
| In the Inspector, turning an option on or off, switching to a variant with fields, and adding or removing a list's elements; by path, a value sets them whole | A component needs one edited |
| `Sprite.texture` and `MeshDraw` are opaque: shown, but not set, saved, hashed or carried across a rebuild | Phase 4, which names assets |
| Flags for fields that saving, the world's hash or replication skip, and fields that are read only; `Parent` is read only as a whole type, which `set_field` refuses | Each with its first user |
| Generic types deriving `Reflect` or `Component` | A game needs one |
| Drag speeds as hints; the Inspector picks them from a field's bounds and kind | A field drags too coarsely or too finely |
| Paths resolved once and kept, for sets made again and again | A profile |
| Picking an entity for a field, a parent or a child picked by clicking it, the Entities panel as a tree, names in the Inspector, and picking in the view; entities are shown by their IDs, and found by an ID pasted in | Picking in the view, or Phase 9's scene tree |
| World transforms stored and propagated after the ticks, with change detection; drawing computes each child's from its ancestors, every frame, as the latest tick left them, for its z, and blended between ticks, to draw it | Physics, or a profile showing the walks up the hierarchy |
| A world matrix for game code, and where an entity draws between ticks, for a camera that follows it, which would otherwise judder against it, since layers read the world up to a tick ahead of where it draws; the engine's drawing computes its own | The camera as a component, a camera that follows an entity, or a system that needs a child's place in the world |
| Turning interpolation off for an entity or a subtree, so that it draws at its ticks; every entity draws between them | A game wants something drawn at its ticks |
| Rotations of more than half a turn a tick, which draw the shorter way round, the other way; Hermite easing from velocities would draw them, and curves between ticks | Something spins that fast, or a curve between ticks shows |
| Drawing an entity despawned in a tick on its way there until the next: it vanishes up to a tick before it reaches where it stopped | A game's despawns show it, as bullets vanishing short of what they hit |
| Copying the world as only the last tick a frame runs starts: a frame that catches up copies it as each of its ticks starts | A profile shows a catching-up frame's copies |
| Copying the world, and finding what each row draws from, on the job system's threads, or only for the rows that moved: both run on the main thread, for the swarm about 0.19 ms a tick and 0.10 ms a frame that ticked; and blending only the rows that moved, which takes the job threads about 2.6 ms of their time a frame | A profile shows them |
| Sets that jump: an Inspector set of a moving entity's transform, and its undo, glide there over a tick, as a drag does, rather than reset its interpolation | Phase 5's tool API, whose agents move things by setting them |
| Headless frames at a fraction of a tick of their own, for captures between ticks and for perf runs that measure blending, which headless frames, drawing their ticks, leave out | Phase 5's headless captures, or a change to blending that needs measuring headless |
| Interpolation across rollback: a world restored in a tick draws where the tick left it, all of it, so rollback that restores every tick would draw at the tick rate; how rollback restores is decided with it | Rollback |
| Reparenting that keeps where a child is in the world, by decomposing its matrix, which loses any shear; `set_parent` keeps its own transform | The editor's scene tree, or an agent attaching something where it stands |
| Spawning a child into its table at once from code: `spawn`, then `set_parent`, moves it to another table; a load and a create operation spawn one there | A profile |
| Children in an order that means something: reordering them, and saving their order; an undo puts a child back last | The editor's scene tree, or UI laid out by the hierarchy |
| Each session's seed and count saved with the world and merged with it, so that an ID maps back to its session and when it was drawn, and a load refuses IDs no session drew | The tool API refusing an ID an agent made up |
| An `EntityId` query term | The first system that reads IDs as it iterates, such as replication |
| Errors carrying their entity, and a scene's line and column, as fields, not only in their message | The tool API returns errors as data |
| Drawing rows at one z in ID order for less. Every frame reads them in that order rather than their tables', about 0.9 ms of the swarm's 4.3 ms frame in release; a frame that changes rows or z's, or follows a restore, sorts them by ID again, about 2.5 ms more for 100,000; and 100,000 sprites each at a z of their own, as sorting by y gives them, take about 2 ms more than before, for a group and an item each. Tables kept in ID order would make the reads a sweep, and a radix sort the sort quicker | A game's frames need the time |
| Linked prefab instances, with overrides, nesting, apply and revert; a placement is a flattened copy, each ID mixed from its instance's and its own, which links will keep | The editor, or a game editing a prefab its scenes hold many copies of |
| A derive hint for `Component::base`, and opaque values in lists and options, which saving refuses, as does a set's undo (`reflect.opaque`) where the old value holds something opaque in a variant, an option or a list; a type with opaque fields implements `Component` by hand to load, as `Sprite` does | A game's component holds something opaque |
| Names in scenes beyond ASCII identifiers, which saving refuses (`scene.bad_name`) | A game names a type or a field so |
| Loading a scene in place of the entities it replaces, in one step; a load refuses IDs the world has | The editor, or Phase 5's scene files applied as operations |
| A version in scene files, a file without one reading as the first | The first change to the format that old files can't be read by |
| Comments in scene files, which a save drops | People annotate scenes |
| Reading and writing scene files in the engine, atomically; the engine takes and gives text, and the sandbox writes its own | Phase 4, with assets |
| The log in `Context`, every layer's and the tool API's clients each undoing their own, and an undo logged as an operation; the Inspector keeps a log of its own | Phase 5, the tool API |
| Replaying a log into another world, which maps values' handles to IDs, as a recording's commands are; `set_field` and operations store a dead handle without complaint | Phase 5, the tool API |
| A delete saying what refers to its entities from outside, or refusing; those references dangle until it's undone, and the world doesn't save in between | An agent deletes through the tool API |
| Layers holding what a restore takes back: the sandbox's swarm is in the world, but the Inspector's log and pick outlive a restore, and nothing tells a layer that `Context`'s world was restored, so the sandbox doesn't snapshot | The Inspector or the tool API deletes, or something restores the running world, such as rollback or Phase 5's restart |
| Creating, deleting, adding, removing and reparenting from the Inspector, which sets fields | The editor, or a game's tools |
| A bound on the log, which keeps a session's entries, a delete's components and slots among them, until it's cleared | A session's memory shows it |
| A log that outlives a run or a restore: inverses that keep components typed can't be saved, and a restored world refuses every inverse from before, the slots kept when the snapshot was taken staying unused; a snapshot could keep the log's place, which a restore would rewind it to | Phase 5's restart from a snapshot |
| A depth limit on values set or created by name, as scenes' 64; a deeper one recurses without one | The tool API takes values from outside |
| Snapshots that copy less, only the tables changed, or on several threads: the swarm's 100,450 entities copy about 20 MB each way, in 0.63 ms taken again or restored, and the frame after a restore sorts the swarm's rows by ID again, about 2.5 ms | A game's rollback frames need the time |
| An app's own tick rate; the simulation runs 64 ticks a second | A game needs another rate |
| Budgeting a frame's ticks against all of its time: a frame stops its ticks only once they take longer than the time they simulate, so ticks that take nearly their 15.625 ms keep real time at the frame rate's cost, as the debug sandbox's 100,000 drifters do, about 8 a frame at 8 fps; counting the rest of the frame too would slow down the simulation of any game drawn below 64 fps | A game's frame rate falls so |
| Headless runs in real time, as a dedicated server's: a headless run takes a tick each frame, as fast as its frames go | The first dedicated server |
| The Inspector showing an undo or a redo before its tick makes it, as it shows a set: above 64 frames a second, a field shows its value from before for a frame or so, and a refusal and the Edit menu's names come at the tick | It shows, or Phase 5's editor in another process |
| Hashing a world for less: incrementally as its components change, on several threads, or with a cheaper step for each word and one mix for each term. `World::hash` reads every component, mixing each word with SplitMix64's mix: 5.5 ms for the swarm's 100,450 entities in release, three times their tick; a recorded run hashes every tick, which takes the release swarm's headless frames from a p50 of 8.7 ms to 16.3 ms, its updates from 14.0 ms to 22.6 ms on average | Recorded runs of large worlds need the time |
| The job system in gg-core, where the renderer can reach it; it's gg-engine's | gg-renderer needs jobs |
| Naming the job system's threads and the render thread for Tracy on macOS, whose client reads no thread's name from the OS there, so they show as numbers: a call into gg-core, which keeps Tracy's client, from rayon-core's start handler and the render thread's start. On Windows and Linux they show as `gg-job-N` and `gg-render` | Someone profiles on macOS |
| Checking that every job thread's floating-point environment, its rounding and flush-to-zero, is the main thread's, which a driver could change on the main thread alone | A replay goes another way on one machine on more threads than one |
| The sign of a zero that f32's `min`, `max` and `clamp` give, which std leaves to the platform, and its `algebraic_*` methods, which clippy doesn't refuse; the simulation calls none of them | The simulation first calls one |
| The ban on std's float methods for games in their own repositories, which the workspace's clippy.toml doesn't reach, though glam's features do, since Cargo merges them | The first game outside the workspace |
| Locating a divergence of the layout alone, to the table, row, children or stream of IDs where two worlds part; `World::difference` compares their content, and a replay names the part of the hash | A replay's layout goes another way first |
| Replaying from a snapshot and the commands after it, and a SyncTest, which runs each tick again from a snapshot and checks its hash, as rollback needs | Rollback |
| Recordings that keep what their attach read, such as the sandbox's grid scene, and the commit they were built from; a file changed since shows as a divergence at the attach, and a recording names its engine's version and build alone | Recordings replayed far from where they were made, such as bug reports from people's runs |
| Recording every run, as a flight recorder; a run records only with `--record` | Bug reports from people's runs |

## Open questions

- Which game is the flagship? Recommendation: a networked game whose design depends on
  determinism, so the simulation core, the tool API and multiplayer are exercised from the
  start. Decide before Phase 5, v0.1.
- Editor UI toolkit: egui, which the engine's overlay already uses, or GPUI? Decide before
  Phase 9.
- Which license? Recommendation: MIT OR Apache-2.0, the Rust convention, chosen once; until
  then, `publish = false` keeps the crates off crates.io. Decide before anyone outside the
  project contributes.
