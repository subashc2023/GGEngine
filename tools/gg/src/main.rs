//! `gg`, the command-line tool for GGEngine's games (ARCHITECTURE.md's Agents and tools): it
//! builds, runs and supervises them, checks, ships, fetches a project's missing files, talks to
//! running games through the tool API, and makes new games. In a game's project, it builds and
//! reaches that project's games alone. It prints JSON, a line at a time.

#[cfg(test)]
mod agents_md;
mod build;
mod classify;
mod dx;
mod fetch;
mod game;
mod mcp;
mod new;
mod patch;
mod probe;
mod run;
mod runs;
mod settings;
mod ship;

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use gg_api::{Failure, Project};
use serde_json::{Value, json};

use game::Pick;
use run::{Check, Run};
use ship::Ship;

/// `gg help`'s head.
const OVERVIEW: &str = "\
Usage: gg [options] <command> [options]   options before or after it: `gg --pid 7 status`
       gg help [<command>]               a command's options, output and codes

In a game's project (the directory of its `project.ron` beside its `Cargo.toml`, or one below it),
or the one `--project` names, gg builds the project's game, runs cargo in its directory and talks to
its games alone. Building that game, `run`, `check` and `ship` first check that it's the package
beside `project.ron` (`gg.project`). A manifest that doesn't read is an error. Elsewhere, gg works
on the whole workspace.
";

/// `gg help`'s tail.
const OPTIONS: &str = "\
Options for talking to a game:
  --pid <PID>          The game in process PID, when several run (else `gg.which_game`, naming
                       each); a lockstep session's peers are reached through the host, player 0,
                       whose edits and patches apply to both
  --app <NAME>         The game whose app is NAME, when several run
  --timeout <SECONDS>  How long to wait for its answer (30)
  --grant <LIST>       Keep the connection to these capabilities, not all: as `read,view`, which
                       the API's `hello` takes
Option for every command but `new`:
  --project <PATH>     Work in this project, by its directory or its `project.ron`

