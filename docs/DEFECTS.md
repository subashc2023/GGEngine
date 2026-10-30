# Known defects

Wrong behavior of code that exists, left in for now, whether its fix waits for a phase or for a
need: each says what it costs, what the fix would be, and when the fix would be worth it. Work not
built yet is in [PLAN.md](PLAN.md)'s Deferred. [AGENTS.md](../AGENTS.md) says how to keep this
file, under Workflow.

## Performance

| Defect | Fix | Worth it when |
|---|---|---|
| While a file dialog is open, the main thread keeps about 80% of a core busy on Windows: 2.3 to 2.6 s of CPU over 2.7 to 3.2 s, against 0.9 s for the same dialog shown outside winit. `Context::pick_file` blocks the frame that asks, inside a winit callback, so winit, finding its handler taken, asks for a paint again at each `WM_PAINT` the dialog's modal loop passes it, and the loop never idles. Windows' window menu, which the UI's title bar opens, spins the same way. On Linux, where the portal's call blocks without passing the window its events, a compositor may say the app isn't responding (not checked) | Show the dialog on a thread of its own, as rfd's `AsyncFileDialog` does on Windows, with frames going on behind it and the path it picks reaching the layer a frame or more later; for the window menu, a window procedure in front of winit's that answers `WM_PAINT` while a callback runs, or a fix in winit | The spin shows, as on a laptop's battery or fan, or a Linux compositor flags the editor |
| On Windows, while the window menu of a window the OS frames, as a game's is, is open, from Alt+Space or a right-click on its title bar, no frame runs, and nothing restarts the clock as it closes: the frame after counts the whole time the menu was open, its update gets that as its `dt`, it runs the most ticks a frame runs and drops the rest, and `engine.perf` counts it as one frame. With the player's menu held open 1.4 s, `frame_ms_max` was 1426 to 1437 ms and 83 or 84 ticks dropped, against 17 to 35 ms and none without the menu. The menu's loop passes on the paint of an invalidated window, but not the internal paint that winit makes of each frame's request for the next, which Windows' move loop passes on | A subclass of the window like `Chrome::InWindowsFrame`'s, which invalidates the window after each paint from `WM_ENTERMENULOOP` to `WM_EXITMENULOOP`, so that frames go on; short of that, one that restarts the clock at `WM_EXITMENULOOP`, as `show_window_menu` does after the UI's menu, so that the simulation pauses with the frames instead of jumping | A game in the OS's frame ships on Windows, or a game can't take a frame that long, as a networked one can't |
| On Windows, a message a driver sends one of the main thread's windows from the render thread can hold a frame up for about 16 ms: the main thread, waiting for the renderer, answers what's sent each time a millisecond's wait times out, which Windows' timer tick stretches that long | A Win32 event the render thread sets, waited on with `MsgWaitForMultipleObjectsEx`, which wakes for both | A driver's sent messages show as stalls, such as while a window resizes |

## Rendering

| Defect | Fix | Worth it when |
|---|---|---|
| A swapchain without an sRGB format falls back to the first one offered, where colors come out too dark | Encode colors to sRGB in the shaders that draw into a swapchain image of another format | A device offers no sRGB swapchain format |
| A swapchain's semaphores are destroyed once the device is idle, which doesn't cover presents, whose waits on them could still be pending; closing torn-off windows makes it more frequent | Wait for present fences (`VK_KHR_swapchain_maintenance1`) before destroying them | Validation reports it, or windows open and close often |
| The `gpu.pipeline_cache` warnings, for saved data that's damaged or that the driver fails to load, name the GPU but not the file, since gg-gpu is given the bytes alone | Pass the file's path, as a label for the warnings, through `Settings::pipeline_cache` to `Device::load_pipeline_cache` | Someone needs to find the file |

## Windows and the dock

