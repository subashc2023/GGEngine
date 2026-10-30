# Plan

The roadmap. [AGENTS.md](../AGENTS.md) says how to keep it, under Workflow; the design is in
[ARCHITECTURE.md](../ARCHITECTURE.md).

## Done

- **Phase 0, workspace** (2026-10-20)
- **Phase 1, entry point, logging and Vulkan bring-up** (2026-10-20)
- **Phase 2, events, layers and the UI overlay** (2026-10-20)
- **Docking and viewports** (2026-10-21)
- **Title bars the UI draws** (2026-10-21)
- **Windows' own frame around the UI's title bars** (2026-10-21)
- **First triangle** (2026-10-21)
- **Shaders and vertex types** (2026-10-21)
- **Cameras and transforms** (2026-10-21)
- **Shader parameters** (2026-10-21)
- **Textures and blending** (2026-10-21)
- **Camera controller** (2026-10-21)
- **Sprites** (2026-10-21)
- **Profiling** (2026-10-21)
- **Batching** (2026-10-21)
- **Sprite sheets** (2026-10-22)
- **The game's view offscreen** (2026-10-22)
- **ECS core** (2026-10-22)
- **Reflection** (2026-10-22)
- **Transform hierarchy** (2026-10-22)
- **Persistent entity IDs** (2026-10-23)
- **Scenes and prefabs in canonical text** (2026-10-23)
- **Operations with inverses and their log** (2026-10-23)
- **Whole-world snapshots and restores** (2026-10-23)
- **A fixed timestep, with commands** (2026-10-23)
- **Deterministic math and the world hash** (2026-10-24)
- **Recordings and replays** (2026-10-24)
- **Input actions** (2026-10-24)
- **Gamepads** (2026-10-24)
- **A job system** (2026-10-24)
- **Phase 3, runtime core** (2026-10-24)
- **The render thread** (2026-10-25)
- **Rendering interpolated between fixed steps** (2026-10-25)
- **Depth and a perspective camera** (2026-10-25)
- **Asset uploads** (2026-10-25)
- **Textured glTF models** (2026-10-25)
- **Shaders checked, hot reloaded and their pipelines cached** (2026-10-25)
- **Frames of apps without UI allocate nothing** (2026-10-26)
- **RenderDoc captures** (2026-10-26)
- **Phase 4, first mesh** (2026-10-26)
- **The editor's shell** (2026-10-27)
- **The editor's camera, tools and menus** (2026-10-27)
- **Names and cameras in the world** (2026-10-27)
- **Native scripts** (2026-10-27)
- **The panels** (2026-10-28)
- **Styling and scene files** (2026-10-28)
- **Picking and transform handles** (2026-10-28)
- **What's under the cursor** (2026-10-28)
- **Content browser** (2026-10-28)
- **Play, 2D physics and duplicating** (2026-10-28)
- **Circles, lines and colliders' outlines** (2026-10-28)
- **Title bar, menus and the Inspector's numbers** (2026-10-28)
- **Simulate and the pick's outline** (2026-10-28)
- **Spring cleaning** (2026-10-29)
- **The tool API and `tools/gg`** (2026-10-29)
- **Restarting a game from a snapshot** (2026-10-29)
- **subsecond's patches** (2026-10-29)
- **Operations through the API** (2026-10-30)
- **The first demo** (2026-10-30)
- **Phase 5, tool API and hot reload: v0.1** (2026-10-30)
- **Projects and `gg new`** (2026-10-30)

## Now

**The game's view in the editor**, Phase 6's second milestone. The editor opens the project it's
started in and runs its game as an agent would, under `cargo gg run` in the project's directory,
ending it as the editor closes, and shows the game's view in its Viewport: the game runs
`--embedded`, offscreen without a window, publishing its view's frames to the editor through the
tool API, as the first open question below decides, and the editor forwards its input to it. A
Logs panel shows `gg run`'s events and the build's errors, and the title bar the run's status,
pause and step. *Done when*, from `games/shards`, the editor opens Shards' project and shows it
running in the Viewport, steered by its keys; an edit inside a function body is patched in, and a
new field rebuilds and restarts the game, both showing in the view and in Logs; a second `gg run`
of the game is refused; closing the editor ends the game; and the cost and age of the view's
frames at 1920x1080 are measured against budgets set as it starts.

## Next

Each phase ends in something you can run and check.

6. **Projects and the editor on the tool API.** A project: a manifest naming its game's crate, its
   asset root, its scenes and the scene it starts in, which `gg new` makes and every tool reads.
   `apps/editor` opens a project and works on its game, running in its own process, through the
   tool API alone: its panels (the scene tree among them), Play, Simulate, Stop, pause and step, and
   its File menu reaching the game through it; a panel of logs and errors; and the game's view,
   embedded. *Done when* the editor opens the demo's project, has no path into the game but the
   tool API, and the v0.1 loop works from inside it. Its milestones, in order: projects and
   `gg new`; the game's view in the editor; the game's world and diagnostics in the editor's
   panels; editing in the game's view; Play, Simulate and Stop in the game, the editor's own world
   deleted; and scenes through the API and the File menu.
7. **Assets.** An asset database: every asset of a project found by a stable ID kept in a text file
   beside it, which scenes name it by, with its import settings; assets loaded once whoever asks,
   by ID or by path, and reloaded as their files change, textures, meshes and shaders alike; and the
   editor's Assets panel dropping a texture onto a sprite, a model into the scene. *Done when* a
   texture dragged from the Assets panel onto a sprite saves in its scene by ID, survives renaming
   its file, and an edit to it in an image editor shows in the running game.
8. **Text and sound.** Text drawn from multi-channel signed distance fields, crisp at any size,
   fonts baked by the asset pipeline; a game's own UI, HUDs and menus, apart from the development
   UI; and audio: sounds and music, mixed, positioned in 2D, loaded as assets. *Done when* the demo
   shows its scores and a menu in text and plays its sounds.
