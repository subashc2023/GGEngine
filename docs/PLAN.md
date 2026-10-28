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
  every kind of value coming back bit for bit but a NaN's payload, a field set changing only its
  lines, refusals at their lines and columns changing nothing, and placements' IDs, and the model
  test saves and loads its world every 100 steps.
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
- **Depth and a perspective camera** (2026-10-25): the game's view renders with a depth image of
  32-bit floats, reversed, from 1 at the near plane to 0 far away, which every draw tests and
  opaque shaders write; cameras project orthographically or with perspective, out to infinity; the
  world's opaque meshes draw first, by shader and mesh, then what blends back to front by z; and
  the engine's fly camera controller moves a camera where it looks and looks about, as the
  sandbox's cubes, which cut through each other, show. A GPU test checks depth against a reference
  from a camera with perspective, and another the depth barrier under synchronization validation;
  unit tests check both projections, the order and the controller.
- **Asset uploads** (2026-10-25): meshes and textures reach the GPU through a queue for transfers
  alone where the GPU has one, which hands them over to the graphics queue; where it hasn't, by
  host image copy if its driver says that costs nothing, and through the graphics queue otherwise.
  The frame that takes them waits for their copies and draws them, and what staged them is freed
  once it finishes, rather than held in the frame's arena. GPU tests run the meshes, textures and
  sheets references on each way the GPU has, the first two reading back the frame that adds an
  asset, draw a 64 MiB texture in the frame that adds it, and write over a filled texture, whose
  layout validation checks; unit tests check the choice of family and of way, and the handover.
- **Textured glTF models** (2026-10-25): layers add glTF 2.0 models, read by the engine's own
  reader, which refuses what the engine doesn't draw and what isn't glTF before adding anything,
  and spawn them into worlds as entities, a node each and a child for each primitive of a mesh with
  several. Their images, PNGs or JPEGs, as layers' textures may now be too, get mip levels that the
  CPU makes in linear space as a frame uploads them, weighted by alpha where it's coverage, sampled
  trilinearly with 16x anisotropy; their materials draw unlit, opaque, masked or blended, and cull
  by `doubleSided`, a mirrored instance's fronts winding the other way; and the renderer counts
  triangles. The sandbox draws Cesium's milk truck beside its cubes. GPU tests check models,
  culling and the alpha modes against a reference, and every mip level on each way of uploading, on
  the RTX 4090 and the AMD iGPU, where every test now passes, its samplers written into the
  bindless set rather than immutable; unit tests check the reader's refusals, files cut short or
  changed byte by byte, the levels' weights and rounding, models spawned, and runs split by faces.
- **Shaders checked, hot reloaded and their pipelines cached** (2026-10-25): games' shaders are
  checked, in the SPIR-V slangc writes, against the contract `Shaders::add` describes, as they're
  added and as they reload, and one that breaks it is refused, naming each way
  (`renderer.shader_mismatch`). Behind gg-engine's new `dev` feature, which the sandbox turns on
  and the player doesn't, a windowed run compiles a game's shader again off the main thread as its
  files change, its includes among them, and draws with it from the next frame; one that doesn't
  compile or breaks the contract warns, and the old one keeps drawing. Every pipeline goes through
  a pipeline cache, which windowed runs load from the per-user cache directory and save as they
  end, behind a header that keeps another GPU's, driver's or damaged data from the driver; and the
  shipping gate joins the gates. Unit tests check the engine's shaders and the fixtures, a fixture
  for each kind of breach, the depfile, shaders compiled again and replaced, the cache's header and
  each refusal, and the watcher's looks, stepped and on a thread; GPU tests check a shader replaced
  with frames in flight, and the cache filling, loading whole and refusing damage, on the RTX 4090
  and the AMD iGPU. In windowed sandboxes there, a second run hits the cache for every pipeline:
  the AMD iGPU makes the sprite pipeline in 126 µs rather than 20.6 ms.
- **Steady-state frames allocate nothing** (2026-10-26): an app none of whose layers draw UI,
  keeping `Layer::ui`'s default, and whose windows the OS frames, runs without egui from its second
  frame, whose empty pass had made 16 allocations a frame, and its input 6 more windowed; the
  world's map of IDs keeps room for twice the most entities it has found, so that entities coming
  and going never grow it at a time its keys pick; and spawning a model borrows the world's
  memory. gg-engine's `frames` test runs such an app through the main loop headless, seeded and
  three fifths of a tick apart, so that it draws between ticks: sprites, games' meshes, a model
  and a mirrored copy, a turning hierarchy and 2,000 drifters that wrap, while its ticks replace
  sprites and models. It counts no allocation on any thread over 160 frames, on the RTX 4090 and
  the AMD iGPU, and CI runs it on lavapipe; `ticks` counts none over 20,000 ticks that replace
  parents and their children, which without the room allocate. Frames that draw UI allocate, in
  egui, as the sandbox's do.
- **RenderDoc captures** (2026-10-26): a run that RenderDoc's layer is active on captures its Nth
  frame with `--capture N`, headless or windowed, through RenderDoc's in-application API, and logs
  the capture's path; asked to without the layer, it doesn't start (`renderer.no_renderdoc`), and
  one that ends first warns (`engine.not_captured`). Frames' command buffers are named again as
  they're begun while the layer is active, and the instance is named; the warning that
  synchronization isn't validated, and the refusal of a GPU that RenderDoc reports Vulkan 1.2 for,
  name RenderDoc as the cause. Captures by hand on the RTX 4090, of the sandbox headless and
  windowed, debug and release, and of the player, replayed for their resources and actions, name
  everything the engine makes, but the swapchain images of a torn-off window that the frame doesn't
  render into, which RenderDoc leaves its own names on, and label every pass; a GPU test checks the
  refusal, in CI on lavapipe.
- **Phase 4, first mesh** (2026-10-26): each item of its done-when has a check. The sandbox draws
  Cesium's milk truck, textured and depth-tested, among its cubes and 100,000 moving sprites, and
  GPU tests check models, depth and sprites against references, in CI on lavapipe. A Tracy capture
  of the release sandbox shows rendering on its own thread beside the ticks. The `frames` test
  counts no allocation in the frames of an app that draws no UI, in CI too. A RenderDoc capture of
  the sandbox, taken with `--capture`, names what the engine makes and labels its passes.
- **The editor's shell** (2026-10-26): `apps/editor` opens a window whose UI's title bar holds its
  menus, over a dock that fills it. `App::view_panel` makes the dock a dockspace, without the
  central area games keep, and the game's view a panel of its own, which docks, tabs and floats
  over the main window but never tears off, drawn where the UI paints a `ViewCallback`, in the UI's
  order. The layers get presses and the wheel over the view and the drags they start, and keys
  while it has focus, which a press on it gives and a press elsewhere takes; releases always. Until
  the tool API brings it a game, the view shows a grid through the orthographic camera controller,
  as 2D games see theirs, drawn by a shader of the editor's own that keeps each line a pixel wide
  at any zoom. Unit tests check the dockspace, the view's panel and the input's routing, and a GPU
  test the view among the UI's shapes; CI runs the editor headless. By hand on the RTX 4090,
  posted keys moved the camera after a click on the view and not after one on a panel, a key let
  go after a click on a panel stopped it, and the wheel over the view zoomed it.