| Defect | Fix | Worth it when |
|---|---|---|
| egui's first pass of a new floating group sizes it at its 600 by 400 point default and keeps it inside the dock's area by that size, so a group restored or floated with its top left less than 600 points from the window's right edge, or 400 from its bottom, moves left or up, and is saved there | Give a new group's `Area` its saved size, through `default_size`, before its first pass | Someone floats a group near the window's right or bottom edge |
| On Windows, an inner size asked of winit once a window whose title bar the UI draws is open (egui's `InnerSize`, `MinInnerSize` or `MaxInnerSize`) leaves its inside a frame smaller, since winit sizes it as though it had no frame | Add the frame to the size asked, as `AdjustWindowRectExForDpi` measures it | Something resizes such a window from code |
| On Windows, a maximized window whose title bar the UI draws covers the whole monitor, which may keep a taskbar that hides itself from showing | Leave such a taskbar a pixel or two beside the window, as Windows Terminal and Chromium do | Someone hides their taskbar |
| A torn-off window opens at its saved position even where that's off every monitor | Move a saved position that's off every monitor onto one | Someone loses a window after their monitors change |
| A torn-off window that opens on a monitor of another scale than the main window's is placed at the main window's scale, away from the cursor | Place a window as it opens by the scale of the monitor it opens on | Someone docks across such monitors |
| In a frame egui runs twice, the torn-off windows' texture changes apply before all of the main window's, so glyphs added then to a font atlas that grows can go missing | Apply the changes in the order egui made them, which needs egui to hand over each of the main window's passes' changes apart | It shows, or egui hands over each pass's changes apart |

## Input

| Defect | Fix | Worth it when |
|---|---|---|
| A click that closes a menu reaches what's under it too: the game's view, central or in a panel | Keep a press from the layers while one of egui's menus is open, as the title bar keeps one from the OS | It surprises someone, or Phase 9's editor |
| In games, a press in the central area within egui's 5-point interact radius of a widget at its edge, a splitter or one of the title bar's buttons, reaches the layers and arms that widget too: egui-winit keeps a press by the areas under it, not by that radius | Count as egui's a press that one of its widgets senses within the radius | A press at the edge of a game's view surprises someone |
| A modifier key pressed before the layers could see it, while one of egui's widgets had focus, which egui keeps, or while another window had the keyboard, such as a torn-off panel's or another app's, counts only from its next press, or from its first repeat where the OS repeats modifiers, as Windows does | Give the layers a press of each modifier held as they gain the keys, from winit's modifier state | Someone holds one first |
| The layers below one that handles a stick's or a trigger's move away from rest keep what they last saw of it until it rests, so that a stick rolled from right to up in a menu leaves the game going right | Pass the layers below the ways a stick or a trigger leaves, as a release goes past a layer that handles it | A game's menus take a stick |

## Recordings

| Defect | Fix | Worth it when |
|---|---|---|
| A command nested deeper than 64 levels, the most a replay reads, is recorded, so its replay refuses the whole recording, at that command's line (`replay.syntax`): the recorder ends a recording before a command only as its queue's `check` says, and neither a `Vec`'s, which makes the command, nor the Inspector's, which checks nothing, counts its levels | Count a command's levels as it's recorded, ending the recording before one nested deeper with `record.bad_command`, as before one its queue's `check` refuses | A command nests that deep, or the tool API takes values from outside |

## Reflection

| Defect | Fix | Worth it when |
|---|---|---|
| A bound whose text starts with `0`, as `min = 0.5`, on a field of an array type, as `[f32; 3]`, fails `clippy -D warnings` with `zero_prefixed_literal`: the derive's check that a bound suits its field's type places the type's tokens at the bound, so that an error shows there, the array's length among them, and clippy reads the bound's text as the length's | Place only the check's own tokens at the bound, leaving the type's literals where they're written | A game bounds an array field so |

## Snapshots

| Defect | Fix | Worth it when |
|---|---|---|
| A restore whose only change to the world is an enum's variants reordered, or one inserted before others, warns `snapshot.hash` that the world goes on otherwise than it would have, though every value came back: a snapshot names a variant, which a restore matches by name, while the hash digests its place, and nothing in the file says where it was | Write a digest of each component type's fields' and variants' names and types, in order, beside its tables, and check the hash only where every type is as it was | The warning misleads someone after such a rebuild |

## Physics

| Defect | Fix | Worth it when |
|---|---|---|
| A body that turns for minutes drifts off unit length: a step multiplies its rotation by the step's turn without normalizing it, which in a model of it passed what a rotation may be after 2 to 5 minutes at 3 to 7 radians a second, so that a scene saved of the world is refused (`reflect.not_normalized`), and a snapshot normalizes it, with a note | Normalize the rotation as each step turns it, writing physics' reference hash again by hand | A game's bodies turn for long, or a game saves scenes as it plays |

## Layers

| Defect | Fix | Worth it when |
|---|---|---|
| `Layer::attach` can't fail, so a layer whose shader doesn't compile, or whose texture or model doesn't load, panics, as the sandbox does, where the engine's own errors end the run with their codes | `Layer::attach` returning a `Result`, whose error ends the run as the engine's own do | A game loads assets as it starts |

## Files

| Defect | Fix | Worth it when |
|---|---|---|
| A scene file, the dock's layout or the pipeline cache saved over a file with other hard links replaces that file rather than writing through it, so the other links keep what it held: writing whole renames a new file over it, which a link elsewhere doesn't follow | Write through a file with other links, in place, giving up writing it whole, or say so as it saves | Someone keeps scenes behind hard links |
| A model reads outside its folder through a symlink inside it: its URIs can't leave the folder, but a link there is followed | Resolve a file's links before reading it, and refuse one that leads out of the model's folder | Games load models from sources they don't trust, such as players' mods |
| A JPEG cut short in its last row of MCUs, its bottom 8 or 16 rows of pixels, can still decode, without an error, the blocks it lost decoded from zeros: zune-jpeg's strict mode, which `decode_jpeg` turns on, sees a scan run short only as the next row starts or where what's left won't decode, and the zero bytes it reads past the end can finish the last row. `image.rs`'s 16x16 test JPEG, all one row, still decodes cut by 3, 10, 20, 60 or 100 bytes, while every cut tried of the sandbox's 2048x2048 truck texture, from its last 2 bytes to 100,000, fails as `engine.bad_texture` | Refuse a JPEG with no end-of-image marker after its last scan, which `decode_jpeg` can look for in its bytes, since entropy-coded data can't hold one; or zune-jpeg's own check, should a release add one | Phase 7's asset hot reload, which can read a texture's file half-written, or a game with textures small enough for a cut to land in their last row |

## Editor

| Defect | Fix | Worth it when |
|---|---|---|
| The File menu's shortcuts, Ctrl+N, Ctrl+O, Ctrl+S and Ctrl+Shift+S, do nothing while a torn-off window has the keys: the editor reads them from the input of the main window, which its `ui` runs in, and a torn-off window's keys go to that window's own. The Inspector's Ctrl+Z works there, since it reads each window's input as its panels draw in it, and the File menu, in the main window, works from any | A `Ui` call that takes a shortcut's press from whichever window has the keys, as the Inspector's panels take theirs | Someone works in torn-off windows, or another layer's shortcuts need the same |
| An entity flipped by its own negative scale shows its handles along its rotation alone, though a parent's flip turns them: a sprite flipped by its scale shows its X arrow away from where its +X draws | Turn the handles by the signs of the entity's own scale too, as by its parent's | A game flips sprites by their scale, and the handles confuse someone |
| An Entities row whose name has a line break is as many lines tall, though the list skips the rows out of view as though each were one line tall, as egui's `show_rows` takes them | Rows of one line, a name's line breaks shown as spaces | A world of more entities than the panel shows, or names from players |
| A scene dragged onto the view from the Assets panel torn off into a window of its own opens nothing: egui's drag stays in the window it started in, so the view never sees it let go | Follow a drag out of the window it started in, so that the view takes its release | Someone drags from a torn-off Assets panel |
| The Inspector sets a rotation whole, and a vector inside an option or a variant, as the panel last read it, after the ticks of the layers below it: turning one angle of something that spins, as the sandbox's triangle does, holds the others where they were, and an undo takes back the spin made since too | Set only the part that changed | Someone edits a rotation, or a vector held so, as it moves |
| After an undo or a redo, above 64 frames a second, a field shows its value from before for a frame or so, and a refusal and the Edit menu's names come at the tick: the Inspector draws a set waiting for its tick as the set will make it, but not an undo or a redo | Draw an undo or a redo waiting for its tick as it will make it, as a set is drawn | It shows, or Phase 9's editor in another process |

## Edits

| Defect | Fix | Worth it when |
|---|---|---|
| A scene file's save reparents its entities in the order of their IDs, so one that swaps a parent and its child, or otherwise moves an entity under one that's under it until a later reparent, is refused whole with `ecs.parent_cycle`, though where it ends is a tree | Make each entity moved a root first, then reparent each into place, parents first | Someone restructures a hierarchy in a scene file while the game runs |
| The authors' logs, the tool API's clients' and the scene files', start afresh as another world takes the world's place, as the editor's Play and Stop swap it, so an agent's undo after Play and Stop finds nothing, and the slots their entries kept stay unused in the world they left. And while a layer holds a world aside, as the editor does as it plays, a scene file's save applies to the world that plays, which a snapshot doesn't save, though the file's text as last applied moves on, so a restored run never applies it | Keep the logs aside by the world's serial, as the Inspector keeps its log; apply a scene file's save to the world held aside, or keep its text as last applied for each world | Agents edit a game in the editor, or the editor watches scene files, in Phase 9 |
| Each author undoes through a log of its own, so an undo of one author's set of a field that another author set since puts back the first's old value over the second's | An undo refused where a field no longer holds what the edit set, unless asked to force it | Two agents, or an agent and a scene file, edit the same fields at once |
| The scene watch loads a saved file's old and new text into worlds of their own and diffs them on the main thread, as a frame starts: a scene of 100,000 sprites, which loads in about 650 ms in release, would hold that frame for over a second | Load and diff on a thread of their own, applying the operations as a later frame starts, against the world as it is then | Large scene files are watched |

## Tools

| Defect | Fix | Worth it when |
|---|---|---|
| A game built before the tool API's clients named themselves in `hello` refuses every connection of a `gg` built since, with `api.unauthorized`, which blames the token: its `hello` takes no field but the token | Say `hello` again without the name when it's refused, its edits then the unnamed client's, `api` | Someone keeps a game built before then running, which a rebuild, as `gg run` makes, ends |
| `gg run` removes the copies of a game's executable that other runs left as it starts, but for those Windows holds while a game runs from them; so one that another `gg run` of the game has just made, and not yet started its game from, goes too, and that run fails with `gg.run` | Hold a lock on a file beside each copy for the run's life, with `File::try_lock`, and remove only the copies whose lock is free | Two runs of one game start at once, as a networked game's peers may |
| Two `gg run`s of one package at once share dx's session cache and executable, so one's rebuild changes what the other's patches are built against, and they break | A session cache and a target directory for each run | Two runs of one game at once, as a networked game's peers on one machine may be |
| `gg run` watches every file in the directories of the packages a game builds with, and only the `.rs` files of its own: an edit to a crate's AGENTS.md rebuilds the game, and one to a file its crate reads as it builds, as `include_str!` does, isn't seen | Watch the files cargo's dep-info says each crate's build read, and those its build scripts' `rerun-if-changed` name | A game reads a file as it builds, or rebuilds for files no build reads get in the way |
| An edit that only a game's own trait object or function pointer reaches, or that's in a function of its own that only `main` or a layer's `load` or `restored` calls, which ran as the game started, is patched, `patched` saying so, but changes nothing until a rebuild, which only another kind of change brings | A `gg` command that rebuilds the running game | A game keeps trait objects or function pointers of its own whose code agents edit, or agents edit what its `load` calls |
| An item that a macro invoked in a function's body declares, such as a type through `bitflags!`, isn't compared, so a change to it is patched, and code laid out anew would read its values that the game made before | Rebuild for any change to a macro's invocation in a body that declares items, or compare what macros expand to | A game declares types through macros inside function bodies |
| Each patch a game takes stays in its memory until it ends: subsecond keeps the patch's library loaded and the jump table it replaced, since code of either may still be running, so the game grows at every patch by its library, 1.7 MB for one of the sandbox's, and its table | None in subsecond, which can't tell when old code has stopped; `gg run` could rebuild after so many patches, as a restart frees them | A long session's memory shows it |
