# Plan

The roadmap. [AGENTS.md](../AGENTS.md) says how to keep it, under Workflow; the design is in
[ARCHITECTURE.md](../ARCHITECTURE.md).

## Done

Each phase's milestones, and what each deferred, are in git history.

- **Phases 0 to 2**: workspace, logging, Vulkan bring-up, events, layers, UI overlay (2026-10-20)
- **Docking, drawn title bars and 2D rendering**, between Phases 2 and 3 (2026-10-22)
- **Phase 3, runtime core**: ECS, scenes, undo, snapshots, determinism, replays, input (2026-10-24)
- **Phase 4, first mesh**: render thread, depth, uploads, glTF, shaders, RenderDoc (2026-10-26)
- **Phase 5, tool API and hot reload: v0.1**: the editor, `gg`, patches, Shards (2026-10-30)
- **Phase 6, projects and the editor on the tool API** (2026-10-31)
- **Phase 7, assets**: IDs beside files, renames, reloads, models in scenes (2026-11-01)
- **Phase 8, text and sound**: fonts, a game's UI, sounds, music (2026-11-01)
- **Phase 9, scripting**: Luau scripts, their debugger, mods, the hub, the showcase (2026-11-03)
- **Phase 10, cooking and the player**: its milestones (2026-11-03); its Mac check is under Open
- **Phase 11, networking beyond lockstep**: desyncs named, SyncTest, rollback, a server (2026-11-04)
- **Phase 12, render graph and PBR**: HDR, lights, IBL, cascaded shadows, Sponza (2026-11-04)
- **Phase 13, GPU-driven rendering**: culling, meshlets, a visibility buffer, TAA (2026-11-06)
- **Phase 14, 3D gameplay**: physics, characters, skeletal animation, Foxball (2026-11-08)
- **Phase 15, particles and linked prefabs** (2026-11-08)
- **Phase 16, gameplay systems**: state machines, joints, a game's UI, navigation (2026-11-08)
- **Phase 17, rendering looks**: bloom, grading, AO, height fog, a sky from its sun (2026-11-09)
- **Phase 18, the frame's finish and a game's widgets**: SSR, exposure, SMAA, DoF (2026-11-09)
- **Debug views** (2026-11-10) and **Ready for games** (2026-11-12), between Phases 18 and 19
- **Phase 19, world streaming**: cells round viewers, f64 positions, a world to cross (2026-11-13)
- **Phase 20, play across the internet**: codes, a relay, states by change, slow links (2026-11-13)
- **Phase 21, a game shipped to players**: sessions from a game's menu and across OSes, a relay
  for players, a shipped folder's name, icon, notices and symbols, round trips logged (2026-11-14);
  its match is under Open
- **Phase 22, games of their own, and a sweep**: the engine reads no game's folder, `gg new`
  outside the workspace, Foxball deleted, the docs checked against the code (2026-11-15)
- **A polish sweep**: a README, leaner docs and comments, crates untangled, a prelude and `gg new`'s
  game, faster frames and tests, cheaper CI (2026-11-17)
- **A reference path tracer**: ray queries, BLASes of packed vertices, specular light scattered as
  often as its microsurface does, `path_traced` shown and captured through the tool API, the
  raster's light held to it and its multiple scattering fitted to the walk, its tests on lavapipe
  (2026-11-18)

## Open

Phase 21 closes once two people on other networks, one on a phone's hotspot, play a match of a
game's shipped builds through the rented relay, its numbers read from both ends' logs and the
relay's, and what they call for fixed. Which game is an open question.

Phase 10 closes once the sandbox is checked by hand on Linux: gamepads, `Context::pick_file`'s
dialogs, the drawn title bars, a torn-off window moved by its tab bar (X11 moves windows
asynchronously), swapchains rebuilt as windows resize, and on Wayland whether the frame in flight
after a maximize presents at its old size.