- **The editor's camera, tools and menus** (2026-10-27): the editor's own orbit camera works alike
  in 2D and 3D, orthographic or with perspective: the wheel dollies toward the point under the
  cursor, the middle button trucks and pedestals, the right button looks about, or pans
  orthographically, and while it's held W, A, S and D fly and Q and E roll; Alt and the left button
  orbit, Alt and the right dolly, and Home eases back to the start. A toolbar over the view picks
  what the left button does (select, pan or orbit), the projection, and the grid and its plane; a
  gizmo at its top right shows the camera's turn, snaps to the six axis views, and to the opposite
  one on a second click, switches the projection and orbits by a drag. The grid draws in either
  projection, crisp in an axis view and anti-aliased otherwise, fading toward the horizon, in place
  however far from the origin. `Ui::over_view` pins UI over a dockspace's view, whose presses leave
  the view the keys, and `Context::view` says where the view is. The dock closes panels and opens
  them from a View menu, which saves, loads, deletes and resets named layouts too, and `App::dock`
  gives an app its first layout: the editor's has Entities left of the view and the Inspector
  right, over Performance. Unit tests check that the camera's moves keep the point under the cursor
  in both projections, the grid's placement and its precision far from the origin, the gizmo's
  hits, and the dock's closing, menus and layouts. By hand on the RTX 4090, through RenderDoc
  captures of whole frames: the wheel and middle, right and Pan drags kept the point under the
  cursor to the pixel, Home went back exactly, Q rolled with the right button held, Alt and the
  left button orbited, the gizmo snapped to Top and flipped to Bottom, its label switched the
  projection, and the View menu closed, reopened, saved, loaded, deleted and reset layouts; File >
  Quit exited cleanly, and the sandbox's arrows still steer its triangle.
- **Names and cameras in the world** (2026-10-27): an entity can have a `Name`, which the Entities
  and Inspector panels show it by and scenes save, and a `Camera`, a projection and whether it's
  primary, which the game's view sees through while it's primary and no layer sets a viewpoint,
  from where its entity draws between ticks, carried by its parents, its scale aside. `primary` is
  the camera's exclusive field: a component can name a `bool` field of its own so, and an
  operation that makes it true on one entity, as the Inspector's checkbox does, makes it false on
  every other, as one edit that one undo reverses; of several cameras that code or a scene made
  primary, the view sees through the lowest ID's, with a warning (`engine.camera_tie`).
  gg-renderer's camera is a `Viewpoint`, which a layer sets for a frame with
  `Context::set_viewpoint`, as the camera controllers and the editor's camera do; a projection that
  can't project is refused wherever reflection makes one (`engine.bad_projection`). The Inspector
  shows a tuple struct of one field without its `0` label. The editor starts with two cameras and
  three named squares, at three sizes and depths, which its orthographic camera shows as the sizes
  they are and its perspective one as one size, and its toolbar sees through the world's primary
  camera until the editor's camera moves. Unit tests check which camera the view sees through, of several primary too, where
  it sees from between ticks against a sprite of its transform, bit for bit, scale and mirroring,
  the tie warning, exclusive fields made true by a create, an add and sets, each undone and redone
  exactly, and their undos refused where the world has moved on, the projection's refusals by
  value, set, operation, save and load, a named camera's scene, the viewpoints of real frames, the
  Inspector's names and one-field tuples, and the editor's toggle; the `frames` test sees through
  a world camera without allocating. By hand on the RTX 4090: the editor's rows named its
  entities, the Inspector showed "Camera"'s projection and primary, the toggle showed the squares
  through "Camera" at their sizes, without grid or gizmo, checking "Second camera"'s primary had the
  view see through it, the squares the same size, and unchecked "Camera"'s, Edit > Undo switched them back, a
  middle press brought the editor's view back to the pixel, and File > Quit exited cleanly.
- **Native scripts** (2026-10-27): a component can act. A type that implements `Script` has a
  `tick` that gets its entity, the world and the tick, which a layer's tick runs among its systems
  with `run_scripts`, for each entity that has one as the pass starts, in the order queries find
  them, listed in memory the world keeps; its state is its component, which scenes, the Inspector,
  operations, the hash and snapshots reach as they do any. The editor's world cameras each have a
  `CameraController`, which W, A, S and D drive, through the editor's commands, at its ticks, while
  the view sees through the world's camera, stopping as it goes back to the editor's. Unit tests
  check a pass's order, each entity run once, entities despawned or stripped during a pass passed
  over, a script given or spawned during one running from the next tick, and one that replaces the
  world ending it; the editor's keys moving both cameras a 64th of 5 units a tick, stopping on a
  release and each way the view goes back, and moving them again by the keys held as it comes
  back; and `ticks` counts no allocation with scripts that replace their own entities. By hand on
  the RTX 4090: W held through "Camera" for 501 ms took it to y 2.5, 32 ticks of 5/64, and the
  squares 258 px down the view, 2.499 units; the Inspector showed its heading while held and 0
  after; a right press mid-hold handed the view back to the editor's camera with "Camera" stopped
  where it was; and File > Quit exited cleanly.
- **The panels** (2026-10-28): the Entities panel shows the world's hierarchy as a tree, finding
  only the rows in view, whose arrows open and close a parent's rows; it creates empty entities
  and children and deletes subtrees, from the Entity menu, a row's context menu and the one below
  the rows, where a click unpicks. The Inspector panel switches an enum's variant, as a camera's
  projection, to the fields it last had while the entity was drawn, or else their defaults; sets a
  vector a component at a time, each beside a button of its axis that resets it to its default, as
  a rotation's angles have; adds components, of their types' defaults, and removes them. Each is
  an operation or a set that the Inspector's log undoes, queued for its next tick as a command
  that recordings keep. Reflection gives a field a default, the one `#[reflect(default = ...)]`
  gives, checked as its type registers (`reflect.bad_default`), or else its type's, and refuses one
  for an entity or a type that holds itself without end (`reflect.no_default`); `World::default_of`
  makes a component's, and `World::roots`, `root_index` and `parent` list the hierarchy. Unit tests
  check the defaults of every kind under bounds, hints read through the derive, a bad default
  refused at registration, a cycle, every engine component's default made, and two compile-fail
  doctests; roots and their places through spawns, parenting, despawns and table moves; the tree's
  pages against a listing of every row at offsets up to 50,000, a frame scrolled over thousands of
  roots, and arrows on parents alone; creating, deleting, adding and removing from each menu,
  undone and redone, and a menu acting on its entity as its row moves; the commands as values;
  switching variants and back, a variant holding what's opaque disabled, each axis's reset, a drag
  of one axis while a system moves another, and sets of a vector and of its components drawn in
  order; and the sandbox's references and live recording replay unchanged. By hand on the RTX 4090:
  Entity > Create Empty picked "Empty Entity"; Add Component gave it a white square and an
  orthographic camera, height 10, not primary, which left the view as it was; Perspective showed
  fov 45° and Edit > Undo took it back; Blue's Z and scale X buttons set them to 0 and 1, an edit
  each, named by its path; Remove Component took the square away; Create Empty Child showed the
  child under its parent's open row, whose arrow closed and opened it; Delete despawned it, Undo
  brought it back under its parent, and a click below the rows unpicked; the sandbox's Entity >
  Unpick unpicked its triangle; and File > Quit exited both cleanly.
