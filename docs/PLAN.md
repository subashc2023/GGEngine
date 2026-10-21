# Plan

The roadmap. [AGENTS.md](../AGENTS.md) says how to keep it, under Workflow; the design is in
[ARCHITECTURE.md](../ARCHITECTURE.md).

## Done

- **Phase 0, workspace** (2026-10-20, the repository's first commit): the crates and apps
  build, the gates pass, and the unsafe policy is checked.

## Now: Phase 1, entry point, logging and Vulkan bring-up

On Windows, with the RTX 4090 and with the AMD iGPU, the sandbox clears the screen at vsync,
survives resize, minimize and close, and runs `--headless --frames 60` without a window, exiting
0, with validation reporting nothing. The GPU tests clear an offscreen image and read back its
color, and find a validation error caused on purpose in the JSON log with its VUID and object
names. CI runs the gates on Windows, Linux and macOS, x86-64 and ARM64, with the GPU tests on
lavapipe in the Linux jobs, and checks the layering rules. What's left: the same checks of the
sandbox on Linux, and on macOS with KosmicKrisp (Vulkan SDK 1.4.363.0 or later, the first
conformant build).

## Next

Each phase ends in something you can run and check.

2. **Runtime core.** Our own ECS with a transform hierarchy, reflected component types
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
3. **Resources and first mesh.** GPU memory through our own binding to VMA, 3.4.0 or later, in
   `gg-gpu`: `cc` compiles VMA, with VMA and the Vulkan headers it needs vendored; the Rust
   declarations are generated from VMA's header once and checked in; a failed VMA assert logs
   through the engine before aborting; custom pools rather than many dedicated allocations,
   since KosmicKrisp allows only 4,096 allocations. Buffers and images; uploads through the
   transfer queue, or by host image copy where a device has no separate transfer queue;
   bindless descriptors; buffer device address and vertex pulling; Slang pipelines with
   reflection; an on-disk pipeline cache; shader hot reload. Render extraction, with the render
   thread a frame behind simulation and rendering interpolated between fixed steps. *Done when*
   the sandbox draws a textured, depth-tested glTF mesh and thousands of moving entities, Tracy
   shows simulation and rendering overlapping, steady-state frames don't allocate, and a
   RenderDoc capture shows named resources and labeled passes.
4. **Tool API and hot reload: v0.1.** The tool API in development builds, as
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
5. **Render graph and PBR.** Passes declare the resources they read and write; the graph
   places barriers and layout transitions, aliases transient memory and schedules async
   compute. Physically based shading (glTF metallic-roughness), image-based lighting, cascaded
   shadow maps, HDR and tonemapping; reference-image tests on lavapipe. *Done when* Sponza
   renders lit and shadowed, synchronization validation is clean, and per-pass GPU timings
   show in Tracy.
6. **Assets and player.** `tools/cook` turns glTF into engine meshes (meshoptimizer: vertex and
   index optimization, meshlets, LODs) and textures into BC-compressed KTX2, packed into a
   memory-mappable package; an async streaming loader with a per-frame upload budget; hot
   reload in the sandbox; the player runs a package under a shipping profile (LTO, one codegen
   unit). *Done when* the player runs a cooked Amazon Lumberyard Bistro on all three OSes,
   with load time and memory measured.
7. **GPU-driven rendering.** GPU culling (frustum, then two-pass occlusion against a depth
   pyramid); indirect draws; meshlets, through mesh shaders where supported and a compute path
   elsewhere; a visibility buffer; clustered lighting; TAA. *Done when* CPU frame time stays
   flat as the instance count grows, and Bistro with many instances meets a frame-time target
   set in this phase on the RTX 4090 and an Apple Silicon Mac.
8. **Editor.** `apps/editor`: scene tree, inspector, play and stop, scene saving and loading,
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
| Shipping profile (LTO, `codegen-units = 1`) | Phase 6, with the player |
| Compiling trace and debug events out of shipping builds, through `tracing`'s `release_max_level_*` features | Phase 6, with the shipping profile |
| Deleting old JSON logs; every run's log is kept | Logs pile up enough to matter |
| A clean shutdown on Ctrl+C, which now ends the process at once; the JSON log keeps every line written until then | A headless run that goes on until it's stopped, such as a dedicated server |
| GPU tests in Windows and macOS CI, which skip them because hosted runners there have no GPU | A self-hosted runner with a GPU, or a Vulkan driver that runs on those runners without one |
| A gate that compiles the shipping configuration: `cargo clippy -p player --all-targets -- -D warnings` | When `gg-engine` gets its `dev` feature |

## Open questions

- Which game is the flagship? Recommendation: a networked game whose design depends on
  determinism, so the simulation core, the tool API and multiplayer are exercised from the
  start. Decide before Phase 2.
- Which text format for project state: RON, TOML, JSON or our own? Recommendation: RON in a
  canonical form keyed by entity ID, since Rust's enums and tuples map onto it directly,
  trailing commas mean adding an entry never touches its neighbors, and the `ron` crate saves
  writing a parser. Decide before Phase 2.
- Editor UI toolkit: egui, Dear ImGui, or our own? Decide before Phase 8.
- Which license? Recommendation: MIT OR Apache-2.0, the Rust convention, chosen once; until
  then, `publish = false` keeps the crates off crates.io. Decide before anyone outside the
  project contributes.
