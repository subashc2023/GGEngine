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
- **The game's view in the editor** (2026-10-30)
- **The game's world and diagnostics in the editor's panels** (2026-10-30)
- **Editing in the game's view** (2026-10-30)
- **Play, Simulate and Stop in the game, the editor's own world deleted** (2026-10-31)
- **Scenes through the API and the File menu** (2026-10-31)
- **Phase 6, projects and the editor on the tool API** (2026-10-31)
- **Asset IDs beside their files** (2026-10-31)
- **Textures named by ID** (2026-10-31)
- **Assets loaded wherever they're named** (2026-11-01)
- **Renames that keep IDs** (2026-11-01)
- **Reloading any change** (2026-11-01)
- **Models in scenes by ID** (2026-11-01)
- **Dropping from the Assets panel** (2026-11-01)
- **Phase 7, assets** (2026-11-01)
- **Fonts and text** (2026-11-01)
- **Text in the world** (2026-11-01)
- **A game's UI** (2026-11-01)
- **Sounds** (2026-11-01)
- **Music** (2026-11-01)
- **Phase 8, text and sound** (2026-11-01)
- **The editor's hub, its Scenes panel and the showcase** (2026-11-02)
- **Hardening** (2026-11-02)
- **Luau scripts** (2026-11-02)
- **Scripts in the editor and in lockstep** (2026-11-02)
- **Debugging scripts** (2026-11-02)
- **Mods** (2026-11-03)
- **Phase 9, scripting** (2026-11-03)
- **The cooker and its package** (2026-11-03)
- **Cooked textures and meshes** (2026-11-03)
- **Streaming** (2026-11-03)
- **Shipping** (2026-11-03)
- **Desyncs named to their field** (2026-11-03)
- **Ticks run again, and a SyncTest** (2026-11-03)
- **Rollback** (2026-11-03)
- **A server that owns the world** (2026-11-04)
- **Phase 11, networking beyond lockstep** (2026-11-04)
- **The render graph** (2026-11-04)
- **HDR and tonemapping** (2026-11-04)
- **Lights and PBR materials** (2026-11-04)
- **Image-based lighting** (2026-11-04)
- **Cascaded shadow maps** (2026-11-04)
- **Sponza** (2026-11-04)
- **Phase 12, render graph and PBR** (2026-11-04)
- **Bistro** (2026-11-05)
- **Culling on the GPU** (2026-11-05)
- **The scene on the GPU** (2026-11-05)
- **Occlusion culling** (2026-11-05)
- **Clustered lighting** (2026-11-05)
- **Async compute** (2026-11-05)
- **Meshlets and packed vertices** (2026-11-06)
- **A visibility buffer** (2026-11-06)
- **TAA** (2026-11-06)
- **Phase 13, GPU-driven rendering** (2026-11-06)
- **3D physics** (2026-11-07)
- **Characters and queries** (2026-11-07)
- **Flipbooks and tweens** (2026-11-07)
- **Skeletal animation** (2026-11-08)
- **A networked 3D demo** (2026-11-08)
- **Phase 14, 3D gameplay** (2026-11-08)
- **Particles** (2026-11-08)
- **Linked prefabs** (2026-11-08)
- **Foxball's particles and prefabs** (2026-11-08)
- **Prefabs in the editor** (2026-11-08)
- **Phase 15, particles and linked prefabs** (2026-11-08)
- **Animation state machines** (2026-11-08)
- **Joints and continuous collision** (2026-11-08)
- **A game's UI** (2026-11-08)
- **Navigation** (2026-11-08)
- **Foxball on Phase 16's systems** (2026-11-08)
- **Phase 16, gameplay systems** (2026-11-08)
- **Bloom and color grading** (2026-11-08)
- **Ambient occlusion** (2026-11-08)
- **Height fog** (2026-11-09)
- **A sky made from its sun** (2026-11-09)
- **Foxball and Sponza on the rendering looks** (2026-11-09)
- **Phase 17, rendering looks** (2026-11-09)
- **Screen-space reflections** (2026-11-09)
- **A game's UI's widgets** (2026-11-09)
- **Exposure that adapts** (2026-11-09)
- **SMAA** (2026-11-09)
- **Depth of field and motion blur** (2026-11-09)

## Now

18. **The frame's finish and a game's widgets.** What a modern frame has beyond Phase 17's looks,
    each a pass or a few on the render graph that a world's `Environment` turns on and sets, alike
    in development runs and cooked ones: SMAA, antialiasing without a history, for games that
    can't take TAA's softness or its ghosts; exposure that adapts to what the view sees, from a
    histogram of its luminance, as eyes adapt between sunlight and shade; screen-space reflections,
    traced through the view's depth and falling back to the sky's light where they miss, with the
    sky's reflections hidden where ambient occlusion says they're hidden; and depth of field and
    motion blur, from a focus and an aperture and from the view's motion, with a vignette,
    chromatic aberration and film grain. And a game's UI's widgets beyond buttons: sliders,
    toggles, drop-downs, text fields and lists that scroll, clipped to their rectangles, which take
    the focus, keys, a pad and the mouse. *Done when* the showcase has a scene for each look and one
    of the widgets, each look a GPU test's reference image with its time on the GPU measured, and
    Foxball has an options menu of the widgets, setting its bot's difficulty, its volumes and its
    antialiasing. Its milestones: SMAA; exposure that adapts; screen-space reflections; depth of
    field and motion blur; a game's UI's widgets; and, once they're in, Foxball and the showcase on
    them.

Phase 10 closes once the demo's .app, as [ship.yml](../.github/workflows/ship.yml) uploads it, is
checked on an Apple Silicon Mac by hand, with the rest that Deferred lists for when one is at hand.

## Next

Each phase ends in something you can run and check.

10. **Cooking and the player.** `tools/cook` turns glTF into engine meshes (meshoptimizer: vertex
    and index optimization, LODs), textures into BC-compressed KTX2, fonts into their fields and
    games' shaders into SPIR-V, packed into a memory-mappable package; an async streaming loader
    with a per-frame upload budget; the player runs a package under a shipping profile (LTO, one
    codegen unit). *Done when* the demo ships as a cooked package its own player runs on all three
    OSes, with load time and memory measured. Its milestones, in order: the cooker and its package,
    a build without `dev` running from its game's package, read by positioned reads, its shaders
    compiled and its fonts baked, the demo's replaying its live recording; cooked textures and
    meshes, BC7 in KTX2 with the cooker's mips on every device, which needs BC, and meshes optimized
    with LODs that a draw picks by its size on screen; streaming, what the simulation reads loaded
    as it's named and the rest uploaded within a budget each frame, without the frame waiting, and
    within the GPU's memory budget, assets removed; and shipping, the shipping profile, egui behind
    `dev`, `gg ship` laying out a folder, a signed .app holding the Vulkan loader and KosmicKrisp on
    macOS, the reference player running a package's scenes and Luau scripts, as the showcase's,
    and load time and memory logged: checked by CI on Linux at each push, on all three OSes by a
    workflow run on demand, and on an Apple Silicon Mac by hand.

Phases 11 to 17 are done, as Done has it, but for what Deferred holds.

19. **World streaming**, to be decided, as Open questions has it: a world of cells, each a scene,
    loaded and unloaded around its viewers as they move, off the main thread, with a world far
    larger than f32's reach from its origin, and in step for lockstep and rollback peers and a
    server's clients.

Later: upscalers (DLSS, FSR, XeSS); ray-traced effects where the driver exposes ray tracing (no
macOS driver does today); and the built-in coding agent for v1.

## Deferred

Defects are in [DEFECTS.md](DEFECTS.md), which says what goes there rather than here.