Each result is a line of JSON on stdout; an error, {\"error\":{\"code\":...,\"message\":...}} on stderr,
and a warning, {\"warning\":{\"code\":...,\"message\":...,\"path\":...}}, as `gg.protocol` where a
game speaks another revision of the tool API than gg, naming its session file. Exits 0 on success;
1 on an error, a check that failed or a run that didn't end cleanly; 2 on bad arguments.
";

/// A command's help: `gg help` lists name and summary, `gg help <name>` adds usage and details.
struct Help {
    name: &'static str,
    /// What follows `gg `; later lines are indented under its options.
    usage: &'static str,
    summary: &'static str,
    /// Empty where the summary says it all.
    details: &'static str,
}

/// The commands' help, by group, in `gg help`'s order.
const COMMANDS: &[(&str, &[Help])] = &[
    (
        "Build and run",
        &[
            Help {
                name: "run",
                usage: "\
run [PACKAGE] [--release] [--no-patch] [--hang <SECONDS>] [--snapshot <SECONDS>]
              [--peers <N>] [--embedded] [-- <ARGS>...]",
                summary: "Build and run the game, supervised: edits patch in or restart it from a snapshot",
                details: "\
Builds PACKAGE's development build (the project's game by default; `built`, with its time) and runs
a copy with ARGS, the game's own flags. Each game gets `--supervised` and a stdin gg holds, so it
ends when gg does, however gg ends. A second `run` of PACKAGE is refused (`gg.running`).

  --release             Optimized, which no patch reaches
  --no-patch            No patches, no watching: a rebuild of the executable restarts it (`rebuilt`)
  --hang <SECONDS>      Frames stopped this long (10), while the game waits for nothing the user
                        does, are a hang: gg ends it. The first frame gets 120 s. 0: never hung
  --snapshot <SECONDS>  Snapshot the world this often (5) from the first frame. 0: no restarts
  --peers 2             A lockstep host (`--host`) and joiner (`--join`) from one build, at a free
                        loopback port, with a `--code` gg makes; each `started` names its `player`.
                        They patch together, and stop and restart together as either ends or the
                        game is rebuilt
  --embedded            For the tool that started gg, such as the editor: the game gets
                        `--embedded`, offscreen, the tool showing its view. When gg's stdin closes
                        (`closed`), after any build under way, gg stops the game and ends cleanly
  -- <ARGS>             A headless game given `--real-time` ticks in real time, to be watched

Events, a JSON line each (the game's and cargo's output go to stderr): `started`, `serving` (the
tool API), and as it ends `exited`, `crashed` or `hung`, with the errors its log holds.

Restarts, from the last snapshot (`snapshot_failed` says why one fails): after a crash or a hang;
after a rebuild (`rebuilt`), from a snapshot taken in the same request as the quit, so no edit is
lost between them; and at once (`killed`, with what it waits for) for a game waiting where it can't
answer: at a script's breakpoint, in a file dialog or in the window menu. A game that ends before
its first snapshot waits for a rebuild (`waiting`); one that refuses its snapshot starts afresh
(`refused`). Pauses and stops last across restarts. Only the world, its entities' components,
survives; a layer's own fields and statics start over. A layer's `load` makes its assets; its
`attach`, which a restart skips, builds the world.

`run` ends as the game comes to its own end (its window closes, `quit` asks or its frames run out),
whatever its status; as a game that never served the API ends; and, with restarts off
(`--snapshot 0`, or `--record`, `--replay` or `--no-api` in ARGS), as the game ends at all.

Patches need dx (Dioxus's tool) on the PATH, of the root Cargo.toml's `subsecond` release. They're
off with `--no-patch`, `--release`, restarts off, or a library beside PACKAGE's binary, which no
patch reaches (`patches_off` says why). gg builds with `dx build --fat-binary`, printing each stage
and, while compiling, the crates done of dx's count, every 5 s at most (`building`). It watches
PACKAGE's modules, the workspace's packages it builds with, and the workspace's manifest and lock
file (`watching`). Then:
- an edit inside function bodies of PACKAGE's modules is a patch (`dx tools hotpatch`), applied
  between ticks: `patched`, or `patch_failed` with rustc's errors, the game running on;
- any other change rebuilds with dx (`rebuilding`, with the file and why, then `rebuilt` or
  `rebuild_failed`): a change to a type, even one declared in a body, or to code run as the game
  started (`main`, a layer's `load` and `restored`), and any change after a failed rebuild;
- an edit inside a layer's `attach` patches (`patched` names its module in `attach`), the game
  warning `gg.attach_changed`. It shows only in a run started afresh, as restarts keep the world.
A game waiting for a rebuild gets one at any change; a restarted one gets the edits since its build
patched in at once. A patch never answered (`patch_unanswered`) is sent again with every edit since
the build. The game logs failures as warnings too: `gg.patch_failed`, `gg.rebuild_failed`. Once the
sources match the running build again after a failed rebuild, `recovered` says so, and the failure
leaves the run's state.

Without dx, or where patches can't reach (as with `--release`), with restarts on and no
`--no-patch`, gg builds with cargo and watches all the same (`watching`, `patches` false): every
change rebuilds and restarts from a snapshot, rustc's errors in `rebuild_failed`. A failed first
build waits for a change (`build_failed`).

While no game answers, the commands that talk to it fail with what `run` is doing: `gg.rebuilding`,
`gg.starting`, `gg.waiting`, or `gg.build_failed` with the last build's errors. `gg wait` waits
until the game runs an edit's code.
",
            },
            Help {
                name: "check",
                usage: "check [PACKAGE] [--release] [--frames <N>] [--timeout <SECONDS>] [-- <ARGS>...]",
                summary: "Build the game and run it headless: its status, warnings, errors and frame times",
                details: "\
Builds PACKAGE (the project's game by default) and runs it headless with ARGS for N frames (60),
then prints its exit status, its warnings and errors, and its frame times. A run that takes longer
than `--timeout` (600 s) is ended. First it warns on stderr (`gg.settings`) where PACKAGE's
settings differ from the engine's: its clippy.toml lacks a method the engine's bans, or its
rust-toolchain.toml pins another Rust release.
",
            },
            Help {
                name: "ship",
                usage: "ship [PACKAGE]",
                summary: "Build the game as it ships, with its cooked package, in `ship/PACKAGE`",
                details: "\
Builds PACKAGE (the project's game by default) without `dev`, in the shipping profile, and cooks its
project, with the cooker of the engine it builds against, into its package beside its executable.
Lays them out in `ship/PACKAGE` in the target directory, emptied first, named for the game with
PACKAGE capitalized (`Shards.exe`, `Shards.ggpk`), with NOTICES.txt: the project's CREDITS.txt and
the licenses of the crates it's built from, which cargo-about 0.9.2 writes from about.toml
(`gg.notices`).
- Windows: side by side, with the Visual C++ runtime from Visual Studio's redist (`gg.no_redist`).
- Linux: side by side; GPUs' drivers install the Vulkan loader.
- macOS: an app bundle (`Shards.app`) with the project's icon, signed ad hoc (`gg.sign`), with the
  loader and KosmicKrisp of the Vulkan SDK 1.4.363.0 or later, whose `macOS` folder `VULKAN_SDK`
  names (`gg.no_sdk`, `gg.sdk_version`).
Symbols go in `ship/PACKAGE-symbols`, with build.txt naming the build (`gg.symbols`): the PDB on
Windows, the unstripped executable (objcopy) on Linux, a dSYM (dsymutil) on macOS. Prints a line
naming the folders and each file in them, with sizes.
",
            },
            Help {
                name: "fetch",
                usage: "fetch",
                summary: "Fetch the files the project's `fetch.ron` lists, which its repository doesn't hold",
                details: "\
Fetches each file the fetch list (`fetch.ron`, beside `project.ron`) names, as models whose licenses
keep them out of the repository: with curl, from a folder at a URL pinned to what it holds, into a
folder of the project the repository ignores. Each must match its SHA-256 before it takes its place
(`gg.fetch`); one there as the list has it is kept. Each that's an asset under the asset root (a
texture, model, font, prefab or UI) gets an ID file holding the ID the list gives it, or else, if
it has no ID file, a fresh one, as a development run would. Prints a line per file, `fetched` or
`kept`, with the `id` it wrote, then one for the list.
",
            },
        ],
    ),
    (
        "Make a game",
        &[Help {
            name: "new",
            usage: "new <NAME> [--engine <SOURCE>]",
            summary: "Write a new game, in the engine's workspace or in one of its own",
            details: "\
Writes over nothing: package NAME, whose binary `gg-NAME` is a development build of the engine's by
default; its project, whose start scene `scenes/main.ron` holds a camera and a square; and its main
module, whose layer moves the square by the arrow keys, WASD or a pad's left stick. Prints a line
naming its directory, its files and `next`, the command that runs it.

In the engine's workspace, the game is a member in `games/NAME`. Elsewhere it's a workspace of its
own in `NAME`, built against the engine `--engine` names: a checkout's directory, or
`<GIT URL>#<REVISION>`. It gets copies of the engine's `.cargo/config.toml` (less its `cargo gg`
alias), `clippy.toml`, `rust-toolchain.toml`, `about.toml`, `.gitattributes`, `.gitignore` and
`Cargo.lock`, and an AGENTS.md and CLAUDE.md pointing agents at the engine's docs. NAME can't be a
package in the engine's lock, as `--package NAME` would name both.
",
        }],
    ),
    (
        "Talk to a running game, through its tool API",
        &[
            Help {
                name: "sessions",
                usage: "sessions",
                summary: "List the running games",
                details: "",
            },
            Help {
                name: "status",
                usage: "status",
                summary: "Its ticks, pause and stop state, frames, entities, view and open scene",
                details: "\
- `scene`: the open scene's file.
- `unsaved`: whether the scene differs from its file, as a playing game's ticks make it.
- `edited`: whether authors edited it since it was saved or opened.
- `cells`: in a world of cells, how many are in, and how many out keep what they held.
",
            },
            Help {
                name: "wait",
                usage: "wait [--timeout <SECONDS>]",
                summary: "Wait until the game under `gg run` runs the code of every edit saved before",
                details: "\
Waits at most `--timeout` (300 s) for the edits to be patched in or rebuilt, then prints what `run`
is doing. Fails with:
- `gg.build_failed` and rustc's errors, if the edits don't build;
- `gg.waiting`, if the game ended and waits for a change;
- `gg.no_run`, without a `gg run`; `gg.which_run`, with several, which `--pid` tells apart;
- `gg.timeout`.
",
            },
            Help {
                name: "pause",
                usage: "pause",
                summary: "Pause its simulation",
                details: "",
            },
            Help {
                name: "resume",
                usage: "resume",
                summary: "Run its simulation in real time again",
                details: "",
            },
            Help {
                name: "play",
                usage: "play",
                summary: "Play a copy of its world as edited, in its place, until `stop`",
                details: "A game already playing is left as it is.\n",
            },
            Help {
                name: "stop",
                usage: "stop",
                summary: "Stop it: no tick runs, and its world is the world as edited",
                details: "\
Only edits, scene files' saves, `scene open` and `scene new` change the world as edited. While a copy
plays, `stop` brings the world as edited back; what the copy did, its edits among them, goes.
Recorded runs and lockstep peers refuse it.
",
            },
            Help {
                name: "step",
                usage: "step [N]",
                summary: "Run N ticks (1) and stay paused; refused while it's stopped",
                details: "",
            },
            Help {
                name: "scene",
                usage: "scene open <PATH> [--discard] | scene save [PATH] | scene new [--discard]",
                summary: "Open, save or empty the open scene",
                // Not `"\`, which would take the first line's indent.
                details: "  open <PATH>  Open the scene file at PATH, in one of the project's scenes folders: its
               entities in place of the open scene's, the game's own kept
  save [PATH]  Save the open scene's entities, never those the game's code made, to its file, or to
               PATH, its file from then on
  new          Delete the open scene's entities, leaving it untitled
`open` and `new` are refused while the scene has unsaved edits (`scene.unsaved`), unless
`--discard`. They start every author's edits afresh.
",
            },
            Help {
                name: "capture",
                usage: "capture [--own] [--show <NAME>[,<NAME>...]]... [PATH]",
                summary: "Write its view as a PNG, or a part of its shading or an image in place of its light",
                details: "\
Writes the view as it's shown, the look included, at PATH, or in `captures` beside the game, which
keeps the newest 32.
  --own        As the game shows it on its own
  --show NAME  That capture alone shows NAME in place of the view's light, as the look's `show`
               does, each pixel's bytes as gg-api's `Shown` says: a part of shading, or else an
               image's debug name (`--show \"view depth\"`). Several, as `--show base_color,normal`
               or repeated, are captured in order from one world, pausing the game for them if it
               plays, and print an array of the answers; PATH refuses them
Parts of shading: `base_color`, `normal`, `surface_normal`, `metallic`, `roughness`, `occlusion`,
`emission`, `specular_color` (what it reflects head-on), `direct_light`, `sky_light`, `reflection`,
`diffuse_light` and `specular_light` (the light it diffuses and reflects, of all three).
",
            },
            Help {
                name: "frames",
                usage: "frames <N> [--viewpoints <FILE>] [--rect X,Y,W,H] [--show <NAME>] <DIR>",
                summary: "Write its next N frames in a row, none skipped, as PNGs in DIR",
                details: "\
Writes `frame-0001.png` on, from the first frame with nothing on its way to the GPU. N is 1 to
1000, holding at most 1 GiB of pixels. Prints each frame's PNG, size, ticks and viewpoint. Pause the
game to see the view alone change.
  --viewpoints <FILE>  Draw each frame from the next viewpoint in FILE, a JSON array of them as
                       `look` takes its `viewpoint`, the last held for the frames after it; else as
                       the view's shown
  --rect X,Y,W,H       Crop each frame to this rectangle of pixels from the view's top left
  --show <NAME>        Show NAME, as `capture --show` does
",
            },
            Help {
                name: "look",
                usage: "look [<LOOK>]",
                summary: "Read or set its look: viewpoint, outline, grid, colliders, show, skip, max_fps",
                details: "\
The look is the view's settings, which any client sets; the last to set a part wins. LOOK is a JSON
object of parts, as the API's `look` takes them, set before the look is printed; `null` gives a part
back to the game:
- `viewpoint`, `grid`, `colliders`;
- `outline`, round entities and every entity under each: a band `width` pixels wide and a glow as
  far again, {\"outline\":{\"ids\":[ID,ID],\"color\":[1,0.5,0,1],\"width\":3}};
- `show`, what the view shows in place of its light, a name as `capture --show` takes it:
  {\"show\":\"roughness\"} or {\"show\":{\"image\":\"view depth\"}};
- `skip`, the passes the view leaves out, to narrow down which one makes what it shows:
  {\"skip\":[\"reflections\",\"shadows\"]}, of `ambient_occlusion`, `reflections`, `shadows`, `fog`,
  `bloom`, `depth_of_field`, `motion_blur`, `antialiasing`, `grade`, `lens` and `adaptation`;
- `max_fps`, the most frames a second a game embedded in a tool renders, as the editor's frame rate
  limit sets it: {\"max_fps\":10}.
",
            },
            Help {
                name: "pick",
                usage: "pick <X> <Y> [--within <ID>]",
                summary: "The entity its view shows at pixel X, Y",
                details: "\
Prints its ID and name, `null` for none. X, Y are from the view's top left, as a capture has it, the
look included. A model's node gives the model's root, but `--within` the root or one of its nodes
gives the node itself, as the editor picks parts.
",
            },
            Help {
                name: "images",
                usage: "images",
                summary: "The images its next frame makes that `--show` and the look's `show` name",
                details: "\
Each one's debug name (as `bloom 1` or `view depth`), what it holds and its size, in the order the
frame makes them. A frame showing a part of shading makes none after the view's passes.
",
            },
            Help {
                name: "probe",
                usage: "probe <X> <Y> [--own]",
                summary: "What shading took and made at a pixel, as numbers",
                details: "\
To find what's wrong at a pixel: its material's values, its normal, and which light reaches it.
Captures pixel X, Y, from the view's top left, not negative, as each part of shading, writing no PNG,
with a pick and a look, from one world, pausing the game for them if it plays. The view is as the
look shows it, or with `--own`, as the game shows it on its own. Prints one JSON object:
- `base_color`, `emission`, `specular_color` (what it reflects head-on): linear RGB, from 0;
- `normal` (with its normal map), `surface_normal` (without): [x, y, z] in the world, -1 to 1;
- `metallic`, `roughness` (from 0.045), `occlusion` (the share of the light from all around that
  reaches the point): 0 to 1;
- `direct_light`, `sky_light`, `reflection`, `diffuse_light`, `specular_light`, `shown` (the view's
  pixel): sRGB bytes [r, g, b], 0 to 255, exposed and tonemapped as the view shows them, not linear;
- `depth`: the view's reversed depth, 1 at the near plane and smaller farther, 0 where nothing drew;
- `distance`: `near / depth` in world units, in front of the viewpoint, not from it; `null` where
  nothing drew and without a perspective;
- `entity`: what `pick` answers; and `size`, `at` and `ticks`;
- `quantized`: values are read from 8-bit bytes, good to about 1/255, `distance` to about 3%.
Every value is `null` for a pixel outside the view.
",
            },
            Help {
                name: "input",
                usage: "input <EVENTS>",
                summary: "Hand its layers input events, as its window would",
                details: "\
EVENTS is a JSON array of the API's input events, handed over in order as a frame starts:
- a key down or up, by its W3C `code`: {\"key\":{\"key\":\"KeyW\",\"pressed\":true}};
- text: {\"text\":\"w\"};
- the cursor at a pixel of the view, from its top left: {\"cursor\":[X,Y]}; \"cursor_entered\",
  \"cursor_left\";
- the mouse's raw motion right and down, as with the cursor grabbed: {\"motion\":[X,Y]};
- a mouse button: {\"button\":{\"button\":\"left\",\"pressed\":true}};
- a scroll: {\"scroll\":{\"lines\":[0,1]}};
- the keys' focus: {\"focused\":true}.
",
            },
            Help {
                name: "snapshot",
                usage: "snapshot [PATH]",
                summary: "Write its world as a snapshot",
                details: "At PATH, or in `snapshots` beside the game, which keeps the newest 8.\n",
            },
            Help {
                name: "entities",
                usage: "entities [--with <COMPONENT>] [--skip <N>] [--take <N>]",
                summary: "List its entities, those with COMPONENT, N at a time (100)",
                details: "",
            },
            Help {
                name: "entity",
                usage: "entity <ID>",
                summary: "An entity and its components' values",
                details: "\
For an instance's entity, its root or one its prefab gives, `instance` too: what differs from its
prefab, each component's fields, and the components it adds.
",
            },
            Help {
                name: "get",
                usage: "get <ID> <COMPONENT> [PATH]",
                summary: "A component's value, or its field at PATH, such as `translation.x`",
                details: "",
            },
            Help {
                name: "types",
                usage: "types",
                summary: "Its world's component types, their defaults and the types they hold",
                details: "\
Each with the value `add` gives it without one, and the descriptions of the types they hold, by
which values are read and written.
",
            },
            Help {
                name: "hash",
                usage: "hash",
                summary: "Its world's hash",
                details: "",
            },
            Help {
                name: "cells",
                usage: "cells",
                summary: "Its world's cells, in a world of cells",
                details: "\
Those in, coming, kept, refused or filed; the streamed roots in each; which a scene save would
write; and the viewers they come in around, the look's viewpoint among them while it's stopped.
",
            },
            Help {
                name: "assets",
                usage: "assets [--kind <KIND>] [--skip <N>] [--take <N>]",
                summary: "List its textures, models, fonts, prefabs and UIs, with their IDs",
                details: "\
By path from the asset root, N at a time (100), those of `--kind` (`texture`, `model`, `font`,
`prefab` or `ui`): each one's ID, whether a layer has added it, and what's wrong with its ID file;
and the ID files left without their assets.
- ID files: the game, but a lockstep session's joiner, whose host does, writes each one's ID in a
  file beside it, `<file>.meta`, as it starts and as it finds new files: commit it with its file. A
  file renamed or moved alone keeps its ID, the game moving its ID file after it, unless it's edited
  in the same step.
- Reloads: an edited file of a texture, model, font, prefab, UI, sound or music the game has added
  reloads in it, as a shader's does, with the nodes its `ModelRoot`s spawn, the instances its
  `PrefabRoot`s link and the UIs of it. In a recorded run or a lockstep session, a model's nodes and
  materials and a prefab's instances change as it next starts.
",
            },
            Help {
                name: "ui",
                usage: "ui",
                summary: "Each UI its layers added from a `.ui` file, laid out: nodes' rectangles and state",
                details: "\
Each UI's focus and whether it takes input. Each node's name, kind, parent, rectangle
[x, y, width, height] from the view's top left in the canvas's units (1,080 to its height) and in
its pixels (`pixels`, which `input` clicks at, as `capture` has them), whether it shows, what it
says, what a widget holds, and whether a button or widget is disabled, focused or pressed. Press a
button by clicking its pixels with `input`; type into a text field clicked so with `text`.
",
            },
            Help {
                name: "apply",
                usage: "apply <OPERATIONS>",
                summary: "Apply operations as one edit, all or none, which `undo` takes back",
                details: "\
OPERATIONS is a JSON array of the API's operations, applied between ticks. Among them:
- {\"unlink\":{\"id\":ID}} unlinks a prefab's instance into entities of its own;
- {\"revert\":{\"id\":ID,\"component\":C,\"path\":P}} sets a field of an instance's entity back to its
  prefab's.
",
            },
            Help {
                name: "save-prefab",
                usage: "save-prefab <ID> [PATH]",
                summary: "Save an entity and its descendants as a new prefab, linked to it",
                details: "\
Writes the prefab and its ID file at PATH, in a folder under the asset root that its scan reaches,
or in `prefabs` named after the entity, writing over nothing; and links ID to it as one edit, which
`undo` unlinks, leaving the file. Its entities keep their IDs. Refused while a copy plays. Prints the
prefab's path and name.
",
            },
            Help {
                name: "set",
                usage: "set <ID> <COMPONENT> <PATH> <VALUE>",
                summary: "Set a component's field, or the whole component, as one edit",
                details: "\
PATH is a field, such as `translation.x`, or empty for the whole component. VALUE is JSON, or else a
string.
",
            },
            Help {
                name: "undo",
                usage: "undo [--author <NAME>]",
                summary: "Undo gg's last edit, or author NAME's, printing it, or `null` if there's none",
                details: "An author is an MCP client's name, `gg`, or a scene file's path, as `scenes/grid.ron`.\n",
            },
            Help {
                name: "redo",
                usage: "redo [--author <NAME>]",
                summary: "Redo the last edit gg, or NAME, undid",
                details: "",
            },
            Help {
                name: "edits",
                usage: "edits [--author <NAME>]",
                summary: "Each author's edits, or NAME's: those to undo and those to redo",
                details: "",
            },
            Help {
                name: "logs",
                usage: "logs [--level <LEVEL>] [--follow]",
                summary: "Its log's events at LEVEL (`warn`) and above",
                details: "\
Prints a line per event, as its JSON log has it, of the last 10,000 the game keeps; with
`--follow`, then each as it's logged, until the game ends. LEVEL is `error`, `warn`, `info`, `debug`
or `trace`. An agent can't hear the game: windowed and embedded runs log each sound or music
started, `Sound started` or `Music started`, at `debug`.
",
            },
            Help {
                name: "debug",
                usage: "debug [break <SCRIPT> [<LINE>...] | pause | continue | into | over | out]
                [--wait <SECONDS>]",
                summary: "Debug its Luau scripts: breakpoints, stepping, the stack and its locals",
                details: "\
Prints the debugger: where a script has stopped, with its stack and each function's locals and
upvalues; the pause or step under way; the breakpoints; and the scripts.
  break <SCRIPT> [<LINE>...]  Set SCRIPT's breakpoints, by its path from the project's root, such
                              as `scripts/spin.luau`, at each LINE or the next line with code, in
                              place of those it had; no LINE clears them
  pause                       Stop the next line a script runs
  continue                    From a stop, go on to the next
  into, over, out             From a stop, step
  --wait <SECONDS>            Wait up to SECONDS, at most 10, for a script to stop, if none has
A stopped script waits inside its tick, and the game with it. The commands but `debug`, `logs` and
`sessions` wait too, until `debug` has it go on. `gg run` doesn't take it for hung.
",
            },
            Help {
                name: "quit",
                usage: "quit",
                summary: "End it cleanly, as closing its window does, without asking",
                details: "",
            },
            Help {
                name: "call",
                usage: "call <METHOD> [<PARAMS>]",
                summary: "Send METHOD with PARAMS, as JSON, and print what it answers",
                details: "",
            },
        ],
    ),
    (
        "Serve agents",
        &[Help {
            name: "mcp",
            usage: "mcp",
            summary: "Serve the tool API to an agent that speaks MCP, on stdin and stdout",
            details: "\
A tool for each command that talks to a game but `quit`, `call`, `logs --follow`, and `capture`'s
and `snapshot`'s PATHs. Its `capture` answers the PNG's path, and with `inline` the PNG; `scene`'s
and `save-prefab`'s paths are from the game's asset root. Each call reaches the game the options
choose, kept to what `--grant` grants, as the agent: its MCP client's name authors its edits.
",
        }],
    ),
];

fn overview() -> String {
    let mut text = OVERVIEW.to_owned() + "\n";
    for (group, commands) in COMMANDS {
        text += &format!("{group}:\n");
        for help in commands.iter() {
            text += &format!("  {:<12} {}\n", help.name, help.summary);
        }
    }
    text + "\n" + OPTIONS.trim_end()
}

/// Fails with `gg.args` for an unknown name, naming the known.
fn section(name: &str) -> Result<String, Failure> {
    let all = COMMANDS.iter().flat_map(|(_, commands)| commands.iter());
    let Some(help) = all.clone().find(|help| help.name == name) else {
        let known: Vec<String> = all.map(|help| format!("`{}`", help.name)).collect();
        return Err(bad(format!(
            "there's no command `{name}`: it's one of {}",
            known.join(", ")
        )));
    };
    let mut text = format!("Usage: gg {}\n\n{}.", help.usage, help.summary);
    if !help.details.is_empty() {
        text += "\n\n";
        text += help.details.trim_end();
    }
    Ok(text)
}

/// The project gg works in: its absolute manifest path, where cargo runs, and its game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    pub(crate) manifest: PathBuf,
    pub(crate) game: String,
}

impl Scope {
    pub(crate) fn dir(&self) -> &Path {
        self.manifest.parent().unwrap_or(Path::new("."))
    }
}

enum Command {
    Help(String),
    Run(Run),
    Check(Check),
    Ship(Ship),
    Fetch(Scope),
    /// A new game's name, and the engine a game outside the workspace builds against.
    New(String, Option<new::Engine>),
    /// The running games, of the project whose manifest it names alone.
    Sessions(Option<PathBuf>),
    Logs {
        pick: Pick,
        level: String,
        follow: bool,
    },
    Call {
        pick: Pick,
        method: String,
        params: Value,
    },
    /// Captures of several shows of one world: `params` are `capture`'s but `show`.
    Capture {
        pick: Pick,
        params: Value,
        shows: Vec<Value>,
    },
    /// A pixel's shading, in the view as shown or, with `own`, the game's own.
    Probe {
        pick: Pick,
        at: [f64; 2],
        own: bool,
    },
    Mcp(Pick),
    /// Waits, at most the time given, until the picked `gg run` runs the saved edits' code.
    Wait(Pick, Duration),
}

fn main() -> ExitCode {
    let args: Result<Vec<String>, _> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.into_string())
        .collect();
    let args = args.map_err(|arg| bad(format!("{arg:?} isn't valid Unicode")));
    let command = match args.and_then(|args| parse(&args)) {
        Ok(command) => command,
        Err(failure) => {
            report(&failure);
            // Bad arguments exit 2; an unreadable manifest, 1.
            return ExitCode::from(if failure.code == "gg.args" { 2 } else { 1 });
        }
    };
    let done = match command {
        Command::Help(text) => {
            print(&text);
            Ok(true)
        }
        Command::Run(run) => run::run(&run),
        Command::Check(check) => run::check(&check),
        Command::Ship(ship) => ship::ship(&ship).map(|()| true),
        Command::Fetch(scope) => fetch::fetch(&scope).map(|()| true),
        Command::New(name, engine) => new::new(&name, engine.as_ref()).map(|()| true),
        Command::Sessions(project) => game::sessions(project.as_deref()).map(|sessions| {
            print(&sessions.to_string());
            true
        }),
        Command::Logs {
            pick,
            level,
            follow,
        } => game::logs(&pick, &level, follow).map(|()| true),
        Command::Call {
            pick,
            method,
            params,
        } => game::call(&pick, &method, params).map(|result| {
            print(result.get());
            true
        }),
        Command::Capture {
            pick,
            params,
            shows,
        } => game::captures(&pick, params, &shows).map(|taken| {
            print(&game::joined(&taken));
            true
        }),
        Command::Probe { pick, at, own } => probe::run(&pick, at, own).map(|probe| {
            print(&probe.to_string());
            true
        }),
        Command::Mcp(pick) => mcp::serve(pick).map(|()| true),
        Command::Wait(pick, timeout) => runs::wait(&pick, timeout).map(|told| {
            print(&told.to_string());
            true
        }),
    };
    match done {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(failure) => {
            report(&failure);
            ExitCode::FAILURE
        }
    }
}

/// Ignores a closed stdout, as under `| head`.
fn print(line: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

fn report(failure: &Failure) {
    let error = json!({"error": {"code": failure.code, "message": failure.message}});
    let _ = writeln!(std::io::stderr().lock(), "{error}");
}

fn warn(code: &str, message: &str, path: &Path) {
    let warning = json!({"warning": {"code": code, "message": message, "path": path}});
    let _ = writeln!(std::io::stderr().lock(), "{warning}");
}

fn bad(message: String) -> Failure {
    Failure::new(
        "gg.args",
        format!("{message}; `gg help` lists the commands"),
    )
}

/// `path` made absolute: the game's working directory may not be gg's.
///
/// Fails with `gg.args` if gg can't tell where it is.
fn absolute(path: &str) -> Result<PathBuf, Failure> {
    std::path::absolute(Path::new(path))
        .map_err(|error| bad(format!("can't tell where `{path}` is: {error}")))
}

/// The flags that take a value: `--name value` or `--name=value`.
const VALUED: [&str; 21] = [
    "--engine",
    "--viewpoints",
    "--rect",
    "--project",
    "--grant",
    "--peers",
    "--author",
    "--pid",
    "--app",
    "--timeout",
    "--hang",
    "--snapshot",
    "--frames",
    "--with",
    "--kind",
    "--skip",
    "--take",
    "--level",
    "--wait",
    "--show",
    "--within",
];

/// The arguments after a command's name; what follows `--` is for the game.
#[derive(Default)]
struct Parsed {
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
    rest: Vec<String>,
}

impl Parsed {
    fn new(args: &[String]) -> Result<Self, Failure> {
        let mut parsed = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if arg == "--" {
                parsed.rest = args.cloned().collect();
                break;
            }
            if !arg.starts_with("--") {
                parsed.positional.push(arg.clone());
                continue;
            }
            let (name, value) = match arg.split_once('=') {
                Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
                None => (arg.clone(), None),
            };
            let value = match (VALUED.contains(&name.as_str()), value) {
                (true, Some(value)) => Some(value),
                (true, None) => {
                    let value = args
                        .next()
                        .ok_or_else(|| bad(format!("`{name}` needs a value")))?;
                    Some(value.clone())
                }
                (false, Some(_)) => return Err(bad(format!("`{name}` takes no value"))),
                (false, None) => None,
            };
            parsed.flags.push((name, value));
        }
        Ok(parsed)
    }

    fn switch(&mut self, name: &str) -> bool {
        self.take(name).is_some()
    }

    fn value(&mut self, name: &str) -> Option<String> {
        self.take(name).flatten()
    }

    fn values(&mut self, name: &str) -> Vec<String> {
        std::iter::from_fn(|| self.take(name)).flatten().collect()
    }

    fn number<T: FromStr>(&mut self, name: &str, default: T) -> Result<T, Failure> {
        match self.value(name) {
            Some(value) => value
                .parse()
                .map_err(|_| bad(format!("`{name}` takes a whole number, not `{value}`"))),
            None => Ok(default),
        }
    }

    /// Fails with `gg.args` for 0, which would give up before any answer.
    fn seconds(&mut self, name: &str, default: u64) -> Result<Duration, Failure> {
        match self.number(name, default)? {
            0 => Err(bad(format!(
                "`{name}` takes a number of seconds more than 0"
            ))),
            seconds => Ok(Duration::from_secs(seconds)),
        }
    }

    fn take(&mut self, name: &str) -> Option<Option<String>> {
        let at = self.flags.iter().position(|(given, _)| given == name)?;
        Some(self.flags.remove(at).1)
    }

    fn pick(&mut self) -> Result<Pick, Failure> {
        Ok(Pick {
            pid: self
                .value("--pid")
                .map(|pid| pid.parse())
                .transpose()
                .map_err(|_| bad("`--pid` takes a process's ID, a whole number".to_owned()))?,
            app: self.value("--app"),
            timeout: self.seconds("--timeout", 30)?,
            client: "gg".to_owned(),
            grant: (self.value("--grant")).map(|grant| {
                grant
                    .split(',')
                    .map(|capability| capability.trim().to_owned())
                    .collect()
            }),
            project: self.scope()?.map(|scope| scope.manifest),
        })
    }

    /// The project gg works in: `--project`'s, else the working directory's, as gg-api's
    /// `Project::find` finds it; `None` outside one.
    ///
    /// Fails with `gg.args` if gg can't tell where the working directory or `--project` is; and
    /// with `project.read` or `project.bad` for a manifest that doesn't read, as `Project::read`
    /// does, rather than working on the whole workspace.
    fn scope(&mut self) -> Result<Option<Scope>, Failure> {
        let dir = match self.value("--project") {
            Some(given) => {
                let path = std::path::absolute(&given)
                    .map_err(|error| bad(format!("can't tell where `{given}` is: {error}")))?;
                match (path.file_name(), path.parent()) {
                    (Some(name), Some(dir)) if name == Project::FILE => dir.to_owned(),
                    _ => path,
                }
            }
            None => {
                let here = std::env::current_dir().map_err(|error| {
                    bad(format!("can't tell which directory gg runs in: {error}"))
                })?;
                let Some(dir) = Project::find(&here) else {
                    return Ok(None);
                };
                dir
            }
        };
        let project = Project::read(&dir)?;
        Ok(Some(Scope {
            manifest: dir.join(Project::FILE),
            game: project.game,
        }))
    }

    /// The package given, else `scope`'s game. Fails with `gg.args` if neither.
    fn package(&mut self, command: &str, scope: Option<&Scope>) -> Result<String, Failure> {
        let given = self
            .positional(0, 1, &format!("{command} [PACKAGE]"))?
            .pop();
        let package = given.or_else(|| Some(scope?.game.clone()));
        package.ok_or_else(|| bad(format!("outside a project, `gg {command}` takes a PACKAGE")))
    }

    fn positional(
        &mut self,
        least: usize,
        most: usize,
        usage: &str,
    ) -> Result<Vec<String>, Failure> {
        let given = self.positional.len();
        if !(least..=most).contains(&given) {
            return Err(bad(format!("it's `gg {usage}`")));
        }
        Ok(std::mem::take(&mut self.positional))
    }

    fn finish(self, command: &str) -> Result<(), Failure> {
        if let Some((flag, _)) = self.flags.first() {
            return Err(bad(format!("`gg {command}` takes no `{flag}`")));
        }
        if !self.rest.is_empty() {
            return Err(bad(format!("`gg {command}` takes nothing after `--`")));
        }
        Ok(())
    }
}

/// `args` with the command's name first: options before it, as in `gg --pid 7 status`, move after
/// it. `--help` stays, as a command.
///
/// Fails with `gg.args` for options no command follows.
fn command_first(args: &[String]) -> Result<Vec<String>, Failure> {
    let mut at = 0;
    while let Some(arg) = args.get(at)
        && arg.starts_with("--")
        && arg != "--"
        && arg != "--help"
    {
        let valued = VALUED.contains(&arg.as_str());
        at += if valued { 2 } else { 1 };
    }
    let Some(name) = args.get(at) else {
        if at == 0 {
            return Ok(Vec::new());
        }
        return Err(bad(format!(
            "`{}` comes with a command",
            args.first().map_or("", String::as_str)
        )));
    };
    let options = &args[..at];
    let rest = &args[at + 1..];
    Ok([std::slice::from_ref(name), options, rest].concat())
}

/// The game's methods that gg's commands and MCP's tools of the same name call with no parameters.
/// Each side adds one: gg's `quit` (MCP has no tool) and MCP's `snapshot` (gg's takes a path).
const NO_PARAMETERS: [&str; 10] = [
    "status", "pause", "resume", "play", "stop", "types", "hash", "cells", "ui", "images",
];

/// Fails with `gg.args` for arguments that aren't a command's, and as `Parsed::scope` does.
fn parse(args: &[String]) -> Result<Command, Failure> {
    let args = command_first(args)?;
    let Some((name, args)) = args.split_first() else {
        return Ok(Command::Help(overview()));
    };
    let mut parsed = Parsed::new(args)?;
    let command = match name.as_str() {
        "help" | "-h" | "--help" => help_args(&mut parsed)?,
        "run" | "check" | "ship" => build_args(name, &mut parsed)?,
        "fetch" | "new" => project_args(name, &mut parsed)?,
        "wait" | "step" | "scene" | "snapshot" => control_args(name, &mut parsed)?,
        "input" | "capture" | "frames" | "look" | "pick" | "probe" => view_args(name, &mut parsed)?,
        "sessions" | "logs" | "entities" | "assets" | "entity" | "get" => {
            read_args(name, &mut parsed)?
        }
        "apply" | "save-prefab" | "set" | "undo" | "redo" | "edits" => {
            edit_args(name, &mut parsed)?
        }
        "debug" => debug_args(&mut parsed)?,
        "mcp" => mcp_args(&mut parsed)?,
        "call" => call_args(&mut parsed)?,
        method if NO_PARAMETERS.contains(&method) || method == "quit" => {
            parsed.positional(0, 0, method)?;
            call(&mut parsed, method, Value::Null)?
        }
        other => return Err(bad(format!("there's no command `{other}`"))),
    };
    parsed.finish(name)?;
    Ok(command)
}

fn call(parsed: &mut Parsed, method: &str, params: Value) -> Result<Command, Failure> {
    Ok(Command::Call {
        pick: parsed.pick()?,
        method: method.to_owned(),
        params,
    })
}

fn help_args(parsed: &mut Parsed) -> Result<Command, Failure> {
    let given = parsed.positional(0, 1, "help [<COMMAND>]")?;
    Ok(Command::Help(match given.first() {
        Some(name) => section(name)?,
        None => overview(),
    }))
}

fn build_args(name: &str, parsed: &mut Parsed) -> Result<Command, Failure> {
    let command = match name {
        "run" => {
            let project = parsed.scope()?;
            let run = Run {
                package: parsed.package("run", project.as_ref())?,
                project,
                release: parsed.switch("--release"),
                patches: !parsed.switch("--no-patch"),
                // 0: no hang detection.
                hang: Some(parsed.number("--hang", 10)?)
                    .filter(|&seconds| seconds > 0)
                    .map(Duration::from_secs),
                // 0: restarts off.
                snapshot: Duration::from_secs(parsed.number("--snapshot", 5)?),
                peers: parsed.number("--peers", 1)?,
                embedded: parsed.switch("--embedded"),
                args: std::mem::take(&mut parsed.rest),
            };
            if run.embedded && run.peers > 1 {
                return Err(bad(
                    "`--embedded` runs the game for the tool that started gg, which shows one \
                     game's view, so it takes no `--peers 2`"
                        .to_owned(),
                ));
            }
            if !(1..=2).contains(&run.peers) {
                return Err(bad(format!(
                    "`--peers` takes 1, a game alone, or 2, a lockstep session's host and joiner, \
                     not {}",
                    run.peers
                )));
            }
            if run.peers > 1
                && let Some(flag) = ["--host", "--join", "--relay", "--code", "--record"]
                    .into_iter()
                    .find(|&flag| run.given(flag))
            {
                return Err(bad(format!(
                    "`--peers 2` gives the peers their `--host`, `--join` and `--code` itself, on \
                     this machine and without a relay, and a joiner records nothing, so the game's \
                     arguments take no `{flag}`"
                )));
            }
            Command::Run(run)
        }
        "check" => {
            let project = parsed.scope()?;
            Command::Check(Check {
                package: parsed.package("check", project.as_ref())?,
                project,
                release: parsed.switch("--release"),
                frames: parsed.number("--frames", 60)?,
                timeout: parsed.seconds("--timeout", 600)?,
                args: std::mem::take(&mut parsed.rest),
            })
        }
        "ship" => {
            let project = parsed.scope()?;
            Command::Ship(Ship {
                package: parsed.package("ship", project.as_ref())?,
                project,
            })
        }
        _ => unreachable!("`parse` passes the build commands alone"),
    };
    Ok(command)
}

fn project_args(name: &str, parsed: &mut Parsed) -> Result<Command, Failure> {
    let command = match name {
        "fetch" => {
            parsed.positional(0, 0, "fetch")?;
            let scope = parsed.scope()?.ok_or_else(|| {
                bad(
                    "`gg fetch` fetches a project's files: run it in a project, or name one with \
                     `--project`"
                        .to_owned(),
                )
            })?;
            Command::Fetch(scope)
        }
        "new" => {
            let name = parsed
                .positional(1, 1, "new <NAME> [--engine <SOURCE>]")?
                .remove(0);
            if new::project(&name).check().is_err() {
                return Err(bad(format!(
                    "`gg new` takes a package's name, of ASCII letters, digits, `-` and `_`, \
                     starting with a letter or `_`, and not one Windows keeps for a device, as \
                     `con`, not `{name}`"
                )));
            }
            let engine = parsed
                .value("--engine")
                .map(|given| new::Engine::parse(&given));
            Command::New(name, engine.transpose().map_err(bad)?)
        }
        _ => unreachable!("`parse` passes the project commands alone"),
    };
    Ok(command)
}

fn control_args(name: &str, parsed: &mut Parsed) -> Result<Command, Failure> {
    let command = match name {
        "wait" => {
            parsed.positional(0, 0, "wait [--timeout <SECONDS>]")?;
            let timeout = parsed.seconds("--timeout", 300)?;
            Command::Wait(parsed.pick()?, timeout)
        }
        "step" => {
            let ticks = parsed.positional(0, 1, "step [N]")?;
            let ticks = match ticks.first() {
                Some(ticks) => ticks.parse::<u64>().map_err(|_| {
                    bad(format!("`gg step` takes a number of ticks, not `{ticks}`"))
                })?,
                None => 1,
            };
            call(parsed, "step", json!({"ticks": ticks}))?
        }
        "scene" => {
            let usage = "scene open <PATH> [--discard] | scene save [PATH] | scene new [--discard]";
            let given = parsed.positional(1, 2, usage)?;
            let params = match (given[0].as_str(), given.get(1)) {
                ("open", Some(path)) => {
                    let discard = parsed.switch("--discard");
                    json!({"action": "open", "path": absolute(path)?, "discard": discard})
                }
                ("save", None) => json!({"action": "save"}),
                ("save", Some(path)) => json!({"action": "save", "path": absolute(path)?}),
                ("new", None) => json!({"action": "new", "discard": parsed.switch("--discard")}),
                _ => return Err(bad(format!("it's `gg {usage}`"))),
            };
            call(parsed, "scene", params)?
        }
        "snapshot" => {
            let path = parsed.positional(0, 1, "snapshot [PATH]")?;
            let params = match path.first() {
                Some(path) => json!({"path": absolute(path)?}),
                None => json!({}),
            };
            call(parsed, "snapshot", params)?
        }
        _ => unreachable!("`parse` passes the control commands alone"),
    };
    Ok(command)
}

fn view_args(name: &str, parsed: &mut Parsed) -> Result<Command, Failure> {
    let command = match name {
        "input" => {
            let given = parsed.positional(1, 1, "input <EVENTS>")?;
            let events: Value = serde_json::from_str(&given[0])
                .map_err(|error| bad(format!("EVENTS isn't JSON: {error}")))?;
            if !events.is_array() {
                return Err(bad("EVENTS is a JSON array of input events".to_owned()));
            }
            call(parsed, "input", json!({"events": events}))?
        }
        "capture" => {
            let usage = "capture [--own] [--show <NAME>[,<NAME>...]]... [PATH]";
            let path = parsed.positional(0, 1, usage)?;
            let mut params = json!({});
            if let Some(path) = path.first() {
                params["path"] = json!(absolute(path)?);
            }
            if parsed.switch("--own") {
                params["own"] = json!(true);
            }
            let given = parsed.values("--show");
            let shows = (given.iter().flat_map(|names| names.split(',')))
                .map(|name| {
                    game::shown(&json!(name.trim())).map_err(|failure| bad(failure.message))
                })
                .collect::<Result<Vec<Value>, Failure>>()?;
            match shows.as_slice() {
                [] => call(parsed, "capture", params)?,
                [show] => {
                    params["show"] = show.clone();
                    call(parsed, "capture", params)?
                }
                _ if !path.is_empty() => {
                    return Err(bad(
                        "a PATH holds one PNG, so `gg capture` takes it with one `--show` at most"
                            .to_owned(),
                    ));
                }
                _ => Command::Capture {
                    pick: parsed.pick()?,
                    params,
                    shows,
                },
            }
        }
        "frames" => {
            let usage = "frames <N> [--viewpoints <FILE>] [--rect X,Y,W,H] [--show <NAME>] <DIR>";
            let given = parsed.positional(2, 2, usage)?;
            let count = given[0].parse::<u32>().map_err(|_| {
                let what = &given[0];
                bad(format!(
                    "`gg frames` takes how many frames, a whole number, not `{what}`"
                ))
            })?;
            let mut params = json!({"dir": absolute(&given[1])?, "count": count});
            if let Some(file) = parsed.value("--viewpoints") {
                let text = fs::read_to_string(&file)
                    .map_err(|error| bad(format!("can't read the viewpoints `{file}`: {error}")))?;
                // Checked here to name the wrong file; passed on as written.
                let viewpoints = serde_json::from_str::<Value>(&text).and_then(|viewpoints| {
                    serde_json::from_value::<Vec<gg_api::Viewpoint>>(viewpoints.clone())
                        .map(|_| viewpoints)
                });
                params["viewpoints"] = viewpoints.map_err(|error| {
                    bad(format!(
                        "`{file}` holds a JSON array of viewpoints, as `look` takes its \
                         `viewpoint`: {error}"
                    ))
                })?;
            }
            if let Some(rect) = parsed.value("--rect") {
                let numbers = (rect.split(','))
                    .map(|number| number.trim().parse::<u32>())
                    .collect::<Result<Vec<u32>, _>>();
                let numbers = numbers.ok().filter(|numbers| numbers.len() == 4);
                params["rect"] = json!(numbers.ok_or_else(|| {
                    bad(format!(
                        "`--rect` takes X,Y,WIDTH,HEIGHT, four whole numbers, not `{rect}`"
                    ))
                })?);
            }
            if let Some(name) = parsed.value("--show") {
                params["show"] =
                    game::shown(&json!(name.trim())).map_err(|failure| bad(failure.message))?;
            }
            call(parsed, "frames", params)?
        }
        "look" => {
            let given = parsed.positional(0, 1, "look [<LOOK>]")?;
            let look: Value = match given.first() {
                Some(look) => serde_json::from_str(look)
                    .map_err(|error| bad(format!("LOOK isn't JSON: {error}")))?,
                None => json!({}),
            };
            if !look.is_object() {
                return Err(bad("LOOK is a JSON object of the look's parts".to_owned()));
            }
            call(parsed, "look", look)?
        }
        "pick" => {
            let given = parsed.positional(2, 2, "pick <X> <Y> [--within <ID>]")?;
            let at = given.iter().map(|given| {
                let at = given.parse::<f64>().ok().filter(|at| at.is_finite());
                at.ok_or_else(|| {
                    bad(format!(
                        "`gg pick` takes a pixel's X and Y, numbers, not `{given}`"
                    ))
                })
            });
            let at: [f64; 2] = at
                .collect::<Result<Vec<f64>, Failure>>()?
                .try_into()
                .expect("two numbers, as many as `positional` took");
            // `within` stays out unless given: a game of an older engine refuses unknown fields.
            let within = parsed.value("--within");
            let pick = serde_json::to_value(gg_api::Pick { at, within }).expect("JSON");
            call(parsed, "pick", pick)?
        }
        "probe" => {
            let given = parsed.positional(2, 2, "probe <X> <Y> [--own]")?;
            let at = given.iter().map(|given| {
                given.parse::<f64>().map_err(|_| {
                    bad(format!(
                        "`gg probe` takes a pixel's X and Y, numbers, not `{given}`"
                    ))
                })
            });
            let at: [f64; 2] = at
                .collect::<Result<Vec<f64>, Failure>>()?
                .try_into()
                .expect("two numbers, as many as `positional` took");
            Command::Probe {
                at: probe::pixel(at).map_err(|failure| bad(failure.message))?,
                own: parsed.switch("--own"),
                pick: parsed.pick()?,
            }
        }
        _ => unreachable!("`parse` passes the view commands alone"),
    };
    Ok(command)
}

fn read_args(name: &str, parsed: &mut Parsed) -> Result<Command, Failure> {
    let command = match name {
        "sessions" => {
            parsed.positional(0, 0, "sessions")?;
            Command::Sessions(parsed.scope()?.map(|scope| scope.manifest))
        }
        "logs" => {
            parsed.positional(0, 0, "logs [--level <LEVEL>] [--follow]")?;
            let level = parsed.value("--level").unwrap_or_else(|| "warn".to_owned());
            let level = level.to_ascii_lowercase();
            if !gg_api::LEVELS.contains(&level.as_str()) {
                return Err(bad(format!(
                    "`--level` is `error`, `warn`, `info`, `debug` or `trace`, not `{level}`"
                )));
            }
            Command::Logs {
                pick: parsed.pick()?,
                level,
                follow: parsed.switch("--follow"),
            }
        }
        "entities" => {
            parsed.positional(
                0,
                0,
                "entities [--with <COMPONENT>] [--skip <N>] [--take <N>]",
            )?;
            let with = parsed.value("--with");
            let skip: usize = parsed.number("--skip", 0)?;
            let take: usize = parsed.number("--take", 100)?;
            call(
                parsed,
                "entities",
                json!({"with": with, "skip": skip, "take": take}),
            )?
        }
        "assets" => {
            parsed.positional(0, 0, "assets [--kind <KIND>] [--skip <N>] [--take <N>]")?;
            // The game refuses an unknown kind.
            let kind = parsed.value("--kind");
            let skip: usize = parsed.number("--skip", 0)?;
            let take: usize = parsed.number("--take", 100)?;
            call(
                parsed,
                "assets",
                json!({"kind": kind, "skip": skip, "take": take}),
            )?
        }
        "entity" => {
            let id = parsed.positional(1, 1, "entity <ID>")?.remove(0);
            call(parsed, "entity", json!({"id": id}))?
        }
        "get" => {
            let mut given = parsed.positional(2, 3, "get <ID> <COMPONENT> [PATH]")?;
            let path = if given.len() == 3 {
                given.remove(2)
            } else {
                String::new()
            };
            let (id, component) = (given.remove(0), given.remove(0));
            call(
                parsed,
                "get",
                json!({"id": id, "component": component, "path": path}),
            )?
        }
        _ => unreachable!("`parse` passes the read commands alone"),
    };
    Ok(command)
}

fn edit_args(name: &str, parsed: &mut Parsed) -> Result<Command, Failure> {
    let command = match name {
        "apply" => {
            let given = parsed.positional(1, 1, "apply <OPERATIONS>")?;
            let operations: Value = serde_json::from_str(&given[0])
                .map_err(|error| bad(format!("OPERATIONS isn't JSON: {error}")))?;
            if !operations.is_array() {
                return Err(bad("OPERATIONS is a JSON array of operations".to_owned()));
            }
            call(parsed, "apply", json!({"operations": operations}))?
        }
        "save-prefab" => {
            let given = parsed.positional(1, 2, "save-prefab <ID> [PATH]")?;
            let mut params = json!({"id": given[0]});
            if let Some(path) = given.get(1) {
                params["path"] = json!(absolute(path)?);
            }
            call(parsed, "save_prefab", params)?
        }
        "set" => {
            let given = parsed.positional(4, 4, "set <ID> <COMPONENT> <PATH> <VALUE>")?;
            let value =
                serde_json::from_str(&given[3]).unwrap_or_else(|_| Value::String(given[3].clone()));
            let set =
                json!({"id": given[0], "component": given[1], "path": given[2], "value": value});
            call(parsed, "apply", json!({"operations": [{"set": set}]}))?
        }
        method @ ("undo" | "redo" | "edits") => {
            parsed.positional(0, 0, &format!("{method} [--author <NAME>]"))?;
            let author = parsed.value("--author");
            call(parsed, method, json!({"author": author}))?
        }
        _ => unreachable!("`parse` passes the edit commands alone"),
    };
    Ok(command)
}

fn debug_args(parsed: &mut Parsed) -> Result<Command, Failure> {
    let usage = "debug [break <SCRIPT> [<LINE>...] | pause | continue | into | over | out] \
                 [--wait <SECONDS>]";
    let given = parsed.positional(0, usize::MAX, usage)?;
    let wait: u64 = parsed.number("--wait", 0)?;
    let mut params = json!({"wait_ms": wait.saturating_mul(1000)});
    match given.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => {}
        ["break", script, ref lines @ ..] => {
            let lines = lines.iter().map(|line| {
                line.parse::<u32>()
                    .ok()
                    .filter(|&line| line > 0)
                    .ok_or_else(|| {
                        bad(format!("`gg debug break` takes lines from 1, not `{line}`"))
                    })
            });
            let lines = lines.collect::<Result<Vec<u32>, Failure>>()?;
            params["breakpoints"] = json!({"script": script, "lines": lines});
        }
        [action @ ("pause" | "continue")] => params["action"] = json!(action),
        [step @ ("into" | "over" | "out")] => params["action"] = json!(format!("step_{step}")),
        _ => return Err(bad(format!("it's `gg {usage}`"))),
    }
    call(parsed, "debug", params)
}

fn mcp_args(parsed: &mut Parsed) -> Result<Command, Failure> {
    parsed.positional(0, 0, "mcp")?;
    // The MCP client's name, once it says, replaces `mcp`.
    let pick = parsed.pick()?;
    Ok(Command::Mcp(Pick {
        client: "mcp".to_owned(),
        ..pick
    }))
}

fn call_args(parsed: &mut Parsed) -> Result<Command, Failure> {
    let mut given = parsed.positional(1, 2, "call <METHOD> [<PARAMS>]")?;
    let params = match given.get(1) {
        Some(params) => serde_json::from_str(params)
            .map_err(|error| bad(format!("PARAMS isn't JSON: {error}")))?,
        None => Value::Null,
    };
    let method = given.remove(0);
    call(parsed, &method, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Command, Failure> {
        let args: Vec<String> = args.iter().map(|&arg| arg.to_owned()).collect();
        parse(&args)
    }

    #[test]
    fn wait_takes_a_timeout_and_a_pick() {
        let Ok(Command::Wait(pick, timeout)) = parsed(&["wait"]) else {
            panic!("a wait");
        };
        assert_eq!((pick.pid, timeout), (None, Duration::from_secs(300)));
        let Ok(Command::Wait(pick, timeout)) = parsed(&["--pid", "7", "wait", "--timeout=5"])
        else {
            panic!("a wait");
        };
        assert_eq!((pick.pid, timeout), (Some(7), Duration::from_secs(5)));
    }

    #[test]
    fn grant_names_the_capabilities_a_connection_is_kept_to() {
        let granted = |args: &[&str]| match parsed(args) {
            Ok(Command::Call { pick, .. } | Command::Mcp(pick)) => pick.grant,
            _ => panic!("{args:?}"),
        };
        let read_view = Some(vec!["read".to_owned(), "view".to_owned()]);
        assert_eq!(granted(&["mcp", "--grant", "read, view"]), read_view);
        assert_eq!(granted(&["status", "--grant=read,view"]), read_view);
        assert_eq!(granted(&["status"]), None);
    }

    #[test]
    fn looks_help_names_every_pass() {
        let look = section("look").expect("a `look` section");
        for pass in gg_api::Pass::ALL {
            let name = serde_json::to_value(pass).expect("a name");
            let name = name.as_str().expect("a string");
            assert!(look.contains(&format!("`{name}`")), "{name}");
        }
    }

    #[test]
    fn the_help_names_every_part_of_shading() {
        let capture = section("capture").expect("a `capture` section");
        let probing = section("probe").expect("a `probe` section");
        for name in probe::SHADING {
            for help in [&capture, &probing] {
                assert!(help.contains(&format!("`{name}`")), "{name}: {help}");
            }
        }
    }

    #[test]
    fn help_lists_each_command_and_prints_its_section() {
        let Ok(Command::Help(overview)) = parsed(&[]) else {
            panic!("help");
        };
        assert!(matches!(parsed(&["help"]), Ok(Command::Help(listed)) if listed == overview));
        for help in COMMANDS.iter().flat_map(|(_, commands)| commands.iter()) {
            let name = help.name;
            let listed = format!("  {name} ");
            let shown = overview.lines().any(|line| line.starts_with(&listed));
            assert!(shown, "{name}");
            let refused = parsed(&[name]).err().map(|failure| failure.message);
            let unknown = refused.is_some_and(|message| message.contains("there's no command"));
            assert!(!unknown, "{name}");
            for args in [["help", name], ["--help", name]] {
                let Ok(Command::Help(text)) = parsed(&args) else {
                    panic!("{args:?}");
                };
                assert!(text.starts_with(&format!("Usage: gg {name}")), "{text}");
                assert!(text.contains(help.summary), "{text}");
                assert!(text.contains(help.details.trim_end()), "{text}");
            }
        }
        let refused = parsed(&["help", "nonsense"]).err().expect("refused");
        assert_eq!(refused.code, "gg.args");
        for named in ["`nonsense`", "`run`", "`save-prefab`", "`gg help`"] {
            assert!(refused.message.contains(named), "{}", refused.message);
        }
    }

    #[test]
    fn commands_parse_into_their_calls_with_their_options() {
        let Ok(Command::Call {
            pick,
            method,
            params,
        }) = parsed(&[
            "get",
            "00000000000000ff",
            "Transform",
            "translation.x",
            "--pid=7",
        ])
        else {
            panic!("a call");
        };
        assert_eq!(
            (pick.pid, pick.app, pick.timeout),
            (Some(7), None, Duration::from_secs(30))
        );
        assert_eq!(method, "get");
        let want =
            json!({"id": "00000000000000ff", "component": "Transform", "path": "translation.x"});
        assert_eq!(params, want);
        let Ok(Command::Call { params, .. }) = parsed(&[
            "entities", "--with", "Sprite", "--take", "5", "--app", "Sandbox",
        ]) else {
            panic!("a call");
        };
        assert_eq!(params, json!({"with": "Sprite", "skip": 0, "take": 5}));
        let Ok(Command::Call {
            pick,
            method,
            params,
        }) = parsed(&["--pid", "7", "--timeout=5", "step", "2", "--app", "Sandbox"])
        else {
            panic!("a call");
        };
        assert_eq!(
            (pick.pid, pick.app.as_deref(), pick.timeout),
            (Some(7), Some("Sandbox"), Duration::from_secs(5))
        );
        assert_eq!((method.as_str(), params), ("step", json!({"ticks": 2})));
        let Ok(Command::Run(run)) = parsed(&["--no-patch", "run", "sandbox", "--", "--vsync"])
        else {
            panic!("a run");
        };
        assert_eq!((run.patches, run.args), (false, vec!["--vsync".to_owned()]));
        assert!(matches!(parsed(&["--help"]), Ok(Command::Help(_))));
        let Ok(Command::Run(run)) = parsed(&["run", "sandbox", "--hang", "30", "--", "--vsync"])
        else {
            panic!("a run");
        };
        assert_eq!(
            (run.package.as_str(), run.release, run.patches),
            ("sandbox", false, true)
        );
        assert_eq!(
            (run.hang, run.snapshot, run.args),
            (
                Some(Duration::from_secs(30)),
                Duration::from_secs(5),
                vec!["--vsync".to_owned()]
            )
        );
        // `--hang 0` never reports a hang.
        for (args, hang) in [
            (&["run", "sandbox"][..], Some(Duration::from_secs(10))),
            (&["run", "sandbox", "--hang", "0"], None),
        ] {
            let Ok(Command::Run(run)) = parsed(args) else {
                panic!("a run");
            };
            assert_eq!(run.hang, hang, "{args:?}");
        }
        let Ok(Command::Run(run)) = parsed(&["run", "sandbox", "--snapshot=0", "--no-patch"])
        else {
            panic!("a run");
        };
        assert_eq!(
            (run.snapshot, run.patches, run.peers),
            (Duration::ZERO, false, 1)
        );
        let Ok(Command::Run(run)) = parsed(&["run", "shards", "--peers", "2"]) else {
            panic!("a run");
        };
        assert_eq!((run.peers, run.embedded), (2, false));
        let Ok(Command::Run(run)) = parsed(&["run", "shards", "--embedded"]) else {
            panic!("a run");
        };
        assert!(run.embedded);
        let refused = parsed(&["run", "shards", "--embedded", "--peers", "2"]).err();
        assert_eq!(
            refused.map(|failure| failure.code).as_deref(),
            Some("gg.args")
        );
        // Tests run outside any project.
        assert_eq!(run.project, None);
        let dir = std::env::temp_dir().join(format!("gg-args-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a directory");
        let manifest = dir.join(Project::FILE);
        fs::write(&manifest, new::project("probe").text()).expect("written");
        let at = dir.to_str().expect("a UTF-8 path");
        for given in [at, manifest.to_str().expect("a UTF-8 path")] {
            let Ok(Command::Run(run)) = parsed(&["run", "--project", given]) else {
                panic!("a run");
            };
            assert_eq!(run.package, "probe", "{given}");
            let scoped = run.project.map(|scope| scope.manifest);
            assert_eq!(scoped.as_ref(), Some(&manifest), "{given}");
        }
        let Ok(Command::Check(check)) = parsed(&["check", "sandbox", &format!("--project={at}")])
        else {
            panic!("a check");
        };
        assert_eq!(
            (check.package.as_str(), check.project.is_some()),
            ("sandbox", true)
        );
        let Ok(Command::Call { pick, .. }) = parsed(&["status", "--project", at]) else {
            panic!("a call");
        };
        assert_eq!(pick.project.as_ref(), Some(&manifest));
        let Ok(Command::Ship(ship)) = parsed(&["ship", "shards"]) else {
            panic!("a ship");
        };
        assert_eq!((ship.package.as_str(), ship.project), ("shards", None));
        let Ok(Command::Ship(ship)) = parsed(&["ship", "--project", at]) else {
            panic!("a ship");
        };
        assert_eq!(ship.package, "probe");
        assert_eq!(ship.project.map(|scope| scope.manifest), Some(manifest));
        fs::remove_dir_all(&dir).expect("removed");
        let Ok(Command::New(name, None)) = parsed(&["new", "probe"]) else {
            panic!("a new game");
        };
        assert_eq!(name, "probe");
        let Ok(Command::New(_, Some(engine))) = parsed(&["new", "probe", "--engine", "../engine"])
        else {
            panic!("a new game");
        };
        assert_eq!(engine, new::Engine::Path(PathBuf::from("../engine")));
        let Ok(Command::New(_, Some(engine))) = parsed(&[
            "--engine=https://example.com/engine.git#4f2a91c",
            "new",
            "probe",
        ]) else {
            panic!("a new game");
        };
        let url = "https://example.com/engine.git".to_owned();
        let rev = "4f2a91c".to_owned();
        assert_eq!(engine, new::Engine::Git { url, rev });
        let snapshot = std::path::absolute("world.snapshot").unwrap();
        let png = std::path::absolute("view.png").unwrap();
        let level = std::path::absolute("scenes/level2.ron").unwrap();
        let frames = std::path::absolute("frames").unwrap();
        for (args, want) in [
            (
                &["scene", "open", "scenes/level2.ron"][..],
                json!({"action": "open", "path": level, "discard": false}),
            ),
            (
                &["scene", "open", "scenes/level2.ron", "--discard"],
                json!({"action": "open", "path": level, "discard": true}),
            ),
            (&["scene", "save"], json!({"action": "save"})),
            (
                &["scene", "save", "scenes/level2.ron"],
                json!({"action": "save", "path": level}),
            ),
            (
                &["scene", "new"],
                json!({"action": "new", "discard": false}),
            ),
            (
                &["scene", "new", "--discard"],
                json!({"action": "new", "discard": true}),
            ),
            (&["snapshot"][..], json!({})),
            (&["snapshot", "world.snapshot"], json!({"path": snapshot})),
            (&["capture"], json!({})),
            (&["capture", "--own"], json!({"own": true})),
            (
                &["capture", "view.png", "--own"],
                json!({"path": png, "own": true}),
            ),
            (
                &["capture", "--show", "roughness"],
                json!({"show": "roughness"}),
            ),
            (
                &["capture", "--show", "view depth", "--own", "view.png"],
                json!({"path": png, "own": true, "show": {"image": "view depth"}}),
            ),
            (
                &["frames", "8", "frames"],
                json!({"dir": frames, "count": 8}),
            ),
            (
                &[
                    "frames",
                    "3",
                    "--rect",
                    "1, 2,3,4",
                    "--show=view depth",
                    "frames",
                ],
                json!({"dir": frames, "count": 3, "rect": [1, 2, 3, 4],
                    "show": {"image": "view depth"}}),
            ),
            (&["look"], json!({})),
            (
                &["look", r#"{"grid":"xz","outline":null}"#],
                json!({"grid": "xz", "outline": null}),
            ),
            (
                &["look", r#"{"show":{"image":"view depth"}}"#],
                json!({"show": {"image": "view depth"}}),
            ),
            (
                &["look", r#"{"skip":["fog","shadows"]}"#],
                json!({"skip": ["fog", "shadows"]}),
            ),
            // A pixel outside the view too; the game answers none.
            (&["pick", "3", "-4.5"], json!({"at": [3.0, -4.5]})),
            (
                &["pick", "3", "4", "--within", "00000000000000ff"],
                json!({"at": [3.0, 4.0], "within": "00000000000000ff"}),
            ),
            (&["types"], Value::Null),
            (&["cells"], Value::Null),
            (&["images"], Value::Null),
            (
                &[
                    "input",
                    r#"[{"key":{"key":"KeyW","pressed":true}},"cursor_left"]"#,
                ],
                json!({"events": [{"key": {"key": "KeyW", "pressed": true}}, "cursor_left"]}),
            ),
            (&["assets"], json!({"kind": null, "skip": 0, "take": 100})),
            (
                &["assets", "--kind", "model", "--take", "5"],
                json!({"kind": "model", "skip": 0, "take": 5}),
            ),
            (&["ui"], Value::Null),
            (&["debug"], json!({"wait_ms": 0})),
            (
                &["debug", "break", "scripts/spin.luau", "10", "12"],
                json!({
                    "wait_ms": 0,
                    "breakpoints": {"script": "scripts/spin.luau", "lines": [10, 12]},
                }),
            ),
            (
                &["debug", "break", "scripts/spin.luau"],
                json!({"wait_ms": 0, "breakpoints": {"script": "scripts/spin.luau", "lines": []}}),
            ),
            (
                &["debug", "continue", "--wait", "5"],
                json!({"wait_ms": 5000, "action": "continue"}),
            ),
            (
                &["debug", "out"],
                json!({"wait_ms": 0, "action": "step_out"}),
            ),
        ] {
            let Ok(Command::Call { method, params, .. }) = parsed(args) else {
                panic!("a call");
            };
            assert_eq!((method.as_str(), params), (args[0], want), "{args:?}");
        }
        for args in [
            &[
                "capture",
                "--show",
                "base_color,normal",
                "--show",
                "view depth",
            ][..],
            &[
                "capture",
                "--show=base_color",
                "--show",
                "normal,view depth",
            ],
        ] {
            let Ok(Command::Capture { params, shows, .. }) = parsed(args) else {
                panic!("captures");
            };
            let want = [json!("base_color"), json!("normal")];
            assert_eq!(shows[..2], want, "{args:?}");
            assert_eq!(shows[2], json!({"image": "view depth"}), "{args:?}");
            assert_eq!(params, json!({}));
        }
        let Ok(Command::Capture { params, shows, .. }) =
            parsed(&["capture", "--own", "--show", "normal,roughness"])
        else {
            panic!("captures");
        };
        assert_eq!((params, shows.len()), (json!({"own": true}), 2));
        let file = std::env::temp_dir().join(format!("gg-viewpoints-{}.json", std::process::id()));
        let viewpoint = json!({"position": [0.0, 1.0, 2.0], "rotation": [0.0, 0.0, 0.0, 1.0],
            "projection": {"Perspective": {"fov": 1.0, "near": 0.1}}});
        fs::write(&file, json!([viewpoint, viewpoint]).to_string()).unwrap();
        let path = file.to_str().unwrap();
        let read = parsed(&["frames", "2", "--viewpoints", path, "frames"]);
        fs::write(&file, r#"[{"position": [0, 1, 2]}]"#).unwrap();
        let refused = parsed(&["frames", "2", "--viewpoints", path, "frames"]);
        fs::remove_file(&file).unwrap();
        let Ok(Command::Call { params, .. }) = read else {
            panic!("frames");
        };
        assert_eq!(params["viewpoints"], json!([viewpoint, viewpoint]));
        assert_eq!(
            refused.err().map(|error| error.code).as_deref(),
            Some("gg.args")
        );
        for (args, want_own) in [
            (&["probe", "3", "4.5"][..], false),
            (&["probe", "--own", "3", "4.5"], true),
        ] {
            let Ok(Command::Probe { at, own, .. }) = parsed(args) else {
                panic!("a probe");
            };
            assert_eq!((at, own), ([3.0, 4.5], want_own), "{args:?}");
        }
        let Ok(Command::Logs { level, follow, .. }) = parsed(&["logs", "--level", "INFO"]) else {
            panic!("logs");
        };
        assert_eq!((level.as_str(), follow), ("info", false));
        let Ok(Command::Logs { level, follow, .. }) = parsed(&["logs", "--follow"]) else {
            panic!("logs");
        };
        assert_eq!((level.as_str(), follow), ("warn", true));
    }

    #[test]
    fn bad_arguments_are_refused_with_a_code() {
        for args in [
            &["nonsense"][..],
            &["help", "nonsense"],
            &["help", "run", "check"],
            &["help", "run", "--pid", "7"],
            &["--pid", "7"],
            &["--pid"],
            &["--release", "status"],
            &["wait", "now"],
            &["wait", "--timeout", "0"],
            &["wait", "--follow"],
            &["run"],
            &["run", "a", "b"],
            &["run", "a", "--snapshot", "often"],
            &["run", "a", "--peers", "3"],
            &["run", "a", "--peers", "0"],
            // Zero waits would kill a check at its start and give up before any answer.
            &["run", "a", "--hang", "soon"],
            &["check", "a", "--timeout", "0"],
            &["status", "--timeout", "0"],
            &["mcp", "--timeout=0"],
            &["run", "a", "--peers", "2", "--", "--host", "0.0.0.0:7700"],
            &[
                "run",
                "a",
                "--peers",
                "2",
                "--",
                "--code",
                "7KQ2-9XMB-4TZD-P0WH",
            ],
            &["run", "a", "--peers", "2", "--", "--record=run.recording"],
            &[
                "run",
                "a",
                "--peers",
                "2",
                "--",
                "--relay",
                "127.0.0.1:7800",
            ],
            &["snapshot", "a", "b"],
            &["snapshot", "--own"],
            &["scene"],
            &["scene", "open"],
            &["scene", "close"],
            &["scene", "new", "a.ron"],
            &["scene", "save", "--discard"],
            &["scene", "save", "a.ron", "b.ron"],
            &["capture", "a", "b"],
            &["capture", "--own=yes"],
            &["capture", "--show"],
            &["capture", "--show", ""],
            &["capture", "--show", "normal,"],
            // A PATH holds one PNG.
            &["capture", "--show", "normal,roughness", "view.png"],
            &[
                "capture",
                "--show",
                "normal",
                "--show",
                "roughness",
                "view.png",
            ],
            &["frames", "3"],
            &["frames", "three", "frames"],
            &["frames", "-1", "frames"],
            &["frames", "3", "frames", "more"],
            &["frames", "3", "frames", "--rect", "1,2,3"],
            &["frames", "3", "frames", "--rect", "1,2,3,a"],
            &["frames", "3", "frames", "--show", ""],
            &["frames", "3", "frames", "--viewpoints", "no such file.json"],
            &["look", "{not json"],
            &["look", "[]"],
            &["look", "{}", "{}"],
            &["pick", "1"],
            &["pick", "1", "2", "3"],
            &["pick", "left", "2"],
            &["pick", "1", "NaN"],
            &["probe"],
            &["probe", "1"],
            &["probe", "1", "2", "3"],
            &["probe", "left", "2"],
            &["probe", "1", "NaN"],
            // Pixels count from the view's top left, never negative.
            &["probe", "-1", "2"],
            &["probe", "1", "2", "--show", "normal"],
            &["probe", "1", "2", "--own=yes"],
            &["types", "Sprite"],
            &["input"],
            &["input", "{not json"],
            &["input", r#"{"key":{"key":"KeyW","pressed":true}}"#],
            &["input", "[]", "[]"],
            &["status", "--frames", "2"],
            &["status", "extra"],
            &["step", "many"],
            &["entities", "--take"],
            &["assets", "x"],
            &["ui", "menu"],
            &["cells", "0_0_0"],
            &["images", "bloom"],
            &["logs", "--level", "loud"],
            &["debug", "break"],
            &["debug", "break", "scripts/spin.luau", "0"],
            &["debug", "break", "scripts/spin.luau", "ten"],
            &["debug", "step"],
            &["debug", "continue", "now"],
            &["debug", "--wait", "soon"],
            &["call", "status", "{not json"],
            &["status", "--release=yes"],
            &["status", "--", "x"],
            &["check"],
            &["ship"],
            &["ship", "a", "b"],
            &["ship", "a", "--release"],
            &["ship", "a", "--", "--headless"],
            &["new"],
            &["new", "2probe"],
            &["new", "-probe"],
            &["new", "aux"],
            &["new", "a", "b"],
            &["new", "probe", "--project", "elsewhere"],
            &["new", "probe", "--engine"],
            &["new", "probe", "--engine="],
            &["new", "probe", "--engine", "https://example.com/engine.git"],
        ] {
            let error = parsed(args).err().map(|error| error.code);
            assert_eq!(error.as_deref(), Some("gg.args"), "{args:?}");
        }
        // An unreadable manifest is an error, but not the arguments'.
        let error = parsed(&["status", "--project", "src"]).err();
        assert_eq!(
            error.map(|error| error.code).as_deref(),
            Some("project.read")
        );
    }
}
