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

## Now

**Debugging scripts**, Phase 9's third milestone: breakpoints, pause and step in a script, the game
waiting rather than hung; its plan says when it's done.

## Next

Each phase ends in something you can run and check.

9. **Scripting.** Luau, as [Game code](../ARCHITECTURE.md#game-code) describes: scripts as assets,
   attached to entities, their fields in the Inspector, reaching other entities and Rust components
   through reflection, reloaded as they're saved with the world kept, deterministic for lockstep
   and replays, and debugged with breakpoints, pause and step; and the tool API's capabilities, so
   that a mod's scripts reach only what they're granted. *Done when* a game's behavior written in
   Luau reloads as it's saved, in sync on both peers of the demo, and a mod runs sandboxed. Its
   milestones, in order: Luau scripts, components scripts declare and act as, run alike on every
   platform and reloaded as they're saved, at a tick, with the demo's shards spun by one; scripts in
   the editor and in lockstep, a reload that changes a script's fields migrating its components in
   place, the Inspector and the tool API following, and both peers reloading at the tick the host
   chooses; debugging, breakpoints, pause and step in a script, the game waiting rather than hung;
   and mods, sandboxed, reaching only what the tool API's capabilities grant them.
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
| Checking the sandbox by hand on Linux, and on macOS with KosmicKrisp (Vulkan SDK 1.4.363.0 or later, the first conformant build), its gamepads, the file dialogs `Context::pick_file` shows and the drawn title bars too, with the menu button's inset clear of macOS's traffic lights (72 points) and a torn-off window moving by its tab bar where X11 and macOS move windows asynchronously, and the render thread there: acquiring, presenting and rebuilding swapchains as windows resize off the main thread under KosmicKrisp, and on Wayland a frame in flight presenting at its old size after a maximize; and the `frames` test on macOS, whose CI runner has no GPU to run it: std's mutexes there are boxed as they're first locked, so a mutex made each frame allocates there alone | A Linux machine and an Apple Silicon Mac are at hand |
| Deleting old JSON logs; every run's log is kept | Logs pile up enough to matter |
| A clean shutdown on Ctrl+C, which now ends the process at once; the JSON log keeps every line written until then, and a recording every whole line but its `end` | A headless run that goes on until it's stopped, such as a dedicated server |
| GPU tests in Windows and macOS CI, which skip them for the reason [AGENTS.md](../AGENTS.md#commands) gives | A self-hosted runner with a GPU, or a Vulkan driver that runs on those runners without one |
| Checking windowed frames' allocations in CI, under Xvfb on Linux, with lavapipe presenting and input through XTest; they're checked by hand, as [gg-engine's AGENTS.md](../crates/gg-engine/AGENTS.md) says | A change to the windowed loop allocates where the check by hand didn't look |
| Checking captures in CI, under RenderDoc on Linux with lavapipe: captures are checked by hand, replayed through qrenderdoc's Python for their names and labels | A name or a label is lost unseen, or the tool API checks captures |
| RenderDoc captures from code or through the tool API, and more than one a run: `--capture` names one frame as the run starts, and the tool API's `capture` writes the view's pixels alone | An agent needs a frame's GPU work as well as its pixels |
| `gg build` and `gg test`, with their results as data: `run` and `check` build through cargo, passing on its messages about the code as it renders them, and cargo's test harness writes JSON on nightly alone | An agent needs a build's or the tests' results as data, or libtest's JSON output is stable |
| Capabilities each client of the tool API is granted, as [Agents and tools](../ARCHITECTURE.md#agents-and-tools) describes: a connection that says hello with the session's token reaches everything, which only processes that can read the session file beside the game, the user's own, do | A client the user doesn't run as their own, such as one on another machine or a mod's, or an agent kept to one scene |
| MCP beyond tools: resources, prompts, progress, cancellation and the client's roots; `gg mcp`'s tools are the API's methods but `hello`, `ping`, `view`, `patch`, `note` and `quit`, and the paths of `capture` and `snapshot`, a capture's PNG coming inline | An agent needs one of them |
| Steps and captures while the window is minimized, which fail with `api.paused`, and requests while a file dialog or Windows' window menu holds the main thread, which wait; `ping` says which. An `assets` while it's minimized answers from the asset database as its thread last sent it, up to a second old | A tool needs them then |
| Labels inside passes: each run of draws labeled by what it draws, and the frame pass's view and UI apart; a capture shows a label for each pass | Phase 12, with the render graph |
| Synchronization validation under RenderDoc, whose layer hides the validation layer's settings: through `VK_EXT_validation_features`, or the validation layer's environment | A synchronization bug shows under RenderDoc alone |
| Captures on macOS, where RenderDoc doesn't run, so `--capture` is refused | Someone needs a capture of a Mac's frames |
| egui, the dock and the Inspector behind `dev`: the player links them, though it runs egui for its first frame alone, as an app none of whose layers draw UI does; `Ui`, which `Layer::ui` takes, is the type to turn into a no-op | Phase 10, with the shipping profile |
| The main thread takes the renderer back once a frame's UI is drawn, so a frame's input is read about a frame before it renders: a refresh under `--vsync`; and drawing between ticks draws the world up to a tick behind the simulation. Starting each frame once the render thread has acquired the image it renders into would win the first back, and extrapolating from the latest ticks the second | A game's input latency needs it |
| Pushing and popping layers while the app runs; `App` fixes the stack before it starts | A tool or game needs a layer for only part of a run |
| Composed text from an input method (winit's `Ime` events), for languages typed through one | A game takes text input, such as chat |
| Frames that draw UI and allocate nothing: egui allocates as it lays out and tessellates, and as it collects its input, where winit reads the window's title, and so do the dock, the Inspector, and the render thread while a panel has a window of its own | A game's shipping frames draw UI, or Phase 10 puts egui behind `dev` |
| Widgets for a game's UI: buttons, lists and their focus, moved by keys, pads and the mouse; Shards draws its menu's buttons, finds what the cursor is over, and keeps their focus itself | A game other than Shards writes the same |
| The canvas scaled by the view's width or its smaller side, or to a height a game sets, rather than 1,080 units to the view's height: a view much narrower than 16:9 crowds what's anchored to its sides, and a portrait one overlaps it | A game runs in portrait, or at aspect ratios far from 16:9 |
| Stretching what's drawn on the canvas between anchors, as a backdrop filling the view: each draw is placed from one anchor, and scaled with the view's height alone | A game's UI fills the view, as a dim behind a menu |
| Circles, meshes and lines on the canvas, and clipping to a rectangle, as a list that scrolls needs: it draws sprites and text | A game's UI draws them |
| Measuring text for laying it out, as a button sized to its label needs: a layer sizes its buttons itself | A game's UI lays out text of lengths it doesn't know, such as translations |
| Hiding a game's UI from the editor's view while it edits, as Unity's Scene view leaves out a canvas over the screen: the canvas draws through the look's viewpoint too, and hides what's under it from picks | Someone edits under a game's HUD |
| Sounds placed in 3D, by their direction and distance from a listener, as kira's spatial tracks place them: a sound is panned and faded by where it shows in the view, so a camera with perspective hears nothing behind it, and what's far as loud as what's near while both are in view | A 3D game's sounds |
| Setting a play's volume and pitch, and stopping it, as a hum that follows an engine's speed would: a sound plays as its file holds it, to its end | A game varies or cuts a sound as it plays it |
| More than 128 sounds at once, as many as kira's mixer takes by default, past which a sound is left out (`audio.full`); and making way for the next by stopping the oldest or the quietest | A game plays that many at once |
| Sounds and music pausing with the simulation, as the tool API pauses or stops it, and while the window is minimized: a sound playing plays to its end, and music plays on | A game's sounds outlast a pause noticeably, or its music should stop with it |
| Leaving out, as rollback runs ticks again, the sounds they queued and played the first time: each could be tagged with its tick as it's queued | Rollback, in Phase 11 |
| Frames that play sounds and allocate nothing, on any thread: kira allocates on the main thread as each sound starts, its state and its commands' buffers, and its thread that follows the system's default output device formats the device's ID every half second, on Windows and Linux; and each play of music decodes on a thread kira starts for it, which allocates each packet's frames, as symphonia allocates each packet. The `frames` test's app adds no sounds or music, so it opens no output | A game's frames must allocate nothing as its sounds and music play |
| A real-time priority for the audio thread, through cpal's `realtime` feature, which on Linux asks for it through D-Bus, against dropouts while the machine is busy | Sounds drop out under load |
| The tool API listing a game's sounds and music, what's playing and the buses' volumes, and playing one, for agents, which hear nothing: its log's debug events say each sound and music started (`Sound started`, `Music started`) | An agent checks a game's audio beyond its log |
| Points to loop music between: it loops whole, from its start, so an intro played once before its loop, or music that plays once and ends, takes them, from its file's tags or a layer | A game's music has an intro, or ends |
| More than one music at once, as stems fading in with the action, and pieces that pick up where they were left: one plays at a time, from its start, the one before fading out | A game layers its music, or goes back to a piece where it left it |
| Buses a game adds, as voice, ambience or the UI's, each play naming its bus, and effects on buses, as a low-pass under a menu: there are the main bus, the music's and the sounds', with volumes alone | A game mixes more than its music and sounds apart, or filters one |
| The buses' volumes kept between runs, per user, as a game's settings: each starts each run at 1 | A game ships its settings, with per-user bindings |
| Music decoded through to its end as it loads, to say whether all of it decodes: its first packet decodes as it's read, and the rest only as it plays, which stops at a packet that doesn't, warning `audio.music_failed` | A game ships music that fails past its start |
| Counting what the Vulkan loader, the drivers, the validation layer and VMA allocate beneath the engine, through `VkAllocationCallbacks`, which drivers may bypass for their own: the `frames` test counts Rust's allocator | A profile shows them allocating each frame |
| Keeping within the GPU's memory budget (`VK_EXT_memory_budget` through VMA) | Phase 10, with streaming |
| Removing meshes, shaders and textures, which last until the app ends; a reload replaces them in place | Phase 10, with streaming |
| Replacing a texture without waiting for the frames before: the frame that takes a reload's image waits for the GPU to finish them, a frame's time at most, as its slot in the bindless set can't change while they may sample it; writing the image in place where its size holds would spare the wait | A reload's hitch shows, as for a texture written many times a second |
| Reloading a model's nodes and materials, and a model of other numbers of primitives or images, in a run that's recorded or in a lockstep session: its roots' nodes keep what they were spawned with until the run starts again (`asset.model_changed`), as their world changes only as the run it goes with changed it | Someone edits models during such runs, as a reload kept as a line of the recording or an input both peers apply at one tick would allow |
| A model's nodes matched by their names as its file changes, rather than by their places: each keeps what code gave it only while every node keeps its place in the model's hierarchy, and else all are spawned again, under the same IDs | A game's code gives nodes state that a model's edit loses |
| A model's nodes named, as by their glTF names: they have no `Name`, so the Entities panel and the tool API show their IDs | Someone looks for a node by name |
| A model's nodes as its file changed while its game was down, as between a crash and its restart: a world restored from a snapshot keeps them as the snapshot did, but for the draws a run of other assets left out, until the model's next change; and an undone delete of a model's root, or a redone create, brings back its nodes as they were before its model changed since. Refitting there would undo what code did to them, and lockstep peers would part, without a print of each model's nodes in the snapshot and the undo to tell a model changed since | Someone edits a model while its game is down, or between deleting its root and undoing that |
| A model standing in for a following run with its nodes, so that the hash still matches: a replay or a joiner that can't load a model stands in with one of no nodes, and its hash parts from its run's where the nodes were | A replay or a joiner runs without a game's model files, as a bug report's might |
| A sprite sheet's texture reloaded at another size, which keeps drawing as it did (`asset.sheet_resized`): its sprites hold its cells as fractions of its size, which a layer cut it by as it added it | A game's sheets grow as they're drawn, as frames added to an animation's |
| Following a texture's, a model's or a font's file outside the asset root, or a sound's or music's anywhere, none of which has an asset ID, as it's renamed or moved: it's watched where it was, missing there, and reloads no more | Someone moves such files while their game runs |
| Per-frame data in a buffer: shaders get the camera's view and projection as one matrix, and see the world through each draw's transform, but not the camera's position, the time or the lights | Phase 12, with lighting |
| Materials: parameters kept on the GPU and written when they change, rather than copied every frame with each instance; `MeshDraw`'s cap of 64 bytes of parameters, past which a shader draws through `Context::draw`, goes with it | Phase 12, with materials |
| Games' textures of data rather than color, such as normal maps, which shaders read as stored rather than decoded from sRGB, as they read fonts' fields: `Context::add_texture` adds color alone | Phase 12, with PBR's maps |
| Glyphs beyond the characters Windows-1252 prints: a font's field holds those alone, and any other character draws as the font's `.notdef`; a field that grows as text needs glyphs, or a list of characters in a font's ID file, would hold more | A game writes a language beyond Western European ones |
| Shaping's features that put glyphs in place of others, such as ligatures and contextual alternates, which text is shaped without, as its font's field may not hold what they'd substitute; and the scripts that need them, those written right to left, and lines of more than one direction | A game's font relies on them, or a game writes such a script |
| Text wrapped to a width, and fallback fonts, a character one lacks drawn from another | A game's UI lays out paragraphs, or mixes scripts in a line |
| Outlines, shadows and glows round text, from the true distance field its field holds in its alpha | A game's text needs one to read over what's behind it |
| Fonts' fields kept between runs, or cooked: each font bakes as it loads, Inter Bold's 218 glyphs in about 50 ms in release and 140 ms in a debug build, on the job system's threads, and the engine's own as text is first drawn in it, inside that frame; a font's file read again bakes on the watcher's thread alone | Phase 10, which cooks them |
| Text that faces the camera, as labels over 3D entities do: a `Text` lies in its entity's XY plane, as a sprite does | A 3D game labels its entities |
| Text of several lines edited in the Inspector, whose field for a string is a line's: a text's other lines come from scenes, code and tools | Someone writes text of several lines in the Inspector |
| Fonts other than a file's default instance of outlines: a variable font draws at its axes' defaults, unhinted, and collections (`.ttc`, `.otc`), color fonts and fonts of bitmaps alone don't load | A game needs one |
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
| A tile map in the engine, which scenes save; the sandbox's map is code of its own, its tiles sprites that snapshots and scenes save each with its texture's name | A game edits its map |
| glTF that `Context::add_model` refuses or ignores, as it lists: lighting and the material's other textures, skins and animations, morph targets, cameras, models without a scene and scenes other than the default, sparse accessors and accessors without a buffer view, primitives other than triangle lists, samplers that wrap across and down differently, minification filters, texture transforms, quantized or compressed meshes and textures (KHR_mesh_quantization, Draco, meshopt, Basis Universal), and integers written as floats, such as `1.0`, which glTF allows | Each with the first model that needs it; lighting and the other textures in Phase 12, compression with Phase 10's cooker |
| Decoding images and sounds off the main thread as they're added, and reading music; `Context::add_texture`, `add_model` and `add_sound` decode before they return, `add_music` reads its file whole, up to 64 MiB, and a texture loaded where text names it decodes before the text is read, a lockstep peer's tick waiting for it; a reload decodes on its watcher's thread | Phase 10, with the streaming loader |
| Models that take more than 4 GiB to read, of their buffers, decoded images, vertices and nodes, which `add_model` refuses with `engine.unsupported_model` | Phase 10, with streaming and the GPU's memory budget |
| Mip levels made on the CPU, on the render thread, as the frame that uploads a model's texture renders: a 2,048 by 2,048 texture's take about 0.3 s of that frame in a debug build | Phase 10, which cooks them; or blits on the GPU, if a hitch shows first |
| Layers making textures from their own pixels; `Textures::add` takes pixels, but `Context` adds only PNGs and JPEGs | A game makes its own textures |
| Keeping the frames' arenas near their frames' peak, within the GPU's memory budget: an arena grows by a buffer at least twice its last, and merges them as it resets, so it can hold a few times what its largest frame took, in memory the CPU writes, of instances' transforms, IDs and parameters and the UI's vertices, indices and texture changes | Phase 10, with the GPU's memory budget |
| A frame's copies through a transfer queue start only once it's submitted, after its passes are recorded, and the frame waits for them | A profile shows a frame waiting for its uploads |
| Staging and the frame's arena kept within the device's `maxMemoryAllocationSize` and `maxBufferSize`, each at least 1 GiB: both grow by doubling, each staging buffer an allocation of its own, so a frame that adds more than that at once fails, which ends the run | A frame adds that much at once, or Phase 10, with the GPU's memory budget |
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
| A slot between the world's opaque and blended draws for the editor's grid, so that it shows over opaque draws behind its plane: it draws after the layers' draws and before the world's, over a layer's backdrop, as Shards' floor, but under the world's opaque draws behind its plane, and what blends behind the plane would need the grid sorted among it | A world's opaque meshes behind the grid's plane hide it |
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
| Games' shaders compile with slangc as they're added, so running a game needs slangc where its build found it | Phase 10, which cooks shaders |
| `MeshDraw`'s mesh and shader named, as `Sprite.texture` is: opaque until then, shown but not set, saved in scenes or hashed, and carried across a restart by their indices only where the layers added the same assets in the same order; and setting them by name | A game builds its draws from data |
| Watching the asset root through the OS's notifications of changes: its thread scans it every second, walking it whole and reading every ID file, about 70 ms for 1,000 assets on Windows, and reads whole each asset's file that changed, for its print, every one in a clone's first run, which delays its next scans but no answer; and a run's start scans it on the main thread, before its layers load, flushing each ID file it writes to the disk, about 6 ms apiece | A project's asset root of tens of thousands of files |
| IDs for shaders, scenes, sounds, music and other kinds, for assets outside the asset root, and for assets reached through links: the scan gives textures, models and fonts under the root alone, never through a link | Each kind as something first names it by ID |
| A recording's scripts as they were as it started, which its replay reads from their files, as it reads scene files', so that a script changed since makes it go another way at its first tick | A recording travels without its game's files, as a bug report does |
| Scripts that spawn and despawn entities, or give and take their components, which Rust code alone does | A game's behavior in Luau makes or unmakes what it acts on |
| Random numbers for scripts, from a state the world holds, in place of `math.random`, which they go without | A script draws random numbers, as Shards' `collect` would |
| A budget of Luau's interrupts for a script's tick, so that one that loops for ever fails, deterministically, rather than hanging the game, which `gg run` then restarts into the same loop | Debugging, Phase 9's third milestone, or mods, whose scripts aren't the game's own |
| Builds without `dev` naming assets by their IDs: they read no ID file, so they name every texture, model and font by its path, and a scene, a recording or a snapshot that names one by its ID finds none there | Phase 10, whose cooked package gives each asset its ID |
| Loading the textures, models and fonts that scene text names where a layer loads it into a world itself, as inside a tick, as the sandbox's grid reload does: a world can't reach the renderer's textures, which are the context's, so `World::load_scene` finds only those loaded, while `Context::load_scene_file`, the tool API and the engine's other reads load them | A game loads scene text in a tick that names assets no layer adds |
| A lockstep joiner loading for its tool API an asset its host hasn't named (`asset.not_given`), whose name could differ from the host's; the host's edits load any | Someone edits a joiner's assets from its own editor |
| `gg mv` and an MCP `mv`, moving an asset and its ID file in one step: a file renamed alone keeps its ID by what it holds, but not one edited in the same step, nor one renamed in a fresh clone before its first run, which has no cache of prints | An agent's renames lose IDs that pairing by content can't keep |
| A lockstep joiner that joins after its host's texture was renamed finding its file where it is now: it loads it by the path its host loaded it from, as the host names it, standing in where that's gone, as it is on one machine | Someone renames assets during a lockstep session |
| Following a rename in case alone, as Windows' and macOS's file systems allow, of an asset whose ID file keeps the old case: the ID file matches only when case is ignored (`asset.meta_case`), so the asset keeps its ID for the run, then goes without until its ID file is renamed too | Someone renames assets in case alone |
| 8- and 16-bit vertex fields, whose features Vulkan 1.4 guarantees but the device doesn't enable | A mesh packs its vertices, as Phase 10's cooker will |
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
| Reparenting that keeps where a child is in the world, by decomposing its matrix, which loses any shear; `set_parent` keeps its own transform | Someone reparents in the editor's panels, or an agent attaches something where it stands |
| Spawning a child into its table at once from code: `spawn`, then `set_parent`, moves it to another table; a load and a create operation spawn one there | A profile |
| Children in an order that means something: reordering them, and saving their order; an undo puts a child back last | Someone orders children in the editor, or UI laid out by the hierarchy |
| Each session's seed and count saved with the world and merged with it, so that an ID maps back to its session and when it was drawn, and a load refuses IDs no session drew | The tool API refusing an ID an agent made up |
| An `EntityId` query term | The first system that reads IDs as it iterates, such as replication |
| Errors carrying their entity, and a scene's line and column, as fields, not only in their message, which the tool API gives as its code and message alone | A tool reads an error's entity or place without parsing its message |
| Drawing rows at one z in ID order for less. Every frame reads them in that order rather than their tables', about 0.9 ms of the swarm's 4.3 ms frame in release; a frame that changes rows or z's, or follows a restore, sorts them by ID again, about 2.5 ms more for 100,000; and 100,000 sprites each at a z of their own, as sorting by y gives them, take about 5 ms, for a group and an item each. Tables kept in ID order would make the reads a sweep, and a radix sort the sort quicker | A game's frames need the time |
| Linked prefab instances, with overrides, nesting, apply and revert; a placement is a flattened copy, each ID mixed from its instance's and its own, which links will keep; and overrides of a model's nodes, which scenes don't save and tools don't change, as links will keep them | A game editing a prefab its scenes hold many copies of, or a node of a model placed in a scene |
| Placing a prefab without parsing it again: `World::place_prefab` parses its text each time, which allocates | A game places prefabs as it runs, rather than as it loads |
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
| Colliders other than boxes and circles, such as capsules and polygons; joints; sensors; and contacts and queries, such as raycasts, for scripts | Each with its first game use |
| A restitution threshold, the speed under which a contact doesn't bounce, as Box2D's and Hazel's: rapier has none | rapier gains one, or a game's bouncy bodies won't settle |
| Telling the colliders physics doesn't simulate from those it does: one without a `RigidBody2D`, or under a parent, outlines as the rest do | Someone wonders why an outlined collider doesn't collide |
| Gravity from a scene or a setting: `Physics2D::gravity` is code's, Earth's by default | A game sets its own |
| Physics steps that allocate nothing: rapier allocates as contacts come and go, and the `ticks` test steps no physics | A game's ticks must allocate nothing with physics |
| Physics' state in the world's hash, `--dump` and `World::difference`, which read entities alone, as they read no resource: a step that goes another way inside rapier shows in the bodies' transforms a tick later | A replay goes another way inside physics unseen for long |
| Replaying a log into another world, which maps values' handles to IDs, as a recording's commands are; `set_field` and operations store a dead handle without complaint | Edits sent to another world, as a server's or a peer's |
| A delete saying what refers to its entities from outside, or refusing; those references dangle until it's undone, and the world doesn't save as a scene in between, though snapshots keep them dead | Someone loses a scene's save to a reference a delete left dangling |
| Undoing a create whose entity has children since that the create didn't make, such as game code gave it, by keeping them where they are; it's refused (`ecs.has_children`), and the log drops it | An agent attaches children to what another's create made |
| Layers holding what a restore inside a tick takes back, as rollback's: `Layer::restored` tells them of a world restored as a run starts from a snapshot, at a lockstep joiner's restore and at the API's `stop`, each between ticks, but nothing of one restored inside a tick; the sandbox doesn't snapshot | Something restores the world inside a tick, such as rollback |
| Reparenting from the panels: dragging a row onto another, and taking a `Parent` away, as Remove Component would, making its entity a root; `Parent` is read only, so the Inspector offers neither | Someone reparents in the editor's panels |
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
| The editor's hub making a project outside the engine's workspace, or from another template than `gg new`'s: its New Project runs `cargo gg new` in the workspace, and is off outside one; its Open opens a project anywhere | The first game outside the workspace, or a second kind of game to start from |
| Locating a divergence of the layout alone, to the table, row, children or stream of IDs where two worlds part; `World::difference` compares their content, and a replay names the part of the hash | A replay's layout goes another way first |
| Replaying from a snapshot and the commands after it, and a SyncTest, which runs each tick again from a snapshot and checks its hash, as rollback needs; and recording a run restored from a snapshot, which `--restore` refuses with `--record`, a recording's replay starting from a seed and the layers' attach, so that `gg run` doesn't restart a run that records | Rollback, or a crash `gg run` restarted from is worth replaying |
| Layers' own state across a restart from a snapshot: the camera controllers' viewpoints, the clear color and the tool API's look, which start as a fresh run's do, the editor in a project setting its parts of the look again as the restarted run serves; the world is as it was, and a layer takes back from it what its panels show, as the sandbox's `restored` does, and a run comes back stopped, or playing a copy with the world as edited held, as it was | Someone misses where they were looking after a restart |
| What's opaque in a component of a type that isn't opaque as a whole, which a snapshot file leaves out, its component coming back on its type's base: `Component::save_opaque` and `load_opaque` carry types opaque as a whole, as `MeshDraw` | A game's component holds an opaque field |
| Snapshots that cost the main thread less than a copy of the world, two while a copy plays, which each takes as a frame starts before a thread of the API's writes it: some 6 MB of text for the debug sandbox's 10,461 entities, taken every 5 seconds under `gg run` | A game's snapshots show as hitches |
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

- Which license? Recommendation: MIT OR Apache-2.0, the Rust convention, chosen once; until
  then, `publish = false` keeps the crates off crates.io. Decide before anyone outside the
  project contributes.