- **Styling and scene files** (2026-10-28): the UI draws in Inter, with egui's fonts behind it
  for what Inter lacks, and the Inspector's X, Y and Z buttons square, in its bold; a component's
  header is framed across the panel, its menu at the end of what shows of it, short of the scroll
  bar, and an Entities row picks from anywhere across it. `World::clear` empties a world, and
  `World::replace_scene` replaces its entities with a scene's, all or nothing; the world is then
  another, by `World::serial`, and refuses the inverses from before, and the Inspector lets go of
  its log, pick and open rows as the serial changes. `write_scene` writes a scene file whole or not
  at all, through a file of its own beside it, and through a link to the file it names, keeping
  that file's permissions and refusing one that's read only, as the dock's layout and the pipeline
  cache are now written too; `read_scene` reads one (`scene.file`); and the sandbox's grid uses
  both. In development builds `Context::pick_file` shows the OS's file dialog, through rfd, modal
  over the main window, frames pausing until it closes. The editor's File menu has New, Open...,
  Save and Save As..., with Ctrl+N, Ctrl+O, Ctrl+S and Ctrl+Shift+S: New and Open are commands that
  recordings keep, Open with the scene's text, and Save and Save As write at once. It says why a
  file can't be read, opened or written until that's dismissed, and a shortcut's key drives none
  of the world's cameras. Unit tests check clearing and replacing, in the world's model test too,
  the Inspector's reset, whole writes that leave a user's file beside them alone and refuse one
  that's read only, and on Unix keep a link and its file's permissions, the fonts and headers, a
  click at a row's far right, a menu opening from either end of its button in a panel smaller than
  its contents, the editor's commands, its shortcuts with each platform's modifiers, Save As's
  `.ron` and its refusal to replace a file unasked, and the message's dismissal; the sandbox's
  references and live recording replay unchanged. By hand on the RTX 4090: Inter and the bold axes
  showed; a click at the far right of Blue's row picked it; Save As through Windows' dialog wrote
  `level.ron`, the dialog adding the extension; New emptied the world, Undo disabled; Open brought
  back the five entities under their IDs; a broken file showed its line and column, the world and
  its Undo kept; opening again dropped the pick and Undo; Save rewrote the file in place, byte for
  byte; the working directory stayed as it was; the frame times left out the seconds the dialogs
  were open (34 ms at most); Ctrl held with S moved no camera, where S alone did; and File > Quit
  exited cleanly. After the review's fixes, the axis buttons showed square, a translation's row
  fitting the Inspector as the editor first lays it out; a click at the right end of a "…" opened
  its menu; and Save As wrote the same scene, its IDs aside, through the new writing.
- **Picking and transform handles** (2026-10-28): every draw into the game's view writes its ID
  beside its color, into an image of IDs the view's size that each frame clears to 0: the world's
  draws their entities' IDs, a layer's own 0. Every view shader writes it, at location 1, from the
  IDs whose address the push constants gain, which the shader check requires; the engine's sprite
  and blended model shaders discard what's fully transparent, so that picks see through it. The
  frame that renders the view reads back up to 64 of its pixels after its pass, and the frame two
  later finds them as it reuses its resources: `Context::pick` asks which entity the view shows at
  a pixel and gives a ticket, or none past 64 a frame, and `Context::picked` gives the answers,
  four frames later. `World::pose` gives an entity's matrix and facing as drawing composes them,
  and `Ui::viewport` the view in pixels. Another layer picks and sets fields through the
  Inspector's link, `Inspector::link`, a gesture's sets of one field one edit. Keys reach the
  view while the cursor is over it, or since a press on it, unless one of egui's widgets has focus
  or a modal is open; modifier keys reach it from anywhere. In the editor a click in the view
  picks, and Q, W, E and R choose Select, Move, Rotate and Scale, whose handles, the same size on
  screen at any distance, move, turn and scale what's picked along its own axes as drawing places
  them, under a mirrored parent too, and the nearest square ones under a parent that skews them,
  the command key snapping to half units, 45° and halves; none grabs while the camera eases. Unit
  tests check the IDs read back two frames later; the picks' bookkeeping through frames that don't
  begin and past what a frame reads, and the clear, which a draw in odd frames alone shows; the
  shader check's refusals; picks riding out with their frame and their answers coming home, and
  the cap; the link's gestures, its sets kept in the order they came, its picks and its refusals;
  keys and modifiers routed; the pose against drawing, bit for bit; the handles' rays, hits, drags
  and snapping in both projections, under turned, uneven, skewing, mirrored and tiny parents, and
  how drags end; and the editor's tool keys, camera arbitration and picks. The `frames` test picks
  every frame and, headless, gets each answer in the frame it's due, allocating nothing; windowed,
  it allocated nothing either. The GPU tests pass on the RTX 4090 and the AMD iGPU. By hand on the
  RTX 4090: clicks picked Red, Green and Blue, and one on nothing unpicked; W showed Move's
  handles, and a drag of its X arrow moved Green 120 px, 1.451 units; E over the view, with a
  panel holding the keys, chose Rotate; ring Z turned Blue about 45°, and Edit > Undo took the
  drag back in one step; R and a 60 px drag of Scale's X arrow set 2.96; W with Ctrl held snapped
  a 1.21-unit move to 1; and the editor exited cleanly under validation.
- **What's under the cursor** (2026-10-28): every frame the cursor is over the editor's view picks
  the pixel under it, and the view's bottom left names what the latest answer found, by its
  `Name`, or its ID without one, four frames behind the cursor: Hazel's hovered entity, the last
  of its mouse picking. The name is painted on the view's own layer, so that the cursor over it is
  still over the view. Unit tests check that the latest answer to a pick asked since the cursor
  came over the view says what's there, while an answer that comes back behind a later one's, or
  to a pick asked before the cursor left, says nothing; and the name, the ID and an entity gone.
  By hand on the RTX 4090, through a build that held the hover where the last click picked, since
  posted input can't hover: the corner named Red, nothing over empty space, Green, and Blue
  through the world's camera, and the editor exited cleanly under validation.
- **Content browser** (2026-10-28): the editor's Assets panel, docked below the view, shows the
  files in `apps/editor/assets` a folder at a time, as Hazel's content browser does: folders
  first, then files, by name, a double click showing a folder and Back the one above, as far up
  as the root; and a scene dragged from it onto the view, which an outline marks while one's held
  over it, opens as Open opens one. It lists its folder again each second it shows, and the folder
  holds `scenes/squares.ron`, the editor's starting world. The rest of Hazel's episode was here
  already, or waits: its camera in a uniform buffer and its shaders as SPIR-V, Vulkan's ways; its
  cache of compiled shaders, for Phase 7, which cooks them; and dropping a texture onto a sprite,
  for Phase 5, which names assets. Unit tests check the listing's order and kinds, a Cyrillic name
  among them, the double click and Back, the listing again a second on and a folder gone; a scene
  dragged from the panel onto a view that senses presses as the dock's does opening, outlined
  while it's over the view, and one let go elsewhere, or another file, opening nothing; and that
  the folder's scenes open and save as they were saved. By hand on the RTX 4090: the panel docked
  below the view, a double click showed `assets/scenes` and Back `assets`; through a build that
  held a scene tapped as though dragged, since posted input can't drag, New emptied the world and
  a tap on the view opened `squares.ron` there, under its file's IDs; both runs exited cleanly
  under validation.