| Skipped | Add when |
|---|---|
| The engine as a dynamic library in development builds, to shorten relinks | Rebuilding a game gets slow |
| Optimized dependencies in dev builds (`[profile.dev.package."*"] opt-level = 3`); zune-jpeg alone is, for the reason the root Cargo.toml gives | Physics or another heavy dependency makes debug builds slow |
| Compiling debug events out of shipping builds as well as trace events: `release_max_level_debug` keeps them in every optimized build, as agents read them of `gg run --release`'s runs, and Cargo can't give one profile features of its own | A shipped game's debug events cost it measurably |
| Linux builds against an older glibc, as the Steam Runtime's: a build needs the glibc it was built against or a later one, and CI builds on Debian trixie | A game ships to players on Linux |
| The notices a shipped game owes its dependencies' licenses, gathered from Cargo's metadata | A game ships to players |
| Signing a macOS .app with a Developer ID and notarizing it, which take a paid Apple Developer account: `gg ship` signs it ad hoc, which Apple Silicon needs to run it, and which Gatekeeper refuses once it's downloaded, until the user opens it anyway from System Settings | A game ships to players on macOS |
| A game's own bundle identifier, name and icon, which its project would give: `gg ship` names a game's .app after its package, capitalized, its identifier `com.ggengine.<package>`, its icon macOS's own | A game ships under a name of its own |
| Symbols for a shipped game's crashes: the shipping profile keeps release's debug info, none, and no PDB or dSYM ships beside the game; a panic names its functions, by the symbols the executable keeps | A shipped game's crash needs symbolicating |
| Shipping a game made of scenes and scripts alone, as the reference player under the game's name: `gg ship` ships a project's own build | A game has no code of its own |
| ship.yml's caches: it builds from nothing each run, so as not to push ci.yml's out of the repository's 10 GB | It runs often enough that its minutes matter |
| An ASTC cook, for devices without BC, as phones: Phase 10 requires BC | The engine runs on Android or iOS |
| Checking the sandbox by hand on Linux, and on macOS with KosmicKrisp (Vulkan SDK 1.4.363.0 or later, the first conformant build), its gamepads, the file dialogs `Context::pick_file` shows and the drawn title bars too, with the menu button's inset clear of macOS's traffic lights (72 points) and a torn-off window moving by its tab bar where X11 and macOS move windows asynchronously, and the render thread there: acquiring, presenting and rebuilding swapchains as windows resize off the main thread under KosmicKrisp, and on Wayland a frame in flight presenting at its old size after a maximize; and the `frames` test on macOS, whose CI runner has no GPU to run it: std's mutexes there are boxed as they're first locked, so a mutex made each frame allocates there alone; and the demo's .app, as `gg ship` lays it out and ship.yml uploads it, which hosted runners may have no Metal to run: that it opens from Finder, its loader loading the bundle's KosmicKrisp alone, that it runs and replays its live recording from its bundle, writing nothing into it, and that KosmicKrisp's library needs nothing else of the SDK's (`otool -L`); and Phase 13's 16 Bistros on a Mac, their frame time against a target set there, their meshlets drawn through vertex shaders, as KosmicKrisp has no mesh shaders | A Linux machine and an Apple Silicon Mac are at hand |
| Deleting old JSON logs; every run's log is kept | Logs pile up enough to matter |
| A clean shutdown on Ctrl+C in a windowed run started from a console, which ends the process at once, as headless runs and servers don't; the JSON log keeps every line written until then, and a recording every whole line but its `end` | Someone ends windowed runs from their console |
| GPU tests in Windows and macOS CI, which skip them for the reason [AGENTS.md](../AGENTS.md#commands) gives | A self-hosted runner with a GPU, or a Vulkan driver that runs on those runners without one |
| Checking windowed frames' allocations in CI, under Xvfb on Linux, with lavapipe presenting and input through XTest; they're checked by hand, as [gg-engine's AGENTS.md](../crates/gg-engine/AGENTS.md) says | A change to the windowed loop allocates where the check by hand didn't look |
| Checking captures in CI, under RenderDoc on Linux with lavapipe: captures are checked by hand, replayed through qrenderdoc's Python for their names and labels | A name or a label is lost unseen, or the tool API checks captures |
| RenderDoc captures from code or through the tool API, and more than one a run: `--capture` names one frame as the run starts, and the tool API's `capture` writes the view's pixels alone | An agent needs a frame's GPU work as well as its pixels |
| `gg build` and `gg test`, with their results as data: `run` and `check` build through cargo, passing on its messages about the code as it renders them, and cargo's test harness writes JSON on nightly alone | An agent needs a build's or the tests' results as data, or libtest's JSON output is stable |
| `gg fetch` taking archives, as a model shipped zipped: it fetches files one at a time, each named in the fetch list with its SHA-256 | A project fetches what comes only as an archive |
| A scene that names a fetched file not fetched yet saying that `gg fetch` fetches it: its load fails naming the file's ID, as for any asset not there (`asset.no_model`) | Someone opens such a scene and is lost |
| Tokens of fewer capabilities, minted for clients that can't read the session file, and grants kept to one scene: a connection's `hello` narrows its own grant, as [Agents and tools](../ARCHITECTURE.md#agents-and-tools) describes, but any process that reads the session file reaches everything | A client the user doesn't run as their own, such as one on another machine, an agent that can read the file but mustn't reach everything, or one kept to one scene |
| `gg mcp --grant` listing only the tools its grant covers: it lists them all, and the game refuses the rest with `api.ungranted` | An agent's tools list misleads it |
| A mods menu, where the player turns mods on and off and approves what each asks for: installing one is consent, and the game's limits cap what any gets | A game's players install mods from strangers |
| Mods that ship assets, or that replace a game's scripts and assets: a mod's scripts ride on the game's components | A mod needs textures of its own, or to change a game's own behavior |
| Dependencies between mods, their settings, and a migration as a mod's version changes, as Factorio's `on_configuration_changed`: a mod's `version` is read, and nothing follows from it | Mods build on each other, or a mod's components change between its versions |
| Mods' files reloaded as they're saved, as a game's scripts are, and mods' scripts debugged, as the debugger debugs the game's alone; and a run's mods listed in `status` or the editor, where a run logs `Mod loaded` for each | Someone writes a mod against a running game |
| MCP beyond tools: resources, prompts, progress, cancellation and the client's roots; `gg mcp`'s tools are the API's methods but `hello`, `ping`, `view`, `patch`, `note` and `quit`, and the paths of `capture` and `snapshot`, a capture's PNG coming inline | An agent needs one of them |
| Steps and captures while the window is minimized, which fail with `api.paused`, and requests while a file dialog or Windows' window menu holds the main thread, which wait; `ping` says which. An `assets` while it's minimized answers from the asset database as its thread last sent it, up to a second old | A tool needs them then |
| Ordering a graph's passes by what they use, rather than as they're added, and leaving out those whose results nothing uses | Passes come from more than one place, as games' own would |
| Passes on the compute queue that sample or write images: images, the graph's own among them, are the graphics queue's family's alone, so a pass there uses buffers alone; the depth pyramid and the culling's second pass, which sample the view's depth, run on the graphics queue, as do a sky's prefiltering and its compute passes. Images shared by both families, which can cost some GPUs their compression, or handed between them by a barrier on each queue | A pass that uses an image gains from running beside the graphics queue's |
| A compute queue on a device whose one family for compute apart from graphics takes the uploads, as one without a family for transfers alone: a second queue of that family, or uploads on the graphics queue, where the device has the queues; its compute passes run on the graphics queue | Such a device is one a game targets |
| Submitting each queue's batches in one call, a wait submitted before what signals it, which timeline semaphores allow: a frame submits runs of one queue's batches each after what it waits for, two calls in the 16 Bistros, 66 µs each at the median on the RTX 4090's render thread, three where the compute queue waits for a buffer the frame wrote | The render thread's time is measured to go in submits |
| Each meshlet's visibility from frame to frame, so that the view's first pass draws only the meshlets the last found unhidden, and its second tests every meshlet against the depth pyramid: the first pass draws all the meshlets in view of what it draws, as the pyramid isn't built yet | A view's large instances, partly hidden, are measured to cost its first pass |
| A run's meshlets past 2^22 in a view, which are left out: a draw through a mesh shader takes at most that many workgroups on some devices, so a run's draw holds no more | A game keeps that many of a run's meshlets in view, as thousands of instances of a mesh of thousands of meshlets at their finest |
| Reusing the room of removed meshes' meshlets in the renderer's table of their bounds, which grows by every mesh with meshlets a run adds | A game streams models in and out as it plays |
| Room for each view's meshlets sized by what the frames before kept, rather than every instance's of its mesh's largest level, up to one run's most, 32 MiB a view: past that, runs are drawn by their levels' indices, their meshlets unculled and outside the visibility buffer, as a few runs of many instances, such as foliage's, reach; 66 MB of the 197 MB the 16 Bistros' graph takes | A scene is measured to draw runs by their indices for want of room, or the graph's memory is measured to matter, as on a GPU of little memory |
| Development runs cutting models into meshlets as they read them, as the cooker does, so that their models are shaded from the visibility buffer too, and gg-engine's `frames` test checks that path allocates nothing: a development run's models have no meshlets, and draw as they shade, beside it; a check outside the repository found its frames allocate nothing | A development run's shading is measured to matter, or a change to the visibility buffer's frames could allocate |
| Games' opaque shaders drawn into the depth pyramid that culls the view's second pass: they draw after the visibility buffer's shading, so what they hide is culled by the models' depth alone, and the rest is drawn | A game's opaque draws of its own shaders hide much of what it draws |
| Shading the visibility buffer's pixels by material, each material's together, rather than each thread its pixel's, whose textures and branches differ across a workgroup where materials meet | The shading pass is measured to lose its time to that, as with many materials on screen |
| The view's IDs drawn without TAA's offset for picks and the outline, as by drawing the outlined entity's IDs again without it: they're drawn off by the frame's offset, so that the editor's outline of the pick, and a pick at an edge, can move a pixel from frame to frame | The outline's shimmer bothers someone, or a pick at an edge misses |
| Motion for a layer's own draws, made each frame, which move with the camera alone under TAA and motion blur, as the renderer knows no transform the frame before of theirs: an identity that lasts from frame to frame, as the scene's slots give its instances | A layer's own draws move fast enough to smear, or should blur as they move |
| Starting TAA's history afresh at a camera's cut: `RenderData::cut` says the view is cut to its viewpoint, as the engine sets it where the viewpoint comes from elsewhere than the frame before's, a tool's look set, changed or cleared, a bare frame among the look's or after one, the primary camera switched or its projection turned, and motion blur takes it, but TAA doesn't, clipping what the frame before resolved to what this frame drew, which keeps it from showing as a ghost, but leaves the cut's first frames softer; and a way for a game to say its camera's teleport is a cut | A game cuts between cameras or teleports its camera, and the cut's first frames show blurred |
| SMAA's other qualities, the reference's LOW, MEDIUM and ULTRA presets, or its threshold and search steps as settings: SMAA runs at the HIGH preset alone | A game needs SMAA cheaper, or its long shallow edges smoother |
| SMAA T2x, S2x and 4x, SMAA with temporal supersampling or with multisampling, which the area texture's other subsample offsets serve; and FXAA: SMAA 1x alone, of subsample offset 0 | A game wants SMAA's sharpness with TAA's stability where what's thinner than a pixel moves, or a device too slow for SMAA's three passes |
| SMAA's edges predicated by the view's depth, or found on its colors rather than their luma: luma alone, which misses edges between colors of one luma | Edges between colors of a luma show aliased in a game's look |
| SMAA's blending weights computed for the pixels with edges alone, as the reference's stencil test keeps its pass to them: the compute pass reads every pixel's edges and stops where there are none | The weights pass is measured to matter |
| SMAA seeing the view as ACES's and AgX's own curves, PBR Neutral's toe and offset, and the grade's white balance, contrast, power and saturation show it: it sees the view by the grade's slope and PBR Neutral's shoulder, or the clamp's under no tonemapper, so a disk 16 times as bright as white on black shows its edge's coverage 0.19 off on average under ACES and 0.16 under AgX, against 0.10 under PBR Neutral, whose toe darkens what's dim, and 0.08 for an edge below the shoulder (0.26 without antialiasing); and white under a contrast of 1.2, after SMAA, 0.02 off at a pixel half covered, of 2, 0.39 | A bright edge under ACES or AgX, or one under a strong contrast, shows its steps |
| Each pass's time on the GPU for agents, in `engine.perf` or the tool API: the log has the frames', and Tracy each pass's | An agent tunes one pass |
| Synchronization validation under RenderDoc, whose layer hides the validation layer's settings: through `VK_EXT_validation_features`, or the validation layer's environment | A synchronization bug shows under RenderDoc alone |
| Captures on macOS, where RenderDoc doesn't run, so `--capture` is refused | Someone needs a capture of a Mac's frames |
| The renderer's UI pass in builds without `dev`, which hand it no UI: it makes the pass's pipeline, `egui`, as it starts | A shipped game's start is measured to wait on it |
| The main thread takes the renderer back once a frame's UI is drawn, so a frame's input is read about a frame before it renders: a refresh under `--vsync`; and drawing between ticks draws the world up to a tick behind the simulation. Starting each frame once the render thread has acquired the image it renders into would win the first back, and extrapolating from the latest ticks the second | A game's input latency needs it |
| Pushing and popping layers while the app runs; `App` fixes the stack before it starts | A tool or game needs a layer for only part of a run |
| Composed text from an input method (winit's `Ime` events), for languages typed through one, a game's UI's text fields among what takes it: a field takes winit's text as it's typed, and shows nothing of a composition under way | A game takes text input in such a language, such as chat |
| Development builds' frames that draw UI allocating nothing: egui allocates as it lays out and tessellates, and as it collects its input, where winit reads the window's title, and so do the dock, the Inspector, and the render thread while a panel has a window of its own; builds without `dev` run no egui | A tool needs frames that allocate nothing, as for measuring one |
| Shards' menu as a UI file: Shards draws its menu's buttons, finds what the cursor is over, and keeps their focus itself, as it did before UI files | Shards' menu changes |
| A game's UI's text fields taking more than a line: copying, cutting and pasting through the OS's clipboard, moving by words with Ctrl and the arrows, selecting by Shift and a click or a double click, the caret moving by graphemes rather than characters, a caret that blinks, and several lines | A game's text field takes more than a name or a short line |
| Telling a layer that a UI's focused text field took a key, so that what's typed doesn't steer the game too: an event gives back what it did, and a key a field took gives back none | A game shows a text field while its game takes keys, as a chat over play |
| A drop-down's options scrolling, as a list does, where they reach past the view: they open whole, below it or above it | A drop-down has more options than the view holds |
| Lists that scroll across, a list's scroll bar dragged, and a list scrolled by a pad's right stick or by dragging what it holds: a list scrolls down, by the wheel or the focus moving into it | A game's UI scrolls across, or a pad's player browses a list with nothing in it to focus |
| Clips that aren't upright rectangles, as a turned panel's or a circle's: the canvas clips by a scissor, to the box round where a clip shows | A game's UI turns or masks what it clips |
| Tweens on a UI's nodes: fades and slides as a menu opens, and a button easing between its looks, which change at once | A game's menus are to move |
| Binding a UI's nodes to the world or a layer's values, so that a text or a node's visibility follows them without code setting them by name each frame | A game's UI shows many values, as an inventory's or a scoreboard's |
| Styles shared between UI files: a button's looks, colors, a font and its sizes named once and taken by name, which each file and node repeats now | A game's UIs are several files of one look |
| The editor showing a game's UI files: their nodes in a panel, picked in the view by their rectangles, edited and saved as text; agents and people edit them as text and see them reload | Someone lays a UI out in the editor rather than as text |
| Writing UI files in a canonical form, as scenes are written, so that a tool's edit changes only what it changes: the engine reads them and writes none | A tool writes or rewrites UI files, as the editor would |
| A pad's D-pad and stick moving the focus, or stepping a slider or a drop-down's choice, again while held, as a key's repeats do: each press, or push of the stick, steps once | A game's menu is long enough, or a slider fine enough, that its players hold to move through it |
| Focus kept within a panel or a group of buttons, and coming round at a UI's edges: the arrows stop at the last button that way, and Tab goes through every button of the UI | A game shows a dialog over a menu whose buttons mustn't take the focus |
| Pressing a button by name through the tool API: an agent clicks its pixels through `input`, as `ui` gives them | An agent presses buttons it can't reach by a click, as one hidden under another |
| A UI's sounds named in its file, played as the focus moves and a button is pressed: the layer plays them on `Focused` and `Pressed` | Several games' menus play the same way |
| The canvas scaled by the view's width or its smaller side, or to a height a game sets, rather than 1,080 units to the view's height: a view much narrower than 16:9 crowds what's anchored to its sides, and a portrait one overlaps it | A game runs in portrait, or at aspect ratios far from 16:9 |
| Stretching what a layer draws on the canvas itself between anchors: each such draw is placed from one anchor, and scaled with the view's height alone; a UI file's fills stretch to the room they're given | A layer draws a backdrop filling the view itself rather than through a UI file |
| Circles, meshes and lines on the canvas: it draws sprites and text, clipped to rectangles | A game's UI draws them |
| Hiding a game's UI from the editor's view while it edits, as Unity's Scene view leaves out a canvas over the screen: the canvas draws through the look's viewpoint too, and hides what's under it from picks | Someone edits under a game's HUD |
| A sound's own distances and spatialization, as kira's spatial tracks take each: a camera with perspective hears every sound full within a unit and silent at 100, panned three quarters of the way to a side, as those tracks' defaults have it; nor Doppler, nor occlusion by what's between | A game's units aren't about a meter, or a sound carries farther or nearer than the rest |
| Setting a play's volume and pitch, and stopping it, as a hum that follows an engine's speed would: a sound plays as its file holds it, to its end | A game varies or cuts a sound as it plays it |
| More than 128 sounds at once, as many as kira's mixer takes by default, past which a sound is left out (`audio.full`); and making way for the next by stopping the oldest or the quietest | A game plays that many at once |
| Sounds and music pausing with the simulation, as the tool API pauses or stops it, and while the window is minimized: a sound playing plays to its end, and music plays on | A game's sounds outlast a pause noticeably, or its music should stop with it |
| Frames that play sounds and allocate nothing, on any thread: kira allocates on the main thread as each sound starts, its state and its commands' buffers, and its thread that follows the system's default output device formats the device's ID every half second, on Windows and Linux; and each play of music decodes on a thread kira starts for it, which allocates each packet's frames, as symphonia allocates each packet. The `frames` test's app adds no sounds or music, so it opens no output | A game's frames must allocate nothing as its sounds and music play |
| A real-time priority for the audio thread, through cpal's `realtime` feature, which on Linux asks for it through D-Bus, against dropouts while the machine is busy | Sounds drop out under load |
| The tool API listing a game's sounds and music, what's playing and the buses' volumes, and playing one, for agents, which hear nothing: its log's debug events say each sound and music started (`Sound started`, `Music started`) | An agent checks a game's audio beyond its log |
| Points to loop music between: it loops whole, from its start, so an intro played once before its loop, or music that plays once and ends, takes them, from its file's tags or a layer | A game's music has an intro, or ends |
| More than one music at once, as stems fading in with the action, and pieces that pick up where they were left: one plays at a time, from its start, the one before fading out | A game layers its music, or goes back to a piece where it left it |
| Buses a game adds, as voice, ambience or the UI's, each play naming its bus, and effects on buses, as a low-pass under a menu: there are the main bus, the music's and the sounds', with volumes alone | A game mixes more than its music and sounds apart, or filters one |
| The buses' volumes kept between runs, per user, as a game's settings: each starts each run at 1 | A game ships its settings, with per-user bindings |
| Music decoded through to its end as it loads, to say whether all of it decodes: its first packet decodes as it's read, and the rest only as it plays, which stops at a packet that doesn't, warning `audio.music_failed` | A game ships music that fails past its start |
| Counting what the Vulkan loader, the drivers, the validation layer and VMA allocate beneath the engine, through `VkAllocationCallbacks`, which drivers may bypass for their own: the `frames` test counts Rust's allocator | A profile shows them allocating each frame |
| Removing shaders and fonts, which last until the app ends, and removing assets through the tool API: layers remove textures, meshes and models | A game swaps shaders or fonts as it plays, or an agent frees an asset |
| Reusing removed textures' slots in the bindless set: a run adds at most 61,440 textures, those removed among them | A game adds and removes textures as it plays, as levels streaming in and out do |
| Streaming a texture's smallest levels first and its larger ones after, so that it shows blurred rather than as its stand-in while it streams | Pop-in shows |
| A game's own upload budget: every frame copies at most 16 MiB of assets | A game needs another |
| A persistent ring of staging for streaming copies, in place of the buffers each frame that streams makes and retires once its copies finish | A profile shows them |
| Reloading a model's nodes and materials, and a model of other numbers of primitives or images, in a run that's recorded or in a lockstep session: its roots' nodes keep what they were spawned with until the run starts again (`asset.model_changed`), as their world changes only as the run it goes with changed it | Someone edits models during such runs, as a reload kept as a line of the recording or an input both peers apply at one tick would allow |
| A model's nodes matched by their names as its file changes, rather than by their places: each keeps what code gave it only while every node keeps its place in the model's hierarchy, and else all are spawned again, under the same IDs | A game's code gives nodes state that a model's edit loses |
| A model's nodes named, as by their glTF names: they have no `Name`, so the Entities panel and the tool API show their IDs | Someone looks for a node by name |
| A model's nodes as its file changed while its game was down, as between a crash and its restart: a world restored from a snapshot keeps them as the snapshot did, but for the draws a run of other assets left out, until the model's next change; and an undone delete of a model's root, or a redone create, brings back its nodes as they were before its model changed since. Refitting there would undo what code did to them, and lockstep peers would part, without a print of each model's nodes in the snapshot and the undo to tell a model changed since | Someone edits a model while its game is down, or between deleting its root and undoing that |
| A model standing in for a following run with its nodes, so that the hash still matches: a replay or a joiner that can't load a model stands in with one of no nodes, and its hash parts from its run's where the nodes were | A replay or a joiner runs without a game's model files, as a bug report's might |
| A sprite sheet's texture reloaded at another size, which keeps drawing as it did (`asset.sheet_resized`): its sprites hold its cells as fractions of its size, which a layer cut it by as it added it | A game's sheets grow as they're drawn, as frames added to an animation's |
| Following a texture's, a model's or a font's file outside the asset root, or a sound's or music's anywhere, none of which has an asset ID, as it's renamed or moved: it's watched where it was, missing there, and reloads no more | Someone moves such files while their game runs |
| An adapting exposure's metering weighted toward the view's middle or a spot, and local exposure, which brightens what's dark in a view apart from what's bright: the meter weighs every pixel alike, and one exposure takes the whole view | A game's subject is lost against a bright sky or a dark room, or a view must show both sunlight and deep shade |
| An adapting exposure that moves in real time while the simulation is paused or stopped, as the editor's view before Play: it moves by the time the view shows, the simulation's, so that captures of the same frames match, and holds while that does, though a change to its bounds or compensation shows at once, and one to its percentiles takes what the meter calls for at once | Someone looks round a stopped game in the editor and expects it to adapt |
| The adapted exposure in `engine.perf` and the tool API's `status`, for agents: `Renderer::exposure` gives what the last frame shaded at, which reaches neither | An agent checks how a game's exposure adapts without reading its pixels |
| Metering an adapting exposure a frame sooner, the adapted exposure kept on the GPU, which every reader of it reads, games' shaders through the frame's data among them: the meter's mean comes back to the CPU two frames later, so that every reader takes the exposure as a fixed one | A game's light changes so fast that two frames' lag shows, as a flash |
| Showing the frame on HDR displays, through HDR10 or scRGB swapchains: frames are 8-bit sRGB | A game targets HDR displays |
| Bloom's radius and a threshold, set by a game: each of the chain's levels lerps 0.7 of the way toward the one below, and light blooms from everything, as in a lens | A game needs a wider or tighter glow, or bloom from its brightest light alone |
| Lens dirt, flares and streaks over bloom, and each of its levels tinted and sized apart, as Unreal's are: bloom is one weighted mean of its levels | A game's look needs them |
| Bloom's first downsample reading each of the view's texels once, through a workgroup's shared memory: it reads them a texel at a time for each of its 13 taps, 52 reads a texel of its own, as the view's colors can be no number, which bilinear taps would spread; about 24 µs of the RTX 4090's at 1,280 by 720, of the chain's 57 | A profile shows it |
| Grading's lookup tables from files, `.cube` from DaVinci Resolve and the like, which are made for the display's encoded colors and so would apply after the tonemapper: the grade is computed from an environment's fields | A game's grade is made in a grading tool |
| Grading's further controls: lift, gamma and gain, shadows, midtones and highlights apart, curves, hue shifts and mixing channels | A game's look needs them |
| Dithering that changes from frame to frame, or of blue noise, which hide it further: each pixel's noise is a hash of the pixel alone, the same every frame, so that captures of the same view match | Dithering shows as a pattern that holds still while the view moves |
| Bokeh of other shapes than a disk, as a lens's blades make polygons, cat's eyes toward the view's edges and an anamorphic lens's ovals, and bokeh textures: depth of field gathers round circles | A game's look needs them |
| Focus that follows, as auto focus on what the view's middle shows or on an entity, eased as a camera pulls focus: an environment's focus distance is set by hand or by a game's code | A game's camera should keep a subject in focus by itself |
| A camera's own sensor and focal length, as a cine camera's filmback and lens: depth of field takes the focal length the field of view makes on a full-frame sensor, 24 mm high | A game matches a real camera's framing and blur, or its artists think in focal lengths |
| Depth of field's quality beyond its gather: tiles sorted into those in focus, of one circle and blurred, each gathered by its own fast path, a postfilter of the half-size fields, as Call of Duty's median, and a gather at a quarter of the view's size or of more rings for large views. Its rings, up to five, stand about 2 texels apart at the largest circle of a view 1,080 high and 4 at 2,160, where a lone bright point's bokeh can show as rings of dots | A game's bokeh shows rings or noise, or depth of field costs too much at 4K |
| Depth of field of what blends, as text, particles and glass, which draw no depth: each blurs as what's behind it does | Something blended in focus shows blurred over a blurred background |
| Motion blur's refinements since McGuire et al.'s filter, as Guertin, McGuire and Nowrouzezahrai's (2014) and Call of Duty's: the inside of a moving edge, which the filter keeps about as sharp as the frame drew it, as it weighs what moves alike at a depth alike, where a camera's exposure would show what's behind through it; tiles' largest velocities taken only from neighbors that move toward them; and more samples where motion is long | What moves shows edges too sharp on their inner side, or streaks |
| A shutter of a time rather than a share of the frame, so that motion blur doesn't shorten as the frame rate rises, and objects left out of it, as a first-person view's weapon: motion blur blurs a frame's motion by its shutter's share of it | A game runs at high refresh rates and wants the blur of a fixed shutter, or keeps some objects sharp |
| A lens's distortion, as barrel and pincushion, chromatic aberration across the spectrum rather than red and blue apart, a vignette's center, shape and color, and grain of colors, of a size and following the light's levels as film's: a lens's effects are its vignette, red and blue parted and grain of a pixel alike in every channel | A game's look needs them |
| The view's size and the frame's data for a shader that takes no parameters, whose push constants end before them, as [`Shaders::add`](../crates/gg-renderer/src/shader/mod.rs) says: such a shader takes parameters it doesn't read | A game's shader needs them without parameters |
| The frame's time as more than an `f32` of seconds, which steps by about 8 ms once a run has simulated a day | A game animates by the time through runs that long |
| Games' own shaders' parameters in a table on the GPU, as models' materials are, written when they change rather than copied every frame with each instance; `MeshDraw`'s cap of 64 bytes of parameters, past which a shader draws through `Context::draw`, goes with it | A game draws many instances of its own shader whose parameters seldom change, or needs more than 64 bytes of them |
| Games' own textures of data rather than color, such as their shaders' normal maps, which shaders read as stored: `Context::add_texture` adds color alone, and the cooker would need to know which hold data, as from their ID files; models' maps are data | A game's own shader samples data |
| Removing a model's materials from the table: a removed model's stay, 92 bytes each, as their handles are never given out again | A game adds and removes many models as it plays, as levels streaming in and out do |
| Specular antialiasing of normal maps' smaller levels, widening roughness by how much their averaged normals shortened (Toksvig): shading makes each normal whole again | Highlights shimmer on a normal-mapped surface seen from afar |
| Emission in physical units, exposed as light is: an emissive color shows as it is under any exposure, KHR_materials_emissive_strength scaling it | A game's emissive surfaces must keep their brightness against its lights as its exposure changes |
| A model's image that holds something else as its file is read again, as colors where it held a normal map, taking a texture of its own: the model takes nothing until the run starts again (`asset.model_changed`) | Someone retargets a model's images while its game runs |
| The editor drawing lights in its view, as icons and spots' cones | Someone places lights by eye |
| Clouds in an atmosphere's sky, which lit and shadowed by its sun would take volumes of their own, as Hillaire's later work raymarches: its sky is clear | A game's sky needs weather |
| Stars, the moon and its light, and the sky's own glow, at night: once an atmosphere's sun has set, its sky darkens through twilight to black and lights nothing, so a night is lit by the world's other lights alone | A game plays through its nights |
| A planet seen from space, and an atmosphere entered from above: the viewer is kept 10 m below an atmosphere's top, and its planet's radius, top and scale heights are Earth's, which only its scattering and absorption change | A game flies above the atmosphere, or to another planet |
| An atmosphere's sun dimmed by the air at each point it lights, rather than once a frame at the viewer, and its haze shadowed by what's between the sun and it, as shafts of light: points kilometers above or below the viewer take its light as dimmed for the viewer's height, and the haze is in the planet's shadow alone | A game's world spans kilometers of height, as a mountain's, or its haze should show shafts of light |
| An atmosphere's haze over what isn't lit, as sprites, text, unlit materials, particles and games' own shaders, which draw as they are however far: lit shading alone hazes what it lights | A game draws such things kilometers away |
| An atmosphere's tables made only as its sun, the viewer or its settings change, and over several frames: every frame that renders the view makes them all, about 36 µs of an RTX 4090's time, the aerial perspective's 16 µs of it, a thread a froxel | Their time is measured to matter, as on a slow GPU |
| Turning a sky, as EXT_lights_image_based turns one: its image's middle column faces +X, and a scene faces its sky's sun by turning itself, as the showcase's Sponza turns its model | A scene can't turn to face its sky's sun, as a level laid out first |
| Finding a sky's sun by its contrast with the sky round it, rather than by 10⁷ candela per square meter, which a sun low in its sky can fall short of, lighting both through the sky's light and through the directional light that stands for it; and its way and illuminance, for that light, which the sun of the showcase's Sponza in fog was given by hand from its sky's texels | A scene under a low sun lights twice, or skies change often enough that setting their suns by hand matters |
| Mip levels for skies and other textures of high dynamic range, which have one: a 4K sky the view shows smaller than it is shimmers as the camera turns | Shimmering shows |
| Prefiltering a sky over several frames, or on a compute queue: the frame that prefilters a 4K sky takes about 0.4 ms of an RTX 4090's time, and one that prefilters an atmosphere's, as its sun turns, by 64 samples a texel, about 35 µs at the median, every frame while a sun of a fast day is low, its pipelines made as the first sky shows, some 60 ms on the render thread in a debug build without the pipeline cache | A sky that changes every few frames, as a day passing in seconds, or a hitch that shows |
| A normal map's horizon hiding its own reflections, where a bump's normal faces from the eye, as Jimenez's horizon occlusion of normal maps: the sky's specular light is taken the way the bumped normal reflects, whatever the surface round it hides | Reflections show through a normal map's bumps |
| Bent normals from ambient occlusion's horizons, as XeGTAO gives them, so that the sky's diffuse light is taken the way the open part of a point's hemisphere faces, and its specular light by the cone they leave: the occlusion darkens the sky's light whole, the way the normal faces | Light from the sky shows coming through what hides it, as in creases |
| Ambient occlusion of what blends, as glass and particles, which takes none: the view's occlusion is of what's opaque, as its depth has it | A game's blended surfaces show lit where what's near should hide their light |
| Ambient occlusion under an orthographic projection, whose depth XeGTAO's reach in pixels and its edges by the depth's scale take as a perspective's: such a view runs none of it | A game seen without perspective is lit by a sky, as an isometric one |
| Ambient occlusion's quality and size set by a game or by quality settings: XeGTAO's high quality, 3 slices of 3 steps each way, at the view's size, and its noise and denoise as antialiasing picks them | A device is too slow for it, or a game's look needs another |
| Drawing the view over the depth alone it draws for ambient occlusion without a visibility buffer, testing depth for equality, rather than drawing depth again as it shades: a game's opaque shader that discards draws whole triangles' depth alone, and the two pipelines would need to place each vertex alike | A development run's frames are measured to lose much to drawing their opaque vertices twice |
| Reflection probes and other light from all around but the one sky's, which lights indoors too | A game lights interiors |
| Reflections' rays sampled from GGX by roughness, as Stochastic Screen-Space Reflections (Stachowiak, 2015), resolved over neighbors and frames, along the shading's normal, a normal map's among it: the trace runs before shading, from the depth alone, so each ray is the mirror of the depth's normal, and a rough surface's reflection is blurred on screen round the point, which bleeds a little across where surfaces meet | A rough or normal-mapped glossy surface's reflections look wrong, flat where its bumps should show or bleeding across its edges |
| Reflections of what moves taken from where its motion says it was, rather than by the camera alone: the frame before's colors are found where the camera saw what a ray found, so a reflection of something moving shows where it was a frame before | A game's fast movers show their reflections lagging |
| Reflections at half the view's size, in a cheaper trace, or set by quality settings: every pixel of what's opaque traces at the view's size, up to 64 steps | A device is too slow for them, as a handheld or lavapipe |
| Reflections of what blends, as water or glass, which takes none: the trace is of what's opaque, as the view's depth has it | A game's water or glass should mirror what's round it |
| Reflections reading TAA's history rather than colors of their own: each frame that reflects keeps the view's resolved colors in an image of its own, a full-screen copy, so that a view without TAA, or antialiased otherwise, reflects too | A view's memory is measured to matter, as at 4K |
| Reflections under an orthographic projection, whose depth the trace takes as a perspective's: such a view runs none of them, as it runs no ambient occlusion | A game seen without perspective has glossy surfaces under a sky |
| Point and spot lights' shadows, as cubes and maps in an atlas: a directional light alone casts them | A game lit by them needs their shadows |
| More than one directional light's shadows: of those that cast them, the brightest does | A scene with two suns, or a sun and a moon at once |
| Leaving a model's lights out where a scene places it: each root spawns all of the model's nodes, lights among them, so the showcase's 16 Bistros shine 16 suns, exposed 4 stops down, the one that casts shadows among 15 that fill them in, and their point lights a 16th as bright | A scene places a model that holds a sun more than once for its look, not its cost |
| Blending between cascades over a band at each split, where shadows' sharpness now steps from one cascade to the next | The step shows, as it doesn't in the showcase's Sponza at 1,280 by 720 |
| Shadow maps' size, the cascades' count and splits, and the shadows' offsets and bias, set by a game or by quality settings: 2,048 texels, four cascades, three quarters logarithmic, and the constants `shaders/model.slang` and `src/shadow.rs` hold | A game needs others, or a device is too slow for them |
| Games' shaders' shadows cut out as they cut out what they draw, by a fragment shader of their own, as the masked model shader's are: a game's opaque shader's shadows, and the depth ambient occlusion takes of it, are of whole triangles, as games' shaders compile `vertex_main` and `fragment_main` alone | A game's opaque shader discards, as foliage's does |
| Shadows that soften with the distance from their casters, as PCSS's, and contact shadows: every shadow's edge is six texels of its cascade | A game's look needs them |
| Shadows from what blends, as glass's colored ones: what blends casts none | A game's transparent objects need them |
| Volumetric fog, its light gathered in froxels, a grid of the view's frustum by slices of its depth, which takes the shadows, so that the sun's light falls through the fog in shafts, and every light's light, point and spot lights' too: height fog takes no shadows, so the sun lights all of it, which `Fog::sun` turns down by hand where walls would shade much of it, and takes the sun's light and the sky's alone | A game needs light shafts, fog in shadow, or fog lit by its lamps |
| Fog that varies by volume, in boxes and spheres a scene places, as local fog volumes, and fog that drifts with noise or wind: one height fog fills the world, alike at every point of a height | A game needs a patch of mist, a smoky room, or fog that moves |
| Games' shaders that blend fogging themselves through the engine, as by a module of the engine's they include, rather than by copying `shaders/fog.slang`'s formula, reading the fog in the frame's data: one that doesn't draws unfogged in fog | A game's own shaders that blend draw in fog |
| Glyphs beyond the characters Windows-1252 prints: a font's field holds those alone, and any other character draws as the font's `.notdef`; a field that grows as text needs glyphs, or a list of characters in a font's ID file, would hold more | A game writes a language beyond Western European ones |
| Shaping's features that put glyphs in place of others, such as ligatures and contextual alternates, which text is shaped without, as its font's field may not hold what they'd substitute; and the scripts that need them, those written right to left, and lines of more than one direction | A game's font relies on them, or a game writes such a script |
| Text wrapped to a width, and fallback fonts, a character one lacks drawn from another | A game's UI lays out paragraphs, or mixes scripts in a line |
| Outlines, shadows and glows round text, from the true distance field its field holds in its alpha | A game's text needs one to read over what's behind it |
| Fonts' fields kept between development runs, as a package keeps a game's baked: each font bakes as a development run loads it, Inter Bold's 218 glyphs in about 50 ms in release and 140 ms in a debug build, on the job system's threads, and the engine's own as text is first drawn in it, inside that frame; a font's file read again bakes on the watcher's thread alone | A development run starts slowly enough to matter, as with many fonts |
| Text that faces the camera, as labels over 3D entities do: a `Text` lies in its entity's XY plane, as a sprite does | A 3D game labels its entities |
| Text of several lines edited in the Inspector, whose field for a string is a line's: a text's other lines come from scenes, code and tools | Someone writes text of several lines in the Inspector |
| Fonts other than a file's default instance of outlines: a variable font draws at its axes' defaults, unhinted, and collections (`.ttc`, `.otc`), color fonts and fonts of bitmaps alone don't load | A game needs one |
| Premultiplied alpha for games' textures: with straight alpha, as PNGs store it, a linear filter at an edge mixes in the color of the transparent pixels beside it | A game's art shows dark or colored fringes |
| Sorting what blends back to front by its depth in the view, rather than by its z in the world, which orders it rightly only as seen from in front, as [Rendering](../ARCHITECTURE.md#rendering) says; and what layers draw themselves goes in the order given. Sorting coplanar sprites each by its own depth would sort the swarm's 100,000 every frame the camera turns, and swap overlapping ones as it does | A game blends in 3D, such as particles or glass |
| Sorting each emitter's particles by their depth in the view, on the GPU: within an emitter they draw in their ring's order, oldest first, so particles that blend by alpha stack by age rather than back to front | A game's alpha-blended emitter seen across, as a column of smoke from the side, shows its particles stacking wrongly |
| Soft particles, fading as they near the depth behind them, which takes the view's depth sampled in the pass that draws them | A game's particles cross floors or walls in hard lines, as smoke at ground level |
| Lit particles, shaded by the frame's lights and sky: particles draw their colors as they are, unexposed | A game's smoke or dust needs to take its scene's light |
| Particles colliding with the world, by depth or physics | A game's sparks, rain or debris need to bounce off or stop at what they hit |
| Forces beyond constant acceleration and linear drag, such as noise, vortices and attractors, which need simulating each frame rather than a closed form of the age | A game's effect needs turbulence or attraction |
| Trails as ribbons, strips joining an emitter's particles in birth order | A game needs a continuous trail, as a sword swing, a tracer or a comet |
| Particles in the editor's own world: a stopped world's time stands still, so an emitter shows its first particle alone, frozen, until Play | Someone authors effects in the editor and wants them playing while stopped |
| Curves of size, color and alpha over a particle's life, beyond a line from birth to death | A game's effect needs to fade in, or more than two colors |
| Particles a layer draws itself, as `Context::draw_sprite` draws a sprite, and particles on the canvas | A game's UI needs particles, or a layer emits without an entity |
| Particles' own motion for TAA and motion blur: they write none, so the resolve takes what's behind them, and small fast particles may dim as the history is clamped, and motion blur blurs them as it does what's behind them | Fast particles visibly dim or smear under TAA, or should blur as they fly |
| Shrinking the pool and rings: both keep the most they grew to while any emitter draws | A game's peak of particles leaves memory it needs |
| Bursts more often than a lifetime, or more than a ring's room in a frame: a ring holds one burst beside its births, so the oldest are recycled | A game bursts an emitter faster than its particles live |
| An emitter that first draws with its count of bursts raised bursts at once, so that one spawned to burst does: a world loaded or restored with counts raised, as from a snapshot under `gg run` or a scene saved mid-run, bursts each emitter once as it first draws | Such a burst shows where it shouldn't, as after a restart |
| Back-face culling for games' own meshes: `MeshDraw::new` and `Context::draw` draw both faces, as sprites mirrored by a negative scale need; models' materials alone cull | A game draws closed meshes of its own |
| A layer's mirrored and unmirrored draws of one culling mesh, interleaved, draw a run each, since a run culls one way: the scene sorts the world's by how they wind, keeping them to two | A layer draws a culling mesh many times, some mirrored |
| Drawing a pass's culled runs a draw for each pipeline, whose count the culling writes, as Vulkan's `*IndirectCount` draws take it, each draw reading its mesh, instances and parameters from a record the culling writes rather than from push constants, as games' shaders would then too: the CPU records a draw for each run, of its meshlets or of each of its levels, up to eight, whatever its instances, in each pass that draws it. In the 16 Bistros, recording them in eight passes a frame takes about 0.55 ms of the RTX 4090's render thread's 0.95, which is about as long a frame as the GPU's 1.0 ms | A game's distinct meshes make the render thread outlast the GPU's frame |
| Counting the shadows' cascades' draws, instances and triangles: what the culling reads back is the view's alone | An agent tunes the shadows' cost by them |
| Culling what blends and the canvas, which draw from the CPU, on its levels, whatever the view sees | A game draws many blending instances outside its view |
| A sphere round the vertices of a mesh a layer adds without levels, from where its vertex type keeps its position: such a mesh is never culled, as `Meshes::add` can't read its positions | A game draws many of its own meshes outside its view |
| A log of the entities that left their tables, so that a frame whose ticks spawned or despawned needn't look over every opaque mesh the scene holds, nor the renderer merge the scene's whole order again, each costing the scene's instances once | A game spawns or despawns every tick in a world of many opaque meshes |
| Culling the scene's instances where its table holds them, through its order, rather than gathering them first, which copies each once a frame on the GPU; the cull's push constants hold no room for a second source | A profile shows the gather |
| Building the depth pyramid in one pass, as AMD's single-pass downsampler does: a compute pass builds each level, 11 at 1,280 by 720, each waiting for the one before | A profile shows them |
| Keeping whether each of the scene's instances was unhidden by its slot, rather than by where it's gathered, which moves as instances before it in the gather's order come and go: in the frame they do, the first pass draws those that took another's word, what's hidden among them, and the second what the first missed | A game spawns or despawns often among many opaque meshes, and its frames show it |
| Culling the shadows' casters by what's hidden from the light, or by the view's depth, keeping only those whose shadows fall where it sees: each cascade keeps every caster in its box | A profile shows the shadows' draws |
| Handing the renderer the lights that changed alone: a frame that finds the lights again copies them all into its data, as it does every frame, and the view's clusters' buffer copies them again on the GPU | A game has many lights that change |
| Binning a spot by its cone, rather than by the sphere of its range, into each cluster that sphere meets, though its cone lights only some of them; and binning by each cluster's frustum, rather than by the box round it, which a perspective's far clusters leave loose | A game's clusters hold many lights that light none of them, as with narrow spots |
| Depth other than what a shader's blend implies: draws that ignore it, as gizmos and overlays do, a skybox that tests it without writing it, decals, and sprites cut out by their alpha that write it | Each with its first user |
| Depth bias for lines, so that one lying on an opaque mesh's face shows whole: across its width, a line's quad keeps the line's depth, so where the face slopes across the line in the view, it hides half of it | Lines drawn on 3D surfaces, as debug drawing in 3D |
| Joins where lines meet: lines end squared off, so a rectangle's sides overlap at its corners, which a translucent one shows darker, and a thick polyline's segments overlap at each bend | A game draws translucent outlines or thick polylines |
| Circle colliders' outlines, and the rings of spheres' and capsules', of as many lines as their size on screen needs: each is 64, round to within a pixel up to some 800 pixels across, so a larger one shows its sides and a smaller one draws more lines than it needs | A game shows the outlines of many such colliders, or a large one shows its sides |
| A far plane for perspective, as glTF's `zfar` gives; it sees out to infinity | A glTF camera with one |
| Keeping the view's depth past its passes, to sample: each frame clears it, and its last pass doesn't store it | Post-processing, finding the point under the cursor, or captures of depth |
| Rendering the view's IDs only in frames that pick or outline: every frame renders them, for the reason [Rendering](../ARCHITECTURE.md#rendering) gives | A profile shows them |
| Answering picks sooner than [Rendering](../ARCHITECTURE.md#rendering) says they come: reading them as soon as the timeline says the GPU has rendered them would win a frame or so | Picking that the latency shows in, such as highlighting what's under the cursor |
| More than 64 picks a frame: `Context::pick` gives none past them, as many as a frame reads back | A layer picks more at once, such as many cursors or touches |
| A depth format for a device without D32_SFLOAT depth, which the engine refuses (`gpu.no_device`) | A target device lacks it |
| The fly camera controller and the editor's camera looking by the mouse's raw motion, the cursor grabbed while the right button is held: they look by the cursor's moves, which stop at the screen's edges | Someone misses it |
| The cursor's grab checked by hand: locked on macOS and Wayland, kept inside the window on Windows and X11, egui's cursor hidden with it, and the editor grabbing its own for its game in Play, letting go at Escape; the tests check what's asked, and the session was locked as it was made, so no window had focus | Someone is at a machine of each, Windows' first |
| Mipmaps and anisotropic filtering for the textures layers add: `add_texture` and `add_sprite_sheet` give them one level, sampled without anisotropy, so one drawn smaller than it is shimmers. A sprite sheet's cells, packed edge to edge, will then mix together at the smaller levels and under anisotropy, which reads past the texels the sprite shader keeps to, and need space around each cell or an array layer each | A game draws its own textures smaller than they are |
| Keeping a masked texture's coverage as its levels shrink: averaging thins what its alpha cuts out, such as leaves and fences, at a distance, where scaling each level's alpha to keep the share above the cutoff would keep it | A masked model thins out at a distance |
| Sprite sheets with space between or around their cells, as some tile sets come; `SpriteSheet` takes cells packed edge to edge from the top left | A game's art comes that way |
| Tiling one cell of a sheet: a repeating sampler repeats the whole texture | A game tiles part of an atlas, such as a background |
| A sprite that samples past its rectangle, so that a seamless texture tiled across sprites side by side blends across their edges under a linear sampler | A game tiles one texture across sprites |
| A tile map in the engine, which scenes save; the sandbox's map is code of its own, its tiles sprites that snapshots and scenes save each with its texture's name | A game edits its map |
| glTF that `Context::add_model` refuses or ignores, as it lists: morph targets and animations' channels of their weights, more than four joints a vertex (JOINTS_1 and WEIGHTS_1), cameras, models without a scene and scenes other than the default, sparse accessors and accessors without a buffer view, primitives other than triangle lists, samplers that wrap across and down differently, minification filters, a material's textures that read a second set of texture coordinates, as occlusion baked for a lightmap does, an image sampled as both colors and data, material extensions past unlit, emission's strength and specular-glossiness (texture transforms, clearcoat, transmission, as the Bistro's 18 glass materials have it, which draw opaque, specular, sheen and the rest), quantized or compressed meshes and textures (KHR_mesh_quantization, Draco, meshopt, Basis Universal), and integers written as floats, such as `1.0`, which glTF allows | Each with the first model that needs it |
| Skinned primitives whose materials blend, which draw as they were bound, unmoved: what blends draws from the frame's own draws, which take no mesh of an entity's own | A skinned model blends, as hair cards and eyelashes do |
| Skinned meshes' own motion under TAA and motion blur: a skinned vertex moves with its instance's transform alone, as the frame before's skinned vertices aren't kept, so what its joints move leaves a frame's smear, and blurs as its instance moves alone | Smearing shows on a character that moves its limbs fast, or its limbs should blur as they swing |
| Sharing a skinned model's bind pose and weights between its instances: each instance's entity draws a copy of its model's mesh, its vertices and weights twice over, and the frame's moved vertices again | A crowd's memory is measured to matter |
| Skinned meshes cut into meshlets and drawn into the visibility buffer, their meshlets' bounds made again each frame from their joints: they draw by their indices, shaded as they draw | Skinned characters are measured to cost the view, as a crowd would |
| Skinned vertices carried past the reach their clips give, as code that poses joints itself, inverse kinematics or a ragdoll, carries them: they clamp to its box, which views cull by | A game poses joints past its clips |
| Root motion: a clip's root travel moving its entity, as a character controller takes it | A character's ground speed should come from its clips, as a fox whose feet slide past its run's speed |
| Layers and masks: a state machine of some joints over another's, as an upper body's, and additive clips | A character acts as it moves, as a fox that kicks while it runs |
| Events at a clip's times, as a footstep's sound or a hit's frame | A game times sound, particles or gameplay to a clip's frames |
| IK: feet planted on uneven ground, a head or a hand turned toward a point | A model walks slopes or stairs, or looks at what it faces |
| 2D blend spaces: clips blended by two parameters, as a direction and a speed | A character strafes or walks backward |
| State machines as asset files of their own, shared by entities and prefabs and reloaded as saved | Several models share a machine, as a game's characters do |
| Sub-state machines, a fade a transition cuts short, a transition's offset into its state, a from-any transition into the state it's in | A machine grows past a dozen states, or a game needs one of them |
| Scripts reading a machine's state, as `gg.state(entity)`, which scripts can't read as a field, holding no strings | A script branches on what its model plays |
| Where frames draw an entity between ticks, for layers: `Context` gives none, so Foxball's view eases toward its fox as each tick left it, a little behind | A layer's camera must sit exactly on what it follows, as a first-person view's does |
| Damping on 3D bodies, of their speed and spin: Foxball slows its ball itself, each tick | A second game needs drag, or rolling resistance |
| Foxball shipped and run as it ships in CI, as Shards is: its tests cook its package and replay its live recording from it | A change could break its shipping alone |
| Foxball's menu sounds, as Shards plays them for its focus moving and its choices: the menu is silent | Foxball has sounds of its own for them |
| Pausing a run alone as Foxball's menu opens: the engine has no pause a game can ask for, so the match goes on behind the menu and the bot can score meanwhile | A game alone needs a pause, or players lose goals to the menu |
| Asking the other player before a session's New match: either player's menu starts the match afresh for both, scores and wins wiped, at the next tick | Players of a session start matches over each other's |
| A bot for a session's empty player, as a server's player 1 before a client takes it: a session's foxes are all people's | A session wants a player it hasn't |
| Foxball's options kept between runs, its volumes and its bot's difficulty: they're the run's, each run starting at full volume and Normal | Players expect their settings kept |
| Foxball's goals' crossbars as instances of its wall prefab: they collide with nothing, and an instance can't drop a component its prefab gives, nor can a prefab's root link another | Prefab variants land, or the bars should stop a lofted ball |
| Foxball's foxes as instances of a prefab: each fox, its model, animator, state machine and dust, is built in code as the arena attaches | The editor places players, or a game has a second kind of character |
| Foxball's players' colors read from one place: each is written in ui/menu.ui's texts and, brightened, in arena.ron's emitter at its scorer's goal mouth | The players' colors change, or a game has more than two players |
| Textures for Foxball's particles, as dust puffs and confetti: its emitters draw soft discs | Its look gets an art pass |
| Reading assets' files off the main thread, and decoding sounds and reading music there: a layer's add, and text that names an asset, read its file whole on the main thread, images' pixels alone decoding on `gg-load`; `add_sound` decodes before it returns, and `add_music` reads its file whole, up to 64 MiB | A game's reads or sounds hitch as it plays |
| Loading meshes' vertices and indices off the main thread: a model's are read with its nodes, as the simulation needs those | A model's read hitches |
| Decoding on more than one thread: `gg-load` decodes one image at a time, so a large model's images hold up the small textures added after | Loading takes long enough to matter |
| Models that take more than 8 GiB to read, of their buffers, images, encoded and decoded, vertices and nodes, which `add_model` refuses with `engine.unsupported_model` | A game's model is that large |
| Mip levels of a model's images read again, as its file changes, made on the render thread as their turn to stream comes: a 2,048 by 2,048 texture's take about 0.3 s of that frame in a debug build; `gg-load` makes a model's as it first decodes them, and a package's come cooked | Blits on the GPU, or the watcher's thread making them, if a reload's hitch matters |
| Layers making textures from their own pixels; `Textures::add` takes pixels, but `Context` adds only PNGs and JPEGs | A game makes its own textures |
| Keeping the frames' arenas near their frames' peak, within the GPU's memory budget: an arena grows by a buffer at least twice its last, and merges them as it resets, so it can hold a few times what its largest frame took, in memory the CPU writes, of instances' transforms, IDs and parameters and the UI's vertices, indices and texture changes: `engine.perf` has the showcase's arenas holding 4 MiB and the sandbox's 6 MiB, about four times their largest frame's 1 and 1.3 MiB | A game's arenas hold tens of megabytes, as its `engine.perf` shows |
| A frame's copies through a transfer queue start only once it's submitted, after its passes are recorded, so an asset draws from the frame after the one that copied its last part at the soonest | Pop-in shows |
| The frame's arena kept within the device's `maxMemoryAllocationSize` and `maxBufferSize`, each at least 1 GiB: it grows by doubling, so a frame that draws more than that at once fails, which ends the run; streaming keeps staging to about a frame's budget | A frame draws that much at once |
| RenderDoc captures of a frame (`--capture N`) waiting for nothing to stream, as the tool API's captures do: an early frame shows stand-ins | Someone captures an early frame |
| What's decoding and streaming in the tool API's `status`, which `Context::streaming` gives layers | An agent needs it |
| Images' sizes checked against the device's `maxImageDimension2D` and `maxFramebufferWidth` and `Height` as they're made; a pass checks its viewport against the device's limits, which an image past those exceeds only if it's past `maxViewportDimensions` too | A device's limits fall below a window's size |
| Host image copy where a transfer queue exists, or where the driver says it costs the GPU in reading the image, as KosmicKrisp's does, turning off Apple's lossless compression, so that macOS uploads through the graphics queue; and meshes written straight into GPU memory the CPU reaches, as ReBAR and unified memory allow, rather than copied | A measurement shows either is faster |
| More than one view, such as split screen, or the editor's scene view beside the game's: the view sees through one camera, the world's primary one, a layer's viewpoint or the tool API's look's | A game or the editor draws several |
| A camera's own aspect ratio, kept by letterboxing the view or by a target of its own, as Unreal's cameras can: a camera fits the view's aspect | Cameras render into targets of their own, or a game needs one aspect on every display |
| The editor drawing the world's cameras in its view, as outlines of what each sees | The editor shows a game's cameras |
| Moving the world's camera from the editor's view, as Unreal's Pilot and Blender's Lock Camera to View do: a move of the editor's camera goes back to its own | Someone frames a camera's shot from the view |
| A scene or prefab loaded with a primary camera making the world's other cameras not primary, as an operation's does: a load sets fields as they're written, and of several primary, the view sees through the lowest ID's, warning | A prefab brings a camera of its own, as a player's would |
| Finding the point in the world under the cursor, as `Context::pick` finds the entity there: the editor's camera finds one on a plane itself, and a model dropped from the Assets panel lands on the grid's plane, but nothing reads the view's depth, and the tool API's `pick` answers the entity alone; and the orthographic camera controller zooms about the view's center | Something places what it adds at the cursor's depth, such as a model dropped onto another, an agent places something at a pixel, or a game zooms toward the cursor |
| A dockspace's view in a window of its own: it docks and floats over the main window alone, since only the main window's input reaches the layers, and its image is made at the main window's scale | The editor shows the game on a display of its own |
| Showing whether keys reach the view, such as an outline around it while they do: from a press on it, or with the cursor over it | Someone misses it |
| Gamepads following a dockspace's view: they reach the layers whichever panel has the keys, as they do in games | The editor binds a pad, or a panel takes a pad's input |
| F framing what's picked, the editor's camera easing to see it whole | Someone frames often |
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
| A rotating cursor for the editor's Orbit tool: egui has none, so it shows a hand | egui adds one, or someone misses it |
| The lines across the editor's perspective grid reaching the horizon: they fade out some 65 to 200 camera heights away, since its f32 coordinates hold four decades of spacing; the lines toward the horizon go on | Someone misses them |
| Cameras far from the origin: their matrices are f32, so 12,345 units out a view a tenth of a unit across moves in steps of about 1% | A game's world is that large |
| Zooming from keys or a gamepad, as a rate beside the wheel's lines; the orthographic camera controller zooms by the wheel alone, in its own event handling. And zooming a camera with perspective, by its field of view or by moving it in, or changing the fly camera controller's speed by the wheel, which does nothing to it | A game binds zoom elsewhere, or zooms a camera with perspective |
| Per-user bindings, kept as RON in the per-user directory beside the dock's layout, and a UI that binds them; a game binds its actions in code | A game ships its settings |
| Chords, modifiers, holds and double taps, such as Ctrl and a key: there's no modifier state, and Ctrl+Z reaches layers as `ControlLeft` and `KeyZ` | A game binds a combination |
| Keys' and gamepad buttons' labels, for prompts that show a binding: both are positions, and the engine drops what a keyboard's layout makes of a key, and whether a pad is an Xbox or a PlayStation one | A game shows its bindings |
| A hold of an axis shorter than a tick, which moves nothing: its two changes apply at one tick | A game needs one, as a flag that the tick clears |
| An engine type for what an action holds in the world, such as a button's held flag and its presses this tick; each game writes its own component | A game other than the sandbox and Shards writes the same |
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
| Codes for dependencies' own warnings and errors, which reach the log as they give them: as a pad connects on Windows.Gaming.Input, gilrs warns, without a code, that it has no mapping for it, since that backend gives every pad a nil UUID, and goes on with its default mapping, the right one there; and as a sound's or music's file doesn't decode, symphonia logs an error, without a code, beside the engine's `engine.bad_sound` or `engine.bad_music`, which `gg run` lists among a run's errors as it ends | A tool acts on a dependency's warnings, or the tool API serves diagnostics |
| Checking a game's vertex and parameter structs field by field: the check compares each struct's size with its Rust type's and finds its fields 32-bit, so fields of one size in another order or of another kind, such as an integer read as a float, draw garbage without a message. A derive on vertex and parameter types would give it each field's offset, scalar type and vector width | A game's struct mixes fields of one size but other kinds, or an agent's edit to one draws garbage unseen |
| Wave operations in games' shaders, and the other SPIR-V capabilities a device's properties decide rather than its features, which the check refuses | A game's shader needs them |
| Hot reloading the engine's own shaders, which the build compiles in | Work on the engine's shaders wants it |
| Watching the files of the shaders, textures, models and fonts loaded through the OS's notifications, rather than by their times and lengths every tenth of a second, which reads every watched file each time, and, while an asset fails to read, every file in the directories its files are in, but none in directories within them: an include in one of those that a failing shader found broken, fixed where it is, waits for another change | A game loads so many assets that its watcher's looks take long |
| The watcher of assets' files allocates as it looks: each asset's stamps, collected at every look on every platform; on Windows, each file's path, which it converts to UTF-16 to read the file's times; and the entries of a failing asset's directories, which it lists. So does the asset watcher, each second, as it lists the asset root and reads its ID files. Both run in the runs that serve the tool API, so the `frames` test runs its app without it (`--no-api`), which leaves the API's part of a frame unchecked for allocations | A development build's frames must allocate nothing on any thread |
| Reading at once the assets whose changes hold on the same look, rather than one after another: after an edit to an include that N shaders share, the last reloads about N tenths of a second after the first, and no file is looked at meanwhile | A game with many shaders that share an include, or many assets edited at once, as by a checkout |
| `VK_KHR_pipeline_binary`, which saves and loads pipelines whole; ash 0.38 lacks it | ash's next release, or a driver whose pipeline cache keeps less |
| Saving the pipeline cache as pipelines are made rather than as the run ends, which a crash skips, so that a game `gg run` restarts after one makes its pipelines again | A restart after a crash takes noticeably longer for it |
| Pruning the pipeline cache, which keeps every pipeline it was given, an edited shader's old ones too | It grows to tens of megabytes |
| Requiring a slangc that decorates sampled images `NonUniform` itself, Slang 2026.11's or later, such as the Vulkan SDK 1.4.363's, and dropping gg-renderer's `decorate_non_uniform`, which does it for older ones | The engine requires the Vulkan SDK 1.4.363 or later |
| Development builds of games compile their shaders with slangc as they're added, so running one needs slangc where its build found it; builds without `dev` run them compiled from their package | A development build runs on a machine other than the one that built it |
| `MeshDraw`'s mesh and shader named, as `Sprite.texture` is: opaque until then, shown but not set, saved in scenes or hashed, and carried across a restart by their indices only where the layers added the same assets in the same order; and setting them by name | A game builds its draws from data |
| Watching the asset root through the OS's notifications of changes: its thread scans it every second, walking it whole and reading every ID file, about 70 ms for 1,000 assets on Windows, and reads whole each asset's file that changed, for its print, every one in a clone's first run, which delays its next scans but no answer; and a run's start scans it on the main thread, before its layers load, flushing each ID file it writes to the disk, about 6 ms apiece | A project's asset root of tens of thousands of files |
| IDs for shaders, scenes, sounds, music and other kinds, for assets outside the asset root, and for assets reached through links: the scan gives textures, models and fonts under the root alone, never through a link | Each kind as something first names it by ID |
| A recording's scripts as they were as it started, which its replay reads from their files, as it reads scene files', so that a script changed since makes it go another way at its first tick | A recording travels without its game's files, as a bug report does |
| Scripts that spawn and despawn entities, or give and take their components, which Rust code alone does | A game's behavior in Luau makes or unmakes what it acts on |
| Whether an entity has a component, from a script: one that reads or writes a field of a component its entity lacks fails there (`script.error`) | A script acts on entities that differ in what they have |
| Finding an entity by its `Name` from a script, which reaches another only through an entity field that a scene, the Inspector or Rust code sets | A script finds what it acts on as it runs, as a quest's would |
| Text for scripts: fields of strings, a script's own and Rust components' such as `Text.text`, which scripts reach none of, reaching numbers, bools, vectors, quaternions and entities alone | A script shows text or chooses by it, as dialogue does |
| Input that scripts read, which reaches them only through the components a game's Rust commands set, as Shards' `Player.heading`, since input reaches the simulation as commands that a recording and lockstep peers keep: a game without Rust code can't be steered through its scripts | A game's controls are written in Luau |
| Random numbers for scripts, from a state the world holds, in place of `math.random`, which they go without | A script draws random numbers, as Shards' `collect` would |
| A budget for all of a tick's calls into a game's own scripts together, as a mod's calls share one, or, in development builds alone, one by time: each call fails past its million interrupts, about 0.9 s of a debug build in a loop that reads a field each turn, so a script that runs past it in every call slows the game to a crawl rather than hanging it | A runaway script's crawl gets in the way |
| The Inspector's hints for scripts' fields, as Rust's `#[reflect(degrees)]` and `min` give a component's: a script's numbers show as they are, the showcase's `Spin`'s speed in radians a second | Someone tunes scripts' fields in the Inspector |
| Type definitions for luau-lsp, of the `gg` API and a game's component types and their fields, so that a code editor completes and checks scripts: none ships, so a script's mistakes show only as it loads (`script.bad`) or runs (`script.error`) | Someone writes scripts in a code editor |
| Debugging scripts from a code editor, through the Debug Adapter Protocol, as a `gg dap` beside `gg mcp` and a VS Code extension that names it: the tool API's `debug`, `gg debug`, its MCP tool and the editor's Debugger panel debug them | Someone debugs scripts where they write them |
| Evaluating expressions where a script has stopped, watches, and the fields of a table or an entity a local holds: a stop shows each local's and upvalue's value, a table as `(table)` and an entity as its ID, which `gg entity` reads once the game goes on | Someone needs more of what a local holds than its value |
| Breakpoints with conditions or hit counts, and logpoints | Someone stops too often at one |
| Stopping where a script fails, or runs past its budget, rather than warning (`script.error`) and going on | Someone debugs a failure that's hard to stop before |
| The tool API's requests that need the main thread answered while a script has stopped: they wait, as during a file dialog, so the Inspector reads the world, an edit applies, and `quit` and the window's close end the game, once it goes on; `gg run`'s end ends a game at a stop, and gg says what a request that timed out waited for | Someone inspects, edits or quits a game at a stop |
| Answering the window system while a script has stopped on Linux and macOS, whose compositors may take a windowed game's window for one that doesn't respond; on Windows its messages are dispatched, as a modal loop does | Someone debugs a windowed game there |
| Breakpoints in what a script's chunk runs as it loads, which places them once it has run | Someone debugs a chunk's own code |
| Stepping on from where a pause broke into a call entered while the debugger wasn't armed, which goes on to the next call: every call entered stepping would slow every development run | Someone pauses into long calls and steps through them |
| Keys for the Debugger panel's buttons, as F5, F10 and F11 | Someone steps often |
| Loading the textures, models and fonts that scene text names where a layer loads it into a world itself, as inside a tick, as the sandbox's grid reload does: a world can't reach the renderer's textures, which are the context's, so `World::load_scene` finds only those loaded, while `Context::load_scene_file`, the tool API and the engine's other reads load them | A game loads scene text in a tick that names assets no layer adds |
| A lockstep joiner loading for its tool API an asset its host hasn't named (`asset.not_given`), whose name could differ from the host's; the host's edits load any | Someone edits a joiner's assets from its own editor |
| `gg mv` and an MCP `mv`, moving an asset and its ID file in one step: a file renamed alone keeps its ID by what it holds, but not one edited in the same step, nor one renamed in a fresh clone before its first run, which has no cache of prints | An agent's renames lose IDs that pairing by content can't keep |
| A lockstep joiner that joins after its host's texture was renamed finding its file where it is now: it loads it by the path its host loaded it from, as the host names it, standing in where that's gone, as it is on one machine | Someone renames assets during a lockstep session |
| Following a rename in case alone, as Windows' and macOS's file systems allow, of an asset whose ID file keeps the old case: the ID file matches only when case is ignored (`asset.meta_case`), so the asset keeps its ID for the run, then goes without until its ID file is renamed too | Someone renames assets in case alone |
| Warning, in a development run, of a path whose case differs from its file's, or on macOS its Unicode form, which Windows' and macOS's file systems find but a package doesn't | A game's path runs in development and fails from its package |
| Checking that a package holds every file a game's code loads, as by listing those a development run read: the cooker holds what the scan of ID files finds, and checks the start scene alone | A file a game loads is left out of its package unseen |
| Levels of detail in development runs: a model read from its glTF draws one level, the cooker's levels showing from a package alone, as `--package` gives a development build | Someone tunes a model's levels without cooking it |
| Hysteresis or a blend between levels of detail: a draw's level is the coarsest whose error spans a pixel of the view, so a draw at a threshold can flip between two from frame to frame | A level's pop shows |
| Simplifying meshes by their colors and texture coordinates too, through meshoptimizer's attribute-aware simplification: levels are simplified by their positions alone, keeping seams, so a coarse level of a model colored by its vertices may shift its colors | A model's coarser levels look off where their error doesn't say so |
| Pruning the small parts a mesh's coarser levels could drop: meshoptimizer's Prune, without an error limit, drops whole parts that should stay, so the cooker doesn't prune | A model of many small parts keeps too many triangles far away |
| Locking the borders a node's primitives share as they simplify, so that their coarser levels meet: each simplifies apart, so their seams may open by as much as the pixel each level's error allows | A model shows gaps between its parts |
| Compressing a package's meshes, by meshoptimizer's vertex and index codecs and zstd: a cooked model holds its vertices and indices as they are, its images alone supercompressed | A package's size matters, as for a download |
| Choosing a texture's compression for each texture, as RGBA8 for pixel art BC7 changes, and BC4 for data of one channel: games' textures cook to BC7, by bc7enc_rdo at its default effort, and models' maps to BC7 or BC5 by what they hold | A game's art suffers |
| A texture that's both a layer's and a model's image held once: a cooked model holds its images, with their chains of levels, and the package the files layers load apart, of one level, so a PNG that's both is held twice | A package's size matters |
| Compressing textures on several threads: the cooker compresses one image at a time, as ctt does without rayon, which deny.toml keeps to gg-engine | Cooking takes long enough to matter |
| Cooking to the same bytes on every OS and architecture, as on one machine: bc7enc_rdo's ISPC picks its instruction set as it runs, and each platform's compiler builds meshoptimizer's C++ | Packages' bytes are compared across machines, as a build cache would |
| Splits in a torn-off window, which holds one tab group | A tool wants panels side by side outside the main window |
| Reordering tabs by dragging them along their bar; a drop among the tabs adds one at the end | Someone reorders tabs |
| A minimum width for each panel, as Hazel keeps its Properties panel 370 pixels wide: a split keeps each side 48 points, and the Inspector's rows of axes clip in a narrower panel | Someone narrows the Inspector past its rows |
| Checking Windows' frame on Windows 10, whose DWM may draw no border along the top of a window whose inside reaches it | Someone runs the tools on Windows 10 |
| Checking Windows' frame around torn-off windows as their scale changes, across monitors of different scales | Someone docks across such monitors |
| A button in the title bar hiding the panels docked above the view, as those left of, below and right of it hide, and keys for the buttons, as VS Code's Ctrl+B, Ctrl+J and Ctrl+Alt+B | Someone docks a panel above the view and wants it out of the way, or reaches for the buttons often |
| On macOS, the menus in the system's menu bar, and a double-click on the title bar doing what the user's setting says | A tool ships on macOS |
| Alt and access keys to reach the menus, and layers adding items to one another's menus | Someone reaches the menus from the keyboard, or a layer adds to another's menu |
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
| Aliases for renamed fields, variants and component types, which a restart, a scene and a script's reload would match values by: a restart drops a field, a variant or a type it doesn't find, and a scene refuses one, as it refuses a field its type hasn't, against typos; a script's reload drops a field renamed, the new one taking its default, and refuses to rename its component (`script.renamed`); and tuple fields reordered among themselves, which match by their places | Someone renames a field whose values they want kept |
| Undoing, across a script's reload that changed its component's fields, a set of a field the reload dropped, or of the whole component as it was: refused, as such a set made now is; what a delete or a remove took comes back with the fields as they are | Someone undoes across such reloads, as an agent editing a script and its entities together would |
| In the Inspector, turning an option on or off, and adding or removing a list's elements; by path, a value sets them whole | A component needs one edited |
| Flags for fields that saving, the world's hash or replication skip, and fields that are read only; `Parent` is read only as a whole type, which `set_field` refuses | Each with its first user |
| Generic types deriving `Reflect` or `Component` | A game needs one |
| Drag speeds as hints; the Inspector picks them from a field's bounds and kind | A field drags too coarsely or too finely |
| Paths resolved once and kept, for sets made again and again | A profile |
| Picking an entity for a field, a parent or a child picked by clicking it; the Entities panel scrolling to an entity found or picked in the view, or to a root that an add or a remove moved to the end of another table, and so of the tree's roots | Someone sets an entity's field in the Inspector, or loses an entity in a long Entities panel |
| Finding an entity by its name: the Find field takes an ID | A world names more entities than the Entities panel shows at once |
| An icon on each of the Entities panel's rows saying what its entity is, from its components, such as a camera's, a sprite's or a mesh's: a row says its name alone | Names alone stop telling entities apart at a glance |
| Where an entity draws between ticks, for a layer's viewpoint that follows it, which would otherwise judder against it, and for the editor's handles, which show what's picked, as it moves, up to a tick ahead of where it draws: layers read the world up to a tick ahead of where it draws, `World::pose` giving where the latest tick left an entity, and the engine's drawing and the world's camera compute their own | A layer's viewpoint that follows an entity, or someone editing something as it moves |
| Turning interpolation off for an entity or a subtree, so that it draws at its ticks; every entity draws between them | A game wants something drawn at its ticks |
| Rotations of more than half a turn a tick, which draw the shorter way round, the other way; Hermite easing from velocities would draw them, and curves between ticks | Something spins that fast, or a curve between ticks shows |
| Drawing an entity despawned in a tick on its way there until the next: it vanishes up to a tick before it reaches where it stopped | A game's despawns show it, as bullets vanishing short of what they hit |
| Copying the world as only the last tick a frame runs starts: a frame that catches up copies it as each of its ticks starts | A profile shows a catching-up frame's copies |
| Copying the world, and finding what each row draws from, on the job system's threads, or only for the rows that moved: both run on the main thread, for the swarm about 0.19 ms a tick and 0.10 ms a frame that ticked; and blending only the rows that moved, which takes the job threads about 2.6 ms of their time a frame | A profile shows them |
| Sets that jump: an Inspector set of a moving entity's transform, and its undo, glide there over a tick, as a drag does, rather than reset its interpolation, as an agent's set through the tool API does too | Someone wants a set of a moving entity to glide |
| A flag for headless frames at a fraction of a tick of their own, for captures between ticks and for perf runs that measure blending, which headless frames, drawing their ticks, leave out; tests have them, through `App::headless_step` | A capture between ticks, or a change to blending that needs measuring headless |
| Reparenting that keeps where a child is in the world, by decomposing its matrix, which loses any shear; `set_parent` keeps its own transform | Someone reparents in the editor's panels, or an agent attaches something where it stands |
| Spawning a child into its table at once from code: `spawn`, then `set_parent`, moves it to another table; a load and a create operation spawn one there | A profile |
| Children in an order that means something: reordering them, and saving their order; an undo puts a child back last | Someone orders children in the editor, or UI laid out by the hierarchy |
| Each session's seed and count saved with the world and merged with it, so that an ID maps back to its session and when it was drawn, and a load refuses IDs no session drew | The tool API refusing an ID an agent made up |
| An `EntityId` query term | The first system that reads IDs as it iterates, such as replication |
| Errors carrying their entity, and a scene's line and column, as fields, not only in their message, which the tool API gives as its code and message alone | A tool reads an error's entity or place without parsing its message |
| Drawing rows at one z in ID order for less. Every frame reads them in that order rather than their tables', about 0.9 ms of the swarm's 4.3 ms frame in release; a frame that changes rows or z's, or follows a restore, sorts them by ID again, about 2.5 ms more for 100,000; and 100,000 sprites each at a z of their own, as sorting by y gives them, take about 5 ms, for a group and an item each. Tables kept in ID order would make the reads a sweep, and a radix sort the sort quicker | A game's frames need the time |
| Overrides of a model's nodes, which scenes don't save and tools don't change, as a prefab's instances keep theirs | A node of a model placed in a scene needs a field of its own |
| Removing one of a prefab's entities or components from an instance: tools refuse it (`ecs.prefab_entity`), and a save refuses an instance code took one away from (`scene.prefab_missing`) | An instance needs one of its prefab's entities or components gone |
| Writing an instance's overrides back into its prefab's file | The editor's prefab UI offers it, or an agent edits a prefab through an instance |
| Prefab variants: a prefab whose root links another, refused (`scene.prefab_variant`) | A game wants one prefab as another with changes |
| Giving an instance's entity a link of its own, or changing or taking away a nested instance's link in one instance (`ecs.prefab_entity`, `scene.prefab_entity`) | One instance swaps a prefab nested in it |
| Overrides finer than a component's top-level fields: a nested struct's field, a list's element, or a tuple struct's or enum's part is written whole where any of it differs | Instances often differ in one part of such a field |
| Templates kept across loads, saves, operations, refits and reads of an instance's overrides, which the editor asks of the picked entity 20 times a second and a game's own Inspector each frame, each of which parses an instance's prefab text again | Scenes of many instances load, save or edit slowly, or picking an instance of a large prefab slows frames |
| Spawning a prefab without parsing its text again: `Prefab::spawn` and `World::place_prefab` parse it each time, which allocates | A game spawns prefabs as it runs, rather than as it loads |
| A prefab standing in for a following run with its entities: a replay or a joiner that can't read a prefab's file stands in with a root alone, which gives no entities, so a scene's overrides of the instance's entities load as roots of their own, the fields they leave out taking their types' defaults, which can refuse a load the recorded run took | A replay or a joiner runs without a game's prefab files, as a bug report's might |
| Instances taking a prefab file's change in a recorded run or a lockstep session, which warn `asset.prefab_changed` and keep it until the run restarts | Someone edits prefabs while recording or in a lockstep session |
| Save as Prefab in a game's own Inspector, which writes files that neither a replay nor a lockstep peer of its commands would have: the Entities panel offers it in the editor alone, through the API's `save_prefab` | A game's own Inspector authors prefabs, as a game without the editor would |
| A place and a name chosen for Save as Prefab: the editor saves in the project's `prefabs/`, named after the entity and numbered while taken; only the API takes a path | Someone wants a prefab saved elsewhere, or named otherwise, from the editor |
| Overrides marked inside a field: the Inspector marks a component's own fields, not the parts of a struct, list or vector within one, and a tuple of one field, as `Name`, or an enum, on its header, a revert setting each back whole | An instance's entity differs in one part of a large field |
| A revert drawn as it will be while it waits: the field shows its value before until the next tick, or in the editor until the game's answer and the read after it, about 50 ms | That wait shows |
| Instances told apart in the Entities panel's rows, as Unity colors them: the Inspector alone says an entity is an instance's | Picking through scenes of many instances gets confusing |
| A derive hint for `Component::base`, and opaque values in lists and options, which saving refuses, as does a set's undo (`reflect.opaque`) where the old value holds something opaque in a variant, an option or a list; a type with opaque fields implements `Component` by hand to load, as `Sprite` does | A game's component holds something opaque |
| Names in scenes beyond ASCII identifiers, which saving refuses (`scene.bad_name`), and a project manifest's strings beyond printable ASCII, which reading refuses (`project.bad`) | A game names a type, a field or a folder so |
| Loading a scene in place of some of a world's entities in one step, as the sandbox's grid reload does in three; a load refuses IDs the world has, and `World::replace_scene` replaces all of its entities; a scene file a game loaded through `Context::load_scene_file`, saved, applies as operations | Another game reloads part of its world from a file of its own |
| A version in scene files, a file without one reading as the first | The first change to the format that old files can't be read by |
| Comments in scene files, which a save drops | People annotate scenes |
| Opening a scene from the editor's Assets panel, by a double click or a drag onto the view, as the drag of the editor's own world did; the File menu's Open and the Scenes panel open one | Someone opens scenes from the Assets panel |
| Renaming, duplicating and deleting scene files from the editor's Scenes panel, which opens them, its New emptying the open scene, which Save As names | Someone manages scenes from the editor |
| Scenes of the showcase for sounds, music and a game's UI, which layers play and draw in code rather than scenes holding them; Shards has all three | Scenes hold behavior that plays or draws them, as Phase 9's scripts will |
| A texture from the Assets panel let go where no sprite takes it making one: on empty space a sprite of its own, on an entity without one a `Sprite`; and a font so a `Text`; the Logs, and the Inspector, say it took nothing | Someone lays out a 2D scene from the Assets panel |
| Drops from the Assets panel onto the Entities panel's rows, as a child of the entity there, and onto the Inspector's texture and font fields: the view alone takes them | Someone drops onto them |
| Dragging from the Assets panel torn off into a window of its own: a drag stays in the window it started in, which takes the button's release, so it never reaches the view | Someone keeps the Assets panel in a window of its own |
| Listing the editor's Assets panel's folder, its Scenes panel's scenes folders and its hub's projects off the main thread, as a thread watching them would: each is listed on the main thread each second it shows, the hub reading each project's manifest, which a folder of many thousands of files would hold up for milliseconds | A project's asset or scenes folder that big, or a user with many projects |
| Keeping edits made while a copy plays, which go with it at Stop, as Unity's do, but a scene file's save; and showing that the game plays beyond the lit Play or Simulate, such as a tint round the view | Someone loses an edit made while the world runs, or edits a running world for the scene |
| Play and Stop, and the open scene's New and Open, in recorded runs and lockstep sessions, and its Save in lockstep sessions: kept as lines a replay brings the world as edited back at, or starts history afresh at, and applied by both peers at one tick, a save's text with them; `stop` answers `api.no_stop` there, `scene` answers `api.no_scene`, and `--stopped` is refused with `--record`, `--host` and `--join` | Someone records, or plays a lockstep session, in the editor |
| Bodies under a parent, moved in the world's space as their ancestors carry them, and colliders on a body's children; a body under a parent isn't moved (`physics.child_body`) | A game builds a body of parts, or nests one |
| Colliders other than 2D's boxes and circles and 3D's boxes, spheres, capsules and models' triangles, such as polygons and convex hulls; sensors; which colliders collide with which, by groups; damping; a body's mass and center of mass set, rather than given by its colliders' densities; and contacts for scripts | Each with its first game use |
| Mesh colliders' outlines: `MeshCollider3D` outlines nothing, as its model shows where it is, and Sponza's would draw some 790,000 lines | Someone needs to see a mesh collider apart from its model, as one of another model's |
| Characters carried by what they stand on, as a moving platform or a turning one; characters of other shapes than a capsule; a character's rotation turned by its controller, toward where it goes; and a character's own collision filter, as one that passes through other characters | Each with its first game use; the platforms with the 3D demo's level, if it moves |
| Casts that give every hit along their way, rather than the first; overlap queries, what's within a shape or at a point; casts that pass through colliders by component or group rather than an entity's; and casts in 2D | Each with its first game use |
| Flipbooks of sheets whose cells don't fill their texture, or have space between them, by cells in pixels as `SpriteSheet` takes them; clips, named runs of cells a game switches between; and events at a flipbook's cells, as a footstep's sound at a foot's | Each with its first game use; clips and events with the 3D demo's animated characters, if they need them |
| Tweens of another entity's fields, or of fields deeper than a vector's component, as a list's element; tweens one after another or together as a sequence; a tween's end told to code and scripts, as an event, and scripts asking whether a field is tweening; and tweens of integers, or of colors through a space other than linear's | Each with its first game use |
| A 3D body's rotation locked about some axes alone, as one that turns about Y but never tips: `RigidBody3D::fixed_rotation` locks all three, and what sets its rotation turns it | A game's body tips about some axes but not others |
| Ragdolls: a skinned model's bones made bodies held by limited joints from its nodes | A game knocks a character down, or one goes limp |
| Breakable joints, and what a joint holds with read back for scripts | A game breaks a bridge, a chain or a door off its hinges, or reacts to a joint's strain |
| Multibody joints, rapier's reduced-coordinate ones, which keep long chains and ragdolls from stretching as impulse joints do | A chain or ragdoll of many links stretches or jitters |
| Joints in the editor's view: anchors and axes as gizmos dragged where they hold, drawn over the meshes that now hide joints' lines | Joints are made in the editor by hand rather than by agents and scene text |
| A joint's frames turned apart: a rotation in each body's space, so a fixed or prismatic joint holds bodies turned otherwise than alike and a revolute's 0 is elsewhere | A game welds or hinges bodies turned apart |
| A spherical joint's limits and motors (cone and twist), and joints' softness (rapier's spring coefficients) | A ragdoll's shoulders, or a joint that should give |
| A joint to a point in the world, without a static body to name | Scenes tire of hook entities |
| Off-mesh links: jumps, drops and ladders between a navmesh's polygons | A game's agents must cross a gap or climb |
| Areas with costs, as roads, water and hazards, and filters an agent takes them by | A game wants paths that prefer or avoid kinds of ground |
| Tiled or streamed navmeshes, baked and baked again by tile | A level is too large to bake whole, or baking it again whole as a static body moves shows as a hitch |
| Dynamic obstacles carved into the navmesh, as doors and moving crates: only static bodies bake, and agents avoid only agents | A game's agents must path round things that move |
| Crowds beyond ORCA: a corridor of polygons kept and pulled again as an agent drifts, as Detour's `dtPathCorridor`, formations, queuing at bottlenecks | Crowds jam at doorways, or pushed agents get stuck |
| 2D navigation | A 2D game needs paths |
| A navmesh part of the tool API's look, and a switch in the editor, showing the navmesh of a game whose layers don't | An agent or the editor needs to see a game's navmesh without changing its code |
| Recast's regions, contours and merged convex polygons in place of rectangles of cells | Walls across the grid's axes cut a navmesh into many strips, or paths bend visibly at rectangles' corners |
| More than one navmesh, one for each agent's size: volumes past the first are warned of (`navigation.volumes`) | A game has agents of different sizes |
| Partial paths, to the nearest point reachable, where no path joins | A game wants agents to get as near as they can |
| A spatial grid for agents' neighbors in place of every pair compared | A game has some hundreds of agents |
| Agents turning to face where they go, which game code does | Games keep writing the same facing code |
| A restitution threshold, the speed under which a contact doesn't bounce, as Box2D's and Hazel's: rapier has none | rapier gains one, or a game's bouncy bodies won't settle |
| Telling the colliders physics doesn't simulate from those it does: one without a `RigidBody2D` or a `RigidBody3D`, or under a parent, outlines as the rest do | Someone wonders why an outlined collider doesn't collide |
| Gravity from a scene or a setting: `Physics2D::gravity` and `Physics3D::gravity` are code's, Earth's by default | A game sets its own |
| Physics steps that allocate nothing: rapier allocates as contacts come and go, and the `ticks` test steps no physics | A game's ticks must allocate nothing with physics |
| Physics' state in the world's hash, `--dump` and `World::difference`, which read entities alone, as they read no resource: a step that goes another way inside rapier shows in the bodies' transforms a tick later | A replay goes another way inside physics unseen for long |
| Replaying a log into another world, which maps values' handles to IDs, as a recording's commands are; `set_field` and operations store a dead handle without complaint | Edits sent to another world, as a server's or a peer's |
| A delete saying what refers to its entities from outside, or refusing; those references dangle until it's undone, and the world doesn't save as a scene in between, though snapshots keep them dead | Someone loses a scene's save to a reference a delete left dangling |
| Undoing a create whose entity has children since that the create didn't make, such as game code gave it, by keeping them where they are; it's refused (`ecs.has_children`), and the log drops it | An agent attaches children to what another's create made |
| Reparenting from the panels: dragging a row onto another, and taking a `Parent` away, as Remove Component would, making its entity a root; `Parent` is read only, so the Inspector offers neither | Someone reparents in the editor's panels |
| Picking several entities, to edit or delete them at once | Someone edits many entities alike |
| A search field in Add Component's menu, which lists every type registered | A game registers more types than the menu shows at once |
| A Delete key for the entity picked: keys reach egui even while the view has them, so it would delete as the world plays, its keys driving it, as Ctrl+Z undoes and Ctrl+D duplicates then; the Entity menu and a row's context menu delete | Keys go to the panel under the pointer, or the view takes them from egui |
| A bound on the log, which keeps a session's entries, a delete's components and slots among them, until it's cleared | A session's memory shows it |
| A log that outlives a run or a restore: inverses that keep components typed can't be saved, and a restored world refuses every inverse from before, the slots kept when the snapshot was taken staying unused, and freed in a run restored from a file; a snapshot could keep the log's place, which a restore would rewind it to | Someone misses undo across a restart |
| Snapshots that copy less, only the tables changed, or on several threads: the swarm's 100,450 entities copy about 20 MB each way, in about 0.5 ms taken again or restored | A game's rollback frames need the time |
| An app's own tick rate; the simulation runs 64 ticks a second | A game needs another rate |
| Budgeting a frame's ticks against all of its time: a frame stops its ticks only once they take longer than the time they simulate, so ticks that take nearly their 15.625 ms keep real time at the frame rate's cost, as the debug sandbox's 100,000 drifters do, about 8 a frame at 8 fps; counting the rest of the frame too would slow down the simulation of any game drawn below 64 fps | A game's frame rate falls so |
| Hashing a world for less: incrementally as its components change, on several threads, or with a cheaper step for each word and one mix for each term. `World::hash` reads every component, mixing each word with SplitMix64's mix: 5.5 ms for the swarm's 100,450 entities in release, three times their tick; a recorded run hashes every tick, which takes the release swarm's headless frames from a p50 of 8.7 ms to 16.3 ms, its updates from 14.0 ms to 22.6 ms on average | Recorded runs of large worlds need the time |
| The job system in gg-core, where the renderer can reach it; it's gg-engine's | gg-renderer needs jobs |
| Naming the job system's threads and the render thread for Tracy on macOS, whose client reads no thread's name from the OS there, so they show as numbers: a call into gg-core, which keeps Tracy's client, from rayon-core's start handler and the render thread's start. On Windows and Linux they show as `gg-job-N` and `gg-render` | Someone profiles on macOS |
| Checking that every job thread's floating-point environment, its rounding and flush-to-zero, is the main thread's, which a driver could change on the main thread alone | A replay goes another way on one machine on more threads than one |
| The sign of a zero that f32's `min`, `max` and `clamp` give, which std leaves to the platform, and its `algebraic_*` methods, which clippy doesn't refuse; the simulation calls none of them | The simulation first calls one |
| The ban on std's float methods for games in their own repositories, which the workspace's clippy.toml doesn't reach, though glam's features do, since Cargo merges them; and `gg new` for such a game, which makes games in the workspace's `games/` alone: its own workspace, the engine a pinned dependency, copies of clippy.toml and the toolchain's pin, and gg and dx installed | The first game outside the workspace |
| The editor's hub making a project outside the engine's workspace, or from another template than `gg new`'s: its New Project runs `cargo gg new` in the workspace, and is off outside one; its Open opens a project anywhere | The first game outside the workspace, or a second kind of game to start from |
| Locating a divergence of the layout alone, to the table, row, children or stream of IDs where two worlds part; `World::difference` compares their content, a replay names the part of the hash, and a lockstep joiner and a SyncTest say the layouts alone part (`net.desync_field`, `sync_test.diverged`) | A replay's, a desync's or a SyncTest's layout goes another way first |
| Recording a run restored from a snapshot, which `--restore` refuses with `--record`, a recording's replay starting from a seed and the layers' attach, so that `gg run` doesn't restart a run that records; and recording a rollback peer, which `--rollback` refuses with `--record`, each tick written once it has run on both peers' inputs, from their text and the hash kept, since one predicted right runs no more | A crash `gg run` restarted from, or a rollback session's run, is worth replaying |
| Layers' own state across a restart from a snapshot: the camera controllers' viewpoints, the clear color and the tool API's look, which start as a fresh run's do, the editor in a project setting its parts of the look again as the restarted run serves; the world is as it was, and a layer takes back from it what its panels show, as the sandbox's `restored` does, and a run comes back stopped, or playing a copy with the world as edited held, as it was | Someone misses where they were looking after a restart |
| What's opaque in a component of a type that isn't opaque as a whole, which a snapshot file leaves out, its component coming back on its type's base: `Component::save_opaque` and `load_opaque` carry types opaque as a whole, as `MeshDraw` | A game's component holds an opaque field |
| Snapshots that cost the main thread less than a copy of the world, two while a copy plays, which each takes as a frame starts before a thread of the API's writes it: some 6 MB of text for the debug sandbox's 10,461 entities, taken every 5 seconds under `gg run` | A game's snapshots show as hitches |
| Removing the snapshot file of a `gg run` that's stopped, or ends in an error, which stays in `snapshots` beside the executable; one whose game exits cleanly removes it | They pile up enough to matter |
| Starting again a game that waits for a rebuild as its scenes or assets change too: only a rebuild starts it again, as a change to its sources brings one, or without patches, its executable changing | A game crashes for its data rather than its code |
| Recordings that keep what their attach read, such as the sandbox's grid scene, and the commit they were built from; a file changed since shows as a divergence at the attach, and a recording names its engine's version and build alone | Recordings replayed far from where they were made, such as bug reports from people's runs |
| Recording every run, as a flight recorder; a run records only with `--record` | Bug reports from people's runs |
| Patching an edit to a constant, which rebuilds the game: `gg run` patches edits inside function bodies alone, though one to a constant that no type, static or `const fn` reads would patch as well | Agents tune constants as the game runs, and the rebuilds' restarts slow them |
| Removing the patches' libraries a `gg run` that's killed leaves; a run removes those its game loaded as it ends | They pile up enough to matter |
| Lockstep and rollback sessions beyond two peers on a LAN, as a server takes any number of clients: more players, spectators, a delay and a window of ticks predicted that follow the link's latency rather than [`DELAY`](../crates/gg-engine/src/net.rs)'s 62.5 ms, or rollback's 31.25 ms and 8 ticks; and play across the internet, through NATs, a server's too | A peer-to-peer game for more than two, or one played across the internet |
| Rollback peers kept alike ahead of each other's inputs, the one ahead slowing a little, as GGPO's peers do: a peer ahead, as a host is of a joiner by the time its world took to come and the link's latency, predicts as far as `PREDICTED` and waits there, its corrections the other's moves late, while the other predicts little | A rollback session's corrections show on one peer |
| Smoothing what a rollback, or a client's correction, corrects, which the frames draw where the ticks run again left it, at once: a client's own player seldom moves, as it predicts it exactly, and the other players draw between the server's states, but entities no player owns, as the demo's shards, jump where another's player moved them | Players find corrections jarring |
| A server's session under `gg run`, and the tool API's edits, scene files' and scripts' saves and patches there: a server serves no tool API and watches no files, and a client refuses the API's edits (`net.no_edit`), as the server's next state would take them away, and applies a patch to itself alone | Agents build a game played through a server |
| Recording a server, which `--serve` refuses with `--record`, though its ticks run once each, on its clients' inputs as they applied | A server session is worth replaying |
| A server that checks that a client's commands name the client's own player: it applies whatever commands a client sends, and a game's commands name their player, so a client can steer another's | A game is played among people who'd cheat |
| Naming the field where a client's world, read from a state's text, differs from the server's: `net.desync` names the tick and both hashes, as the client has the server's text alone; a server could read its own text back as it writes it, where a client says so | A client's world reads otherwise than the server's |
| `gg run --peers 2`'s two windows opening apart: the OS puts them one over the other | Someone plays both peers on one screen |
| Recording a joiner, whose world comes from the host's, which `--join` refuses with `--record`; the host's recording replays the session | A desync is worth replaying from the joiner's side |
| Rebuilding or restarting a running game as its project's manifest changes, which `gg run` doesn't watch | Someone edits a manifest while its game runs |
| The sandbox as a project, which names its root itself | A tool opens it as a project |
| The project in `hello`'s answer: tools find a project's games by their session files | A client reaches a game without reading its session file, as one on another machine would |
| An app's name and `gg run`'s defaults for it, such as `--peers 2`, in the project's manifest | A tool needs them |
| Sharing the game's view without copies on the CPU: the game copies each frame from the GPU's readback into the shared memory, and the editor out of it into the frame's upload arena, about 0.35 ms each at 1920x1080 on the RTX 4090; Vulkan can take the shared memory as the readback buffer itself (`VK_EXT_external_memory_host`), or share the image (external memory), where the driver has either | A view larger than 1080p, or a refresh rate, the copies don't keep up with |
| Gamepads in an embedded game: windowed runs poll the pads, and the editor forwards keys, the mouse and focus | Someone plays a project's game in the editor with a pad |
| Captures of a view that shows pixels from elsewhere, as the editor's does a game's: `capture` fails there (`api.no_view`); the game's capture has the same pixels but the editor's handles and overlay | A tool captures the editor's view |
| The editor's camera taking a perspective viewpoint of another field of view than its 60°, as a game's camera or an agent's look may give: it takes where the viewpoint sees from, and keeps its own field, so the view's widens or narrows as it turns to the editor's camera | A game's camera or an agent sees through another field of view |
| Handles, and the keys Q, W, E and R, through the game's camera while it's stopped, as through the editor's: through the game's camera, no handles show, and the keys pick no tool | Someone edits a game through its own camera, as a 2D game's fixed one |
| An embedded game's `Context::scale`, which is 1, so that what it draws in points, as its colliders' outlines, draws thinner in an editor on a display of more pixels to a point; the outline the editor's look sets is in pixels | Someone misses them on such a display |
| `gg run --embedded` with `--peers 2`, which the editor's one view would show one of: it's refused | The editor shows a lockstep session's peers |
| Filtering the Logs panel by level or text, and its game's debug events kept out of it: it shows every event the game logs, which it keeps the last 10,000 of, each one's fields in their names' order rather than as logged, and all `gg run` prints | Logs gets too long to read |
| The editor's panels reading only what they show of a game's world: a remote Inspector reads the game's whole list of entities, their names, parents, children and components' types, every quarter of a second, which costs the game's main thread a few steps an entity, and keeps the list in memory; `entities` by parent, for the rows a panel shows, would bound both by its height | A project's game of tens of thousands of entities |
| A remote Inspector's set of an option or an enum whole, as the panel sets what's inside one, holding an entity despawned since it was read, which the game refuses (`ecs.no_entity`); and a component holding a texture without a path, which it can't read, saying why: every texture a layer adds comes from a file so far | Layers make textures of their own, or someone edits such an option or enum |
| `logs` reading at most so many events, and `gg logs` and the editor's Logs saying how many the game dropped before they were read: a read gives up to 8 MiB of them, and neither says what `missed` does, which `gg mcp` gives | An agent follows a busy game's logs through MCP, or misses events unseen |
| The last of a run's events through the API: `Exited cleanly` is logged once the API has stopped, so its file alone has it | A tool needs to know through the API how a run ended |

## Open questions

- Which phase comes after Phase 18, world streaming or the built-in coding agent? Recommendation:
  the built-in coding agent, v1's headline, in the editor, modeled on the pi coding agent: it edits
  a project's files and works on the running game through the tool API, as Claude Code does now;
  world streaming once a game's world outgrows a scene, since how its cells stay in step across
  peers and a server is a design of its own. Decide before Phase 18 ends.