What grows and what is frozen meanwhile is in [ARCHITECTURE.md's Goals](../ARCHITECTURE.md#goals).

## Deferred

Defects are in [DEFECTS.md](DEFECTS.md).

### Agents and tools

| Deferred | Add when |
|---|---|
| A coding agent built into the editor: the agent is Claude Code, through the user's subscription | One built in serves the user better |
| Results as data: `gg build` and `gg test`, codes on dependencies' warnings, an error's entity and scene line as fields | An agent needs them unparsed |
| MCP resources, prompts, progress and roots; tools for `hello`, `view`, `patch`, `note`, `quit`; `--grant` listing granted tools alone | An agent needs one |
| Tokens for clients without the session file, grants narrower or kept to one scene, the project in `hello` | A client on another machine, or kept to less |
| RenderDoc captures through the tool API, several a run, after streaming, with synchronization validation | An agent needs a frame's GPU work |
| Captures on macOS, and of the editor's view (`api.no_view`) | Someone captures there |
| More through the API: each pass's GPU time, shadow draw counts, exposure, streaming, sounds playing, mods loaded, a run's last event | An agent needs it |
| Debug views of lights per cluster, shadow cascades, TAA's history and the navmesh; `gg probe` at its sources' precision | They need debugging by sight |
| Through the API: game actions pressed and listed by name, UI buttons pressed by name, `gg mv` with the ID file | An agent plays through it, or loses IDs |
| Steps while minimized (`api.paused`); requests, and the compositor, answered while a dialog or a script's stop holds the main thread | A tool needs them |
| `gg fetch` of archives, and a scene's load of an unfetched file saying `gg fetch` fetches it | A project fetches an archive, or someone is lost |
| Cleaning up old JSON logs and a killed run's patch libraries | They pile up |
| `gg run` patching edits to constants, and restarting on crashes from data and on manifest edits | Agents tune constants live, or data crashes a game |
| `gg run --peers 2` windows apart, through a relay or embedded; an app's name and run defaults in a manifest, the sandbox's too | Someone needs them |
| `gg input`: a hold of N ticks in one call, the cursor in canvas units, clicks that leave the OS cursor ungrabbed; `cursor_grabbed` in `status` | An agent plays a game through it |

### Shipping and CI

| Deferred | Add when |
|---|---|
| macOS notarizing; a bundle's identifier and name other than its package's | A game ships to players on macOS, or under another name |
| A Linux desktop entry, without which Wayland shows no window icon; minidumps of native crashes, which log nothing | A game ships with an installer, or crashes outside Rust's panics |
| `gg ship` of a game without code, as the reference player under its name | A game has no code of its own |
| Faster development builds: the engine as a dylib, every dependency optimized (.cargo/config.toml names those that are) | Rebuilds or debug physics get slow |
| Debug events compiled out of shipping builds, which keep them for agents (`release_max_level_debug`) | They cost a shipped game |
| CI: GPU tests on Windows and macOS, windowed frames' allocations, captures' names, time gates | A runner with a GPU, or a break CI misses |
| ship.yml's caches; micro-benchmarks | ship.yml runs often, or a hot path gets a target |
| Faster CI builds: rust-lld on Windows (−9 to −16 s a job), gg-engine's four one-test binaries as one (−2 to −6 s), slangc processes in parallel (up to −13 s on Linux) | CI's minutes or waits matter more |
| Tracy: thread names on macOS, egui's own scopes (`profiling` 1.0.18 takes tracy-client 0.18) | Someone profiles on macOS, or `profiling` moves to 0.19 |
| A clean exit on Ctrl+C in windowed runs; layers pushed and popped mid-run; a layer reading and drawing in one loop | A console ends runs, or a tool needs it |
| Hand checks: Windows' frame on Windows 10 and across monitors of mixed scale; the cursor's grab on each OS | Someone is at such a machine |
| Devices without BC (ASTC) or D32_SFLOAT depth, and compute where the one spare compute family takes uploads | A target device is such |
| Host image copy, and meshes written straight to GPU memory (ReBAR, unified memory) | Either measures faster |
| Android: `android_main` in a `cdylib`, a small crate of its own, the one exception to a thin `main` | The engine targets Android |
| Crates split from gg-engine: `gg-assets` (cooked formats the cooker shares), `gg-scene`, `gg-physics`, `gg-audio` | The cooker builds without the engine, or a part gets a second user |

### Rendering

| Deferred | Add when |
|---|---|
| The graph ordering passes by use and dropping unused ones; image passes on the compute queue; one submit a queue | Games add passes, or a profile shows them |
| `*IndirectCount` draws, one a pipeline, none recorded a run; visibility kept per slot and meshlet across frames | The render thread outlasts the GPU |
| Culling shadow casters by what's hidden, spots by their cones, blended draws, the canvas, layers' own meshes | A profile shows them |
| Less work a frame: lights that changed, rows that left tables, culling in place, IDs only when picked, a one-pass depth pyramid | A profile shows it |
| The render scale in the title bar's settings, the tool API's look and Luau; sharpening after its upscale | A player or tool sets it |
| Visibility buffer for dev runs' models, skinned meshes, games' opaque shaders (and in the pyramid); shading by material | A profile shows the cost |
| Several views (split screen, the editor's beside the game's), a camera's own aspect, glTF's `zfar` | A game or the editor draws several |
| Picks: the world point under the cursor, more than 64 a frame, answers a frame sooner | Placing at the cursor, many touches, or hover latency |
| Depth modes: draws that ignore it, skyboxes that test it, decals, alpha-cut sprites that write it | Each with its first user |
| Lines with joins, circles by screen size; outlines of hidden parts, of hovers, of several entities, by jump flooding | Each with its first user |
| Games' shaders: the view's size without parameters, GPU-table parameters past 64 bytes, data textures, wave ops, a fog module | A shader needs one |
| `MeshDraw`'s mesh and shader by name: saved, hashed, set and restored as `Sprite.texture` is | A game builds its draws from data |
| Hot reload of the engine's own shaders | Work on them wants it |
| `VK_KHR_pipeline_binary` (ash 0.38 lacks it); the pipeline cache saved as it fills, and pruned | ash gains it, crash restarts slow, or it grows |
| Requiring Slang 2026.11 (SDK 1.4.363), dropping `decorate_non_uniform`; dev builds run away from their build's slangc | When required, or builds move |
| Layers' textures: mips, anisotropy, spaced sheets, one cell tiled, made from pixels, resized live; an engine tile map | A game's 2D art needs it |
| glTF that `add_model` refuses or ignores: morph targets, over 4 joints, cameras, sparse accessors, transmission, Draco | Each with its first model |
| Materials: back-face culling for games' meshes, emission in physical units | A game needs one |
| SMAA's other presets, T2x, S2x and 4x, edges by depth or color; FXAA; a run's graphics options beyond antialiasing | A game needs them |
| HDR displays (HDR10, scRGB); exposure metered by center or spot, local, adapting while paused, on the GPU without lag | A game's light needs it |
| Bloom's radius and threshold, lens dirt and flares, `.cube` LUTs, curves, lens distortion, grain, moving dither | A look needs them |
| Depth of field: bokeh shapes, autofocus, a camera's sensor and focal length, faster gathers for 4K | A look or its cost needs it |
| Motion blur: inner edges and tile neighbors as later filters do, a shutter in time, objects kept sharp | Edges look wrong, or high refresh rates |
| Shadows from point and spot lights and several suns, blended cascades, size and count settings, soft and contact shadows | A game needs them |
| A model's lights left out where a scene places it: the 16 Bistros shine 16 suns | A scene places a sun-holding model more than once |
| Skies: clouds, night, space, sunlight dimmed at each point, haze on unlit draws; image skies turned, mipped, prefiltered over frames | A sky needs it |
| Volumetric fog, lit and shadowed in froxels; local fog volumes; fog that drifts | A game needs shafts or mist |
| AO: bent normals, normal maps' horizons, ortho views, quality settings, a visibility bitmask, ray traced | A look, projection or device needs it |
| Reflections: probes, rays sampled by roughness, cheaper traces, ortho views, TAA's history reused | Interiors, rough glossy floors, slow devices |
| What blends sorted back to front by view depth (particles too, now by age), and taken by depth of field, AO, reflections, shadows | A game blends in 3D |
| Particles: soft, lit, colliding, forced, ribbons, curves over life, drawn by layers or on the canvas, playing while stopped | A game's effect needs it |
| Particle pools that shrink, and bursts faster than a lifetime | A game's peak or burst rate needs it |
| Upscalers behind the render scale: DLSS, FSR, XeSS, or TAA resolving into the view's size | A game's frame needs one |
| Ray-traced GI and reflections over the path tracer's scene, where the GPU has ray queries (no macOS driver does) | A game's look needs them |

### Text, a game's UI and audio

| Deferred | Add when |
|---|---|
| Text: glyphs past Windows-1252, ligatures, right to left, fallback, wrapping, outlines, variable and color fonts | A game's language or look needs it |
| Text facing the camera; multiline Inspector text; fonts' fields kept between dev runs (50 ms or more each) | A 3D game labels, or starts slow |
| UI text fields: words, selection, several lines, input methods; a key a field took kept from the game's own layer and `Input` | A game takes chat |
| UI layout: scrolling drop-downs and lists, non-rectangular clips, other canvas scalings, stretched layer draws, circles and meshes | A UI needs it |
| UI authoring: tweens on nodes, bindings to values, shared styles, canonical writing, UI files in the editor | Menus move or show many values |
| UI focus: repeats while a pad holds, kept in a panel, wrapping; sounds named in the file | Long menus, dialogs, or menus alike |
| Sounds: their distances, spatialization, Doppler, occlusion; volume, pitch and stop as they play; over 128 at once; pausing minimized | A game needs it |
| Music: loop points, layers, resuming; buses with effects; volumes kept per user; a real-time audio thread; decoding whole to check it | A game mixes audio |

### Input

| Deferred | Add when |
|---|---|
| Bindings: per user, rebound in a UI, chords, modifiers, dead zones, sticks as buttons, labels for prompts | A game ships settings or shows bindings |
| Gamepads polled in `Input`, connect events and names, one per player, rumble, in an embedded game | Local multiplayer, rumble, or a pad in the editor |
| An engine type for an action's state, holds shorter than a tick, fewer stick commands, a raw-mouse fly camera, zoom by keys | A game needs one |
| The cursor as a binding; a first-person look and walk of the engine's | Two games write their own |

### Assets and cooking

| Deferred | Add when |
|---|---|
| Loading off the main thread: assets' files, sounds' decoding, music's reads, meshes' vertices; models over 8 GiB | A game's reads hitch |
| Streaming small mips first, a game's own upload budget (16 MiB a frame), a staging ring, copies started before submit | Pop-in shows, or a profile |
| Removing shaders and fonts, and any asset through the tool API | A game swaps them as it plays, or an agent frees one |
| Model and prefab edits taken live in recorded and lockstep runs and across restarts; nodes matched by name; retargeted images | Such edits are made |
| Renames followed for files outside the asset root, and for sounds and music | Someone moves them while their game runs |
| IDs for shaders, scenes, sounds and music, for assets outside the root and through links | Each kind as something names it by ID |
| A warning of a path whose case differs from its file's, and a check that a package holds every file code loads | A file fails from its package alone |
| Loading assets that scene text names in a tick, for a lockstep joiner's API, and at a server's scene change (`net.no_scene_change`) | A game loads so |
| Levels of detail: in dev runs, hysteresis, simplified by attributes, small parts pruned, shared borders locked | A pop, color shift or gap shows |
| Packages: meshes compressed, compression chosen per texture, one image compressed on several threads, bytes alike on every OS | Size, cook time, art or build caches need it |
| Import settings in assets' ID files: a part a `.meta` file may add, which older files take the default of | An asset needs settings of its own |

### ECS, scenes and simulation

| Deferred | Add when |
|---|---|
| Changes queued during queries (Bevy's `Commands`, `Deferred` here), `par_for_each`'s too, applied in row order | A system spawns as it iterates |
| Query filters (`With`, `Without`, `Option`, `Changed`), an `EntityId` term, ordered reductions, generic derives | Each with its first user |
| ECS speed: systems in parallel, scripts on jobs, cached query matches, sparse storage, resolved paths, children spawned in place | A profile shows it |
| Reflection: aliases for renames, undo across reloads that changed fields, field flags, opaque values inside lists and fields | A rename must keep values |
| Scene files: a version, comments, names past ASCII, saved child order, partial loads, entities kept across levels | A format change, or a game needs it |
| IDs traced to the session that drew them, a load refusing IDs no session drew | The API should refuse made-up IDs |
| Prefabs: variants, removals per instance, overrides written back or finer than a field, nested links swapped, model nodes' overrides | A game needs it |
| Save as Prefab from a game's own Inspector, and elsewhere; overrides marked inside fields; spawns without reparsing | Prefabs made or spawned in play |
| Undo across restarts, and of a create whose entity has gained children | Someone misses it |
| An app's own tick rate (64 now), and a frame's ticks budgeted against the whole frame | A game needs another rate, or runs below 64 fps |
| Interpolation: draw poses for layers and handles, turned off per entity, Hermite curves, gliding sets, headless sub-tick frames | Cameras follow entities |
| Cheaper hashes and snapshots: incremental or threaded hashing, copying only changed tables, interpolation's copies on jobs | Large worlds record or network |
| Determinism checks: job threads' FP environment, zero signs of f32 `min` and `max`, physics in the hash, layout divergences located | A replay parts unseen |
| Recordings of restored runs, rollback peers, servers and joiners, and of every run as a flight recorder | Such a run is worth replaying |
| Recordings that travel: holding what attach read, scripts, the commit, and stand-ins that keep the hash | A bug report replays elsewhere |
| Layers' own state (cameras, the look) kept across a restart from a snapshot | Someone misses where they looked |
| Frames allocating nothing with egui, sounds, a pad, keys on Windows, asset watchers, physics, or job threads' first ticks | Frames must allocate nothing |
| Counting what the Vulkan loader, drivers, validation and VMA allocate (`VkAllocationCallbacks`) | A profile shows them allocating each frame |
| The job system in gg-core, where the renderer can reach it | gg-renderer needs jobs |

### Scripts and mods

| Deferred | Add when |
|---|---|
| Scripts: `has` a component, find by `Name`, strings, input, random numbers from the world, a machine's state; create and destroy hooks | A script needs it |
| Calls into a script with arguments, or events scripts hook | A game hands a script what happened, as the card demo encodes plays in a field |
| A tick's budget for all of a game's script calls, or one by time in development builds | A runaway script's crawl gets in the way |
| luau-lsp type definitions, and debugging over DAP (`gg dap`) | Someone works on scripts in a code editor |
| Debugger: watches, expressions, conditions, logpoints, stops on errors, chunk breakpoints, stepping from a pause, keys | Someone debugs at length |
| Mods: a menu to turn on and approve them, assets of their own, dependencies, settings, migrations, live reloads and debugging | Players install mods |

### Physics, animation and navigation

| Deferred | Add when |
|---|---|
| Physics: polygons, hulls, damping, a set mass, bodies under parents, children's colliders, rotation locked per axis, contacts' normals and points, collision layers by name | Each with its first game use |
| Characters of other shapes, turning; casts giving every hit, overlaps, filters, 2D casts | Each with its first game use |
| Joints: ragdolls, breaking, multibody, turned frames, spherical limits and motors, to world points, gizmos in the editor | A game breaks, hinges or ragdolls |
| A restitution threshold, gravity from a scene, unsimulated colliders told apart, mesh colliders' outlines | rapier gains one, or someone is confused |
| Animation: root motion, layers and masks, events, IK, 2D blend spaces, shared machine assets, sub-machines, shared bind poses | Movement needs it |
| Flipbooks of spaced sheets, with clips and events; tweens of other entities and deeper fields, in sequences, with end events | Each with its first game use |
| Navigation: off-mesh links, area costs, tiled navmeshes, moving obstacles, crowds past ORCA, 2D | A game's agents need it |
| Recast's regions, a navmesh per agent size, partial paths, a neighbor grid, agents facing where they go | Paths bend, sizes differ, or hundreds of agents |

### Networking and worlds

| Deferred | Add when |
|---|---|
| Lockstep and rollback past two peers, spectators, and smoothed corrections | More players, or corrections jar |
| A server under `gg run` with the API's edits, file watching and patches; a client's `net.desync` naming the field | Agents build a served game |
| Short codes stretched by a PAKE; TURN beside the engine's relay | Players read codes out, or a game is hosted where TURN runs |
| Sessions measured over Docker's NATs with `--link`, packets kept on the relay: so far on loopback and one LAN, packets direct | Phase 21's sessions on other networks keep their packets on the relay |
| `--link` holding the handshake; states compressed by zstd's patch-from (2.7 times quicker, 10% larger) | A test needs it, or the states thread falls behind |
| Worlds: proxies for far cells (HLOD), kept states spilled to disk, terrain and heightfields | A world's horizon, memory or ground needs it |
| QUIC links in place of TCP and UDP, for congestion control and migration | A game needs congestion control, or links that survive an address change |

### Editor and demos

| Deferred | Add when |
|---|---|
| Lights and cameras drawn in the editor's view, a camera piloted from it, handles through the game's camera | Someone frames or places by eye |
| The view in a window of its own, key focus shown round it outside Play, a game's HUD hidden while editing | Someone misses it |
| The editor's camera: eased projection switches, a view lock, an Orbit cursor, kept settings, the grid to the horizon | Someone misses one |
| Handles: snap steps of one's own, drag readouts, trackball turns, picking through overlaps | Someone misses one |
| Dock: splits and tab drags in torn-off windows, minimum widths, top panels hidden, Alt keys, macOS's menu bar, Wayland, deferred viewports | A tool needs it |
| Inspector: list elements, drag speeds, picked entity fields, multi-edit, component search, script hints; Entities' icons and instance marks | Edits need it |
| Scenes from the editor: opened from Assets, renamed, deleted; Play's edits kept; Play and Stop recorded or in lockstep | Someone works on scenes there |
| More drops from Assets: textures onto space or rows, fonts as text, from a torn-off panel | Someone lays out 2D so |
| The embedded game: its view shared without CPU copies (0.35 ms each way at 1080p), Logs filtered, `logs` saying what dropped | Past 1080p, or busy logs |
| The hub making a project from another template than `gg new`'s | A second kind of game to start from |
| A pause a game alone can ask for, as its menu opens; asking the other player before a session's New match | Players lose points to a menu, or start matches over each other's |
| Shards' menu as a UI file | Shards' menu changes |

## Open questions

- Where uploads go on a device whose transfer queue copies whole images alone (granularity 0, 0,
  0): it's refused, so uploads take the compute queue's family and async compute is off.
  Recommendation: upload on the graphics queue there, keeping async compute. Decide before a game
  targets such a device.
- Whether scripts' spawns get a limit of their own, a number a tick: only each call's interrupt
  budget caps them, so a game's script, or a mod granted `write`, may spawn hundreds of thousands
  of entities a tick, which every peer simulates. Recommendation: no cap for a game's own scripts,
  as Rust has none; for mods, a few hundred a tick beside their interrupt budget, failing as it
  does. Decide before mods that spawn ship.
- Which game Phase 21's match is played on, now that the 3D demo, whose menu hosted and joined, is
  deleted: Shards by its flags (`--relay`, the host printing its code), or Shards given a Host and
  Join menu. Recommendation: Shards by flags, as the match measures the network, which a menu
  doesn't change, and `Context::host` and `join` have tests of their own. Decide before the match.