- **Play, 2D physics and duplicating** (2026-10-28): Hazel's next four episodes, made the engine's
  way. A world holds resources beside its entities, which snapshots copy and restores bring back,
  clearing and replacing its entities keep, and no scene, hash, recording or dump holds;
  `World::resource_scope` lends a system the world and one apart. 2D physics is rapier2d's, built
  with `enhanced-determinism`: `RigidBody2D`, dynamic, kinematic or static, with its rotation
  fixed or not and its velocities, and `BoxCollider2D` are the engine's components, and
  `step_physics`, a system, steps the world's `Physics2D` resource a tick, taking in bodies and
  edits as they come and writing back translations, turns about Z and velocities; a body under a
  parent isn't moved (`physics.child_body`). Duplicating is an operation, which copies an entity's
  subtree typed, under IDs drawn for it, references among it to the copies and exclusive fields
  false, and which the Entity menu, a row's menu and Ctrl+D apply, picking the copy; undoing a
  create or a duplicate takes out what it made. The editor's toolbar has Play, which has a copy of
  the world play in its place, scripts and physics running and the view seeing through its camera,
  where W, A, S and D drive the cameras; and Stop, which brings back the world as it was edited,
  the very one, whose edits' history the Inspector finds again, putting a world's aside as it
  leaves it; outside play, only edits change the world. Save saves the world as edited, and New and
  Open stop it first; `apps/editor/assets/scenes/physics.ron` holds a floor, a ramp, crates and a
  bouncy box. The rest was here: entities' persistent IDs, which scenes keep, Hazel's UUIDs; a
  world copied whole, Hazel's scene copy; Ctrl+S; and the sprite shader's texture index, read by
  the instance, which isn't interpolated. Unit tests check resources through snapshots, clears,
  replaces and scopes; bodies falling and resting, edits between steps, bodies going with their
  entities, children, kinematic pushes and fixed rotations, and a snapshot stepping as its world
  did, bit for bit, where bodies made again don't; a pile's hash, the same in debug and release,
  which CI checks on every platform; duplicates' copies, IDs, references, exclusive fields and
  refusals, undone and redone; the Inspector's Duplicate, Ctrl+D and the world it left coming back;
  and the editor's Play and Stop, W, A, S and D only while playing, Save and New meanwhile. By hand
  on the RTX 4090: Add Component listed RigidBody2D and BoxCollider2D and gave them to Red; Entity >
  Duplicate made a second Red, picked; Play showed both falling through the world's camera, the
  copy still picked, and Stop brought them back, with Undo duplicate in the Edit menu, which took
  the copy out; through a build that opened `physics.ron` as it started, Play stacked the crates on
  the floor, one resting on the ramp, and Stop left the view as it started, pixel for pixel; both
  runs exited cleanly under validation.

## Now: the editor's next milestone, or Phase 5's first

