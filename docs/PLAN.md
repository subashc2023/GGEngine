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

## Now: persistent entity IDs

Phase 3's fourth milestone: every entity gets a persistent ID, random rather than sequential,
which files, operations, diagnostics and messages name it by, as
[ARCHITECTURE.md](../ARCHITECTURE.md#project-state) describes; handles stay the world's own.

*Done when* the world finds each entity by its ID, and the Inspector and the engine's logs name
entities by their IDs. Its plan is made when it starts.

## Next

Each phase ends in something you can run and check.

3. **Runtime core.** Our own ECS with a transform hierarchy, reflected component types
   registered at runtime, stable entity IDs, scenes and prefabs in canonical text, operations
   with inverses and their log, and whole-world snapshots and restores. A fixed timestep, with
   commands as the only way into the simulation; the deterministic mode, with a world hash per
   tick, recordings and replays, and math that is identical everywhere (glam's `scalar-math`
   and `libm` features, or math types of our own); input actions; a job system. All of it runs
   headless. *Done when* a scene saved, loaded and saved again is byte-identical; undoing a log
   of operations gives back the original world's hash; a restored snapshot matches the world it
   was taken from; steady-state ticks don't allocate; and a recording replays to the same
   hashes on all three OSes and both architectures, with a forced divergence reported as its
   first tick, entity and field.
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
| Checking the sandbox by hand on Linux, and on macOS with KosmicKrisp (Vulkan SDK 1.4.363.0 or later, the first conformant build) | Phase 4, once the sandbox draws a glTF mesh |
| Deleting old JSON logs; every run's log is kept | Logs pile up enough to matter |
| A clean shutdown on Ctrl+C, which now ends the process at once; the JSON log keeps every line written until then | A headless run that goes on until it's stopped, such as a dedicated server |
| GPU tests in Windows and macOS CI, which skip them because hosted runners there have no GPU | A self-hosted runner with a GPU, or a Vulkan driver that runs on those runners without one |
| A gate that compiles the shipping configuration: `cargo clippy -p player --all-targets -- -D warnings` | When `gg-engine` gets its `dev` feature |
| egui, the dock and the Inspector behind `dev`: the player links them and runs an empty UI pass every frame; `Ui`, which `Layer::ui` takes, is the type to turn into a no-op | When `gg-engine` gets its `dev` feature |
| Waiting for the swapchain image before a frame reads input and the time, not after the layers update, which would cut latency by the wait: up to a refresh under `--vsync` | Phase 3, with input actions |
| Pushing and popping layers while the app runs; `App` fixes the stack before it starts | A tool or game needs a layer for only part of a run |
| Composed text from an input method (winit's `Ime` events), for languages typed through one | A game takes text input, such as chat |
| Steady-state frames that draw UI allocate: in egui's layout and tessellation, in the dock's own bookkeeping, which copies each frame's panel and menu names and torn-off windows' titles, and makes its arrangement and its list of groups left to draw anew, and in the Inspector, which reads its rows' names, the picked entity's components as values and its children's handles anew | Phase 4, with the allocation check; the overlay is for development |
| Keeping within the GPU's memory budget (`VK_EXT_memory_budget` through VMA) | Phase 7, with streaming |
| Removing meshes, shaders and textures; each lasts until the app ends | Something adds and removes them as it runs, such as asset hot reload in Phase 4 |
| Perspective cameras, and a controller for them; a camera is orthographic, and so is the engine's controller | Phase 4, with depth |
| Per-frame data in a buffer: shaders get the camera's view and projection as one matrix, and see the world through each draw's transform, but not the camera's position, the time or the lights | Phase 6, with lighting |
| Materials: parameters kept on the GPU and written when they change, rather than copied every frame with each instance | Phase 6, with materials |
| Counts of vertices, indices or triangles, which follow from the instances; the renderer counts draws and instances | Phase 4, with glTF meshes |
| Textures of data rather than color, such as normal maps, which shaders read as stored rather than decoded from sRGB | Phase 6, with PBR's maps |
| Premultiplied alpha for games' textures: with straight alpha, as PNGs store it, a linear filter at an edge mixes in the color of the transparent pixels beside it | A game's art shows dark or colored fringes |
| Sorting what blends back to front by depth; the world's entities draw back to front by their z's in the world, and what layers draw themselves in the order given | Phase 4, with depth |
| Mipmaps, and anisotropic filtering: a texture drawn smaller than it is samples its one level, and shimmers. A sprite sheet's cells, packed edge to edge, will then mix together at the smaller levels and under anisotropy, which reads past the texels the sprite shader keeps to, and need space around each cell or an array layer each | Phase 4, with the textured glTF mesh |
| Sprite sheets with space between or around their cells, as some tile sets come; `SpriteSheet` takes cells packed edge to edge from the top left | A game's art comes that way |
| Tiling one cell of a sheet: a repeating sampler repeats the whole texture | A game tiles part of an atlas, such as a background |
| A sprite that samples past its rectangle, so that a seamless texture tiled across sprites side by side blends across their edges under a linear sampler | A game tiles one texture across sprites |
| A tile map in the engine, which scenes save; the sandbox's map is code of its own | Phase 3, with scenes |
| Images other than PNG, such as the JPEGs glTF allows; and PNGs' own color profiles and gamma, which are ignored, every PNG taken as sRGB | Phase 4, with glTF; Phase 7 cooks KTX2 |
| Decoding images off the main thread; `Context::add_texture` decodes before it returns | Phase 7, with the streaming loader |
| Layers making textures from their own pixels; `Textures::add` takes pixels, but `Context` adds only PNGs | A game makes its own textures |
| Pixels, vertices and instances' transforms and parameters reach the GPU through the frame's arena, which merges the buffers it grew into, holding two to three times the largest frame for each frame in flight: by its growth rule, about 20 MB each for 100,000 sprites, which use 10.4 MB a frame; and a frame that adds many textures leaves that much host memory held | Phase 4, with uploads through the transfer queue |
| More than one camera or view, such as split screen or the editor's scene view | A game or the editor draws several |
| The camera as a component of an entity, which a scene saves and a parent carries along; layers set the camera through `Context` | Phase 3, with scenes |
| The game's view in a panel that docks and tabs, and keys reaching the layers only while it has focus, as editors' views do; the view is the central area, and keys reach the layers wherever the pointer is unless one of egui's widgets has focus | A tool in the game's process shows the view among its panels |
| Converting between the view's pixels and the world, to pick what's under the cursor or zoom toward it; the camera controller zooms about the view's center | Something picks with the mouse, or a tool zooms toward the cursor |
| The camera controller's keys, W, A, S, D, Q and E, and the mouse wheel, are fixed, and it takes no gamepad | Phase 3, with input actions |
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
| Torn-off windows across monitors of different scales: one that opens on another monitor is placed at the main window's scale, away from the cursor. Nor is Windows' frame checked there, which grows back around a window as its scale changes | Someone docks across such monitors |
| In a frame egui runs twice, the torn-off windows' texture changes apply before all of the main window's, so glyphs added then to a font atlas that grows can go missing | It shows, or egui hands over each pass's changes apart |
| Present fences (`VK_KHR_swapchain_maintenance1`) before a swapchain's semaphores are destroyed, which closing torn-off windows makes more frequent | Validation reports it, or windows open and close often |
| egui's own scopes in Tracy, which egui marks for the `profiling` crate: `profiling` 1.0.18 takes tracy-client 0.18, and two Tracy clients don't link together | `profiling` takes tracy-client 0.19; deny.toml's wrappers for tracy-client then add it |
| Changing the world's tables while a query runs, through a buffer applied after it: Bevy's `Commands`, to be called `Deferred` here, since commands are the simulation's inputs | The first system that spawns or despawns as it iterates |
| Query filters: `With`, `Without`, `Option` and `Changed` | Each with its first query |
| Queries that keep the tables they match, and tables that keep where adding or removing each component leads; a query looks over every table, and a change of components finds its table by its types | Tables number in the hundreds, or changes of components show in a profile |
| Sparse storage for components added and removed often, which move their entity's row between tables | A profile shows those moves |
| A layer reading the world and drawing in one loop: `Context` lends one at a time | A layer draws from the world itself, such as debug shapes |
| A tiebreak for entities at one z, which draw in the order queries find them | Persistent IDs, or Phase 4's depth sort |
| Spawning many entities at once, faster than one by one: the swarm's 100,000 take about 17 ms in release, in one frame | A spawn hitch matters |
| `MeshDraw` parameters of more than 64 bytes, which draw through `Context::draw` | Phase 6, with materials |
| Micro-benchmarks, with criterion | A CPU hot path gets a target of its own; batching's is a frame time, which `engine.perf` measures |
| Gates on time in CI: hosted runners' times vary too much to gate on | A self-hosted runner, as for the GPU tests |
| Adding and removing components by name, each returning its inverse, and each type's default as a value | Phase 3, with operations |
| Moving values to a type that changed: matching fields by name and by alias, defaults for fields added, and fields dropped | Phase 3's scenes, or Phase 5's hot reload |
| Undo, and an Inspector drag merged into one edit; edits apply at once | Phase 3, with operations |
| In the Inspector, turning an option on or off, switching to a variant with fields, and adding or removing a list's elements; by path, a value sets them whole | A component needs one edited |
| `Sprite.texture` and `MeshDraw` are opaque: shown, but not set, saved or carried across a rebuild | Phase 4, which names assets |
| Flags for fields that saving, the world's hash or replication skip, and fields that are read only; `Parent` is read only as a whole type, which `set_field` refuses | Each with its first user |
| Generic types deriving `Reflect` or `Component` | A game needs one |
| Drag speeds as hints; the Inspector picks them from a field's bounds and kind | A field drags too coarsely or too finely |
| Paths resolved once and kept, for sets made again and again | Operation logs, or a profile |
| Picking an entity for a field, a parent or a child picked by clicking it, the Entities panel as a tree, names in the Inspector, and picking in the view; entities are shown by handle | Persistent IDs, or picking |
| World transforms stored and propagated after the updates, with change detection; drawing computes each child's from its ancestors, every frame | Physics, or a profile showing the walks up the hierarchy |
| A world matrix for game code; the engine's drawing computes its own | The camera as a component, or a system that needs a child's place in the world, once the deterministic mode settles its math |
| Reparenting that keeps where a child is in the world, by decomposing its matrix, which loses any shear; `set_parent` keeps its own transform | The editor's scene tree, or an agent attaching something where it stands |
| Spawning a child into its table at once: `spawn`, then `set_parent`, moves it to another table | Prefabs, or a profile |
| Children in an order that means something: reordering them, and saving their order | The editor's scene tree, or UI laid out by the hierarchy |

## Open questions

- Which game is the flagship? Recommendation: a networked game whose design depends on
  determinism, so the simulation core, the tool API and multiplayer are exercised from the
  start. Decide before Phase 5, v0.1.
- Editor UI toolkit: egui, which the engine's overlay already uses, or GPUI? Decide before
  Phase 9.
- Which license? Recommendation: MIT OR Apache-2.0, the Rust convention, chosen once; until
  then, `publish = false` keeps the crates off crates.io. Decide before anyone outside the
  project contributes.
