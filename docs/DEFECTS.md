# Known defects

Wrong behavior or costs of code that exists, left in for now. Each row says what it costs, the
fix, and when the fix is worth it. Work not built yet is in [PLAN.md](PLAN.md)'s Deferred;
[AGENTS.md](../AGENTS.md) says how to keep this file, under Workflow.

## Performance

| Defect | Fix | Worth it when |
|---|---|---|
| A file dialog (`Context::pick_file`) or Windows' window menu spins the main thread at ~80% of a core: winit asks for a paint at each `WM_PAINT` the modal loop passes | Show the dialog on its own thread (rfd's `AsyncFileDialog`); answer `WM_PAINT` during callbacks, or fix winit | The spin shows, or a Linux compositor flags the app |
| On Windows, an OS-framed window's menu (Alt+Space) stops frames, and the next frame's `dt` spans the menu's time: it runs the most ticks a frame runs and drops the rest | Invalidate the window after each paint in the menu loop (`WM_ENTERMENULOOP`), or restart the clock at its exit | A game in the OS's frame ships on Windows |
| A development build holds each scene file's text twice and stats every one every 200 ms on the main thread (the asset watcher watches no `.ron`) | Hold the text once; take changes from the asset watcher | Scenes number in the hundreds or run to megabytes |
| While a game plays from its start, each `status` after a tick saves the open scene again to tell if it's unsaved: milliseconds for thousands of entities, four times a second | Track entities changed since the last check, or check a copy off the main thread | A large scene is open under a polling client |
| On Windows, a driver's message sent from the render thread can stall a frame ~16 ms: the main thread polls in 1 ms waits the timer tick stretches | Wait on a Win32 event with `MsgWaitForMultipleObjectsEx` | Such stalls show, as during resizes |
| A swapchain rebuild waits for the whole GPU, and a resize drag rebuilds each frame, so a resize's frames run one at a time (not measured) | Retire the old swapchain by its frames, with present fences (`VK_KHR_swapchain_maintenance1`), as Rendering's semaphores row has it | A resize stutters |
| Culling shaders underuse the GPU: `offsets_main` scans each run on one thread (~12,500 reads for 100,000 instances), and `meshlets_main` gives a small mesh's meshlets a whole 64-thread round (not measured) | A workgroup scan; fill a round with several instances' meshlets | A profile shows culling, with long runs or small meshes |
| Without mesh shaders (AMD's iGPU, KosmicKrisp), each kept meshlet draws unindexed: 372 vertex shader runs for 124 triangles, ~6 times an indexed draw | Have culling write kept triangles as indices, drawn indexed | A target lacks mesh shaders and its vertex work costs |
| Each `gg` command and `gg mcp` call runs `cargo metadata` to find the sessions folder: ~80 ms a call | Find it once per `gg mcp`, or from `CARGO_TARGET_DIR` and Cargo's config | Agents' calls are many and quick |
| `gg fetch` hashes every listed file with gg's unoptimized SHA-256: ~5 minutes for the showcase's 2.1 GB when nothing changed. PLAN's *Faster development builds* row wouldn't reach it | Build gg's hash optimized, or skip files whose size and mtime match | `gg fetch` runs often |
| Without `VK_KHR_calibrated_timestamps`, Tracy's GPU zones sit off the CPU's by a per-run offset and drift: the GPU clock is read once. Pass times and `gpu_ms` are right | Use `VK_EXT_calibrated_timestamps`, or re-read the clock each second | Someone profiles on such a GPU against CPU zones |
| A scene's load builds each component as reflection `Value`s first, 48 bytes a number: a list of 100 MB of text takes 250 MB to 4.8 GB. A snapshot parses the whole world before building | Read text into columns through a seed per type; build rows as read | A component holds a large list, or players' scenes load |
| A duplicate finds each reference among its copies by linear search: 16,000 entities referring to each other take 560 ms in debug | Map each original to its copy once | Someone duplicates thousands of linked entities |
| Prefab work reparses and remaps: a save or refit maps the prefab's parts per entity (quadratic), and every load, save, refit and override read (the editor's 20 a second) reparses its text | Map parts once per instance; keep parsed templates | Large or many-instance prefabs save, load or pick slowly |
| A state machine's blend whose clip crosses weight 0, or a state fading in, allocates its play's name | Keep zero-weight plays while the state lasts, or pool names per root | Ticks must allocate nothing while blends cross points |
| In a world of cells, the unsaved-edits check that `status` and `cells` ask writes, parses and compares every cell in, on the main thread | Keep each cell's canonical text; compare only cells an edit touched | Many or large cells are edited under a polling client |
| A world of cells scans its folder's listing each tick, tests every cell against every viewer, lists the folder per read in dev builds to check case, and keeps every cell text read | Search within viewers' reach, index cells, check names against the kept listing, drop texts as cells go | Thousands of cell files, or ~100,000 cells |
| Assets that cells name stay loaded after the cells go out: memory follows every asset the viewers crossed | Count each asset's cells; unload at the last | A world names distinct assets across thousands of cells |
| A tick run again that brings a cell in (rollback, SyncTest) prepares it on the main thread: ~1 ms a cell in debug | Keep prepared cells until their tick is final (`Peer::settled`) | Rollbacks across large cells stall |
| A shipped game's first start on a Mac took 6.2 s to its first frame, later ones 0.35 s; not profiled (Gatekeeper, the pipeline cache's fill or the package's first read) | Profile a cold start; cache or defer what it finds | A game ships on macOS |
| Drawing rows at one z in ID order costs ~0.9 ms of the swarm's 4.3 ms frame, a re-sort after changes ~2.5 ms more, and 100,000 sprites at their own z ~5 ms | Keep tables in ID order; radix sort | A game's frames need the time |
| Spawns and loads go one entity at a time: 100,000 sprites spawn in ~16 ms and load from 45 MB of text in ~650 ms (release) | Batch spawns; build columns in bulk | A spawn hitch or a scene's load time matters |
| The asset watchers poll: the root is walked and every ID file read each second (~70 ms per 1,000 assets), files checked every 0.1 s, changes reloaded 0.1 s apart; a start scans on the main thread (~6 ms an ID file written) | OS change notifications; reload changes together | Tens of thousands of assets, or many edited at once |
| The Assets and Scenes panels and the hub list their folders on the main thread each second they show, the hub reading every manifest | List them on a watching thread | Folders of thousands of files, or many projects |
| A remote Inspector reads the game's whole entity list four times a second, a few steps an entity on the game's main thread | Read only the rows shown (`entities` by parent) | A game of tens of thousands of entities |
| A reloaded model's mips are made on the render thread as they stream: ~0.3 s of a frame for a 2,048² texture in debug | Make them on the GPU, or on the watcher's thread | A reload's hitch matters |
| Frame arenas grow by doubling and merge on reset, holding ~4 times their largest frame (the sandbox's 6 MiB for 1.3) | Shrink toward the peak, within the GPU's budget | Arenas reach tens of megabytes |
| Frames redo work: an atmosphere's tables each frame (~36 µs on an RTX 4090), bloom's first downsample reading texels 13 times (~24 of 57 µs), interleaved mirrored layer draws a run each | Tables as inputs change; taps in workgroup memory; mirrored draws sorted together | A profile shows them |
| A development run draws its opaque vertices twice, depth for AO and then shading, as it has no visibility buffer | Draw over that depth, testing for equality, with vertices placed alike | Its frames lose much to it |
| A build without `dev` still makes the UI pass's pipeline at start, though it's handed no UI | Make it only when a UI is drawn | A shipped game's start waits on it |

## Rendering

| Defect | Fix | Worth it when |
|---|---|---|
| Layers' world draws (`Context::draw`, `draw_sprite`, `draw_line`) take world positions as `f32`: 1,000 km from the origin they're placed in 6 cm steps | `f64` positions, or a transform from `Context::origin` | A game draws its own things far from the origin |
| A swapchain without an sRGB format falls back to the first offered, and colors come out too dark | Encode sRGB in the shaders that draw to it | A device offers no sRGB format |
| A swapchain's semaphores are destroyed at device idle, which doesn't cover pending presents; closing torn-off windows makes it likelier | Wait for present fences (`VK_KHR_swapchain_maintenance1`) | Validation reports it, or windows open and close often |
| gg-gpu's GPU waits have no deadline: device loss ends them (`gpu.device_lost`), but a hang without a watchdog (lavapipe, a display holding images, a missed signal) hangs the run with no code | Wait in slices, warning with a code; fail past a deadline, drops leaking | A run hangs in a GPU wait |
| `gpu.pipeline_cache` warnings name the GPU, not the file | Pass the path through `Settings::pipeline_cache` | Someone needs the file |
| Bindless slots past the placed ones (UI textures, the view's pixels and images) hold their view before any write and after it's destroyed: sampling one can fault the device, unseen by GPU-AV | Point them at a stand-in when reserved and freed | Indices come from data, or a fault traces there |
| Removed assets' room isn't reused: bindless slots (61,440 textures a run, removed ones included), material table rows (108 bytes each), meshlet bounds | Free lists | A game streams levels in and out |
| The graph keeps images no frame declares any more: a world's 64 MiB of shadow maps stay after shadows stop | Retire images undeclared for a few frames | Shadows are turned off to save memory |
| Images are checked against `maxImageDimension2D` as they're made (`gpu.image_too_large`), but not against `maxFramebufferWidth`/`Height` | Check those too, as `check_size` does | A device's limits fall below a window's size |
| A frame drawing more than `maxMemoryAllocationSize` or `maxBufferSize` (each ≥1 GiB) at once fails, ending the run: the frame arena grows by doubling | Split the arena across buffers | A frame draws that much |
| A view keeps at most 2^22 meshlets a run (some devices' limit), dropping the rest; past 32 MiB of meshlet room a view, runs draw unculled by index. The room, sized for every instance's finest level, is 66 of the 16 Bistros' 197 MB | Split runs; size the room by recent frames | Such runs show, or memory matters |
| Games' vertex and parameter structs are checked by size and 32-bit fields alone: same-size fields reordered or retyped draw garbage without a message | A derive giving each field's offset, scalar type and width | A struct mixes kinds, or an agent's edit draws garbage |
| A game's shader the driver can't make a pipeline for ends the run at first use (`MeshPass::add_shaders`); a recompiled one keeps its old pipeline | Warn with a code and draw nothing from that slot until it compiles again | A driver refuses a shader the check passes |
| Built by slangc 2026.20 (CI's and ship.yml's), the exposure meter's `average_main` stores to address 0 on AMD's driver 25.10.30.02, losing the device in adapting views. Found in the ISA alone | A store shape 2026.20 compiles right, checked in both compilers' ISA | Before such a build runs on an AMD GPU |
| A glyph whose contours overlap (common in variable fonts) can draw seams: fdsm doesn't resolve overlaps | Union contours before baking, or msdfgen with Skia | A font shows seams |
| Straight alpha, as PNGs store it, filters transparent pixels' colors into edges: dark or colored fringes | Premultiplied alpha | A game's art shows fringes |
| Images that bring their own levels (DDS, as Bistro's 114) keep them: masked ones thin out at distance, and normal maps' spread doesn't widen roughness, so highlights sparkle. A normal map without a metallic-roughness map sparkles too | Decode BC7 at load and remake levels as for PNGs; a spread texture beside each normal map | Such a model's distance thinning or sparkle shows |
| Quads (sprites, text, particles) are fogged at their corners and interpolated, so a large quad in steep fog is fogged unlike what's opaque beside it | Fog per fragment where corners differ much | Large sprites or text show fog seams |
| An adapting exposure meters unlit draws (sprites, text, unlit, emission, the clear color) as if lit, so a mostly unlit view drifts to a bound instead of settling | Mark lit pixels (an ID bit or a target) and meter only those | A game that adapts draws much of its view unlit |
| An adapted exposure lives in the renderer, so every start, `gg run`'s restarts included, shades its first frames at the environment's exposure and then flashes | Keep it in the snapshot | Restarts' flash is seen, as at night |
| An image sky's sun is found as texels over 10⁷ cd/m²: a low sun falls short, and a sky lit by its directional stand-in too lights twice | Find the sun by contrast with the sky round it | A scene under a low sun lights twice |
| The frame's time is an `f32` of seconds, stepping ~8 ms after a day of simulation | `f64` time | A game animates by time through runs that long |
| The view's IDs carry TAA's jitter, so the editor's outline can shimmer a pixel and picks at edges miss | Draw IDs, or the outlined entity's, without the offset | The shimmer bothers someone, or edge picks miss |
| Under TAA and motion blur, layers' own draws, particles and skinned limbs move with their instance or the camera alone: fast ones smear or dim, and don't blur | Motion from a lasting identity for layers' draws, particles' own, last frame's skinned vertices | Fast movers visibly smear, or should blur |
| TAA keeps its history across a camera cut (`RenderData::cut` reaches motion blur alone), so a cut's first frames are soft; games can't mark a teleport as a cut | Reset the history at a cut; a cut flag for games | Cuts' first frames show blurred |
| Under TAA, sub-pixel and near-cutoff detail (railings, ivy) still wanders in pans, at about a quarter of its range without the flicker test; still views settle. Behind what crosses it, it flickers for tens of frames; hue flickers too | Blend share by the pixel's own swings; evidence from neighbors; chroma kept as luma is | Thin detail flickers in pans, or wakes show |
| Under TAA, what changes faster than every 20 frames while moving, or in step with the offsets' 1 to 8 frame cycle while still, shows its mean (a 4-pixel square blinking 5 frames in 10 leaves 127 of 231 levels) | A material or shader mark that its pixels change, ending their evidence | A fast blink should be seen |
| Under TAA, depth of field takes circles from the jittered depth but blurs resolved colors: in-focus edges over blur shimmer up to 15 levels over the offsets' cycle, 4 without it; the 3x3's nearest circle only moved it a pixel | Run depth of field before the resolve, or resolve circles over frames | In-focus edges shimmer |
| SMAA sees the view through the grade's slope and PBR Neutral's shoulder, not ACES's or AgX's curves or contrast: a bright edge's coverage is ~0.19 off under ACES (0.26 unantialiased) | Find edges on the tonemapped, graded view | Bright edges under ACES, AgX or strong contrast show steps |
| Reflections take last frame's colors where the camera alone moved them: a mover's reflection lags a frame | Reproject by the view's motion | Fast movers' reflections visibly lag |
| Reflection edges where rays pass behind the depth are serrated: each ray stops at the first pixel behind it | Refine the crossing within that pixel | Mirrors are seen without TAA, or large |
| A game's opaque shader reflects about its facets' normals: curved meshes mirror in steps and normal maps don't show; the contract has no `normal_main` | Let games' shaders add a checked `normal_main` | A game reflects curved meshes it shades itself |
| A reflection is hazed and fogged twice: its colors already carry the camera-to-hit haze and fog | Take those out by transmittance and add the hit-to-point ones, or keep colors from before fog and haze | Mirrors show far things in thick haze or fog |
| AO lags a few frames round movers: its history follows the camera alone, bounded by the frame's own range ±0.02; a rolling sphere leaves ~800 pixels off by 2 to 8 levels | Draw motion before AO; reproject per pixel | Moving occluders' AO visibly lags |
| Games' opaque shaders that discard cast whole-triangle shadows and AO depth: they compile `vertex_main` and `fragment_main` alone | A cutout fragment shader for depth, as the masked model shader has | A game's opaque shader discards, as foliage's |
| Skinned primitives that blend draw in their bind pose: blended draws take no entity's own mesh | Skin blended primitives too | A skinned model blends, as hair cards |
| An emitter's births between frames are placed by an entry-wise blend of two matrices: after a hitch a turning emitter's trail cuts corners, and its shape shrinks mid-blend | Keep each tick's pose; blend rotations the short way | Trails kink after hitches, or emitters spin fast |
| An emitter whose burst count is raised when it first draws bursts then: a restored world or a scene saved mid-run bursts every emitter once | Track bursts in the world, not at first draw | Such a burst shows, as after a restart |
| A line's quad keeps the line's depth across its width, so a face sloping across it hides half the line | Depth bias for lines | Lines are drawn on 3D surfaces |
| An entity despawned in a tick vanishes up to a tick before it reaches where it stopped | Keep drawing it on its way there until the next tick | Despawns show, as bullets vanishing short of their target |

## Windows and the dock

| Defect | Fix | Worth it when |
|---|---|---|
| egui's first pass of a new floating group sizes it 600 by 400 points and clamps it inside the dock, so a group floated near the right or bottom edge moves and is saved there | Give the group's `Area` its saved size (`default_size`) | Someone floats a group near those edges |
| On Windows, an inner size asked of a UI-titled window (`InnerSize`, `MinInnerSize`, `MaxInnerSize`) leaves it a frame smaller | Add the frame, as `AdjustWindowRectExForDpi` measures it | Code resizes such a window |
| On Windows, a maximized UI-titled window covers the monitor, which can keep an auto-hiding taskbar from showing | Leave it a pixel or two, as Windows Terminal does | Someone hides their taskbar |
| A torn-off window opens at its saved position even off every monitor | Move it onto one | Someone's monitors change |
| Frames pause for a minimized window only at zero area, as Windows gives it: macOS reports occlusion and Wayland nothing, so frames go on unseen and `ping` says nothing waits (unchecked) | Pause at winit's `Occluded(true)`; give paced acquires a deadline | Someone minimizes a game on a Mac or Wayland |
| A torn-off window opening on a monitor of another scale is placed at the main window's scale, away from the cursor | Place it by its own monitor's scale | Someone docks across such monitors |
| In a frame egui runs twice, torn-off windows' texture changes apply before the main window's, so glyphs added to a growing atlas can go missing | Apply changes in egui's order, which needs egui to hand them over per pass | It shows |

## Input

| Defect | Fix | Worth it when |
|---|---|---|
| A frame reads input about a frame before it renders (a refresh under `--vsync`), and draws the world up to a tick behind the simulation | Start a frame once the render thread has its image; extrapolate from the latest ticks | A game's input latency needs it |
| A click that closes a menu also reaches what's under it, the game's view included | Keep a press from the layers while an egui menu is open | It surprises someone |
| In games, a press within egui's 5-point interact radius of an edge widget (a splitter, a title bar button) reaches the layers too | Count as egui's a press its widgets sense | A press at the view's edge surprises someone |
| A modifier pressed while egui or another window had the keys counts only from its next press or repeat | Give the layers a press of each held modifier as they gain the keys | Someone holds one first |
| Layers below one that handles a stick's move off rest keep its last value, so rolling a stick right to up in a menu leaves the game going right | Pass the stick's return below, as a release passes | A game's menus take a stick |

## Recordings

| Defect | Fix | Worth it when |
|---|---|---|
| A run that goes on after `Context::load_scene_file` refused a scene for a missing texture replays otherwise (`replay.diverged`): the replay's stand-in loads it | Record the names that failed and refuse them in the replay | A game goes on after a scene fails to load |
| A command nested deeper than the 64 levels a replay reads is recorded, so the replay refuses the whole recording (`replay.syntax`) | Count levels while recording, ending with `record.bad_command` | A command nests that deep, or the tool API takes outside values |
| A build without `dev` refuses a replay or a lockstep peer whose Inspector queued commands (`replay.bad_command`): its `Inspector` takes none | Apply the Inspector's edits without its UI in such builds | A shipped recording, or peers across builds, holds them |

## Reflection

| Defect | Fix | Worth it when |
|---|---|---|
| A bound starting with `0` (`min = 0.5`) on an array field (`[f32; 3]`) fails clippy's `zero_prefixed_literal`: the derive's check places the type's tokens at the bound | Place only the check's own tokens there | A game bounds an array field so |
| A list's `#[reflect(opaque)]` state is kept by its ends when its length changes and by place otherwise: swaps trade state, and `[a, b, c]` to `[a, c2]` gives b's to c2, silently (`kept_from`) | Callers that edit by element pass its index | Tools reorder or edit lists of opaque state |

## Snapshots

| Defect | Fix | Worth it when |
|---|---|---|
| A restore whose only change is reordered enum variants warns `snapshot.hash`, though every value came back: snapshots name variants, the hash digests places | Write a digest of each type's fields and variants; check the hash only where all match | The warning misleads someone |
| A restore retakes a texture's or model's removal where its notes line up, and can't tell a layer's add from text's load of the same file: a rebuilt layer adding it draws its stand-in, unwarned | Note text's loads apart from layers' adds | A game removes text-loaded assets and is rebuilt to add them |

## Networking

| Defect | Fix | Worth it when |
|---|---|---|
| Each accepted connection holds a thread until its handshake ends, up to 5 s: a flood costs a thread each | Handshakes on one polling thread, capped per address | Sessions listen where strangers reach them |
| A peer or client with the code can make the host or server hold what it sends for ticks up to a second ahead | Cap a tick's lines and held inputs in bytes | Sessions take untrusted players |
| A session's code passed as `--code` shows in the process list to the machine's other users | Take it from the environment or a file | Sessions are hosted on shared machines |
| A joiner's restored world starts the authors' logs afresh while the host keeps its own: an undo across the join applies on the host alone and forces a resync (`net.desync`) | Send the logs with the world, or reset the host's too | An agent undoes across a join |
| Each peer tick hashes and copies the whole world, kept until the other's hash comes: ~5.5 ms a hash and 0.5 ms and 20 MB a copy for the sandbox's 100,000, more copies on slow links and under rollback. Without commands a tick still allocates 8 times: 3 for the hash and copy, 5 for inputs' maps | As PLAN's *Cheaper hashes and snapshots* row; reuse the hash's line and the inputs' entries, as packets reuse their text | Large networked worlds, or ticks that mustn't allocate |
| A tool API edit to a paused lockstep peer waits for a step, its answer too | Agree on a point between ticks and apply there | Someone edits a paused networked game |
| Peers number patches by order, matching only under one `gg run`: two machines' runs can mix them, and a patch reaching or applying on one peer alone stalls the joiner or splits their code | Name patches by their sources' hash; restart both from snapshots when either fails | Peers on two machines are patched |
| A resync just before a patch applies takes the joiner back onto code its host doesn't run yet: they part and resync again | Send the world only once the joiner is behind the host's next tick | Resyncs come often |
| A joiner's edit in flight during a resync is answered from its side alone: refused though the host ran it (a retry applies twice), or applied at a tick the restored world never ran; `gg` edits through the host | Answer once the host's hash after its tick matches, or pass edits to the host | An agent edits through a joiner (`--pid`) |
| A peer whose main thread hangs while its link's threads run isn't lost: the other waits at its next tick forever. A stopped or cut-off peer is lost after 10 s | Send presence only while frames count on, as `ping` tells hung from busy | A peer hangs outside `gg run` |
| A minimized peer runs no frames, so the other peer, edits included, waits until it's restored | Keep a lockstep peer ticking while minimized, without rendering | Someone minimizes one, as `--peers 2` stacks them |
| A step asked of a joiner counts in the numbering its host's world then replaces: its answer comes early or late | Shift waiting steps by the jump, or count ticks run | An agent steps a joiner as it joins or resyncs |
| A development build that reads its project's files names its build by its source alone: two such runs whose assets differ on disk join, and resync forever if their simulations read the difference | Digest the asset root's files as the session starts, or each as it loads; skipped, as the showcase's are hundreds of MB | Peers on two machines play checkouts whose assets differ |
| A world that doesn't round-trip its snapshot text never resyncs: a rotation drifted off unit length comes back normalized on the joiner alone, and a client corrects at every state | Restore the sender's world from its own text, or Physics' rotation row's fix | Bodies turn freely for minutes |
| A server's client reads each state's text whole into its own world and hashes it, however little changed: 2.4 ms a state of two skinned characters and a ball in release, 19 ms in debug, where clients fall behind; each correction reruns every tick since the state | Apply changed tables in place; correct only what changed | Worlds much larger than the demo's, or debug clients |
| States hold every animated model's nodes: ~47% of such a state, physics ~40% (next row); a client downloads ~0.75 Mbit/s at 32 states a second | Leave animator-posed nodes out, posed by the client (a change to what a networked world holds) | Many animated characters, or slow links |
| A server writes physics' state whole as RON in every state, per view, copying it for each view that hides bodies: 83 of such a state's 130 KB, ~0.6 ms of the states thread's 1.9 ms a tick for two clients | Write only bodies that changed, or binary | Cells hold many bodies, or many clients see apart |
| A server's client brings no cell in or out, so its predictions part at each tick its view's cells change: a flier crossing ~30 cells a second corrected at ~70% of states | Have clients run the cells step for their view, as the server does | Viewers cross cells quickly |
| A client learns its server's texture names only at join: a state naming a texture added later doesn't read, and the client warns and stops correcting | Send names as the server adds them, before the states that name them, as a lockstep host does | A served game adds textures as it plays |
| A joining client gets its server's world, and a joiner its host's, as uncompressed text (159 KB for such a game) and ticks none meanwhile: 1.0 to 1.5 s to join over a 75 ms loopback link, 0.72 s from send to restore through a relay 145 ms away | Send it compressed, as states are; restore as it comes | Worlds grow, or players join over slow links |
| A server's client frees, as it loads each state, the vacant slots the server keeps for undos, so its predicted spawns take other slots and are corrected (`ecs/file.rs`'s load) | Keep vacant the slots a state already names vacant, as a lockstep host renews its world | Clients predict spawns |
| A joiner that joins after its host's texture was renamed loads it by the host's old path, a stand-in where that's gone | Send the ID, or the current path | Assets are renamed during a lockstep session |
| `net::tests::patches_apply_on_both_peers_before_the_ticks_the_host_chooses` can fail on a loaded machine (tick 14, not 13): the host ticks before sending its world, or the joiner applies late | Compare the joiner's first tick with the host's; fix whichever is wrong | It fails in CI |
| Tests that run sessions or links pace themselves by the clock. CI runs gg-engine's `net::` tests in a step of their own, but the gate runs them beside the rest, where a machine of few cores fails some (2 to 4 a run on two CPUs) | Give sessions a clock the tests drive | They fail in CI's own step, or the gate fails them where the rest pass |
| A relay runs a thread per connection, two for a host's, and one for all datagrams: 4,096 connections by default (`full`) | Poll sockets from a few threads (`mio`); forward datagrams on more | A relay serves thousands of sessions |
| A relay is open: anyone who reaches it registers rooms and pairs through them. It caps connections from one address and holds each end of a pair to rates, but many addresses together only to its most connections and its link | An operator's key to register, as a project naming its relay would ship | Others than a game's players use it, or many addresses fill its link |
| A relay's JSON log is never rotated or removed: ~600 bytes a pair | Rotate by size or day, or leave a service's to the journal | A relay runs for months, thousands of sessions a day |
| A relay listens on IPv4 alone by default, and runs resolve its name to IPv4 first: a network of IPv6 alone, without NAT64, can't reach it | Listen on both families | Players on networks of IPv6 alone |
| A session's room goes to the relay in plain text: an observer can make joins fail at the handshake, or hold the room after the host's relay connection drops | Prove the code to the relay without showing it (a PAKE), or register over TLS | Someone on the path denies joins |
| A link whose UDP can't reach its relay gets no packets: lockstep joins, then waits for inputs; a server's client gets no states | Send packets on the stream until the relay answers the socket | Players' networks block UDP |
| Direct paths are tested only on Linux NATs in Docker: consumer routers, carrier-grade and mobile NATs and Windows Firewall are untried. An open-input Linux router goes relayed, its mapping taken by the other end's early probe | Measure on real networks (`net.direct`, `net.relayed`); first probes with a TTL that dies before the other router | A players' relay keeps their packets |
| A peer with the code can have the other send its packets for 5 s to any addresses its `at` names | Probe with tiny packets before sending real ones | Sessions take untrusted players |
| A link stays direct only while a packet came direct within 250 ms (`QUIET`): an end sending a few packets a second, as a debug client on lavapipe, falls back to the relay at each loss, logging each switch; so does a joiner at the frame that restores its world | Keep a way direct for a few of the other's packet gaps | Players run at a few frames a second |
| A lockstep session's hashes (`h` lines) go on the stream, so they never go direct: ~3.7 KB a second each way through the relay | Send hashes in packets, or every few ticks | A relay carries many lockstep sessions |
| Packets repeat as text every input the other side hasn't acknowledged, up to twice a tick: ~200 kbit/s each way for Shards' peers, ~175 up for such a game's client | Send once a tick; one line for runs of empty ticks, or binary | Players are on metered or slow links |
| A run in a session its game's menu started comes back alone when `gg run` restarts it from a snapshot, which doesn't hold the session: the other run loses it | Snapshot the session's end and code, and start it again on restore | Someone edits under `gg run` while playing such a session |

## Scripts

| Defect | Fix | Worth it when |
|---|---|---|
| Scripts read and set positions as Luau's `f32` vectors: 1,000 km out they step 6 cm, 16 km out 2 mm (`Field::DVec3`) | Two vectors, or an `f64` userdata, slower | A game scripts movement far from the origin |
| `pairs` over a table keyed by tables or functions goes in address order, differing per run, peer and platform, and so does its growth | Hash such keys by a number each gets as it's made (`luaH`), or refuse them | A script iterates such a table |
| A mod's VM memory can differ between peers (a joiner's mods start afresh; a rollback peer's reruns leave garbage), so a mod near its limit fails on one alone (`net.desync`, `sync_test.diverged`) | Restart the host's mods at a join through a recorded edit; collect each VM after settled ticks | A mod runs near its limit in a session |
| Luau's C library calls (`table.sort`, `buffer.fill`, plain `string.find`, `utf8.len`) cost one interrupt however long they run: sorting 2,000,000 took 0.74 s, so a loop can hang every peer for hours | Charge their work by elements or bytes | Untrusted mods, or loops over large data |
| `ipairs` and `utf8.codes` iterators called with a script's own number cast it as C does, which platforms do differently past `int`; `string.format` and string-to-number use each C library's conversions, untested | Check those arguments (costing `ipairs` its fast path); test or replace the conversions | A script calls them so, or a libc differs |

## Physics

| Defect | Fix | Worth it when |
|---|---|---|
| Physics read from a snapshot, a host's world or a server's state is taken as rapier serialized it: text naming bodies rapier's sets lack can panic its step; the engine checks its own handles alone (`physics.load`) | Validate rapier's handles as read, or rebuild bodies from entities | Worlds come from untrusted peers or files |
| A body turning for minutes drifts off unit length, as steps don't normalize: its scene's save is refused (`reflect.not_normalized`), its rotation's undo drops, and a snapshot normalizes it | Normalize at each step, rewriting physics' reference hashes | Bodies turn long, or scenes save in play |
| A `MeshCollider3D`'s triangles and BVH save whole: Sponza's write 76 MB in 0.4 s and read in 1.2 s, in snapshots, joins and states. Naming the model loses exactness, as rapier marks a placed shape changed | A rapier method placing a shape unmarked, upstream or in a fork | A networked level is a mesh, or snapshots take long |
| Every model keeps its meshes' positions and indices on the CPU for colliders, used or not (~4 MB for Sponza) | Keep them for models an ID file marks as colliding, or reread | Memory is short, or many large models never collide |
| A mesh collider is built on the main thread at its body's step and again at each Play: 180 ms for Sponza in release, 1.2 s in debug; each body of one model and scale builds its own | Build at model load, off the main thread, shared | A level's first tick hitches, or a model is placed many times |
| A prismatic motor pushes at its anchor, and rapier gives way to the turn an off-center push makes: a box anchored off-center moved at a fifth of the speed and sank, driven up | Move the joint's frames to the center of mass, or more solver iterations | A game drives an off-center slider |
| Taking bodies' components in marks every rapier body changed each step, so rapier redoes its changed-body pass, joint selection and solver layout for all | Reach a body mutably only when its components changed | Steps cost with many bodies (reference hashes change) |
| A character a rising kinematic body carries into a ceiling stays wedged, and the game isn't told | A crushed event or flag; stop the platform or let the game choose | Lifts or crushers under low ceilings |
| Pairs without a dynamic body have no contacts, so no events: kinematic against kinematic or static, characters against walls or each other. A sensor works, at a collider's cost | Report the controller's hits as contacts; let colliders opt into such pairs | A game needs those touches |
| A sensor's density weighs its body: a large pickup sensor makes a dynamic body heavy unless its density is 0 | Make sensors massless (existing bodies would move differently) | Sensors on dynamic bodies feel heavy |
| rapier silently caps every body at 400 units a second (`normalized_max_linear_velocity`), kinematic ones and set velocities too | A cap field on `Physics2D` and `Physics3D`, documented on `velocity`, with sweeps past it | Vehicles or projectiles go faster |

## Audio

| Defect | Fix | Worth it when |
|---|---|---|
| Music whose file claims more frames than it decodes spins kira's decoder thread at full speed for the rest of the run once a play reaches its end | Have `Stream` answer past its end with silence up to the claimed frames | Music reloads as it's written, or comes from mods |

## Files

| Defect | Fix | Worth it when |
|---|---|---|
| A save over a hard-linked file (scene, dock layout, pipeline cache) renames a new file over it, so the other links keep the old content | Write such files in place, or say so | Someone keeps scenes behind hard links |
| A model reads outside its folder through a symlink inside it | Resolve links and refuse one that leads out | Models come from untrusted sources, as mods |
| On FAT or exFAT, which lack hard links, a run writes no ID files (`asset.unwritten`) | Fall back to create-new where linking fails | Someone develops on such a drive |
| On macOS, an asset and its ID file named in different Unicode normalizations get no ID (`asset.no_id`), the ID file called an orphan (`asset.orphaned`), whose advice loses the ID | Ask the file system for the ID file; take it as misnamed (`asset.meta_case`) | Someone names assets outside ASCII on a Mac |
| A rename in case alone, which Windows and macOS allow, keeps the ID only for the run (`asset.meta_case`); then the asset goes without until the ID file is renamed | Rename the ID file with its asset | Someone renames assets in case alone |
| The tool API's `assets` lists a model renamed since a layer added it as not loaded: a model's ID names its images alone | Track loaded models by ID too | A tool acts on whether a model is loaded |
| Two runs writing ID files (the editor's game and an agent's) can each write one for a renamed asset in overlapping scans: two ID files hold the ID (`asset.duplicate_id`) | After linking, list the folder again and remove ours if another holds the ID | Several such runs are common |
| On Windows, a fetch list's or manifest's path ending in `.` or a space names another file (`x.` is `x`) | Have gg-api's `is_relative` refuse such parts and device names (`CON`) | Fetch lists or manifests come from outside the repository |
| A model's JSON is parsed whole before `MAX_MEMORY` counts anything, at ~64 times its text: 100 MB of empty nodes takes ~6 GB, and a 4 GiB one aborts | Count objects, arrays and strings in a pass before parsing | Models come from untrusted sources, as mods |
| While a shader fails, the watcher looks at its files' folders but not subfolders: a broken include in a subfolder, fixed, waits for another change | Watch the includes the compiler named | Shaders include from subfolders |

## Cooking

| Defect | Fix | Worth it when |
|---|---|---|
| The cooker cooks a model's images twice, in the model with mips and as one-level textures, taking every PNG and JPEG under the root: Sponza's 69 add 50 MB to the showcase's 141 MB | Skip an image a model names unless a scene names it or the project says code adds it | A package's size matters |

## Shipping

| Defect | Fix | Worth it when |
|---|---|---|
| The Visual C++ runtime a Windows folder holds beside the game gets no Windows Update fixes, as a system-wide install does | An installer that runs the Redistributable's own | The game ships through an installer, or the runtime gets a security fix |

## Animation

| Defect | Fix | Worth it when |
|---|---|---|
| A blend plays its nearest clip at its own rate past its points: a model slower than its slowest point, as an agent slowing at its destination, plays that clip at full rate, its feet sliding | Scale a state's speed by a parameter, or blend in a still pose, a clip of no length, at 0 | Slow walking visibly slides |
| Skinned vertices posed past what their clips reach (by code, IK or a ragdoll) clamp to the box views cull by | Grow the box from the posed joints | A game poses joints past its clips |

## Navigation

| Defect | Fix | Worth it when |
|---|---|---|
| Navigation bakes and walks in `f32` world space: precise within ~16 km of the origin, steps of 6 cm at 1,000 km, `World::find_path` and `gg.find_path` too | Bake each volume in a space from its origin, and walk agents there | A game navigates far from its origin |
| The navmesh's fingerprint is made again each tick and each path search, over every static body (tens of µs a thousand), which is why a search takes the world mutably | Skip it while no static body or volume changed | Scripts search many paths in scenes of thousands of static bodies |
| A bake runs on the tick's thread: ~1 ms for a small walled arena, 8 to 13 ms for the showcase's maze in debug | Bake off the main thread; swap at a tick peers agree on | Static bodies move in play, or levels are large |
| Replanning is broad: a bake replans every agent, and an agent no path reaches searches every tick, a failed A* visiting every reachable polygon | Replan only agents whose corridors cross changed polygons; retry on a navmesh or polygon change | Bakes come often, or many agents can't reach |
| Agents keep no corridor, so one pushed far off its path heads for its next corner and may stick behind a wall | Keep a polygon corridor and pull it from where the agent is | Agents visibly stall in crowds |
| ORCA's nudge to the right bends an agent's line about a degree whenever neighbors are near | Nudge only where the velocity obstacle is head on | Someone notices the sidestep |
| Dense crowds crossing each other clear slowly: 52 to 64 of 100 agents swapping sides of a small walled arena arrived in 17 s | Crowds beyond ORCA, or ORCA lines for the navmesh's edges | A game's crowds cross through each other |
| An agent's corners hold room for 16: a longer path grows them once, and a world copied afresh drops that room | Bound the corners and replan at the last, as Detour does | Long paths in a game that measures allocations |

## A game's UI

| Defect | Fix | Worth it when |
|---|---|---|
| A field a UI node's kind doesn't take is refused at the node's end, not at the field (`ui.bad_field`): serde reads the whole node first | Check each field as read, through a seed that knows the kind | People search large UI files for refused fields |
| On the frame a resize comes in, a UI's fills still span the old size, while what's anchored keeps to its edges | Lay out at hand-off | That frame is noticed, as in a capture |

## Editor

| Defect | Fix | Worth it when |
|---|---|---|
| An entity flipped by its own negative scale shows its handles by its rotation alone, so a flipped sprite's X arrow points away from its +X | Turn the handles by the signs of its own scale too | A game flips sprites by scale, and the handles confuse |
| Under a parent scaled unevenly, a turned entity's handles show the nearest square axes, so drags follow the cursor only roughly | Elliptical rings, and Scale along the drawn axis | Someone edits under such a parent |
| An Entities row whose name has a line break is that many lines tall, but `show_rows` skips rows as one line each | Show line breaks as spaces | More entities than the panel shows, or names from players |
| The Inspector sets a rotation, or a vector in an option or variant, whole as last read: turning one angle of a spinner holds the others, and an undo takes back the spin too | Set only the part that changed | Someone edits a moving rotation |
| Undo, redo and revert show the old value until the tick, or in the editor until the game answers and is read again (~50 ms); refusals and the Edit menu's names come then too. A set draws ahead | Draw undo, redo and revert ahead as a set is | It shows |
| The editor learns another client played, stopped or paused from a `status` polled every quarter second: the title bar lags, and input sent to a just-stopped game applies at the next Play | Carry the run's state in each shared frame | Agents play and stop games the user plays in the editor |
| Started outside a project, the Entities and Inspector panels wait for a game that won't serve | Let the editor's link say why, for the panels to show | Someone starts the editor outside a project |
| In Play, the Inspector's Ctrl+D and Ctrl+Z reach the game too (a duplicate steers Shards' player 1 right); the File menu's shortcuts are kept from it | Keep every key the editor takes from the game, as `route` does the File menu's | A game binds D or Z |
| On a layout that moves N, O or S, the File menu's shortcuts are QWERTY positions while named Ctrl+N, O, S | Name keys by the layout's labels, as PLAN's *Bindings* row has it | Someone on such a layout misses them |
| A close asked while the editor is minimized shows nothing until restored: the unsaved-scene prompt runs in frames | Restore the window to ask, or ask between frames | Someone closes it from the taskbar with unsaved edits |
| While stopped, a cell whose entities differ from its file stays in however far the camera goes, until saved | Let edited cells go out as text, as in play, and have a save write them | Edits between saves outgrow memory |
| A save prompt before New or Open is lost when `gg run` restarts the game, and the New or Open with it | Keep the prompt and its action across the restart | Someone loses one to a restart |
| The editor's Inspector shows a texture by its ID (`55bcfa88ac30e5a5`), as the tool API names it, not its path | Show the path `assets` lists beside the ID | Someone reads textures in the editor's Inspector |
| The Assets panel hides every ID file by name, even one the game warns of (`asset.bad_meta`, `asset.duplicate_id`) | Show ID files that `assets` lists with a problem | Someone looks for such an ID file |
| A turn away from the game's camera and back within one frame keeps the view's adoption of it (`adopting`, read once a frame), so the view leaves the game's camera at the next frame drawn through it in Edit | Clear `adopting` wherever `world_camera` turns off | A tool turns the view as the user clicks, and the jump bothers someone |
| The editor's camera keeps its own 60° field of view when it takes a viewpoint, so the view widens or narrows as it switches to it | Take the viewpoint's field of view | A game's camera or an agent's look uses another |
| An embedded game's `Context::scale` is 1, so what it draws in points, as colliders' outlines, is thinner on high-DPI displays | Pass the editor's scale to the game | Someone misses them on such a display |
| A remote Inspector sets an option or an enum whole, so one holding an entity despawned since its read is refused (`ecs.no_entity`); a texture without a path can't be read | Set what's inside one; say why a component can't be read | Layers make their own textures, or such fields are edited |

## Edits

| Defect | Fix | Worth it when |
|---|---|---|
| A scene file's save reparents in ID order, so swapping a parent and child is refused whole (`ecs.parent_cycle`), though the result is a tree | Make each moved entity a root first, then reparent parents first | Someone restructures a hierarchy in a scene file live |
| An undo of an edit to a cell's entity, once the cell has gone out, is refused as for a deleted one (by reading, not run) | Bring the cells in, then apply | Someone undoes past a save with the camera far away |
| Each author undoes through its own log, so undoing one author's set puts its old value back over another author's later set | Refuse an undo where the field no longer holds what it set, unless forced | Two agents, or an agent and a scene file, edit the same fields |
| The scene watch loads a saved file's old and new text and diffs them on the main thread: 100,000 sprites would hold a frame over a second | Load and diff on a thread; apply at a later frame | Large scene files are watched |
| What a layer queues as it attaches applies at the first Play alone: Stop empties queues and snapshots hold none, so a second Play of the sandbox has no swarm | Hold queues with the world as edited, restoring them at Stop, and snapshot them | A project's game queues as it attaches |
| A game's own Inspector edits at its ticks: while stopped, its edits wait for Play, whose first tick drops them | Apply them between ticks through the context's authors | Someone edits a stopped windowed game through its own panels |
| What a game's own Inspector creates or duplicates isn't the open scene's: a save leaves it out, and the scene doesn't count as unsaved | Apply its edits through the authors, as the row above says | Someone builds a scene through those panels |
| An append not right after its edit (only hand-written recordings or peers give one) rolls back that edit's sets whole when refused, over later ticks; a refused rollback leaves it applied (`ops.kept`) | Refuse an append unless its author applied just before | Inputs come from outside the engine |
| The open scene keeps the IDs of its gone entities, for undos, until New or Open, and snapshots keep them though a restored run can't bring them back | Drop them on restore, and as the log releases | A session deletes thousands of a scene's entities |
| In a SyncTest run, the Inspector copies its whole log at each tick with commands, and again for the rerun | Share entries between the log and its copies | A SyncTest or rollback run is edited at length |
| Changing a model's root, and its undo, redo and rollback, respawn its nodes under their IDs but new handles: code holding one holds a dead handle, and what code gave the nodes is gone | Vacate the nodes in the inverse and put them back, as a delete does | Code holds nodes' handles or puts entities under them |
| Several cameras end up primary (`engine.camera_tie`, the lowest ID wins): an undo or redo putting one back clears no others, nor does a scene or prefab that loads one | Clear the others as a primary comes back or loads, keeping them in the inverse | Authors or code switch cameras, or a prefab brings one |
| A delete leaves references to its entities dangling until undone, and a save refuses any component naming a gone entity (`ecs.no_entity`): a despawned target blocks saving | Save such a field as no entity, warning, where its type can hold none | Components hold despawnable entities |
| A scene file's edit that links an instance again, or to another prefab, deletes its own entities holding IDs the link takes, and what's under them | Move what's under them to their parent first, as a refit does | Someone reverts an unlink in git |
| `set_field` and operations store a dead entity handle without complaint | Refuse dead handles; map handles to IDs to replay a log into another world | Edits are sent to another world, as a server's or a peer's |
| The operation log keeps a session's entries, a delete's components and slots among them, until cleared | Bound it | A session's memory shows it |

## Tools

| Defect | Fix | Worth it when |
|---|---|---|
| A game built before `hello` named its clients refuses every newer `gg` with `api.unauthorized`, blaming the token | Say `hello` again without the name when refused | Someone keeps such an old game running |
| `gg run` watches every file in the folders of the game's packages but only its own `.rs`: an AGENTS.md edit rebuilds, an `include_str!` file's doesn't | Watch what cargo's dep-info and `rerun-if-changed` name | A game reads files as it builds, or stray rebuilds get in the way |
| An edit reached only through a game's own trait object or function pointer, or in code only `main`, `load` or `restored` call, is patched (`patched` says so) but changes nothing until a rebuild or a rerun | A `gg` command that rebuilds the running game | Agents edit such code |
| An item a macro declares inside a function body (`bitflags!`) isn't compared, so its change is patched, and new code reads old values | Rebuild for any change to a declaring macro's invocation, or compare expansions | A game declares types through macros in bodies |
| Each patch stays loaded until the game ends (1.7 MB for one of the sandbox's), with the jump table it replaced, as subsecond can't tell when old code stopped | `gg run` rebuilds after so many patches | A long session's memory shows it |
| `gg run` spots an edit inside `attach` by the build's text alone: each later patch of the module warns `gg.attach_changed` again, and a rebuild warns of none | Compare `attach` with what the processes run, and with the old build's at a rebuild | Agents miss the warning, or its repeats mislead |
| On Windows, `sessions` inherits its parent's ACEs: where others may create files, they can plant a session file naming their server, which `gg` and the editor send the token and requests to | An owner-only DACL; skip session files the user doesn't own | Other users can write beside the executable |
| What a run waits on, which `ping` answers, is per process, and two API tests share one: one can read `menu` for `breakpoint` and fail | Have those tests hold one lock, or keep it per API | It fails CI |
| A game hung, or in a modal loop, outlives a killed `gg run`: `--supervised` ends a run at stdin's end only as a frame starts | A job object on Windows, `PR_SET_PDEATHSIG` on Linux | Hung games are found after their `gg run` is gone |
| Breakpoints and pauses outlive their client: an agent that sets one and ends leaves the game stopped for good, and `ping`'s `breakpoint` keeps `gg run` from restarting it | A lease on a client's stops, or a stop that resumes once no `debug` comes for a while | An agent leaves a game at a breakpoint |
| Outside Windows, a client that shrinks the POSIX shared memory its `view` named crashes the game with `SIGBUS`: any user process can, one granted `view` alone too | A sealed memfd (`F_SEAL_SHRINK`) on Linux; elsewhere memory the game makes | Clients granted less than everything run on Linux or macOS |
| A client proves it holds the token, but the game doesn't: after a crash leaves its session file, whatever next listens on its port gets the token and every request, patches included | A mutual challenge in `hello`, a keyed hash of nonces (HMAC-SHA-256, BLAKE3) | Untrusted processes can listen on the loopback |
| The tool API's token and `gg run --peers 2`'s session code are SipHash outputs under std's `RandomState` keys, a best effort, not a secure source | Use the OS's secure source: snow's resolver for the token, getrandom for the code | A security review asks, or the OS's randomness is weak |
| A game loading for longer than the hang deadline (10 s) is restarted as hung; `--hang` lengthens or disables it, but a game can't say it's busy (from reading) | A busy note from the game to `gg run` | Development builds load for long |
| A mutating call that times out may still apply: its error says so, but a retry applies it twice | IDs for mutating calls, answered again rather than reapplied | Agents retry calls that time out |
| A game answers from its first frame's start, before any layer's update, so a read right after a restart (`ui`, `status`) can show the run before it, a HUD's count as its file's "0" | Answer from the second frame, or have `gg` wait for a drawn frame | An agent reads straight after a restart |