The next milestone is the editor's, or [Phase 5](#next)'s first once the flagship game is decided,
as [Open questions](#open-questions) asks. It's chosen as it starts, and its plan says when it's
done.

## Next

Each phase ends in something you can run and check.

5. **Tool API and hot reload: v0.1.** The tool API in development builds, as
   [ARCHITECTURE.md](../ARCHITECTURE.md#agents-and-tools) describes. `tools/gg`, the
   command-line tool: build, run and test with machine-readable results; launch and supervise a
   game's development build, restarting it from its last snapshot after a crash or hang; talk to
   it through the API. An MCP server with the same reach. Assets named by their paths, which scenes
   save and a restart from a snapshot carries across a rebuild. Scene files edited while the game
   runs apply as operations. `subsecond` patches edits inside function bodies into the running game;
   it patches only the game's main crate and needs Dioxus's `dx` tool to build patches. The engine
   calls each layer's methods through its `HotFn` and applies patches between ticks, as
   [Game code](../ARCHITECTURE.md#game-code) describes. Any other change rebuilds the game and
   restarts it from a snapshot, matching each changed component to its new layout by field name.
   *Done when* the v0.1 loop works: watching the running game, the user asks Claude Code for a
   change, Claude Code edits the game's Rust code or applies operations through the API, and the
   game shows the change with its state intact; and an agent runs a headless check through
   `tools/gg` and acts on its result.
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

Later: Luau scripting and mods; networking (replication, lockstep, rollback); 3D physics, audio,
animation, UI; upscalers (DLSS, FSR, XeSS); ray-traced effects where the driver exposes ray
tracing (no macOS driver does today); and the built-in coding agent for v1.

## Deferred

Defects left in because fixing them costs more than they do are in [DEFECTS.md](DEFECTS.md).

| Skipped | Add when |
|---|---|
| The engine as a dynamic library in development builds, to shorten relinks | Rebuilding a game gets slow |
| Optimized dependencies in dev builds (`[profile.dev.package."*"] opt-level = 3`); zune-jpeg alone is, as it took about 0.75 s to decode the sandbox's truck unoptimized, and rapier2d isn't, which steps a pile of 13 bodies in about half a millisecond a tick unoptimized | Physics or another heavy dependency makes debug builds slow |
| Shipping profile (LTO, `codegen-units = 1`) | Phase 7, with the player |
| Compiling trace and debug events out of shipping builds, through `tracing`'s `release_max_level_*` features | Phase 7, with the shipping profile |
| No console window for shipping builds on Windows (`windows_subsystem = "windows"` on the player); the JSON log still records the run | Phase 7, with the shipping profile |
| Checking the sandbox by hand on Linux, and on macOS with KosmicKrisp (Vulkan SDK 1.4.363.0 or later, the first conformant build), its gamepads and the editor's file dialogs too, and the render thread there: acquiring, presenting and rebuilding swapchains as windows resize off the main thread under KosmicKrisp, and on Wayland a frame in flight presenting at its old size after a maximize; and the `frames` test on macOS, whose CI runner has no GPU to run it: std's mutexes there are boxed as they're first locked, so a mutex made each frame allocates there alone | A Linux machine and an Apple Silicon Mac are at hand |
| Deleting old JSON logs; every run's log is kept | Logs pile up enough to matter |
| A clean shutdown on Ctrl+C, which now ends the process at once; the JSON log keeps every line written until then, and a recording every whole line but its `end` | A headless run that goes on until it's stopped, such as a dedicated server |
| GPU tests in Windows and macOS CI, which skip them because hosted runners there have no GPU | A self-hosted runner with a GPU, or a Vulkan driver that runs on those runners without one |
| Checking windowed frames' allocations in CI, under Xvfb on Linux, with lavapipe presenting and input through XTest; they're checked by hand, with `cargo test -p gg-engine --test frames -- --windowed` | A change to the windowed loop allocates where the check by hand didn't look |
| Checking captures in CI, under RenderDoc on Linux with lavapipe: captures are checked by hand, replayed through qrenderdoc's Python for their names and labels | A name or a label is lost unseen, or the tool API checks captures |
| Capturing from code or through the tool API, and more than one frame a run: `--capture` names one frame as the run starts | Phase 5, the tool API |
| Labels inside passes: each run of draws labeled by what it draws, and the frame pass's view and UI apart; a capture shows a label for each pass | Phase 6, with the render graph |
| Synchronization validation under RenderDoc, whose layer hides the validation layer's settings: through `VK_EXT_validation_features`, or the validation layer's environment | A synchronization bug shows under RenderDoc alone |
| Captures on macOS, where RenderDoc doesn't run, so `--capture` is refused | Someone needs a capture of a Mac's frames |
| egui, the dock and the Inspector behind `dev`: the player links them, though it runs egui for its first frame alone, as an app none of whose layers draw UI does; `Ui`, which `Layer::ui` takes, is the type to turn into a no-op | Phase 7, with the shipping profile |
| The main thread takes the renderer back once a frame's UI is drawn, so a frame's input is read about a frame before it renders: a refresh under `--vsync`; and drawing between ticks draws the world up to a tick behind the simulation. Starting each frame once the render thread has acquired the image it renders into would win the first back, and extrapolating from the latest ticks the second | A game's input latency needs it |
| Answering at once what a driver sends the main thread's windows from the render thread: the main thread's wait answers each time a millisecond's wait times out, which Windows' timer tick stretches to about 16 ms, so a sent message can hold a frame that long. A Win32 event the render thread sets, waited on with `MsgWaitForMultipleObjectsEx`, would wake for both | A driver's sent messages show as stalls, such as while a window resizes |
| Pushing and popping layers while the app runs; `App` fixes the stack before it starts | A tool or game needs a layer for only part of a run |
| Composed text from an input method (winit's `Ime` events), for languages typed through one | A game takes text input, such as chat |
| Frames that draw UI allocate, the sandbox's about 900 times a frame and egui's empty pass 16: egui as it lays out and tessellates, and as it collects its input, 6 times a frame windowed, where winit reads the window's title; the dock, which copies each frame's panel and menu names and torn-off windows' titles, and makes its arrangement and its list of groups left to draw anew; the Inspector, which reads its rows' names, the picked entity's components as values and its children's IDs anew; and the render thread, which lists the windows of their own each frame one is open | A game's shipping frames draw UI, or Phase 7 puts egui behind `dev` |
| Counting what the Vulkan loader, the drivers, the validation layer and VMA allocate beneath the engine, through `VkAllocationCallbacks`, which drivers may bypass for their own: the `frames` test counts Rust's allocator | A profile shows them allocating each frame |
| Keeping within the GPU's memory budget (`VK_EXT_memory_budget` through VMA) | Phase 7, with streaming |
| Removing meshes, shaders and textures, which last until the app ends, and replacing meshes and textures; shaders are replaced as they reload | Phase 7, with asset hot reload |
| Per-frame data in a buffer: shaders get the camera's view and projection as one matrix, and see the world through each draw's transform, but not the camera's position, the time or the lights | Phase 6, with lighting |
| Materials: parameters kept on the GPU and written when they change, rather than copied every frame with each instance | Phase 6, with materials |
| Textures of data rather than color, such as normal maps, which shaders read as stored rather than decoded from sRGB | Phase 6, with PBR's maps |
| Premultiplied alpha for games' textures: with straight alpha, as PNGs store it, a linear filter at an edge mixes in the color of the transparent pixels beside it | A game's art shows dark or colored fringes |
| Sorting what blends back to front by its depth in the view, rather than by its z in the world, which orders it rightly only as seen from in front of layers parallel to XY: from behind or the side, what blends draws in the wrong order among itself, though opaque meshes, drawn first, hide it rightly; and what layers draw themselves goes in the order given. Sorting coplanar sprites each by its own depth would sort the swarm's 100,000 every frame the camera turns, and swap overlapping ones as it does | A game blends in 3D, such as particles or glass |
| Back-face culling for games' own meshes: `MeshDraw::new` and `Context::draw` draw both faces, as sprites mirrored by a negative scale need; models' materials alone cull | A game draws closed meshes of its own |
| Mirrored and unmirrored instances of one culling mesh, interleaved in the order of their IDs, draw a run each, since a run culls one way; sorting a run's instances by how they wind would keep it to two | A model instanced many times, some mirrored |
| Depth other than what a shader's blend implies: draws that ignore it, as gizmos and overlays do, a skybox that tests it without writing it, decals, and sprites cut out by their alpha that write it | Each with its first user |
| Depth bias, whose sign flips with depth reversed | Phase 6, with shadow maps |
| A far plane for perspective, as glTF's `zfar` gives; it sees out to infinity | A glTF camera with one, or Phase 8's culling |
| Keeping the view's depth past its pass, to sample: each frame clears it and doesn't store it | Post-processing, finding the point under the cursor, or the tool API's captures |
| Rendering the view's IDs only in frames that pick: every frame renders them, since a pass without them takes pipelines made without them, a second for each shader, or `VK_EXT_dynamic_rendering_unused_attachments` | A profile shows them |
| Answering picks sooner: a frame's picks are read as the frame two after it begins, after its wait for the GPU, so an answer comes four frames after its pick, about 67 ms at 60 Hz; reading them as soon as the timeline says the GPU has rendered them would win a frame or so | Picking that the latency shows in, such as highlighting what's under the cursor |
| More than 64 picks a frame: `Context::pick` gives none past them, as many as a frame reads back | A layer picks more at once, such as many cursors or touches |
| A depth format for a device without D32_SFLOAT depth, which the engine refuses (`gpu.no_device`) | A target device lacks it |
| Raw mouse motion and a grabbed cursor, for looking about past the window's edges: the fly camera controller and the editor's camera look by the cursor's moves while the right button is held | A game looks with the mouse, as first-person games do |
| Mipmaps and anisotropic filtering for the textures layers add: `add_texture` and `add_sprite_sheet` give them one level, sampled without anisotropy, so one drawn smaller than it is shimmers. A sprite sheet's cells, packed edge to edge, will then mix together at the smaller levels and under anisotropy, which reads past the texels the sprite shader keeps to, and need space around each cell or an array layer each | A game draws its own textures smaller than they are |
| Keeping a masked texture's coverage as its levels shrink: averaging thins what its alpha cuts out, such as leaves and fences, at a distance, where scaling each level's alpha to keep the share above the cutoff would keep it | A masked model thins out at a distance |
| Sprite sheets with space between or around their cells, as some tile sets come; `SpriteSheet` takes cells packed edge to edge from the top left | A game's art comes that way |
| Tiling one cell of a sheet: a repeating sampler repeats the whole texture | A game tiles part of an atlas, such as a background |
| A sprite that samples past its rectangle, so that a seamless texture tiled across sprites side by side blends across their edges under a linear sampler | A game tiles one texture across sprites |
| A tile map in the engine, which scenes save; the sandbox's map is code of its own | Phase 5, which names assets |
| glTF that `Context::add_model` refuses or ignores, as it lists: lighting and the material's other textures, skins and animations, morph targets, cameras, models without a scene and scenes other than the default, sparse accessors and accessors without a buffer view, primitives other than triangle lists, samplers that wrap across and down differently, minification filters, texture transforms, quantized or compressed meshes and textures (KHR_mesh_quantization, Draco, meshopt, Basis Universal), and integers written as floats, such as `1.0`, which glTF allows | Each with the first model that needs it; lighting and the other textures in Phase 6, compression with Phase 7's cooker |
| Decoding images off the main thread; `Context::add_texture` and `add_model` decode before they return | Phase 7, with the streaming loader |
| Models that take more than 4 GiB to read, of their buffers, decoded images, vertices and nodes, which `add_model` refuses with `engine.unsupported_model` | Phase 7, with streaming and the GPU's memory budget |
| Keeping a model's reads inside its folder where symlinks there lead out of it: its URIs can't leave the folder, but a link inside it is followed | Games load models from sources they don't trust, such as players' mods |
| Mip levels made on the CPU, on the render thread, as the frame that uploads a model's texture renders: a 2,048 by 2,048 texture's take about 0.3 s of that frame in a debug build | Phase 7, which cooks them; or blits on the GPU, if a hitch shows first |
| Layers making textures from their own pixels; `Textures::add` takes pixels, but `Context` adds only PNGs and JPEGs | A game makes its own textures |
| Instances' transforms, IDs and parameters, and the UI's vertices, indices and texture changes, reach the GPU through the frame's arena, which merges the buffers it grew into, holding two to three times the largest frame for each frame in flight, in memory the CPU writes: by its growth rule, about 20 MB each for 100,000 sprites, which use 11.2 MB a frame | Phase 7, with the GPU's memory budget |
| A frame's copies through a transfer queue start only once it's submitted, after its passes are recorded, and the frame waits for them | A profile shows a frame waiting for its uploads |
| Staging and the frame's arena kept within the device's `maxMemoryAllocationSize` and `maxBufferSize`, each at least 1 GiB: both grow by doubling, each staging buffer an allocation of its own, so a frame that adds more than that at once fails, which ends the run | A frame adds that much at once, or Phase 7, with the GPU's memory budget |
| Images' sizes checked against the device's `maxImageDimension2D` and `maxFramebufferWidth` and `Height` as they're made; a pass checks its viewport against the device's limits, which an image past those exceeds only if it's past `maxViewportDimensions` too | A device's limits fall below a window's size |
| Host image copy where a transfer queue exists, or where the driver says it costs the GPU in reading the image, as KosmicKrisp's does, turning off Apple's lossless compression, so that macOS uploads through the graphics queue; and meshes written straight into GPU memory the CPU reaches, as ReBAR and unified memory allow, rather than copied | A measurement shows either is faster |
| More than one view, such as split screen, or the editor's scene view beside the game's: the view sees through one camera, the world's primary one, or a layer's viewpoint | A game or the editor draws several |
| A camera's own aspect ratio, kept by letterboxing the view or by a target of its own, as Unreal's cameras can: a camera fits the view's aspect | Cameras render into targets of their own, or a game needs one aspect on every display |
| The editor drawing the world's cameras in its view, as outlines of what each sees | The editor shows a game's cameras |
| Moving the world's camera from the editor's view, as Unreal's Pilot and Blender's Lock Camera to View do: a move of the editor's camera goes back to its own | Someone frames a camera's shot from the view |
| A scene or prefab loaded with a primary camera making the world's other cameras not primary, as an operation's does: a load sets fields as they're written, and of several primary, the view sees through the lowest ID's, warning | A prefab brings a camera of its own, as a player's would |
| Finding the point in the world under the cursor, as `Context::pick` finds the entity there: the editor's camera finds one on a plane itself, but nothing reads the view's depth; and the orthographic camera controller zooms about the view's center | Something places what it adds at the cursor's depth, or a game zooms toward the cursor |
| A dockspace's view in a window of its own: it docks and floats over the main window alone, since only the main window's input reaches the layers, and its image is made at the main window's scale | The editor shows the game on a display of its own |
| Showing whether keys reach the view, such as an outline around it while they do: from a press on it, or with the cursor over it | Someone misses it, or Phase 9's editor |
| Gamepads following a dockspace's view: they reach the layers whichever panel has the keys, as they do in games | The editor binds a pad, or a panel takes a pad's input |
| Keeping a click that closes a menu from what's under it: it reaches the game's view there too, central or in a panel | It surprises someone, or Phase 9's editor |
| F framing what's picked, the editor's camera easing to see it whole | Someone frames often, or Phase 9's editor |
| The handles along the world's axes as well as along the entity's own, as other editors' toggles have it: they turn with what's picked | Someone moves things along the world's axes |
| Snapping steps of one's own: the command key snaps to half units, 45° and halves | Someone needs other steps |
| The amount a drag of the handles has moved, turned or scaled, shown beside them as it goes | Someone wants exact drags without the Inspector |
| Turning freely, as a trackball, by a drag inside Rotate's rings | Someone misses it |
| The handles where the picked entity draws between ticks: they show where the latest tick left it, up to a tick ahead of where a moving entity draws | Someone edits something as it moves |
| The handles along a skewed entity's axes: under a parent that scales unevenly, a turned entity's drawn axes aren't square to each other, and the handles show the nearest square ones, so their drags follow the cursor only roughly there; following the skew takes elliptical rings, and Scale along the drawn axis | Someone edits under a parent scaled unevenly |
| An entity's own negative scale flipping its handles, as a parent's does: they run along its rotation alone, so a sprite flipped by its scale shows its X arrow away from where its +X draws | A game flips sprites by their scale, and the handles confuse someone |
| Picking the next entity down where several overlap, by clicking again; and highlighting in the view what a click would pick, which the view's corner only names | Someone picks what's hidden, or misses the highlight |
| Locking the editor's view against turning, as Unity's padlock does | Someone turns a 2D view by accident |
| Switching to orthographic on an axis snap, as Blender's and Godot's gizmos do by default: the editor switches projection only when asked | Someone wants it |
| The wheel setting the editor's fly speed while the right button is held, as Unity's, Unreal's and Godot's do: it dollies then too, and flying goes at the distance a second | Flying wants another speed |
| Keeping the editor's camera, tool and grid between runs, as the dock keeps its layout | Someone misses them |
| Easing the editor's switch between projections, which jumps | It jars someone |
| A slot between the world's opaque and blended draws for the editor's grid, so that what's in front of its plane hides it and it shows over what's behind: it draws first, a backdrop that everything covers | The editor shows a world |
| A modifier key held from before the layers could see it: from a press while one of egui's widgets had focus, which egui keeps, or while another window had the keyboard, such as a torn-off panel's or another app's, it counts from its next press, or from its first repeat where the OS repeats modifiers, as Windows does | Someone holds one first |
| A rotating cursor for the editor's Orbit tool: egui has none, so it shows a hand | egui adds one, or someone misses it |
| The lines across the editor's perspective grid reaching the horizon: they fade out some 65 to 200 camera heights away, since its f32 coordinates hold four decades of spacing; the lines toward the horizon go on | Someone misses them |
| Cameras far from the origin: their matrices are f32, so 12,345 units out a view a tenth of a unit across moves in steps of about 1% | A game's world is that large |
| Zooming from keys or a gamepad, as a rate beside the wheel's lines; the orthographic camera controller zooms by the wheel alone, in its own event handling. And zooming a camera with perspective, by its field of view or by moving it in, or changing the fly camera controller's speed by the wheel, which does nothing to it | A game binds zoom elsewhere, or zooms a camera with perspective |
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
| gilrs's allocations: on Windows.Gaming.Input, its polling thread's, which reads every pad every 8 ms, and the string of a pad's ID that each event the frame takes asks Windows for; and the channel's blocks, which the frame that takes their last event frees. The engine's own feeding of pads to actions allocates only as a pad first reports, and as a second pad first holds more than one does; the windowed check of frames runs without a pad | A game's frames must allocate nothing with a pad connected |
| winit's allocations on Windows as it takes each key's press and release: a list for each message's text, and one of the messages it collects; Linux's keyboard reuses a buffer | A game's frames must allocate nothing as keys are pressed on Windows, or winit stops |
| Making the job threads' first allocations as the pool starts: each thread's first steal allocates its record in crossbeam-epoch, and on macOS its first sleep std's mutex and condition variable; the pool's other threads do both in its first ticks, and the ticks' thread sleeps only when a wait drags on, which may come at any tick. rayon-core doesn't say when a thread has slept, so a warm-up could only wait and hope | A game's ticks must allocate nothing from their first, or rayon-core says when a thread sleeps |
| Letting go, for the layers below one that handles a stick or a trigger away from rest, of the ways it left: they keep what they last saw until it rests, so that a stick rolled from right to up in a menu leaves the game going right | A game's menus take a stick |
| Controls the engine doesn't name, C, Z and paddles, and triggers a Linux driver reports as Z axes ([gilrs #200](https://gitlab.com/gilrs-project/gilrs/-/issues/200)), which are dropped | A pad that needs them, or the check by hand on Linux |
| Codes for dependencies' own warnings, which reach the log as they give them: as a pad connects on Windows.Gaming.Input, gilrs warns, without a code, that it has no mapping for it, since that backend gives every pad a nil UUID, and goes on with its default mapping, the right one there | Phase 5, as the tool API serves diagnostics |
| Checking a game's vertex and parameter structs field by field: the check compares each struct's size with its Rust type's and finds its fields 32-bit, so fields of one size in another order or of another kind, such as an integer read as a float, draw garbage without a message. A derive on vertex and parameter types would give it each field's offset, scalar type and vector width | Agents edit games' shaders, in Phase 5 |
| Wave operations in games' shaders, and the other SPIR-V capabilities a device's properties decide rather than its features, which the check refuses | A game's shader needs them |
| Hot reloading the engine's own shaders, which the build compiles in | Work on the engine's shaders wants it |
| Watching shaders' files through the OS's notifications, rather than by their times and lengths every tenth of a second, which reads every watched file each time, and, while a shader fails, every file in the directories its files are in, but none in directories within them: an include in one of those that a failing shader found broken, fixed where it is, waits for another change | Phase 7, with asset hot reload, or a game with many shaders |
| The shader watcher allocates as it looks: each shader's stamps, collected at every look on every platform; on Windows, each file's path, which it converts to UTF-16 to read the file's times; and the entries of a failing shader's directories, which it lists; the windowed check of frames runs without `dev` | A development build's frames must allocate nothing on any thread |
| Compiling at once the shaders whose changes hold on the same look, rather than one after another: after an edit to an include that N shaders share, the last reloads about N tenths of a second after the first, and no file is looked at meanwhile | A game with many shaders that share an include |
| `VK_KHR_pipeline_binary`, which saves and loads pipelines whole; ash 0.38 lacks it | ash's next release, or a driver whose pipeline cache keeps less |
| Saving the pipeline cache as pipelines are made rather than as the run ends, which a crash skips | Phase 5, which restarts games after a crash |
| Pruning the pipeline cache, which keeps every pipeline it was given, an edited shader's old ones too | It grows to tens of megabytes |
| Requiring a slangc that decorates sampled images `NonUniform` itself, Slang 2026.11's or later, such as the Vulkan SDK 1.4.363's, and dropping gg-renderer's `decorate_non_uniform`, which does it for older ones | The engine requires the Vulkan SDK 1.4.363 or later |
| Games' shaders compile with slangc as they're added, so running a game needs the Vulkan SDK | Phase 7, which cooks shaders |
| A library of assets: shaders, textures and models load from the paths the game gives, the sandbox giving its source's, and nothing finds one by path or name, so a file added twice loads twice; scenes and the tool API will name them | Phase 5, which names assets |
| `Layer::attach` can't fail, so a layer whose shader doesn't compile, or whose texture or model doesn't load, panics, as the sandbox does | A game loads assets as it starts |
| 8- and 16-bit vertex fields, whose features Vulkan 1.4 guarantees but the device doesn't enable | A mesh packs its vertices, as Phase 7's cooker will |
| The instance and VMA asking for Vulkan 1.4, and the validation layer's settings given through ash's `LayerSettingEXT::values`: ash 0.38 has 1.3's headers, so they ask for 1.3 and reach what 1.4 added through the extensions it promoted, and the settings are filled in by hand, since `values` counts bytes ([ash #985](https://github.com/ash-rs/ash/issues/985)) | ash's next release |
| A swapchain without an sRGB format falls back to the first one offered, where colors come out too dark | A device offers no sRGB swapchain format |
| Splits in a torn-off window, which holds one tab group | A tool wants panels side by side outside the main window |
| Reordering tabs by dragging them along their bar; a drop among the tabs adds one at the end | Phase 9, the editor |
| A minimum width for each panel, as Hazel keeps its Properties panel 370 pixels wide: a split keeps each side 48 points, and the Inspector's rows of axes clip in a narrower panel | Someone narrows the Inspector past its rows |
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
| A script's create, run once before its first tick: a pending bit for each slot and script type, set wherever a script arrives, as an entity is spawned or loaded, given one, or put back by an undo, and cleared as it's created or leaves, which snapshots, the layout's hash and `--dump` would carry. A script that needs setup does it in its tick, behind a field of its own | A second script type needs setup behind a field of its own |
| A script's destroy, given its value as it leaves: a despawn drops it, and a delete keeps it for its undo, which would have to destroy it only once the log lets it go | A script needs cleanup as it leaves, beyond what despawning its entity's descendants does |
| Scripts on the job system's threads: each gets the whole world, so they run one entity at a time on the tick's thread | A profile shows a script type's pass |
| Query filters: `With`, `Without`, `Option` and `Changed` | Each with its first query |
| Queries that keep the tables they match, and tables that keep where adding or removing each component leads; a query looks over every table, and a change of components finds its table by its types | Tables number in the hundreds, or changes of components show in a profile |
| Sparse storage for components added and removed often, which move their entity's row between tables | A profile shows those moves |
| A layer reading the world and drawing in one loop: `Context` lends one at a time | A layer draws from the world itself, such as debug shapes |
| Spawning many entities at once, faster than one by one: 100,000 sprites take about 16 ms in release, 5.5 ms of it their IDs, and the swarm spawns its 100,000 in one tick, longer than a tick is. A load spawns its entities one by one too, building each component from a value into a column of its own: 100,000 sprites, 45 MB of text, load in about 650 ms in release, and save in about 170 ms | A spawn hitch matters, or a scene's load time |
| `MeshDraw` parameters of more than 64 bytes, which draw through `Context::draw` | Phase 6, with materials |
| Micro-benchmarks, with criterion | A CPU hot path gets a target of its own; batching's is a frame time, which `engine.perf` measures |
| Gates on time in CI: hosted runners' times vary too much to gate on | A self-hosted runner, as for the GPU tests |
| Moving values to a type that changed: matching fields by name and by alias, defaults for fields added, and fields dropped; a scene with a field its type hasn't, or without one it has, is refused | Phase 5, when `Sprite` gains its texture's name, or with its hot reload |
| In the Inspector, turning an option on or off, and adding or removing a list's elements; by path, a value sets them whole | A component needs one edited |
| `Sprite.texture` and `MeshDraw` are opaque: shown, but not set, saved, hashed or carried across a rebuild; so the Assets panel's images drag nowhere, where Hazel's drop onto a sprite | Phase 5, which names assets |
| Flags for fields that saving, the world's hash or replication skip, and fields that are read only; `Parent` is read only as a whole type, which `set_field` refuses | Each with its first user |
| Generic types deriving `Reflect` or `Component` | A game needs one |
| Drag speeds as hints; the Inspector picks them from a field's bounds and kind | A field drags too coarsely or too finely |
| The Inspector setting only the part that changed of a rotation, and of a vector inside an option or a variant: it sets them whole, as the panel last read them, after the ticks of the layers below it, so that turning one angle of something that spins, as the sandbox's triangle does, holds the others where they were, and an undo takes back the spin made since too | Someone edits a rotation, or a vector held so, as it moves |
| Paths resolved once and kept, for sets made again and again | A profile |
| Picking an entity for a field, a parent or a child picked by clicking it; the Entities panel scrolling to an entity found or picked in the view, or to a root that an add or a remove moved to the end of another table, and so of the tree's roots; entities are shown by their names, where they have one, and their IDs, and found by an ID pasted in | Phase 9's scene tree |
| Finding an entity by its name: the Find field takes an ID | A world names more entities than the Entities panel shows at once |
| Entities rows of one line: a row whose name has a line break is as many lines tall, though the list skips the rows out of view as though each were one line tall, as egui's `show_rows` takes them; a row wider than its panel stays one line, cut off at its edge; and an empty name shows as it is | A world of more entities than the panel shows, or names from players |
| World transforms stored and propagated after the ticks, with change detection; drawing computes each child's from its ancestors, every frame, as the latest tick left them, for its z, and blended between ticks, to draw it | Bodies under a parent, or a profile showing the walks up the hierarchy |
| Where an entity draws between ticks, for a layer's viewpoint that follows it, which would otherwise judder against it, since layers read the world up to a tick ahead of where it draws: `World::pose` gives where the latest tick left it, and the engine's drawing and the world's camera compute their own | A layer's viewpoint that follows an entity |
| Turning interpolation off for an entity or a subtree, so that it draws at its ticks; every entity draws between them | A game wants something drawn at its ticks |
| Rotations of more than half a turn a tick, which draw the shorter way round, the other way; Hermite easing from velocities would draw them, and curves between ticks | Something spins that fast, or a curve between ticks shows |
| Drawing an entity despawned in a tick on its way there until the next: it vanishes up to a tick before it reaches where it stopped | A game's despawns show it, as bullets vanishing short of what they hit |
| Copying the world as only the last tick a frame runs starts: a frame that catches up copies it as each of its ticks starts | A profile shows a catching-up frame's copies |
| Copying the world, and finding what each row draws from, on the job system's threads, or only for the rows that moved: both run on the main thread, for the swarm about 0.19 ms a tick and 0.10 ms a frame that ticked; and blending only the rows that moved, which takes the job threads about 2.6 ms of their time a frame | A profile shows them |
| Sets that jump: an Inspector set of a moving entity's transform, and its undo, glide there over a tick, as a drag does, rather than reset its interpolation | Phase 5's tool API, whose agents move things by setting them |
| A flag for headless frames at a fraction of a tick of their own, for captures between ticks and for perf runs that measure blending, which headless frames, drawing their ticks, leave out; tests have them, through `App::headless_step` | Phase 5's headless captures, or a change to blending that needs measuring headless |
| Interpolation across rollback: a world restored in a tick draws where the tick left it, all of it, so rollback that restores every tick would draw at the tick rate; how rollback restores is decided with it | Rollback |
| Reparenting that keeps where a child is in the world, by decomposing its matrix, which loses any shear; `set_parent` keeps its own transform | The editor's scene tree, or an agent attaching something where it stands |
| Spawning a child into its table at once from code: `spawn`, then `set_parent`, moves it to another table; a load and a create operation spawn one there | A profile |
| Children in an order that means something: reordering them, and saving their order; an undo puts a child back last | The editor's scene tree, or UI laid out by the hierarchy |
| Each session's seed and count saved with the world and merged with it, so that an ID maps back to its session and when it was drawn, and a load refuses IDs no session drew | The tool API refusing an ID an agent made up |
| An `EntityId` query term | The first system that reads IDs as it iterates, such as replication |
| Errors carrying their entity, and a scene's line and column, as fields, not only in their message | The tool API returns errors as data |
| Drawing rows at one z in ID order for less. Every frame reads them in that order rather than their tables', about 0.9 ms of the swarm's 4.3 ms frame in release; a frame that changes rows or z's, or follows a restore, sorts them by ID again, about 2.5 ms more for 100,000; and 100,000 sprites each at a z of their own, as sorting by y gives them, take about 2 ms more than before, for a group and an item each. Tables kept in ID order would make the reads a sweep, and a radix sort the sort quicker | A game's frames need the time |
| Linked prefab instances, with overrides, nesting, apply and revert; a placement is a flattened copy, each ID mixed from its instance's and its own, which links will keep | The editor, or a game editing a prefab its scenes hold many copies of |
| Placing a prefab without parsing it again: `World::place_prefab` parses its text each time, which allocates | A game places prefabs as it runs, rather than as it loads |
| A derive hint for `Component::base`, and opaque values in lists and options, which saving refuses, as does a set's undo (`reflect.opaque`) where the old value holds something opaque in a variant, an option or a list; a type with opaque fields implements `Component` by hand to load, as `Sprite` does | A game's component holds something opaque |
| Names in scenes beyond ASCII identifiers, which saving refuses (`scene.bad_name`) | A game names a type or a field so |
| Loading a scene in place of some of a world's entities in one step, as the sandbox's grid reload does in three; a load refuses IDs the world has, and `World::replace_scene` replaces all of its entities | Phase 5's scene files applied as operations |
| A version in scene files, a file without one reading as the first | The first change to the format that old files can't be read by |
| Comments in scene files, which a save drops | People annotate scenes |
| A prompt before the editor's New, Open or Quit, or a scene dropped on its view, discards edits not saved: they discard them | Someone loses work, or Phase 9's editor |
| The scene's file named in the editor's title bar, and a list of recent scenes: Save writes to the file opened or saved last, which nothing shows | Someone loses track of which file Save writes |
| Dragging a scene onto the view from the Assets panel torn off into a window of its own: egui's drag stays in the window it started in, so the view never sees it let go | Someone drags from a torn-off Assets panel |
| Listing the Assets panel's folder off the main thread, as a thread watching it would: it's listed on the main thread each second it shows, which a folder of many thousands of files would hold up for milliseconds | A game's folder that big, once the tool API brings one |
| The Play button centered over the view, as Hazel's is, or in the title bar, as Unity's: it's at the end of the editor's toolbar, which spans the view's middle | The title bar takes a layer's UI, or the toolbar moves |
| Pause, step and Simulate, physics through the editor's camera, as Hazel's later episodes add | Someone steps through a play |
| Play in a process of its own, through the tool API, so that a crash as the world plays stops only the game | Phase 9's editor |
| Keeping edits made as the world plays, which go with the copy, as Unity's do; and showing that it plays beyond the lit Stop and the view through the world's camera, such as a tint round the view | Someone loses an edit made while playing, or edits a playing world for the scene |
| Bodies under a parent, moved in the world's space as their ancestors carry them, and colliders on a body's children; a body under a parent isn't moved (`physics.child_body`) | A game builds a body of parts, or nests one |
| Colliders other than boxes, such as circles, capsules and polygons; joints; sensors; and contacts and queries, such as raycasts, for scripts | Each with its first game use |
| 3D physics | A 3D game |
| Gravity from a scene or a setting: `Physics2D::gravity` is code's, Earth's by default | A game sets its own |
| Physics steps that allocate nothing: rapier allocates as contacts come and go, and the `ticks` test steps no physics | A game's ticks must allocate nothing with physics |
| Physics' state in the world's hash, `--dump` and `World::difference`, which read entities alone, as they read no resource: a step that goes another way inside rapier shows in the bodies' transforms a tick later | A replay goes another way inside physics unseen for long |
| The log in `Context`, every layer's and the tool API's clients each undoing their own, and an undo logged as an operation; the Inspector keeps a log of its own | Phase 5, the tool API |
| Replaying a log into another world, which maps values' handles to IDs, as a recording's commands are; `set_field` and operations store a dead handle without complaint | Phase 5, the tool API |
| A delete saying what refers to its entities from outside, or refusing; those references dangle until it's undone, and the world doesn't save in between | An agent deletes through the tool API |
| Undoing a create whose entity has children since that the create didn't make, such as game code gave it, by keeping them where they are; it's refused (`ecs.has_children`), and the log drops it | An agent attaches children to what another's create made, or the editor |
| Layers holding what a restore takes back: the sandbox's swarm is in the world, and the Inspector drops its log and pick once the world's serial is another's, but nothing else tells a layer that `Context`'s world was restored, so the sandbox doesn't snapshot | Something restores the running world, such as rollback or Phase 5's restart |
| Reparenting from the panels: dragging a row onto another, and taking a `Parent` away, as Remove Component would, making its entity a root; `Parent` is read only, so the Inspector offers neither | The editor's scene tree |
| Picking several entities, to edit or delete them at once | Someone edits many entities alike |
| A search field in Add Component's menu, which lists every type registered | A game registers more types than the menu shows at once |
| A Delete key for the entity picked: keys reach egui even while the view has them, so it would delete as the world plays, its keys driving it, as Ctrl+Z undoes and Ctrl+D duplicates then; the Entity menu and a row's context menu delete | Keys go to the panel under the pointer, or the view takes them from egui |
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
- Which license? Recommendation: MIT OR Apache-2.0, the Rust convention, chosen once; until
  then, `publish = false` keeps the crates off crates.io. Decide before anyone outside the
  project contributes.
