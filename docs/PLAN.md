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

## Now: Phase 3, runtime core

Our own ECS with a transform hierarchy, reflected component types registered at runtime, stable
entity IDs, scenes and prefabs in canonical text, operations with inverses and their log, and
whole-world snapshots and restores. A fixed timestep, with commands as the only way into the
simulation; the deterministic mode, with a world hash per tick, recordings and replays, and math
that is identical everywhere (glam's `scalar-math` and `libm` features, or math types of our
own); input actions; a job system. All of it runs headless.

*Done when* a scene saved, loaded and saved again is byte-identical; undoing a log of operations
gives back the original world's hash; a restored snapshot matches the world it was taken from;
steady-state ticks don't allocate; and a recording replays to the same hashes on all three OSes
and both architectures, with a forced divergence reported as its first tick, entity and field.

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
| Checking the sandbox by hand on Linux, and on macOS with KosmicKrisp (Vulkan SDK 1.4.363.0 or later, the first conformant build) | Once the sandbox draws a mesh |
| Deleting old JSON logs; every run's log is kept | Logs pile up enough to matter |
| A clean shutdown on Ctrl+C, which now ends the process at once; the JSON log keeps every line written until then | A headless run that goes on until it's stopped, such as a dedicated server |
| GPU tests in Windows and macOS CI, which skip them because hosted runners there have no GPU | A self-hosted runner with a GPU, or a Vulkan driver that runs on those runners without one |
| A gate that compiles the shipping configuration: `cargo clippy -p player --all-targets -- -D warnings` | When `gg-engine` gets its `dev` feature |
| Waiting for the swapchain image before a frame reads input and the time, not after the layers update, which would cut latency by the wait: up to a refresh under `--vsync` | Phase 3, with input actions |
| Polling input state (which keys and buttons are down); layers get events only | Phase 3, with input actions |
| Pushing and popping layers while the app runs; `App` fixes the stack before it starts | A tool or game needs a layer for only part of a run |
| Composed text from an input method (winit's `Ime` events), for languages typed through one | A game takes text input, such as chat |
| Steady-state frames that draw UI allocate, in egui's layout and tessellation | Phase 4, with the allocation check; the overlay is for development |
| Keeping within the GPU's memory budget (`VK_EXT_memory_budget` through VMA) | Phase 7, with streaming |
| A swapchain without an sRGB format falls back to the first one offered, where colors come out too dark | A device offers no sRGB swapchain format |

## Open questions

- Which game is the flagship? Recommendation: a networked game whose design depends on
  determinism, so the simulation core, the tool API and multiplayer are exercised from the
  start. Decide before Phase 5, v0.1.
- Editor UI toolkit: egui, which the engine's overlay already uses, or GPUI? Decide before
  Phase 9.
- Which license? Recommendation: MIT OR Apache-2.0, the Rust convention, chosen once; until
  then, `publish = false` keeps the crates off crates.io. Decide before anyone outside the
  project contributes.