9. **Scripting.** Luau, as [Game code](../ARCHITECTURE.md#game-code) describes: scripts as assets,
   attached to entities, their fields in the Inspector, reaching other entities and Rust components
   through reflection, reloaded as they're saved with the world kept, deterministic for lockstep
   and replays, and debugged with breakpoints, pause and step; and the tool API's capabilities, so
   that a mod's scripts reach only what they're granted. *Done when* a game's behavior written in
   Luau reloads as it's saved, in sync on both peers of the demo, and a mod runs sandboxed.
10. **Cooking and the player.** `tools/cook` turns glTF into engine meshes (meshoptimizer: vertex
    and index optimization, meshlets, LODs), textures into BC-compressed KTX2, fonts into their
    fields and games' shaders into SPIR-V, packed into a memory-mappable package; an async
    streaming loader with a per-frame upload budget; the player runs a package under a shipping
    profile (LTO, one codegen unit). *Done when* the demo ships as a cooked package its own player
    runs on all three OSes, with load time and memory measured.
11. **Networking beyond lockstep.** Rollback, from the snapshots and replays the world already has,
    with a SyncTest; a server that owns the world, replicating it to clients that predict; and a
    desync debugger that names the entity and field where peers part. *Done when* the demo plays
    over a link with 100 ms of latency and loss, under rollback and under a server, without a
    desync, and a desync forced in a test is named to its field.
12. **Render graph and PBR.** Passes declare the resources they read and write; the graph
    places barriers and layout transitions, aliases transient memory and schedules async
    compute. Physically based shading (glTF metallic-roughness), image-based lighting, cascaded
    shadow maps, HDR and tonemapping; reference-image tests on lavapipe. *Done when* Sponza
    renders lit and shadowed, synchronization validation is clean, and per-pass GPU timings
    show in Tracy.
13. **GPU-driven rendering.** GPU culling (frustum, then two-pass occlusion against a depth
    pyramid); indirect draws; meshlets, through mesh shaders where supported and a compute path
    elsewhere; a visibility buffer; clustered lighting; TAA. *Done when* CPU frame time stays
    flat as the instance count grows, and a cooked Amazon Lumberyard Bistro with many instances
    meets a frame-time target set in this phase on the RTX 4090 and an Apple Silicon Mac.

Later: 3D physics, animation (sprite flipbooks and tweens, then skeletal), particles, linked
prefabs; upscalers (DLSS, FSR, XeSS); ray-traced effects where the driver exposes ray tracing (no
macOS driver does today); and the built-in coding agent for v1.

## Deferred

Defects are in [DEFECTS.md](DEFECTS.md), which says what goes there rather than here.

| Skipped | Add when |
|---|---|
| The engine as a dynamic library in development builds, to shorten relinks | Rebuilding a game gets slow |
| Optimized dependencies in dev builds (`[profile.dev.package."*"] opt-level = 3`); zune-jpeg alone is, for the reason the root Cargo.toml gives | Physics or another heavy dependency makes debug builds slow |
| Compiling trace and debug events out of shipping builds, through `tracing`'s `release_max_level_*` features | Phase 10, with the shipping profile |
| No console window for shipping builds on Windows (`windows_subsystem = "windows"` on the player); the JSON log still records the run | Phase 10, with the shipping profile |
| Checking the sandbox by hand on Linux, and on macOS with KosmicKrisp (Vulkan SDK 1.4.363.0 or later, the first conformant build), its gamepads, the editor's file dialogs and the drawn title bars too, with the menu button's inset clear of macOS's traffic lights (72 points) and a torn-off window moving by its tab bar where X11 and macOS move windows asynchronously, and the render thread there: acquiring, presenting and rebuilding swapchains as windows resize off the main thread under KosmicKrisp, and on Wayland a frame in flight presenting at its old size after a maximize; and the `frames` test on macOS, whose CI runner has no GPU to run it: std's mutexes there are boxed as they're first locked, so a mutex made each frame allocates there alone | A Linux machine and an Apple Silicon Mac are at hand |
| Deleting old JSON logs; every run's log is kept | Logs pile up enough to matter |
| A clean shutdown on Ctrl+C, which now ends the process at once; the JSON log keeps every line written until then, and a recording every whole line but its `end` | A headless run that goes on until it's stopped, such as a dedicated server |
| GPU tests in Windows and macOS CI, which skip them for the reason [AGENTS.md](../AGENTS.md#commands) gives | A self-hosted runner with a GPU, or a Vulkan driver that runs on those runners without one |
| Checking windowed frames' allocations in CI, under Xvfb on Linux, with lavapipe presenting and input through XTest; they're checked by hand, as [AGENTS.md](../AGENTS.md#engine-rules) says | A change to the windowed loop allocates where the check by hand didn't look |
| Checking captures in CI, under RenderDoc on Linux with lavapipe: captures are checked by hand, replayed through qrenderdoc's Python for their names and labels | A name or a label is lost unseen, or the tool API checks captures |
| RenderDoc captures from code or through the tool API, and more than one a run: `--capture` names one frame as the run starts, and the tool API's `capture` writes the view's pixels alone | An agent needs a frame's GPU work as well as its pixels |
| `gg build` and `gg test`, with their results as data: `run` and `check` build through cargo, passing on its messages about the code as it renders them, and cargo's test harness writes JSON on nightly alone | An agent needs a build's or the tests' results as data, or libtest's JSON output is stable |
| Capabilities each client of the tool API is granted, as [Agents and tools](../ARCHITECTURE.md#agents-and-tools) describes: a connection that says hello with the session's token reaches everything, which only processes that can read the session file beside the game do | A client the user doesn't run as their own, such as one on another machine or a mod's, or an agent kept to one scene |
| Diagnostics served by the tool API: `gg logs` reads the JSON log the session file names | A client on another machine, or one that follows a run's events as they come |
| MCP beyond tools: resources, prompts, progress, cancellation and the client's roots; `gg mcp`'s tools are the API's methods but `quit`, `patch` and the paths of `capture` and `snapshot`, a capture's PNG coming inline | An agent needs one of them |
| Watching the scene files the editor opens, saves and reloads, which it reads through `World::replace_scene` rather than `Context::load_scene_file` | Phase 6's editor |
| Steps and captures while the window is minimized, which fail with `api.paused`, and requests while a file dialog or Windows' window menu holds the main thread, which wait; `ping` says which | A tool needs them then |
| Labels inside passes: each run of draws labeled by what it draws, and the frame pass's view and UI apart; a capture shows a label for each pass | Phase 12, with the render graph |
| Synchronization validation under RenderDoc, whose layer hides the validation layer's settings: through `VK_EXT_validation_features`, or the validation layer's environment | A synchronization bug shows under RenderDoc alone |
| Captures on macOS, where RenderDoc doesn't run, so `--capture` is refused | Someone needs a capture of a Mac's frames |
| egui, the dock and the Inspector behind `dev`: the player links them, though it runs egui for its first frame alone, as an app none of whose layers draw UI does; `Ui`, which `Layer::ui` takes, is the type to turn into a no-op | Phase 10, with the shipping profile |
| The main thread takes the renderer back once a frame's UI is drawn, so a frame's input is read about a frame before it renders: a refresh under `--vsync`; and drawing between ticks draws the world up to a tick behind the simulation. Starting each frame once the render thread has acquired the image it renders into would win the first back, and extrapolating from the latest ticks the second | A game's input latency needs it |
| Pushing and popping layers while the app runs; `App` fixes the stack before it starts | A tool or game needs a layer for only part of a run |
| Composed text from an input method (winit's `Ime` events), for languages typed through one | A game takes text input, such as chat |
| Frames that draw UI and allocate nothing: egui allocates as it lays out and tessellates, and as it collects its input, where winit reads the window's title, and so do the dock, the Inspector, and the render thread while a panel has a window of its own | A game's shipping frames draw UI, or Phase 10 puts egui behind `dev` |
| Counting what the Vulkan loader, the drivers, the validation layer and VMA allocate beneath the engine, through `VkAllocationCallbacks`, which drivers may bypass for their own: the `frames` test counts Rust's allocator | A profile shows them allocating each frame |
| Keeping within the GPU's memory budget (`VK_EXT_memory_budget` through VMA) | Phase 10, with streaming |
| Removing meshes, shaders and textures, which last until the app ends, and replacing meshes and textures; shaders are replaced as they reload | Phase 7, with asset hot reload |
| Per-frame data in a buffer: shaders get the camera's view and projection as one matrix, and see the world through each draw's transform, but not the camera's position, the time or the lights | Phase 12, with lighting |
| Materials: parameters kept on the GPU and written when they change, rather than copied every frame with each instance; `MeshDraw`'s cap of 64 bytes of parameters, past which a shader draws through `Context::draw`, goes with it | Phase 12, with materials |
| Textures of data rather than color, such as normal maps, which shaders read as stored rather than decoded from sRGB | Phase 12, with PBR's maps |
| Premultiplied alpha for games' textures: with straight alpha, as PNGs store it, a linear filter at an edge mixes in the color of the transparent pixels beside it | A game's art shows dark or colored fringes |
| Sorting what blends back to front by its depth in the view, rather than by its z in the world, which orders it rightly only as seen from in front, as [Rendering](../ARCHITECTURE.md#rendering) says; and what layers draw themselves goes in the order given. Sorting coplanar sprites each by its own depth would sort the swarm's 100,000 every frame the camera turns, and swap overlapping ones as it does | A game blends in 3D, such as particles or glass |
| Back-face culling for games' own meshes: `MeshDraw::new` and `Context::draw` draw both faces, as sprites mirrored by a negative scale need; models' materials alone cull | A game draws closed meshes of its own |
| Mirrored and unmirrored instances of one culling mesh, interleaved in the order of their IDs, draw a run each, since a run culls one way; sorting a run's instances by how they wind would keep it to two | A model instanced many times, some mirrored |
| Depth other than what a shader's blend implies: draws that ignore it, as gizmos and overlays do, a skybox that tests it without writing it, decals, and sprites cut out by their alpha that write it | Each with its first user |
| Depth bias, whose sign flips with depth reversed | Phase 12, with shadow maps |
| Depth bias for lines, so that one lying on an opaque mesh's face shows whole: across its width, a line's quad keeps the line's depth, so where the face slopes across the line in the view, it hides half of it | Lines drawn on 3D surfaces, as debug drawing in 3D |
| Joins where lines meet: lines end squared off, so a rectangle's sides overlap at its corners, which a translucent one shows darker, and a thick polyline's segments overlap at each bend | A game draws translucent outlines or thick polylines |
| Circle colliders' outlines of as many lines as their size on screen needs: each is 64, round to within a pixel up to some 800 pixels across, so a larger one shows its sides and a smaller one draws more lines than it needs | A game shows the outlines of many circle colliders, or a large one shows its sides |
| A far plane for perspective, as glTF's `zfar` gives; it sees out to infinity | A glTF camera with one, or Phase 13's culling |
| Keeping the view's depth past its pass, to sample: each frame clears it and doesn't store it | Post-processing, finding the point under the cursor, or captures of depth |
| Rendering the view's IDs only in frames that pick or outline: every frame renders them, for the reason [Rendering](../ARCHITECTURE.md#rendering) gives | A profile shows them |
| Answering picks sooner than [Rendering](../ARCHITECTURE.md#rendering) says they come: reading them as soon as the timeline says the GPU has rendered them would win a frame or so | Picking that the latency shows in, such as highlighting what's under the cursor |
| More than 64 picks a frame: `Context::pick` gives none past them, as many as a frame reads back | A layer picks more at once, such as many cursors or touches |
| A depth format for a device without D32_SFLOAT depth, which the engine refuses (`gpu.no_device`) | A target device lacks it |
| Raw mouse motion and a grabbed cursor, for looking about past the window's edges: the fly camera controller and the editor's camera look by the cursor's moves while the right button is held | A game looks with the mouse, as first-person games do |
| Mipmaps and anisotropic filtering for the textures layers add: `add_texture` and `add_sprite_sheet` give them one level, sampled without anisotropy, so one drawn smaller than it is shimmers. A sprite sheet's cells, packed edge to edge, will then mix together at the smaller levels and under anisotropy, which reads past the texels the sprite shader keeps to, and need space around each cell or an array layer each | A game draws its own textures smaller than they are |
| Keeping a masked texture's coverage as its levels shrink: averaging thins what its alpha cuts out, such as leaves and fences, at a distance, where scaling each level's alpha to keep the share above the cutoff would keep it | A masked model thins out at a distance |
| Sprite sheets with space between or around their cells, as some tile sets come; `SpriteSheet` takes cells packed edge to edge from the top left | A game's art comes that way |
| Tiling one cell of a sheet: a repeating sampler repeats the whole texture | A game tiles part of an atlas, such as a background |
| A sprite that samples past its rectangle, so that a seamless texture tiled across sprites side by side blends across their edges under a linear sampler | A game tiles one texture across sprites |
| A tile map in the engine, which scenes save; the sandbox's map is code of its own, its tiles sprites that snapshots and scenes save each with its texture's path | A game edits its map |
| glTF that `Context::add_model` refuses or ignores, as it lists: lighting and the material's other textures, skins and animations, morph targets, cameras, models without a scene and scenes other than the default, sparse accessors and accessors without a buffer view, primitives other than triangle lists, samplers that wrap across and down differently, minification filters, texture transforms, quantized or compressed meshes and textures (KHR_mesh_quantization, Draco, meshopt, Basis Universal), and integers written as floats, such as `1.0`, which glTF allows | Each with the first model that needs it; lighting and the other textures in Phase 12, compression with Phase 10's cooker |
| Decoding images off the main thread; `Context::add_texture` and `add_model` decode before they return | Phase 10, with the streaming loader |
| Models that take more than 4 GiB to read, of their buffers, decoded images, vertices and nodes, which `add_model` refuses with `engine.unsupported_model` | Phase 10, with streaming and the GPU's memory budget |
| Mip levels made on the CPU, on the render thread, as the frame that uploads a model's texture renders: a 2,048 by 2,048 texture's take about 0.3 s of that frame in a debug build | Phase 10, which cooks them; or blits on the GPU, if a hitch shows first |
| Layers making textures from their own pixels; `Textures::add` takes pixels, but `Context` adds only PNGs and JPEGs | A game makes its own textures |
| Keeping the frames' arenas near their frames' peak, within the GPU's memory budget: an arena grows by a buffer at least twice its last, and merges them as it resets, so it can hold a few times what its largest frame took, in memory the CPU writes, of instances' transforms, IDs and parameters and the UI's vertices, indices and texture changes | Phase 10, with the GPU's memory budget |
| A frame's copies through a transfer queue start only once it's submitted, after its passes are recorded, and the frame waits for them | A profile shows a frame waiting for its uploads |
| Staging and the frame's arena kept within the device's `maxMemoryAllocationSize` and `maxBufferSize`, each at least 1 GiB: both grow by doubling, each staging buffer an allocation of its own, so a frame that adds more than that at once fails, which ends the run | A frame adds that much at once, or Phase 10, with the GPU's memory budget |
| Images' sizes checked against the device's `maxImageDimension2D` and `maxFramebufferWidth` and `Height` as they're made; a pass checks its viewport against the device's limits, which an image past those exceeds only if it's past `maxViewportDimensions` too | A device's limits fall below a window's size |
| Host image copy where a transfer queue exists, or where the driver says it costs the GPU in reading the image, as KosmicKrisp's does, turning off Apple's lossless compression, so that macOS uploads through the graphics queue; and meshes written straight into GPU memory the CPU reaches, as ReBAR and unified memory allow, rather than copied | A measurement shows either is faster |
| More than one view, such as split screen, or the editor's scene view beside the game's: the view sees through one camera, the world's primary one, or a layer's viewpoint | A game or the editor draws several |
| A camera's own aspect ratio, kept by letterboxing the view or by a target of its own, as Unreal's cameras can: a camera fits the view's aspect | Cameras render into targets of their own, or a game needs one aspect on every display |
| The editor drawing the world's cameras in its view, as outlines of what each sees | The editor shows a game's cameras |
| Moving the world's camera from the editor's view, as Unreal's Pilot and Blender's Lock Camera to View do: a move of the editor's camera goes back to its own | Someone frames a camera's shot from the view |
| A scene or prefab loaded with a primary camera making the world's other cameras not primary, as an operation's does: a load sets fields as they're written, and of several primary, the view sees through the lowest ID's, warning | A prefab brings a camera of its own, as a player's would |
| Finding the point in the world under the cursor, as `Context::pick` finds the entity there: the editor's camera finds one on a plane itself, but nothing reads the view's depth; and the orthographic camera controller zooms about the view's center | Something places what it adds at the cursor's depth, or a game zooms toward the cursor |
| A dockspace's view in a window of its own: it docks and floats over the main window alone, since only the main window's input reaches the layers, and its image is made at the main window's scale | The editor shows the game on a display of its own |
| Showing whether keys reach the view, such as an outline around it while they do: from a press on it, or with the cursor over it | Someone misses it, or Phase 6's editor |
| Gamepads following a dockspace's view: they reach the layers whichever panel has the keys, as they do in games | The editor binds a pad, or a panel takes a pad's input |
| F framing what's picked, the editor's camera easing to see it whole | Someone frames often, or Phase 6's editor |
| The handles along the world's axes as well as along the entity's own, as other editors' toggles have it: they turn with what's picked | Someone moves things along the world's axes |
| Snapping steps of one's own: the command key snaps to half units, 45° and halves | Someone needs other steps |
| The amount a drag of the handles has moved, turned or scaled, shown beside them as it goes | Someone wants exact drags without the Inspector |
| Turning freely, as a trackball, by a drag inside Rotate's rings | Someone misses it |
| The handles along a skewed entity's axes: under a parent that scales unevenly, a turned entity's drawn axes aren't square to each other, and the handles show the nearest square ones, so their drags follow the cursor only roughly there; following the skew takes elliptical rings, and Scale along the drawn axis | Someone edits under a parent scaled unevenly |
| Picking the next entity down where several overlap, by clicking again | Someone picks what's hidden |
| Outlining several entities a frame, such as several picked, what's picked with its children, lighter, or what a click would pick, which the view's corner only names, fainter than the pick: `Context::outline` takes one, whose pass reads the IDs for it alone | Picking several entities, or someone misses the children's outlines or the highlight |
| Outlining what's hidden of the picked entity, through what's in front of it: the outline goes round what the view shows of it, from the IDs, so one wholly behind another shows none | Someone loses track of what's picked behind something |
| An outline whose cost doesn't grow with its width: each pixel of the view reads up to 49 of the IDs around it for the editor's 2 points at 1.25 or 1.5 pixels a point, and the width is kept to 8 pixels; jump flooding, a pass for each power of two up to the width, would read 9 a pass | A wide outline, or a profile showing it |
| Locking the editor's view against turning, as Unity's padlock does | Someone turns a 2D view by accident |
| Switching to orthographic on an axis snap, as Blender's and Godot's gizmos do by default: the editor switches projection only when asked | Someone wants it |
| The wheel setting the editor's fly speed while the right button is held, as Unity's, Unreal's and Godot's do: it dollies then too, and flying goes at the distance a second | Flying wants another speed |
| Keeping the editor's camera, tool and grid between runs, as the dock keeps its layout | Someone misses them |
| Easing the editor's switch between projections, which jumps | It jars someone |
| A slot between the world's opaque and blended draws for the editor's grid, so that it shows over opaque draws behind its plane: it draws first, a backdrop that everything covers, and what blends behind the plane would need the grid sorted among it | The editor shows opaque meshes |
| A rotating cursor for the editor's Orbit tool: egui has none, so it shows a hand | egui adds one, or someone misses it |
| The lines across the editor's perspective grid reaching the horizon: they fade out some 65 to 200 camera heights away, since its f32 coordinates hold four decades of spacing; the lines toward the horizon go on | Someone misses them |
| Cameras far from the origin: their matrices are f32, so 12,345 units out a view a tenth of a unit across moves in steps of about 1% | A game's world is that large |
| Zooming from keys or a gamepad, as a rate beside the wheel's lines; the orthographic camera controller zooms by the wheel alone, in its own event handling. And zooming a camera with perspective, by its field of view or by moving it in, or changing the fly camera controller's speed by the wheel, which does nothing to it | A game binds zoom elsewhere, or zooms a camera with perspective |
| Per-user bindings, kept as RON in the per-user directory beside the dock's layout, and a UI that binds them; a game binds its actions in code | A game ships its settings |
| Chords, modifiers, holds and double taps, such as Ctrl and a key: there's no modifier state, and Ctrl+Z reaches layers as `ControlLeft` and `KeyZ` | A game binds a combination |
| Keys' and gamepad buttons' labels, for prompts that show a binding: both are positions, and the engine drops what a keyboard's layout makes of a key, and whether a pad is an Xbox or a PlayStation one | A game shows its bindings |
| A hold of an axis shorter than a tick, which moves nothing: its two changes apply at one tick | A game needs one, as a flag that the tick clears |
| An engine type for what an action holds in the world, such as a button's held flag and its presses this tick; each game writes its own component | A game other than the sandbox and the editor's own world writes the same |
| Pressing and setting actions from code, ahead of the commands, and listing a game's actions by name, for the tool API and agents | An agent plays a game through the tool API, or a test presses its actions |
| Gamepads in `Input`, for layers to poll: `Input` holds keys and mouse buttons alone, and actions read the pads | A layer polls a pad |
| Gamepads connecting and disconnecting as events, and their names, which only the log has | A game prompts as a pad connects |
| Binding a source to one gamepad, or the keyboard to one player: every pad drives every layer's actions | Local multiplayer |
| Dead zones a game sets, for a pad, a stick or a binding: every stick and trigger has the engine's, from a tenth to nine tenths of its travel | A game ships its settings, with per-user bindings |
| A stick's way or a trigger's travel as a button, pressed past a point and released before it, as menus take a stick; `bind` refuses one, and a trigger's press is a button of its own | A game's menus take a stick |
| Rumble: gilrs's force feedback is off, and with it its thread | A game rumbles |
| Fewer commands from a moving stick, by quantizing its value or folding a layer's queue: a frame gives one for each stick and trigger that moved, and one more for each of its pad's button events between its moves, up to the pad's report rate, about 125 a second on Windows.Gaming.Input | Recordings grow too large, or input goes over a network |
| gilrs's allocations: on Windows.Gaming.Input, its polling thread's, which reads every pad every 8 ms, and the string of a pad's ID that each event the frame takes asks Windows for; and the channel's blocks, which the frame that takes their last event frees. The engine's own feeding of pads to actions allocates only as a pad first reports, and as a second pad first holds more than one does | A game's frames must allocate nothing with a pad connected |
| winit's allocations on Windows as it takes each key's press and release: a list for each message's text, and one of the messages it collects; Linux's keyboard reuses a buffer | A game's frames must allocate nothing as keys are pressed on Windows, or winit stops |
| Making the job threads' first allocations as the pool starts: each thread's first steal allocates its record in crossbeam-epoch, and on macOS its first sleep std's mutex and condition variable; the pool's other threads do both in its first ticks, and the ticks' thread sleeps only when a wait drags on, which may come at any tick. rayon-core doesn't say when a thread has slept, so a warm-up could only wait and hope | A game's ticks must allocate nothing from their first, or rayon-core says when a thread sleeps |
| Codes for dependencies' own warnings, which reach the log as they give them: as a pad connects on Windows.Gaming.Input, gilrs warns, without a code, that it has no mapping for it, since that backend gives every pad a nil UUID, and goes on with its default mapping, the right one there | A tool acts on a dependency's warnings, or the tool API serves diagnostics |
| Checking a game's vertex and parameter structs field by field: the check compares each struct's size with its Rust type's and finds its fields 32-bit, so fields of one size in another order or of another kind, such as an integer read as a float, draw garbage without a message. A derive on vertex and parameter types would give it each field's offset, scalar type and vector width | A game's struct mixes fields of one size but other kinds, or an agent's edit to one draws garbage unseen |
| Wave operations in games' shaders, and the other SPIR-V capabilities a device's properties decide rather than its features, which the check refuses | A game's shader needs them |
| Hot reloading the engine's own shaders, which the build compiles in | Work on the engine's shaders wants it |
| Watching shaders' files through the OS's notifications, rather than by their times and lengths every tenth of a second, which reads every watched file each time, and, while a shader fails, every file in the directories its files are in, but none in directories within them: an include in one of those that a failing shader found broken, fixed where it is, waits for another change | Phase 7, with asset hot reload, or a game with many shaders |
| The shader watcher allocates as it looks: each shader's stamps, collected at every look on every platform; on Windows, each file's path, which it converts to UTF-16 to read the file's times; and the entries of a failing shader's directories, which it lists | A development build's frames must allocate nothing on any thread |
| Compiling at once the shaders whose changes hold on the same look, rather than one after another: after an edit to an include that N shaders share, the last reloads about N tenths of a second after the first, and no file is looked at meanwhile | A game with many shaders that share an include |
| `VK_KHR_pipeline_binary`, which saves and loads pipelines whole; ash 0.38 lacks it | ash's next release, or a driver whose pipeline cache keeps less |
| Saving the pipeline cache as pipelines are made rather than as the run ends, which a crash skips, so that a game `gg run` restarts after one makes its pipelines again | A restart after a crash takes noticeably longer for it |
| Pruning the pipeline cache, which keeps every pipeline it was given, an edited shader's old ones too | It grows to tens of megabytes |
| Requiring a slangc that decorates sampled images `NonUniform` itself, Slang 2026.11's or later, such as the Vulkan SDK 1.4.363's, and dropping gg-renderer's `decorate_non_uniform`, which does it for older ones | The engine requires the Vulkan SDK 1.4.363 or later |
| Games' shaders compile with slangc as they're added, so running a game needs slangc where its build found it | Phase 10, which cooks shaders |
| Beyond textures named by their paths: a library that finds an asset by its path or name, so that a file added twice loads once, where now it loads twice, and that loads one a scene names; `MeshDraw`'s mesh and shader named, as `Sprite.texture` is: opaque until then, shown but not set, saved in scenes or hashed, and carried across a restart by their indices only where the layers added the same assets in the same order; setting them by name, and dropping the Assets panel's images onto a sprite, whose texture the Inspector shows but doesn't set | A game builds its draws from data, or someone edits a sprite's texture in the editor |
| 8- and 16-bit vertex fields, whose features Vulkan 1.4 guarantees but the device doesn't enable | A mesh packs its vertices, as Phase 10's cooker will |
| Splits in a torn-off window, which holds one tab group | A tool wants panels side by side outside the main window |
| Reordering tabs by dragging them along their bar; a drop among the tabs adds one at the end | Phase 6, the editor |
| A minimum width for each panel, as Hazel keeps its Properties panel 370 pixels wide: a split keeps each side 48 points, and the Inspector's rows of axes clip in a narrower panel | Someone narrows the Inspector past its rows |
| Checking Windows' frame on Windows 10, whose DWM may draw no border along the top of a window whose inside reaches it | Someone runs the tools on Windows 10 |
| Checking Windows' frame around torn-off windows as their scale changes, across monitors of different scales | Someone docks across such monitors |
| A button in the title bar hiding the panels docked above the view, as those left of, below and right of it hide, and keys for the buttons, as VS Code's Ctrl+B, Ctrl+J and Ctrl+Alt+B | Someone docks a panel above the view and wants it out of the way, or reaches for the buttons often |
| On macOS, the menus in the system's menu bar, and a double-click on the title bar doing what the user's setting says | A tool ships on macOS |
| Alt and access keys to reach the menus, and layers adding items to one another's menus | Phase 6, the editor |
| Windows of their own on Wayland, where groups float over the main window | winit can place windows on Wayland |
| egui's deferred viewports, which don't open (`engine.viewport_deferred`); immediate ones do | A tool needs a window that repaints apart from the main one |
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
| Spawning many entities at once, faster than one by one: 100,000 sprites take about 16 ms in release, 5.5 ms of it their IDs, and the swarm spawns its 100,000 in one tick, longer than a tick is. A load spawns its entities one by one too, building each component from a value into a column of its own: 100,000 sprites, 45 MB of text, load in about 650 ms in release | A spawn hitch matters, or a scene's load time |
| Micro-benchmarks, with criterion | A CPU hot path gets a target of its own; batching's is a frame time, which `engine.perf` measures |
| Gates on time in CI: hosted runners' times vary too much to gate on | A self-hosted runner, as for the GPU tests |
| Aliases for renamed fields, variants and component types, which a restart and a scene would match values by: a restart drops a field, a variant or a type it doesn't find, and a scene refuses one, as it refuses a field its type hasn't, against typos; and tuple fields reordered among themselves, which match by their places | Someone renames a field whose values they want kept |
| In the Inspector, turning an option on or off, and adding or removing a list's elements; by path, a value sets them whole | A component needs one edited |
| Flags for fields that saving, the world's hash or replication skip, and fields that are read only; `Parent` is read only as a whole type, which `set_field` refuses | Each with its first user |
| Generic types deriving `Reflect` or `Component` | A game needs one |
| Drag speeds as hints; the Inspector picks them from a field's bounds and kind | A field drags too coarsely or too finely |
| Paths resolved once and kept, for sets made again and again | A profile |
| Picking an entity for a field, a parent or a child picked by clicking it; the Entities panel scrolling to an entity found or picked in the view, or to a root that an add or a remove moved to the end of another table, and so of the tree's roots | Phase 6's scene tree |
| Finding an entity by its name: the Find field takes an ID | A world names more entities than the Entities panel shows at once |
| An icon on each of the Entities panel's rows saying what its entity is, from its components, such as a camera's, a sprite's or a mesh's: a row says its name alone | Names alone stop telling entities apart at a glance |
| World transforms stored and propagated after the ticks, with change detection; drawing computes each child's from its ancestors, every frame, as the latest tick left them, for its z, and blended between ticks, to draw it | Bodies under a parent, or a profile showing the walks up the hierarchy |
| Where an entity draws between ticks, for a layer's viewpoint that follows it, which would otherwise judder against it, and for the editor's handles, which show what's picked, as it moves, up to a tick ahead of where it draws: layers read the world up to a tick ahead of where it draws, `World::pose` giving where the latest tick left an entity, and the engine's drawing and the world's camera compute their own | A layer's viewpoint that follows an entity, or someone editing something as it moves |
| Turning interpolation off for an entity or a subtree, so that it draws at its ticks; every entity draws between them | A game wants something drawn at its ticks |
| Rotations of more than half a turn a tick, which draw the shorter way round, the other way; Hermite easing from velocities would draw them, and curves between ticks | Something spins that fast, or a curve between ticks shows |
| Drawing an entity despawned in a tick on its way there until the next: it vanishes up to a tick before it reaches where it stopped | A game's despawns show it, as bullets vanishing short of what they hit |
| Copying the world as only the last tick a frame runs starts: a frame that catches up copies it as each of its ticks starts | A profile shows a catching-up frame's copies |
| Copying the world, and finding what each row draws from, on the job system's threads, or only for the rows that moved: both run on the main thread, for the swarm about 0.19 ms a tick and 0.10 ms a frame that ticked; and blending only the rows that moved, which takes the job threads about 2.6 ms of their time a frame | A profile shows them |
| Sets that jump: an Inspector set of a moving entity's transform, and its undo, glide there over a tick, as a drag does, rather than reset its interpolation, as an agent's set through the tool API does too | Someone wants a set of a moving entity to glide |
| A flag for headless frames at a fraction of a tick of their own, for captures between ticks and for perf runs that measure blending, which headless frames, drawing their ticks, leave out; tests have them, through `App::headless_step` | A capture between ticks, or a change to blending that needs measuring headless |
| Interpolation across rollback: a world restored in a tick draws where the tick left it, all of it, so rollback that restores every tick would draw at the tick rate; how rollback restores is decided with it | Rollback |
| Reparenting that keeps where a child is in the world, by decomposing its matrix, which loses any shear; `set_parent` keeps its own transform | Phase 6's scene tree, or an agent attaching something where it stands |
| Spawning a child into its table at once from code: `spawn`, then `set_parent`, moves it to another table; a load and a create operation spawn one there | A profile |
| Children in an order that means something: reordering them, and saving their order; an undo puts a child back last | Phase 6's scene tree, or UI laid out by the hierarchy |
| Each session's seed and count saved with the world and merged with it, so that an ID maps back to its session and when it was drawn, and a load refuses IDs no session drew | The tool API refusing an ID an agent made up |
| An `EntityId` query term | The first system that reads IDs as it iterates, such as replication |
| Errors carrying their entity, and a scene's line and column, as fields, not only in their message, which the tool API gives as its code and message alone | A tool reads an error's entity or place without parsing its message |
| Drawing rows at one z in ID order for less. Every frame reads them in that order rather than their tables', about 0.9 ms of the swarm's 4.3 ms frame in release; a frame that changes rows or z's, or follows a restore, sorts them by ID again, about 2.5 ms more for 100,000; and 100,000 sprites each at a z of their own, as sorting by y gives them, take about 5 ms, for a group and an item each. Tables kept in ID order would make the reads a sweep, and a radix sort the sort quicker | A game's frames need the time |
| Linked prefab instances, with overrides, nesting, apply and revert; a placement is a flattened copy, each ID mixed from its instance's and its own, which links will keep | Phase 6's editor, or a game editing a prefab its scenes hold many copies of |
| Placing a prefab without parsing it again: `World::place_prefab` parses its text each time, which allocates | A game places prefabs as it runs, rather than as it loads |
| A derive hint for `Component::base`, and opaque values in lists and options, which saving refuses, as does a set's undo (`reflect.opaque`) where the old value holds something opaque in a variant, an option or a list; a type with opaque fields implements `Component` by hand to load, as `Sprite` does | A game's component holds something opaque |
| Names in scenes beyond ASCII identifiers, which saving refuses (`scene.bad_name`), and a project manifest's strings beyond printable ASCII, which reading refuses (`project.bad`) | A game names a type, a field or a folder so |
| Loading a scene in place of some of a world's entities in one step, as the sandbox's grid reload does in three; a load refuses IDs the world has, and `World::replace_scene` replaces all of its entities; a scene file a game loaded through `Context::load_scene_file`, saved, applies as operations | Another game reloads part of its world from a file of its own |
| A version in scene files, a file without one reading as the first | The first change to the format that old files can't be read by |
| Comments in scene files, which a save drops | People annotate scenes |
| A prompt before the editor's New, Open or Quit, or a scene dropped on its view, discards edits not saved: they discard them | Someone loses work, or Phase 6's editor |
| The scene's file named in the editor, whose title bar's middle holds Play, and a list of recent scenes: Save writes to the file opened or saved last, which nothing shows | Someone loses track of which file Save writes |
| Listing the Assets panel's folder off the main thread, as a thread watching it would: it's listed on the main thread each second it shows, which a folder of many thousands of files would hold up for milliseconds | A game's folder that big, once the tool API brings one |
| Pause and step, as Hazel's later episodes add | Someone steps through a play or a simulation |
| Keeping edits made as the world plays or simulates, which go with the copy, as Unity's do; and showing that it runs beyond the lit Stop and, as it plays, the view through the world's camera, such as a tint round the view | Someone loses an edit made while the world runs, or edits a running world for the scene |
| Bodies under a parent, moved in the world's space as their ancestors carry them, and colliders on a body's children; a body under a parent isn't moved (`physics.child_body`) | A game builds a body of parts, or nests one |
| Colliders other than boxes and circles, such as capsules and polygons; joints; sensors; and contacts and queries, such as raycasts, for scripts | Each with its first game use |
| A restitution threshold, the speed under which a contact doesn't bounce, as Box2D's and Hazel's: rapier has none | rapier gains one, or a game's bouncy bodies won't settle |
| Telling the colliders physics doesn't simulate from those it does: one without a `RigidBody2D`, or under a parent, outlines as the rest do | Someone wonders why an outlined collider doesn't collide |
| Gravity from a scene or a setting: `Physics2D::gravity` is code's, Earth's by default | A game sets its own |
| Physics steps that allocate nothing: rapier allocates as contacts come and go, and the `ticks` test steps no physics | A game's ticks must allocate nothing with physics |
| Physics' state in the world's hash, `--dump` and `World::difference`, which read entities alone, as they read no resource: a step that goes another way inside rapier shows in the bodies' transforms a tick later | A replay goes another way inside physics unseen for long |
| Replaying a log into another world, which maps values' handles to IDs, as a recording's commands are; `set_field` and operations store a dead handle without complaint | Edits sent to another world, as a server's or a peer's |
| A delete saying what refers to its entities from outside, or refusing; those references dangle until it's undone, and the world doesn't save as a scene in between, though snapshots keep them dead | Someone loses a scene's save to a reference a delete left dangling |
| Undoing a create whose entity has children since that the create didn't make, such as game code gave it, by keeping them where they are; it's refused (`ecs.has_children`), and the log drops it | An agent attaches children to what another's create made, or Phase 6's editor |
| Layers holding what a restore in place takes back: the sandbox's swarm is in the world, and the Inspector drops its log and pick once the world's serial is another's, but nothing tells a layer that `Context`'s world was restored within a run, as `Layer::restored` does in a run started from a snapshot, so the sandbox doesn't snapshot | Something restores the running world in place, such as rollback |
| Reparenting from the panels: dragging a row onto another, and taking a `Parent` away, as Remove Component would, making its entity a root; `Parent` is read only, so the Inspector offers neither | Phase 6's scene tree |
| Picking several entities, to edit or delete them at once | Someone edits many entities alike |
| A search field in Add Component's menu, which lists every type registered | A game registers more types than the menu shows at once |
| A Delete key for the entity picked: keys reach egui even while the view has them, so it would delete as the world plays, its keys driving it, as Ctrl+Z undoes and Ctrl+D duplicates then; the Entity menu and a row's context menu delete | Keys go to the panel under the pointer, or the view takes them from egui |
| A bound on the log, which keeps a session's entries, a delete's components and slots among them, until it's cleared | A session's memory shows it |
| A log that outlives a run or a restore: inverses that keep components typed can't be saved, and a restored world refuses every inverse from before, the slots kept when the snapshot was taken staying unused, and freed in a run restored from a file; a snapshot could keep the log's place, which a restore would rewind it to | Someone misses undo across a restart |
| Snapshots that copy less, only the tables changed, or on several threads: the swarm's 100,450 entities copy about 20 MB each way, in about 0.5 ms taken again or restored | A game's rollback frames need the time |
| An app's own tick rate; the simulation runs 64 ticks a second | A game needs another rate |
| Budgeting a frame's ticks against all of its time: a frame stops its ticks only once they take longer than the time they simulate, so ticks that take nearly their 15.625 ms keep real time at the frame rate's cost, as the debug sandbox's 100,000 drifters do, about 8 a frame at 8 fps; counting the rest of the frame too would slow down the simulation of any game drawn below 64 fps | A game's frame rate falls so |
| Headless runs in real time, as a dedicated server's: a headless run takes a tick each frame, as fast as its frames go | The first dedicated server |
| Hashing a world for less: incrementally as its components change, on several threads, or with a cheaper step for each word and one mix for each term. `World::hash` reads every component, mixing each word with SplitMix64's mix: 5.5 ms for the swarm's 100,450 entities in release, three times their tick; a recorded run hashes every tick, which takes the release swarm's headless frames from a p50 of 8.7 ms to 16.3 ms, its updates from 14.0 ms to 22.6 ms on average | Recorded runs of large worlds need the time |
| The job system in gg-core, where the renderer can reach it; it's gg-engine's | gg-renderer needs jobs |
| Naming the job system's threads and the render thread for Tracy on macOS, whose client reads no thread's name from the OS there, so they show as numbers: a call into gg-core, which keeps Tracy's client, from rayon-core's start handler and the render thread's start. On Windows and Linux they show as `gg-job-N` and `gg-render` | Someone profiles on macOS |
| Checking that every job thread's floating-point environment, its rounding and flush-to-zero, is the main thread's, which a driver could change on the main thread alone | A replay goes another way on one machine on more threads than one |
| The sign of a zero that f32's `min`, `max` and `clamp` give, which std leaves to the platform, and its `algebraic_*` methods, which clippy doesn't refuse; the simulation calls none of them | The simulation first calls one |
| The ban on std's float methods for games in their own repositories, which the workspace's clippy.toml doesn't reach, though glam's features do, since Cargo merges them; and `gg new` for such a game, which makes games in the workspace's `games/` alone: its own workspace, the engine a pinned dependency, copies of clippy.toml and the toolchain's pin, and gg and dx installed | The first game outside the workspace |
| Locating a divergence of the layout alone, to the table, row, children or stream of IDs where two worlds part; `World::difference` compares their content, and a replay names the part of the hash | A replay's layout goes another way first |
| Replaying from a snapshot and the commands after it, and a SyncTest, which runs each tick again from a snapshot and checks its hash, as rollback needs; and recording a run restored from a snapshot, which `--restore` refuses with `--record`, a recording's replay starting from a seed and the layers' attach, so that `gg run` doesn't restart a run that records | Rollback, or a crash `gg run` restarted from is worth replaying |
| Layers' own state across a restart from a snapshot: the camera controllers' viewpoints, the clear color, and the editor's scene file, tool and camera, which start as a fresh run's do; the world is as it was, and a layer takes back from it what its panels show, as the sandbox's `restored` does, and the editor comes back stopped, with the world as it was edited | Someone misses where they were looking after a restart |
| What's opaque in a component of a type that isn't opaque as a whole, which a snapshot file leaves out, its component coming back on its type's base: `Component::save_opaque` and `load_opaque` carry types opaque as a whole, as `MeshDraw` | A game's component holds an opaque field |
| Snapshots that cost the main thread less than a copy of the world, which each takes as a frame starts before a thread of the API's writes it: some 6 MB of text for the debug sandbox's 10,461 entities, taken every 5 seconds under `gg run` | A game's snapshots show as hitches |
| Removing the snapshot file of a `gg run` that's stopped, or ends in an error, which stays in `snapshots` beside the executable; one whose game exits cleanly removes it | They pile up enough to matter |
| Starting again a game that waits for a rebuild as its scenes or assets change too: only a rebuild starts it again, as a change to its sources brings one, or without patches, its executable changing | A game crashes for its data rather than its code |
| Recordings that keep what their attach read, such as the sandbox's grid scene, and the commit they were built from; a file changed since shows as a divergence at the attach, and a recording names its engine's version and build alone | Recordings replayed far from where they were made, such as bug reports from people's runs |
| Recording every run, as a flight recorder; a run records only with `--record` | Bug reports from people's runs |
| Patching an edit to a constant, which rebuilds the game: `gg run` patches edits inside function bodies alone, though one to a constant that no type, static or `const fn` reads would patch as well | Agents tune constants as the game runs, and the rebuilds' restarts slow them |
| Removing the patches' libraries a `gg run` that's killed leaves; a run removes those its game loaded as it ends | They pile up enough to matter |
| Lockstep beyond two peers on a LAN: more players, spectators, a delay that follows the link's latency rather than [`DELAY`](../crates/gg-engine/src/net.rs)'s 62.5 ms, and play across the internet | Phase 11, or a game for more than two |
| `gg run --peers 2`'s two windows opening apart: the OS puts them one over the other | Someone plays both peers on one screen |
| Recording a joiner, whose world comes from the host's, which `--join` refuses with `--record`; the host's recording replays the session | A desync is worth replaying from the joiner's side |
| Rebuilding or restarting a running game as its project's manifest changes, which `gg run` doesn't watch | Someone edits a manifest while its game runs |
| Builds without `dev` reading their project's manifest: `App::project` ends their runs (`engine.no_project`), as shipping builds start from cooked data | Phase 10, with the cooked start scene |
| The sandbox and the editor as projects, which name their roots themselves | A tool opens them as projects |
| The project in `hello`'s answer: tools find a project's games by their session files | A client reaches a game without reading its session file, as one on another machine would |
| An app's name and `gg run`'s defaults for it, such as `--peers 2`, in the project's manifest | A tool needs them |

## Open questions

- Which license? Recommendation: MIT OR Apache-2.0, the Rust convention, chosen once; until
  then, `publish = false` keeps the crates off crates.io. Decide before anyone outside the
  project contributes.
- How does the game's view reach the editor, and is a channel that a request of the tool API opens
  part of the API, as Phase 6 has the editor reach the game through the API alone? Recommendation:
  the game publishes its view's frames into OS shared memory that the editor makes and a `view`
  request names, defined in gg-api so that agents can read live frames too, the editor's input going
  back through an `input` request, and the game's diagnostics through a `logs` request rather than
  its log's file. It works on Windows, X11, Wayland and macOS alike, where embedding the game's own
  window works on Windows and X11 alone, and the editor couldn't draw over it. Decide before the
  game's view in the editor.
- Who owns the game's one view while the editor drives it: its viewpoint, outline, grid and
  colliders? Recommendation: one set for each game, which any client sets, the last setter winning,
  with `capture` able to ask for the world's own camera, as the engine has one view. Decide before
  editing in the game's view.
- What does Simulate do in a game whose ticks are all its own code, as Shards' are?
  Recommendation: run the game as Play does, from the world as edited, seen through the editor's
  camera, with no input forwarded. Decide before Play, Simulate and Stop in the game.
- Which entities does a scene file own, and so what do Save, Open and New do in a game whose attach
  builds entities beside its scene, as Shards' players are? Recommendation: those its text held as
  last applied, which the scene watch already tracks, and those made into it while it's the open
  scene, so that Save writes none the game made itself. Decide before scenes through the API.
