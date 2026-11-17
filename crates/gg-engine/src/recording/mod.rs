//! Recordings and replays. A recording holds a run's seed, the commands its layers' ticks applied,
//! and the world's hash at attach and after each tick. A replay reruns the ticks without a window
//! or GPU, each layer's with its recorded commands, and names the first hash that differs.
//!
//! A recording is text, a line at a time:
//!
//! ```text
//! gg-recording 7
//! app Sandbox
//! engine 0.1.0 debug
//! layers Sandbox PerfPanel Inspector
//! seed 3f8a6c2e9b1d4057
//! mod List([String("(name: \"rainbow\", ..."), String("rainbow.luau"), String("local ...")])
//! named List([String("55bcfa88ac30e5a5"), String("assets/sheet.png")])
//! attach 58183b6352a09a44 fb695ce29daa8479
//! > 0 Variant("Swarm", [("0", UInt(10000))])
//! 0 8c35cd01914c7df5 fb695ce29daa8479
//! end
//! ```
//!
//! In order:
//!
//! - Header: format and version, app, engine and build, layers bottom up, and the seed its world
//!   drew its IDs from.
//! - Each loaded mod: manifest text, then each script's path and text. A replay runs these, not
//!   the installed mods.
//! - The world's hash at attach: content, then layout.
//! - Per tick, commands as `>`, the applying layer's index and the value as `text` writes it;
//!   then the tick's number and the world's hash after it.
//! - `end`, once the run ended.
//!
//! A `patch` line (the library name) precedes the tick after a patch of the run's code applied
//! between ticks through the tool API. A replay can't apply it and stops there.
//!
//! A `cell` line in a tick, `cell 3_0_-2 9c1e5a07b2d84f36`, gives a cell of a world of
//! [cells](crate::Cells) whose file came in as the tick started, and its file's digest. A replay
//! stops where the file it read differs.
//!
//! A `named` line, before the first line that may name it, gives each texture named otherwise than
//! by path (as by ID): its name and path. A replay names it so, scanning no ID file, so it goes as
//! the run went wherever the ID files differ.

pub(crate) mod text;

use std::fmt::{self, Write as _};
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use gg_core::{Error, Result};
use tracing::{error, info, warn};

use crate::args::Dump;
use crate::assets::Counts;
use crate::cells::Cell;
use crate::ecs::WorldHash;
use crate::edits::Edit;
use crate::layer::Commands;
use crate::luau::ModText;
use crate::{App, BUILD, Context, Tick, World, perf};
use text::Text;

/// A recording's first line. A change to its lines or values' text takes the version up.
const FORMAT: &str = "gg-recording 7";

/// The format before `cell` lines. Read too.
const BEFORE_CELLS: &str = "gg-recording 6";

/// The format when `Amend` held several operations (now one `operation`) and the Inspector's
/// `Delete` and `Duplicate` several entities (now one `entity`). Read too.
const BEFORE_GESTURES: &str = "gg-recording 5";

/// The format before `patch` lines. Read too.
const BEFORE_PATCHES: &str = "gg-recording 4";

/// The format before `mod` lines. Read too, as runs without mods.
const BEFORE_MODS: &str = "gg-recording 3";

/// The format before `named` lines: values name every texture by path. Read too.
const BEFORE_NAMES: &str = "gg-recording 2";

/// The format before `@` lines (edits from outside the layers). Read too.
const BEFORE_EDITS: &str = "gg-recording 1";

/// Version and build, as a recording's `engine` line and the tool API's session file name it.
pub(crate) fn engine() -> String {
    format!("{} {BUILD}", env!("CARGO_PKG_VERSION"))
}

/// Writes a run's recording as it goes, each whole line unbuffered, so a killed or crashed run
/// leaves every line before; a panicking tick leaves its commands too.
pub(crate) struct Recorder {
    path: PathBuf,
    file: File,
    app: &'static str,
    /// Reused, so a tick without commands allocates nothing.
    line: String,
    /// Why it stopped (nothing more is written): the first failed write or unreplayable command.
    failed: Option<Error>,
    /// How many textures, models and fonts there were at the last `named` write, the engine's
    /// white texture included.
    named: Counts,
}

impl Recorder {
    /// Creates the file at `path`, replacing any, for a recording of `app`.
    ///
    /// Fails with `record.file` if it can't.
    pub(crate) fn create(path: &Path, app: &'static str) -> Result<Self> {
        let file = File::create(path).map_err(|error| {
            let message = format!("can't create the recording {}", path.display());
            Error::new("record.file", message).with_source(error)
        })?;
        Ok(Self {
            path: path.to_owned(),
            file,
            app,
            line: String::new(),
            failed: None,
            named: (0, 0, 0, 0),
        })
    }

    /// Writes the header: `layers` are type names, bottom up; `seed` starts the world's ID stream.
    pub(crate) fn start(&mut self, layers: &[&str], seed: u64) {
        info!(
            path = %self.path.display(),
            seed = %format_args!("{seed:016x}"),
            "Recording the run"
        );
        self.line.clear();
        let lines = [
            format!("{FORMAT}\n"),
            format!("app {}\n", self.app),
            format!("engine {}\n", engine()),
            format!("layers {}\n", layers.join(" ")),
            format!("seed {seed:016x}\n"),
        ];
        self.line.extend(lines);
        self.flush();
    }

    /// Writes the loaded `mods` and the world's hash once the layers have attached.
    pub(crate) fn attached(&mut self, world: &World, mods: &[ModText]) {
        for mod_text in mods {
            self.line.clear();
            self.line.push_str("mod ");
            text::write_mod(&mut self.line, mod_text);
            self.line.push('\n');
            self.flush();
        }
        self.names(world);
        self.line.clear();
        let hash = world.hash();
        writeln!(self.line, "attach {hash}").expect("a string takes any text");
        self.flush();
    }

    /// Writes a `named` line for each texture, model and font added since the last call that isn't
    /// named by its path, before any line that may name it.
    fn names(&mut self, world: &World) {
        let assets = world.assets();
        if assets.counts() == self.named {
            return;
        }
        for (name, path) in assets.named_from(self.named) {
            self.line.clear();
            self.line.push_str("named ");
            text::write_named(&mut self.line, name, path);
            self.line.push('\n');
            self.flush();
        }
        self.named = assets.counts();
    }

    /// Writes `commands`, the queue of layer `layer` (from the bottom) of type `name`, before its
    /// tick applies them, entities by ID in `world`. Stops the recording before a command the
    /// queue's [`check`](Commands::check) says a replay would refuse, such as one holding
    /// something opaque.
    pub(crate) fn queued(
        &mut self,
        layer: usize,
        name: &str,
        commands: &dyn Commands,
        world: &World,
    ) {
        self.names(world);
        for i in 0..commands.len() {
            let value = commands.value(i);
            self.line.clear();
            write!(self.line, "> {layer} ").expect("a string takes any text");
            let start = self.line.len();
            text::write(&mut self.line, &value, world);
            if self.failed.is_none()
                && let Err(error) = commands.check(&value)
            {
                let command = &self.line[start..];
                let message = format!(
                    "layer {layer}, {name}, queued `{command}`, which won't replay: {error}; the \
                     recording ends before it"
                );
                self.failed = Some(Error::new("record.bad_command", message));
            }
            self.line.push('\n');
            self.flush();
        }
    }

    /// Writes `edit` (from outside the layers, applied between ticks), entities by ID in `world`.
    /// It belongs to the tick after, before its commands.
    pub(crate) fn edited(&mut self, edit: &crate::reflect::Value, world: &World) {
        self.names(world);
        self.line.clear();
        self.line.push_str("@ ");
        text::write(&mut self.line, edit, world);
        self.line.push('\n');
        self.flush();
    }

    /// Writes a patch of the run's code between ticks, naming the library at `library` by file
    /// name alone, not where the machine kept it. It belongs to the tick after, before its edits
    /// and commands; a replay stops there.
    #[cfg(feature = "dev")]
    pub(crate) fn patched(&mut self, library: &Path) {
        let name = library.file_name().unwrap_or(library.as_os_str());
        self.line.clear();
        self.line.push_str("patch ");
        text::write_patch(&mut self.line, &name.to_string_lossy());
        self.line.push('\n');
        self.flush();
    }

    /// Writes that `cell`'s file, of `digest`, came in as the tick started; it belongs to that
    /// tick.
    pub(crate) fn cell_came(&mut self, cell: Cell, digest: u64) {
        self.line.clear();
        writeln!(self.line, "cell {cell} {digest:016x}").expect("a string takes any text");
        self.flush();
    }

    pub(crate) fn ticked(&mut self, tick: Tick, world: &World) {
        self.names(world);
        self.line.clear();
        let hash = world.hash();
        writeln!(self.line, "{} {hash}", tick.number).expect("a string takes any text");
        self.flush();
    }

    pub(crate) fn end(&mut self) {
        self.line.clear();
        self.line.push_str("end\n");
        self.flush();
    }

    fn flush(&mut self) {
        if self.failed.is_none()
            && let Err(error) = self.file.write_all(self.line.as_bytes())
        {
            let path = self.path.display();
            let message =
                format!("can't write the recording {path}: {error}; it ends where it failed");
            self.failed = Some(Error::new("record.file", message));
        }
    }

    /// Fails once the recording has stopped, at the last whole line: `record.file` after a failed
    /// write; `record.bad_command`, naming the layer, command and what a replay would refuse,
    /// after a layer queued one that wouldn't replay.
    pub(crate) fn check(&self) -> Result<()> {
        match &self.failed {
            Some(error) => Err(Error::new(error.code(), error.to_string())),
            None => Ok(()),
        }
    }
}

struct Recording {
    app: String,
    engine: String,
    /// Layer type names, bottom up.
    layers: Vec<String>,
    seed: u64,
    /// The mods the run loaded. A replay runs these, not the installed ones.
    mods: Vec<ModText>,
    /// Each texture not named by its path: its name and path.
    #[cfg_attr(
        not(feature = "dev"),
        allow(
            dead_code,
            reason = "builds without `dev` name every texture by its path"
        )
    )]
    names: Vec<(Arc<str>, Arc<str>)>,
    /// The world's hash at attach, and its line.
    attach: (WorldHash, usize),
    ticks: Vec<Ticked>,
    /// Whether it has its `end` line; a run cut short hasn't.
    complete: bool,
}

/// A recorded tick: its commands in order, and the world's hash after it with its line. The hash
/// is `None` for the commands after the last hash of a recording cut short.
#[derive(Default)]
struct Ticked {
    /// The library of the first patch applied before it, and its line. A replay stops there.
    patch: Option<(String, usize)>,
    /// The edits applied before it, each a value and its line.
    edits: Vec<(Text, usize)>,
    /// The cells whose files came in as it started: cell, file digest and line.
    cells: Vec<(Cell, u64, usize)>,
    commands: Vec<Recorded>,
    hash: Option<(WorldHash, usize)>,
}

struct Recorded {
    layer: usize,
    value: Text,
    line: usize,
}

/// The lines of `text` that end in a line break, numbered from 1: all but a last one cut off
/// mid-write.
fn whole_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let lines = text
        .split_inclusive('\n')
        .map_while(|line| line.strip_suffix('\n'));
    lines.enumerate().map(|(i, line)| (i + 1, line))
}

/// Line `line` isn't a recording's.
fn syntax(line: usize, what: impl fmt::Display) -> Error {
    Error::new("replay.syntax", format!("line {line}: {what}"))
}

/// The recording isn't one the app replays.
fn format_error(what: impl fmt::Display) -> Error {
    Error::new("replay.format", what.to_string())
}

fn parse_hash(text: &str) -> Option<WorldHash> {
    let (content, layout) = text.split_once(' ')?;
    Some(WorldHash {
        content: hex16(content)?,
        layout: hex16(layout)?,
    })
}

/// `text` as 16 hex digits: a seed or half a hash.
fn hex16(text: &str) -> Option<u64> {
    let digits = text.len() == 16 && text.bytes().all(|b| b.is_ascii_hexdigit());
    digits.then(|| u64::from_str_radix(text, 16).ok()).flatten()
}

impl Recording {
    /// Reads `text`, but for a last line without its line break (a run killed mid-write).
    ///
    /// Fails with:
    ///
    /// - `replay.format`: not a recording, another version, or ends in its header.
    /// - `replay.syntax`, naming the line: not a recording's line, a bad value, or a tick out of
    ///   order.
    /// - `replay.bad_command`, naming the line: a command of a layer the recording hasn't.
    fn parse(text: &str) -> Result<Self> {
        let mut lines = whole_lines(text).peekable();
        match lines.next() {
            Some((
                _,
                FORMAT | BEFORE_CELLS | BEFORE_GESTURES | BEFORE_PATCHES | BEFORE_MODS
                | BEFORE_NAMES | BEFORE_EDITS,
            )) => {}
            Some((_, line)) if line.starts_with("gg-recording ") => {
                return Err(format_error(format!(
                    "it's a recording of format `{line}`, but this engine reads `{FORMAT}`"
                )));
            }
            first => {
                let first = first.map_or("", |(_, line)| line);
                return Err(format_error(format!(
                    "it isn't a recording: a recording's first line is `{FORMAT}`, and its is \
                     `{first}`"
                )));
            }
        }
        let header = |lines: &mut Peekable<_>, key: &str| {
            let (number, line): (usize, &str) = lines.next().ok_or_else(|| {
                format_error(format!("it ends in its header, before its `{key}` line"))
            })?;
            let value = line
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(' '));
            let value = value.ok_or_else(|| {
                syntax(number, format!("expected the `{key}` line, found `{line}`"))
            })?;
            Ok::<_, Error>((number, value))
        };
        let app = header(&mut lines, "app")?.1.to_owned();
        let engine = header(&mut lines, "engine")?.1.to_owned();
        // Empty for an app without layers, like the player.
        let layers = header(&mut lines, "layers")?.1.split_whitespace();
        let layers: Vec<String> = layers.map(str::to_owned).collect();
        let (number, seed) = header(&mut lines, "seed")?;
        let seed = hex16(seed).ok_or_else(|| syntax(number, "a seed is 16 hex digits"))?;
        let mut mods = Vec::new();
        while let Some((number, line)) = lines.next_if(|(_, line)| line.starts_with("mod ")) {
            let read = text::read_mod(&line["mod ".len()..], "mod ".len() + 1);
            let what = "a mod is a list of its manifest's text, then each of its scripts' path and \
                        text";
            mods.push(read.ok_or_else(|| syntax(number, what))?);
        }
        let named = |number: usize, named: &str| {
            let read = text::read_named(named, "named ".len() + 1);
            let what = "a texture's name is a list of it and the path it names";
            read.ok_or_else(|| syntax(number, what))
        };
        // The textures the layers added as they loaded, before the attach.
        let mut names = Vec::new();
        while let Some((number, line)) = lines.next_if(|(_, line)| line.starts_with("named ")) {
            names.push(named(number, &line["named ".len()..])?);
        }
        let (number, attach) = header(&mut lines, "attach")?;
        let attach =
            parse_hash(attach).ok_or_else(|| syntax(number, "a hash is two of 16 hex digits"))?;
        let mut recording = Self {
            app,
            engine,
            layers,
            seed,
            mods,
            names,
            attach: (attach, number),
            ticks: Vec::new(),
            complete: false,
        };
        let mut ticked = Ticked::default();
        for (number, line) in lines {
            if recording.complete {
                return Err(syntax(number, "a line after `end`, which ends a recording"));
            }
            if line == "end" {
                recording.complete = true;
            } else if let Some(texture) = line.strip_prefix("named ") {
                recording.names.push(named(number, texture)?);
            } else if let Some(library) = line.strip_prefix("patch ") {
                let library = text::read_patch(library, "patch ".len() + 1);
                let library = library.ok_or_else(|| syntax(number, "a patch's is its library"))?;
                ticked.patch.get_or_insert((library, number));
            } else if let Some(came) = line.strip_prefix("cell ") {
                let came = came.split_once(' ').and_then(|(cell, digest)| {
                    Some((Cell::of_file(&format!("{cell}.ron"))?, hex16(digest)?))
                });
                let what = "a cell's is its place on the grid, as `3_0_-2`, and 16 hex digits";
                let (cell, digest) = came.ok_or_else(|| syntax(number, what))?;
                ticked.cells.push((cell, digest, number));
            } else if let Some(command) = line.strip_prefix("> ") {
                ticked.commands.push(recording.command(number, command)?);
            } else if let Some(edit) = line.strip_prefix("@ ") {
                let edit = Text::parse(edit, "@ ".len() + 1).map_err(|error| {
                    let message = format!("line {number}: {error}");
                    error.with_message(message)
                })?;
                ticked.edits.push((edit, number));
            } else {
                let tick = line.split_once(' ').and_then(|(tick, hash)| {
                    Some((tick.parse::<usize>().ok()?, parse_hash(hash)?))
                });
                let Some((tick, hash)) = tick else {
                    return Err(syntax(
                        number,
                        format!(
                            "expected a command, `> <layer> <value>`, an edit, `@ <value>`, a tick's hash, \
                             `<tick> <content> <layout>`, a texture's name, `named <value>`, a \
                             patch, `patch <library>`, a cell's file, `cell <cell> <digest>`, or \
                             `end`; found `{line}`"
                        ),
                    ));
                };
                let next = recording.ticks.len();
                if tick != next {
                    return Err(syntax(
                        number,
                        format!("tick {tick}'s hash, where tick {next}'s comes"),
                    ));
                }
                ticked.hash = Some((hash, number));
                recording.ticks.push(std::mem::take(&mut ticked));
            }
        }
        // A tick cut short runs its commands again unchecked, so what stopped it stops the replay
        // too. Edits or patches alone, applied after the last tick, have no tick to run with.
        if !ticked.commands.is_empty() {
            recording.ticks.push(ticked);
        }
        Ok(recording)
    }

    fn command(&self, number: usize, command: &str) -> Result<Recorded> {
        let (layer, value) = command.split_once(' ').unwrap_or((command, ""));
        // Where the value starts: after `> `, the layer and a space.
        let column = "> ".len() + layer.len() + 2;
        let layers = self.layers.len();
        let layer = layer.parse().ok().filter(|&layer| layer < layers);
        let layer = layer.ok_or_else(|| {
            let what = format!("a command for a layer the recording's {layers} haven't");
            Error::new("replay.bad_command", format!("line {number}: {what}"))
        })?;
        let value = Text::parse(value, column).map_err(|error| {
            let message = format!("line {number}: {error}");
            error.with_message(message)
        })?;
        Ok(Recorded {
            layer,
            value,
            line: number,
        })
    }
}

/// A recording run again: the app's layers attach in a world from its seed, then its ticks run
/// alone (no update, UI or event between), without a window or GPU. Each layer's tick gets its
/// recorded commands, not what it queued, and each is checked against the recorded hash. Dropped,
/// it detaches the layers.
#[doc(hidden)]
pub struct Replay {
    app: App,
    cx: Context,
    recording: Recording,
    ran: usize,
}

impl Replay {
    /// Reads `text`, a recording of `app`, attaches the layers in a world from its seed and checks
    /// the world's hash.
    ///
    /// Fails with:
    ///
    /// - `replay.format`: not a recording, another version, ends in its header, or another app or
    ///   other layers.
    /// - `replay.syntax`, naming the line: not a recording's line.
    /// - `replay.bad_command`, naming the line: a command of a layer the recording hasn't.
    /// - `replay.diverged`, like [`step`](Self::step).
    ///
    /// Also fails if the project manifest didn't read, as [`App::project`] says, and as a layer's
    /// [`Layer::load`](crate::Layer::load) does.
    pub fn new(app: App, text: &str) -> Result<Self> {
        let replay = Self::attach(app, text)?;
        replay.check(None)?;
        Ok(replay)
    }

    /// Like [`new`](Self::new), without checking the world's hash.
    fn attach(mut app: App, text: &str) -> Result<Self> {
        if let Some(Err(error)) = app.project.take_if(|project| project.is_err()) {
            return Err(error);
        }
        let recording = Recording::parse(text)?;
        if recording.app != app.name {
            return Err(format_error(format!(
                "it's a recording of app `{}`, not `{}`",
                recording.app, app.name
            )));
        }
        let names = app.stack.names();
        if !recording
            .layers
            .iter()
            .map(String::as_str)
            .eq(names.iter().copied())
        {
            return Err(format_error(format!(
                "its layers are `{}`, from the bottom up, but the app's are `{}`",
                recording.layers.join(" "),
                names.join(" ")
            )));
        }
        let mut cx = Context::seeded(recording.seed);
        cx.mods.game = app.name.to_owned();
        cx.mods.recorded = Some(recording.mods.clone());
        // Textures are named as in the run: by the recording's names or paths from the root,
        // scanning no ID file (an unloadable one stands in). It starts in the run's scene, read
        // from the game's package if any.
        cx.root.clone_from(&app.root);
        cx.files = app.files.clone();
        #[cfg(feature = "dev")]
        cx.name_as(recording.names.iter().cloned());
        cx.follows = true;
        cx.start.clone_from(&app.start);
        app.stack.attach(&mut cx)?;
        Ok(Self {
            app,
            cx,
            recording,
            ran: 0,
        })
    }

    /// Runs the next tick and returns it, or `None` past the last. Each layer's tick gets its
    /// recorded commands, read against the world as the tick finds it. The hash after the tick is
    /// checked, except for commands after the last hash of a recording cut short.
    ///
    /// Fails, naming the line, with:
    ///
    /// - `replay.bad_command`: a command for a layer that queues none, or that its layer refuses.
    /// - `replay.patched`, running nothing: the run's code was patched before this tick, and the
    ///   replay runs its own.
    /// - `replay.cell_file`: a cell came in from another file than the run's, as
    ///   [`check_cells`](Self::check_cells) says.
    /// - `replay.diverged`: the hash differs. It says which part (content, layout or both), gives
    ///   both hashes, and both builds if another build recorded it. The world stays as the tick
    ///   left it.
    pub fn step(&mut self) -> Result<Option<Tick>> {
        let Some(tick) = self.run_tick()? else {
            return Ok(None);
        };
        self.check(Some(tick))?;
        Ok(Some(tick))
    }

    /// Fails with `replay.cell_file`, naming line, tick and file, where a cell that came in as
    /// `tick` started came in here from a file of another digest, or not at all. The replay reads
    /// the cells' files as they are now, which an edit since changes. What came is what the tick
    /// noted as it started: a cell that came may go out again as the tick ends.
    fn check_cells(&self, tick: Tick) -> Result<()> {
        let ticked = &self.recording.ticks[tick.number as usize];
        let world = self.cx.world();
        let came = &self.cx.cells.came;
        for &(cell, digest, line) in &ticked.cells {
            let here = came.iter().find(|&&(came, _)| came == cell);
            let here = here.map(|&(_, digest)| digest);
            if here == Some(digest) {
                continue;
            }
            let here = here.map_or_else(
                || "none, as it didn't come in".to_owned(),
                |here| format!("one of digest {here:016x}"),
            );
            let message = format!(
                "line {line}: tick {} brought cell {cell} in from the file {}, of digest \
                 {digest:016x} in the run, and here from {here}: the replay reads the cells' files as \
                 they are now, and this one isn't the run's",
                tick.number,
                world.cell_file(cell)
            );
            return Err(Error::new("replay.cell_file", message));
        }
        Ok(())
    }

    /// Like [`step`](Self::step), without checking the world's hash.
    fn run_tick(&mut self) -> Result<Option<Tick>> {
        let Some(ticked) = self.recording.ticks.get(self.ran) else {
            return Ok(None);
        };
        let tick = Tick {
            number: self.ran as u64,
        };
        if let Some((library, line)) = &ticked.patch {
            let message = format!(
                "line {line}: the run's code was patched before tick {}, with {library}, which a \
                 replay can't apply, running its own code: it follows the run only to there",
                tick.number
            );
            return Err(Error::new("replay.patched", message));
        }
        self.ran += 1;
        // Refused where the run refused them (the hash checks it), each read once the textures it
        // names are loaded, as in the run.
        for (edit, line) in &ticked.edits {
            self.cx.load_followed(edit);
            let edit = Edit::from_value(&edit.value(self.cx.world())).map_err(|error| {
                let message = format!("line {line}: an edit that isn't one: {error}");
                Error::new("replay.bad_command", message)
            })?;
            let _ = self.cx.apply_edit(edit);
        }
        for command in &ticked.commands {
            self.cx.load_followed(&command.value);
        }
        let names = &self.recording.layers;
        self.app
            .stack
            .replay_tick(&mut self.cx, tick, |layer, commands, world| {
                let mut recorded = ticked.commands.iter().filter(|c| c.layer == layer);
                let name = &names[layer];
                let Some(commands) = commands else {
                    return match recorded.next() {
                        Some(command) => Err(Error::new(
                            "replay.bad_command",
                            format!(
                                "line {}: a command for layer {layer}, {name}, which queues none",
                                command.line
                            ),
                        )),
                        None => Ok(()),
                    };
                };
                commands.clear();
                for command in recorded {
                    commands
                        .push(&command.value.value(world))
                        .map_err(|error| {
                            let line = command.line;
                            let what =
                                format!("layer {layer}, {name}, refuses the command: {error}");
                            Error::new("replay.bad_command", format!("line {line}: {what}"))
                        })?;
                }
                Ok(())
            })?;
        Ok(Some(tick))
    }

    /// Checks the world's hash against the recorded one at attach (`None`) or after `tick`, if
    /// any; for a tick, the cells' files first, as [`check_cells`](Self::check_cells) does.
    fn check(&self, tick: Option<Tick>) -> Result<()> {
        if let Some(tick) = tick {
            self.check_cells(tick)?;
        }
        let recorded = match tick {
            None => Some(self.recording.attach),
            Some(tick) => self.recording.ticks[tick.number as usize].hash,
        };
        let Some((recorded, line)) = recorded else {
            return Ok(());
        };
        let hash = self.world().hash();
        if hash == recorded {
            return Ok(());
        }
        let part = match (
            hash.content != recorded.content,
            hash.layout != recorded.layout,
        ) {
            (true, true) => "content and layout go",
            (true, false) => "content goes",
            _ => "layout goes",
        };
        let at = tick.map_or("once the layers attach".to_owned(), |tick| {
            format!("after tick {}", tick.number)
        });
        let mut message = format!(
            "line {line}: the world's {part} another way {at}: the recording has `{recorded}`, and \
             the replay `{hash}`"
        );
        let here = engine();
        if self.recording.engine != here {
            let there = &self.recording.engine;
            message += &format!("; engine {there} recorded it, and engine {here} replays it");
        }
        Err(Error::new("replay.diverged", message))
    }

    /// The world after the last tick, or the attach before the first.
    pub fn world(&self) -> &World {
        self.cx.world()
    }

    /// The recording `text`, of `app`, with this build's hashes in place of the recorded ones, for
    /// a change meant to change the simulation or what the hash reads. A last line without its
    /// line break is dropped.
    ///
    /// Fails like [`new`](Self::new) and [`step`](Self::step), except for a differing hash.
    pub fn rewrite(app: App, text: &str) -> Result<String> {
        let mut replay = Self::attach(app, text)?;
        let mut lines: Vec<String> = whole_lines(text).map(|(_, line)| line.to_owned()).collect();
        let (_, at) = replay.recording.attach;
        lines[at - 1] = format!("attach {}", replay.world().hash());
        while let Some(tick) = replay.run_tick()? {
            if let Some((_, at)) = replay.recording.ticks[tick.number as usize].hash {
                let hash = replay.world().hash();
                lines[at - 1] = format!("{} {hash}", tick.number);
            }
        }
        Ok(lines.iter().map(|line| format!("{line}\n")).collect())
    }

    /// Runs as `--replay` does: checks the attach hash, then runs and checks each tick to the last
    /// or, with `dump`, where it says, writing the world there to `out` as [`World::dump`] does.
    pub(crate) fn run(&mut self, dump: Option<Dump>, out: &mut impl io::Write) -> Result<()> {
        let attached = self.check(None);
        if dump == Some(Dump::Attach) {
            self.dump(out)?;
            return attached;
        }
        attached?;
        while let Some(tick) = self.run_tick()? {
            let checked = self.check(Some(tick));
            if dump == Some(Dump::Tick(tick.number)) {
                self.dump(out)?;
                return checked;
            }
            checked?;
        }
        match dump {
            Some(Dump::Tick(number)) => Err(Error::new(
                "replay.no_tick",
                format!(
                    "the recording has {} ticks, so no tick {number} to stop after",
                    self.ran
                ),
            )),
            _ => Ok(()),
        }
    }

    fn dump(&self, out: &mut impl io::Write) -> Result<()> {
        let written = out.write_all(self.world().dump().as_bytes());
        written.and_then(|()| out.flush()).map_err(|error| {
            let message = "can't write the world to stdout";
            Error::new("replay.stdout", message).with_source(error)
        })
    }
}

impl Drop for Replay {
    fn drop(&mut self) {
        self.app.stack.detach(&mut self.cx);
    }
}

/// Replays the recording at `path`, of `app`, as `--replay` does: to its end or, with `dump`, where
/// it says, writing the world to stdout. Returns the exit status: 0 if every checked hash matched.
pub(crate) fn replay(app: App, path: &Path, dump: Option<Dump>) -> ExitCode {
    let started = Instant::now();
    let failed = |error: Error, hint: &str| {
        let shown = path.display();
        error!(code = error.code(), path = %shown, "{shown}: {error:#}{hint}");
        ExitCode::FAILURE
    };
    // `fs::read`, not `dirs::read_file`: a recording may come through a pipe.
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let message = "can't read the recording";
            return failed(Error::new("replay.file", message).with_source(error), "");
        }
    };
    // Whole lines alone: a last line cut off mid-write may end partway through a character.
    let whole = bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    let text = match std::str::from_utf8(&bytes[..whole]) {
        Ok(text) => text,
        Err(error) => {
            let message = "the recording isn't UTF-8 text";
            return failed(Error::new("replay.file", message).with_source(error), "");
        }
    };
    let mut replay = match Replay::attach(app, text) {
        Ok(replay) => replay,
        Err(error) => return failed(error, ""),
    };
    if !replay.recording.complete {
        warn!(
            code = "replay.cut_short",
            path = %path.display(),
            "The recording has no `end`, so its run was cut short: it replays the ticks it has \
             whole, and the commands after the last unchecked"
        );
    }
    match replay.run(dump, &mut io::stdout()) {
        Ok(()) => {
            info!(
                code = "replay.matched",
                ticks = replay.ran,
                ms = perf::millis(started.elapsed()),
                "Every hash the replay checked is the one recorded"
            );
            ExitCode::SUCCESS
        }
        Err(error) if error.code() == "replay.diverged" => {
            let at = match replay.ran.checked_sub(1) {
                None => "attach".to_owned(),
                Some(tick) => tick.to_string(),
            };
            let hint = format!(
                "; `--replay {} --dump {at}`, run here and where it replays to its hashes, writes \
                 each world's content, whose diff is the field where they part, though where the \
                 layout alone goes another way they dump the same",
                path.display()
            );
            failed(error, &hint)
        }
        Err(error) => failed(error, ""),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::ops::ControlFlow;
    use std::rc::Rc;
    use std::time::Duration;

    use gg_core::math::Quat;

    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::layer::Stack;
    use crate::main_loop::Stepper;
    use crate::main_loop::tests::{Turner, red};
    use crate::reflect::tests::Handle;
    use crate::{
        Actions, Binding, Change, Component, Event, Key, Layer, Reflect, Source, Sprite, Transform,
        args, recording,
    };

    /// Spawns an entity turned by this angle.
    #[derive(Reflect)]
    struct Spawn(f32);

    /// Queues a spawn at attach and every third update.
    #[derive(Default)]
    struct Queuer {
        commands: Vec<Spawn>,
        updates: u32,
    }

    impl Layer for Queuer {
        fn attach(&mut self, _: &mut Context) {
            self.commands.push(Spawn(0.5));
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Spawn(angle) in self.commands.drain(..) {
                world.spawn(Transform {
                    rotation: Quat::from_rotation_z(angle),
                    ..Transform::IDENTITY
                });
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.commands)
        }

        fn update(&mut self, _: &mut Context, _: Duration) {
            self.updates += 1;
            if self.updates.is_multiple_of(3) {
                self.commands.push(Spawn(self.updates as f32 * 0.25));
            }
        }
    }

    /// Spawns this sprite, with a handle of the layer's own: opaque, so no value makes it.
    #[derive(Reflect)]
    struct Place(Sprite, #[reflect(opaque)] Handle);

    /// Queues a sprite at attach.
    #[derive(Default)]
    struct Placer(Vec<Place>);

    impl Layer for Placer {
        fn attach(&mut self, _: &mut Context) {
            self.0.push(Place(red(), Handle(1)));
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Place(sprite, _) in self.0.drain(..) {
                world.spawn((Transform::IDENTITY, sprite));
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.0)
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Act {
        Push,
        Tap,
    }

    /// A change of one of [`Pilot`]'s actions.
    #[derive(Reflect)]
    enum Control {
        Push(f32),
        Tap,
    }

    /// What [`Pilot`]'s commands leave: its push, the ticks it was pushed at, and its taps.
    #[derive(Clone, Copy, Component)]
    struct Piloted {
        push: f32,
        pushed: u32,
        taps: u32,
    }

    /// Pushes while an arrow key is held and counts Space's taps, queuing action changes as events
    /// come and noting the tick each applies at.
    struct Pilot {
        actions: Actions<Act>,
        commands: Vec<Control>,
        applied: Rc<RefCell<Vec<u64>>>,
    }

    impl Pilot {
        fn new(applied: Rc<RefCell<Vec<u64>>>) -> Self {
            let mut actions = Actions::default();
            let push = Binding::Axis {
                negative: Source::Key(Key::ArrowLeft),
                positive: Source::Key(Key::ArrowRight),
            };
            let tap = Binding::Button(Source::Key(Key::Space));
            let bound = actions.bind(Act::Push, push);
            bound
                .and_then(|()| actions.bind(Act::Tap, tap))
                .expect("bound");
            Self {
                actions,
                commands: Vec::new(),
                applied,
            }
        }
    }

    impl Layer for Pilot {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Piloted {
                push: 0.0,
                pushed: 0,
                taps: 0,
            });
        }

        fn tick(&mut self, world: &mut World, tick: Tick) {
            let mut piloted = world.query_mut::<&mut Piloted>();
            let piloted = piloted.next().expect("the pilot's entity");
            for command in self.commands.drain(..) {
                match command {
                    Control::Push(by) => piloted.push = by,
                    Control::Tap => piloted.taps += 1,
                }
                self.applied.borrow_mut().push(tick.number);
            }
            if piloted.push != 0.0 {
                piloted.pushed += 1;
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.commands)
        }

        fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
            let Self {
                actions, commands, ..
            } = self;
            actions.event(event, |change| match change {
                Change::Axis(Act::Push, by) => commands.push(Control::Push(by)),
                Change::Pressed(Act::Tap) => commands.push(Control::Tap),
                _ => {}
            });
            ControlFlow::Continue(())
        }
    }

    /// Spawns a sprite of this texture.
    #[cfg(feature = "dev")]
    #[derive(Reflect)]
    struct Paint(crate::Texture);

    /// Adds `assets/stone.png` under the root as a texture at load and queues a sprite of it at
    /// attach.
    #[cfg(feature = "dev")]
    #[derive(Default)]
    struct Painter {
        stone: Option<crate::Texture>,
        commands: Vec<Paint>,
    }

    #[cfg(feature = "dev")]
    impl Layer for Painter {
        fn load(&mut self, cx: &mut Context) -> Result<()> {
            let root = cx.root.clone().expect("a root");
            let stone = cx.add_texture("stone", root.join("assets/stone.png"));
            self.stone = Some(stone.expect("a texture"));
            Ok(())
        }

        fn attach(&mut self, _: &mut Context) {
            self.commands.push(Paint(self.stone.expect("loaded")));
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            for Paint(texture) in self.commands.drain(..) {
                let sprite = Sprite {
                    texture: Some(texture),
                    ..Sprite::default()
                };
                world.spawn((Transform::IDENTITY, sprite));
            }
        }

        fn commands(&mut self) -> Option<&mut dyn Commands> {
            Some(&mut self.commands)
        }
    }

    /// A command-queuing layer under one without.
    fn two_layers() -> App {
        let app = App::new("Recorded").layer(Queuer::default());
        app.layer(Turner(Rc::default()))
    }

    /// Records `app` over frames `frames` apart, as a windowed run steps them: the recording,
    /// ticks run and final world hash.
    fn recorded(app: App, frames: &[Duration]) -> (String, u64, WorldHash) {
        let (text, ticks, world) = recorded_with(app, frames, |_, _, _| {});
        (text, ticks, world.hash())
    }

    /// Like [`recorded`], with `between` sending events before each frame (given the ticks run so
    /// far), and the final world.
    fn recorded_with(
        app: App,
        frames: &[Duration],
        between: impl FnMut(&mut Stack, &mut Context, u64),
    ) -> (String, u64, World) {
        recorded_in(app, Context::new(None), frames, between)
    }

    fn recorded_in(
        mut app: App,
        mut cx: Context,
        frames: &[Duration],
        mut between: impl FnMut(&mut Stack, &mut Context, u64),
    ) -> (String, u64, World) {
        // Per thread: recording tests run at once, and a new scratch directory empties its name.
        let thread = std::thread::current().id();
        let dir = Scratch::new(&format!("recording-{thread:?}"));
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("run.recording");
        let recorder = Recorder::create(&path, app.name).expect("created");
        app.stack.record(recorder);
        app.stack.attach(&mut cx).expect("loaded");
        let mut stepper = Stepper::default();
        for &dt in frames {
            between(&mut app.stack, &mut cx, stepper.ticks);
            stepper.step(dt, |tick| app.stack.tick(&mut cx, tick), || Duration::ZERO);
            app.stack.update(&mut cx, dt);
        }
        let ended = cx.world().clone();
        app.stack.detach(&mut cx);
        app.stack.recorded().expect("written");
        let text = std::fs::read_to_string(&path).expect("written");
        (text, stepper.ticks, ended)
    }

    /// Replays `text` to its end: ticks run and final hash, or ticks run before the error.
    fn replay_all(app: App, text: &str) -> Result<(u64, WorldHash), (u64, gg_core::Error)> {
        let mut replay = Replay::new(app, text).map_err(|error| (0, error))?;
        let mut ticks = 0;
        loop {
            match replay.step() {
                Ok(Some(tick)) => {
                    assert_eq!(tick.number, ticks);
                    ticks += 1;
                }
                Ok(None) => return Ok((ticks, replay.world().hash())),
                Err(error) => return Err((ticks, error)),
            }
        }
    }

    fn changed(text: &str, at: usize, line: &str) -> String {
        let mut lines: Vec<&str> = text.lines().collect();
        lines[at] = line;
        lines.iter().map(|line| format!("{line}\n")).collect()
    }

    /// A texture named by ID is recorded with its path before the first line naming it. Replay
    /// names it so, reading no ID file, and matches its hashes once the ID file is gone, loading
    /// the texture an edit names as it reads the edit. Without the `named` lines, stand-ins take
    /// the places of the IDs the command and edit name; hashes read names alone, so they still
    /// match.
    #[cfg(feature = "dev")]
    #[test]
    fn a_replay_names_textures_as_its_recording_says_reading_no_id_file() {
        let logged = crate::testing::logged();
        let dir = Scratch::new("recording-named");
        let root = dir.0.clone();
        fs::create_dir_all(root.join("assets")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        let (id, edited) = ("5f3c9a1e07b2d846", "0a1b2c3d4e5f6071");
        for (name, id) in [("stone", id), ("edited", edited)] {
            fs::write(root.join(format!("assets/{name}.png")), &png).expect("a PNG");
            let meta = gg_api::asset::Meta {
                id: id.parse().expect("an ID"),
            };
            let file = root.join(format!("assets/{name}.png.meta"));
            fs::write(file, meta.text()).expect("its ID file");
        }
        let app = || App::new("Named").root(&root).layer(Painter::default());

        let path = dir.0.join("run.recording");
        let mut run = app();
        run.stack
            .record(Recorder::create(&path, run.name).expect("created"));
        let mut cx = Context::seeded(1).root(root.clone());
        run.stack.attach(&mut cx).expect("loaded");
        run.stack.tick(&mut cx, Tick { number: 0 });
        // An edit naming a texture loaded for it, as the tool API loads one.
        cx.load_named([crate::assets::Asset::Texture(edited)], false);
        let sprite = Sprite {
            texture: cx.world().assets().texture(edited),
            ..Sprite::default()
        };
        let create = crate::ecs::Operation::Create {
            entity: None,
            parent: None,
            components: vec![("Sprite".into(), sprite.to_value())],
        };
        let edit = Edit::Apply {
            author: "agent".to_owned(),
            operations: vec![create],
        };
        run.stack.edit(&mut cx, edit).expect("applied");
        run.stack.tick(&mut cx, Tick { number: 1 });
        let ended = cx.world().hash();
        run.stack.detach(&mut cx);
        run.stack.recorded().expect("written");
        let text = fs::read_to_string(&path).expect("written");
        let lines: Vec<&str> = text.lines().collect();
        let named = format!("named List([String(\"{id}\"), String(\"assets/stone.png\")])");
        let at = lines.iter().position(|line| *line == named);
        let at = at.unwrap_or_else(|| panic!("{text}"));
        assert!(lines[at + 1].starts_with("attach "), "{text}");
        let painted = format!("> 0 Struct([(\"0\", Texture(\"{id}\"))])");
        assert!(lines.contains(&&*painted), "{text}");

        for name in ["stone", "edited"] {
            let file = root.join(format!("assets/{name}.png.meta"));
            fs::remove_file(file).expect("removed");
        }
        let stood_in = || {
            logged
                .codes
                .try_iter()
                .filter(|code| code == "asset.stand_in")
                .count()
        };
        let _ = stood_in();
        let replayed = replay_all(app(), &text).unwrap_or_else(|(_, error)| panic!("{error}"));
        assert_eq!((replayed, stood_in()), ((2, ended), 0));
        let unnamed = text.lines().filter(|line| !line.starts_with("named "));
        let unnamed: String = unnamed.map(|line| format!("{line}\n")).collect();
        let replayed = replay_all(app(), &unnamed).unwrap_or_else(|(_, error)| panic!("{error}"));
        assert_eq!((replayed, stood_in()), ((2, ended), 2));
    }

    /// A run recorded at its own frame rate replays, without frames, to its hashes and final
    /// world. A copy with a changed command or hash diverges at that tick; one cut off mid-line
    /// replays its whole lines.
    #[test]
    fn a_recorded_run_replays_to_its_hashes() {
        let app = two_layers;
        let ms = Duration::from_millis;
        // Some frames run no tick, and some several.
        let frames = [ms(1), ms(10), ms(40), Tick::DT, ms(2), ms(48), ms(130)].repeat(6);
        let (text, ticks, ended) = recorded(app(), &frames);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[3], "layers Queuer Turner", "{text}");
        assert_eq!(lines.last(), Some(&"end"), "{text}");
        let commands = lines.iter().filter(|line| line.starts_with("> 0 ")).count();
        assert_eq!(
            commands,
            1 + 42 / 3 - 1,
            "the last update's waits for a tick: {text}"
        );

        let replayed = |text: &str| replay_all(app(), text);
        assert_eq!(replayed(&text).ok(), Some((ticks, ended)));
        assert_eq!(Replay::rewrite(app(), &text).expect("rewritten"), text);

        let tick_of = |line: &str| line.split_once(' ')?.0.parse::<u64>().ok();
        let changed = |at: usize, line: &str| changed(&text, at, line);
        // The last command's tick, whose hash follows.
        let command = lines.iter().rposition(|line| line.starts_with("> 0 "));
        let command = command.expect("a command");
        let hash = lines[command..]
            .iter()
            .position(|line| tick_of(line).is_some());
        let hash = command + hash.expect("its tick's hash");
        let tick = tick_of(lines[hash]).expect("a tick");
        let (ran, error) = replayed(&changed(command, "> 0 Struct([(\"0\", F32(9.0))])"))
            .expect_err("another way");
        assert_eq!((ran, error.code()), (tick, "replay.diverged"), "{error}");
        let layout = lines[hash].rsplit(' ').next().expect("a layout");
        let rehashed = format!("{tick} 0000000000000000 {layout}");
        let (ran, error) = replayed(&changed(hash, &rehashed)).expect_err("another way");
        assert_eq!((ran, error.code()), (tick, "replay.diverged"), "{error}");
        let says = format!("the world's content goes another way after tick {tick}");
        assert!(error.to_string().contains(&says), "{error}");
        let rewritten = Replay::rewrite(app(), &changed(hash, &rehashed)).expect("rewritten");
        assert_eq!(rewritten, text);
        // Cut off in its hash, the tick runs its commands, unchecked.
        let whole: String = lines[..hash]
            .iter()
            .map(|line| format!("{line}\n"))
            .collect();
        let cut = whole + &lines[hash][..10];
        let (ticks, _) = replayed(&cut).unwrap_or_else(|(_, error)| panic!("{error}"));
        assert_eq!(ticks, tick + 1);
    }

    /// Actions fed key events between frames change at the first tick after, as queued commands.
    /// An axis holds every tick until released from either key; a tap shorter than a frame applies
    /// once; a repeat, a release without its press and an unbound key queue none. The run replays
    /// to its hashes.
    #[test]
    fn actions_change_at_the_first_tick_after_their_events() {
        let ms = Duration::from_millis;
        let key = |key, pressed| Event::Key {
            key,
            pressed,
            repeat: false,
        };
        let repeat = Event::Key {
            key: Key::ArrowRight,
            pressed: true,
            repeat: true,
        };
        // After which tick each event comes, and whether it queues a command (not Space's release).
        let script = [
            (4, key(Key::ArrowRight, true), true),
            (6, repeat, false),
            (12, key(Key::ArrowRight, false), true),
            (20, key(Key::Space, true), true),
            (20, key(Key::Space, false), false),
            (24, key(Key::Space, false), false),
            (30, key(Key::ArrowLeft, true), true),
            (34, key(Key::KeyW, true), false),
            (40, Event::Focused(false), true),
        ];
        let app = |applied| App::new("Piloted").layer(Pilot::new(applied));
        for frame in [ms(1), ms(10), ms(40), ms(130)] {
            let count = (Tick::DT * 50).as_nanos().div_ceil(frame.as_nanos());
            let frames = vec![frame; count as usize];
            let applied = Rc::new(RefCell::new(Vec::new()));
            let (mut next, mut arrived) = (0, Vec::new());
            let between = |stack: &mut Stack, cx: &mut Context, ran| {
                while let Some(&(after, event, changes)) = script.get(next)
                    && after <= ran
                {
                    let _ = stack.event(cx, &event);
                    if changes {
                        arrived.push(ran);
                    }
                    next += 1;
                }
            };
            let (text, ticks, world) = recorded_with(app(applied.clone()), &frames, between);
            assert_eq!(next, script.len(), "each event came, at {frame:?}");
            assert_eq!(*applied.borrow(), arrived, "at {frame:?}");
            let [pushed, let_go, _, left, unfocused] = arrived[..] else {
                panic!("{arrived:?} at {frame:?}");
            };
            let piloted = world.query::<&Piloted>().next().copied();
            let piloted = piloted.map(|p| (p.push, u64::from(p.pushed), p.taps));
            let pushed = let_go - pushed + unfocused - left;
            assert_eq!(piloted, Some((0.0, pushed, 1)), "at {frame:?}");
            let replayed = replay_all(app(Rc::default()), &text);
            assert_eq!(replayed.ok(), Some((ticks, world.hash())), "at {frame:?}");
        }
    }

    #[cfg(feature = "dev")]
    /// Edits between ticks are recorded before the next tick's commands and replay to the recorded
    /// hashes: a create (drawing its ID as the run did), two sets in one gesture, a refused edit,
    /// and an undo of the whole gesture.
    #[test]
    fn edits_between_ticks_replay() {
        let ms = Duration::from_millis;
        let frames = [ms(20), ms(40), Tick::DT, ms(30)].repeat(4);
        let author = || "agent".to_owned();
        let mut frame = 0;
        let (text, ticks, ended) = recorded_with(two_layers(), &frames, |stack, cx, _| {
            frame += 1;
            let made = cx.authors.logs(cx.world()).next().and_then(|(_, log)| {
                let edit = log.edits().next()?;
                edit.first().and_then(|operation| operation.entity())
            });
            let edit = match (frame, made) {
                (2, _) => Edit::Apply {
                    author: author(),
                    operations: vec![crate::ecs::Operation::Create {
                        entity: None,
                        parent: None,
                        components: vec![("Transform".into(), Transform::IDENTITY.to_value())],
                    }],
                },
                (5 | 6, Some(made)) => Edit::Amend {
                    author: author(),
                    gesture: 7,
                    operations: vec![crate::ecs::Operation::Set {
                        entity: made,
                        component: "Transform".into(),
                        path: "translation.y".to_owned(),
                        value: crate::reflect::Value::F64(frame as f64),
                    }],
                },
                (7, _) => Edit::Apply {
                    author: author(),
                    operations: vec![crate::ecs::Operation::Delete {
                        entity: "0000000000000001".parse().expect("an ID"),
                    }],
                },
                (9, _) => Edit::Undo { author: author() },
                _ => return,
            };
            let _ = stack.edit(cx, edit);
        });
        let edits: Vec<&str> = text.lines().filter(|line| line.starts_with("@ ")).collect();
        assert_eq!(edits.len(), 5, "{text}");
        assert!(edits[0].starts_with("@ Variant(\"Apply\""), "{text}");
        assert!(edits[1].starts_with("@ Variant(\"Amend\""), "{text}");
        let (_, _, unedited) = recorded_with(two_layers(), &frames, |_, _, _| {});
        let transforms = |world: &World| world.query::<&Transform>().count();
        assert_eq!(
            transforms(&ended),
            transforms(&unedited) + 1,
            "created, set, and the sets undone"
        );
        // The sets set 5 and 6, a frame each.
        let set = ended
            .query::<&Transform>()
            .any(|t| [5.0, 6.0].contains(&t.translation.y));
        assert!(!set, "the undo undid both sets of the gesture");
        let replayed = replay_all(two_layers(), &text).map_err(|(_, error)| error.to_string());
        assert_eq!(replayed, Ok((ticks, ended.hash())));
        // A differing edit diverges at its tick.
        let at = text
            .lines()
            .position(|line| line.starts_with("@ "))
            .expect("an edit");
        let other = text
            .lines()
            .nth(at)
            .expect("the line")
            .replace("Transform", "Nope");
        let (ran, error) =
            replay_all(two_layers(), &changed(&text, at, &other)).expect_err("refused");
        assert_eq!(error.code(), "replay.diverged", "{error}");
        assert!(ran <= 3, "{ran}");
        // A recording from before edits replays as its format.
        let (text, ticks, hash) = recorded(two_layers(), &frames);
        let older = changed(&text, 0, "gg-recording 1");
        assert_eq!(replay_all(two_layers(), &older).ok(), Some((ticks, hash)));
    }

    /// A patched run replays the ticks before the patch to their hashes, then stops at the tick
    /// after with `replay.patched`, naming line and tick, rather than diverge unsaid. One patched
    /// after its last tick replays whole.
    #[test]
    fn a_replay_stops_where_its_run_was_patched() {
        let (text, ticks, hash) = recorded(two_layers(), &[Tick::DT; 4]);
        assert!(ticks > 2, "{ticks}");
        let lines: Vec<&str> = text.lines().collect();
        let first = lines.iter().position(|line| line.starts_with("1 "));
        let first = first.expect("tick 1's hash");
        let patch = r#"patch String("lib\"patch\".dylib")"#;
        let patched = changed(&text, first, &format!("{}\n{patch}", lines[first]));
        let (ran, error) = replay_all(two_layers(), &patched).expect_err("stopped");
        assert_eq!((ran, error.code()), (2, "replay.patched"), "{error}");
        let says = format!(
            "line {}: the run's code was patched before tick 2, with lib\"patch\".dylib, ",
            first + 2
        );
        assert!(error.to_string().starts_with(&says), "{error}");
        // After the last tick, before `end`.
        let end = lines.len() - 1;
        assert_eq!(lines[end], "end");
        let after = changed(&text, end, &format!("{patch}\nend"));
        assert_eq!(replay_all(two_layers(), &after).ok(), Some((ticks, hash)));
        // A patch's line names its library.
        let bad = changed(&text, first, &format!("{}\npatch 12", lines[first]));
        let (_, error) = replay_all(two_layers(), &bad).expect_err("refused");
        assert_eq!(error.code(), "replay.syntax", "{error}");
        // The recorder writes the library by file name alone.
        #[cfg(feature = "dev")]
        {
            let dir = Scratch::new("recording-patched");
            fs::create_dir_all(&dir.0).expect("a directory");
            let path = dir.0.join("run.recording");
            let mut recorder = Recorder::create(&path, "Recorded").expect("created");
            recorder.patched(&dir.0.join(r#"lib"patch".dylib"#));
            let written = fs::read_to_string(&path).expect("written");
            assert_eq!(written, "patch String(\"lib\\\"patch\\\".dylib\")\n");
        }
    }

    /// Runs a Luau script at `path` that moves its entity each tick.
    #[cfg(feature = "dev")]
    struct Stepped {
        scripts: crate::Scripts,
        path: std::path::PathBuf,
    }

    #[cfg(feature = "dev")]
    impl Layer for Stepped {
        fn load(&mut self, cx: &mut Context) -> Result<()> {
            let loaded = self.scripts.load(cx, &self.path);
            loaded.unwrap_or_else(|error| panic!("{error:#}"));
            Ok(())
        }

        fn attach(&mut self, cx: &mut Context) {
            let world = cx.world_mut();
            let stepped = world.spawn(Transform::IDENTITY);
            world.add_default(stepped, "Step").expect("added");
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            self.scripts.run(world);
        }
    }

    /// A script reloaded between ticks, its component given a field, is recorded as an edit. It
    /// replays to the recorded hashes, reloading there from its file as the run started.
    #[cfg(feature = "dev")]
    #[test]
    fn a_script_reloaded_between_ticks_replays() {
        let dir = Scratch::new("recording-script");
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("step.luau");
        let script = |fields: &str, step: &str| {
            format!(
                "local Transform = gg.component(\"Transform\")\n\
                 local Step = gg.declare(\"Step\", {{ {fields} }})\n\
                 function Step.tick(entity, dt)\n\
                     entity[Transform.translation] = entity[Transform.translation]\n\
                         + vector.create({step}, 0, 0)\n\
                 end\n\
                 return Step\n"
            )
        };
        fs::write(&path, script("", "1")).expect("written");
        let app = || {
            let scripts = crate::Scripts::new().expect("Luau starts");
            let path = path.clone();
            App::new("Recorded").layer(Stepped { scripts, path })
        };
        let name = crate::assets::name_of(None, &path).to_string();
        let frames = [Tick::DT; 8];
        let mut frame = 0;
        let (text, ticks, ended) = recorded_with(app(), &frames, |stack, cx, _| {
            frame += 1;
            if frame == 4 {
                let edit = Edit::Script {
                    path: name.clone(),
                    text: script("by = 10", "entity[Step.by]"),
                };
                stack.edit(cx, edit).expect("reloaded");
            }
        });
        let edits: Vec<&str> = text.lines().filter(|line| line.starts_with("@ ")).collect();
        assert_eq!(edits.len(), 1, "{text}");
        assert!(edits[0].starts_with("@ Variant(\"Script\""), "{text}");
        let x = ended
            .query::<&Transform>()
            .next()
            .map(|at| at.translation.x);
        assert_eq!(
            x,
            Some((3 + 10 * (ticks - 3)) as f64),
            "reloaded at the fourth tick"
        );
        let stepped = ended.entities(Some("Step")).expect("a type").next();
        let by = ended.get_field(stepped.expect("stepped"), "Step", "by");
        assert_eq!(by.expect("its field"), crate::reflect::Value::F64(10.0));
        let replayed = replay_all(app(), &text).map_err(|(_, error)| error.to_string());
        assert_eq!(replayed, Ok((ticks, ended.hash())));
    }

    /// Runs the installed mods with every grant, on an entity it spawns.
    struct Modded(Option<crate::Mods>);

    impl Layer for Modded {
        fn load(&mut self, cx: &mut Context) -> Result<()> {
            self.0 = Some(crate::Mods::load(cx, crate::Grant::all()));
            Ok(())
        }

        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Transform::IDENTITY);
        }

        fn tick(&mut self, world: &mut World, _: Tick) {
            if let Some(mods) = &mut self.0 {
                mods.run(world);
            }
        }
    }

    /// A run's mods go whole in its recording; a replay runs them uninstalled, to the recorded
    /// hashes.
    #[test]
    fn a_recording_replays_its_mods_where_they_aren_t_installed() {
        let dir = Scratch::new("recording-mods");
        let folder = dir.0.join("mover");
        fs::create_dir_all(&folder).expect("a directory");
        let manifest = "(name: \"mover\", version: \"1.0.0\", game: \"Modded\", \
            capabilities: [\"read:Transform\", \"write:Transform\"], scripts: [\"mover.luau\"])";
        fs::write(folder.join("mod.ron"), manifest).expect("written");
        let script = "local Transform = gg.component(\"Transform\")\n\
            local Mover = gg.declare(\"Mover\", { by = 2 }, { on = \"Transform\" })\n\
            function Mover.tick(entity, dt)\n\
                entity[Transform.translation] = entity[Transform.translation]\n\
                    + vector.create(entity[Mover.by], 0, 0)\n\
            end\n\
            return Mover\n";
        fs::write(folder.join("mover.luau"), script).expect("written");
        let app = || App::new("Modded").layer(Modded(None));
        let mut cx = Context::new(None);
        cx.mods.game = "Modded".to_owned();
        cx.mods.dir = Some(dir.0.clone());
        let (text, ticks, ended) = recorded_in(app(), cx, &[Tick::DT; 4], |_, _, _| {});
        assert!(
            text.lines()
                .nth(5)
                .is_some_and(|line| line.starts_with("mod ")),
            "{text}"
        );
        let x = ended
            .query::<&Transform>()
            .next()
            .map(|at| at.translation.x);
        assert_eq!(x, Some(2.0 * ticks as f64));
        fs::remove_dir_all(&folder).expect("removed");
        let replayed = replay_all(app(), &text).map_err(|(_, error)| error.to_string());
        assert_eq!(replayed, Ok((ticks, ended.hash())));
    }

    /// A recording of an app without layers, like the player, replays too.
    #[test]
    fn a_recording_of_no_layers_replays() {
        let (text, ticks, ended) = recorded(App::new("Empty"), &[Tick::DT; 3]);
        assert_eq!(text.lines().nth(3), Some("layers "));
        let mut replay = Replay::new(App::new("Empty"), &text).unwrap_or_else(|e| panic!("{e}"));
        let mut ran = 0;
        while replay
            .step()
            .unwrap_or_else(|error| panic!("{error}"))
            .is_some()
        {
            ran += 1;
        }
        assert_eq!((ran, replay.world().hash()), (ticks, ended));
    }

    /// Each fault is refused with its code and line (unless it's the recording's as a whole); a
    /// value's column is the line's.
    #[test]
    fn a_recording_is_refused_where_it_goes_wrong() {
        let (text, ..) = recorded(two_layers(), &[Tick::DT; 4]);
        let lines: Vec<&str> = text.lines().collect();
        let command = lines.iter().position(|line| line.starts_with("> 0 "));
        let command = command.expect("the command its attach queued");
        let second = lines.iter().position(|line| line.starts_with("1 "));
        let second = second.expect("tick 1's hash");
        let hash = lines[second].split_once(' ').expect("a hash").1;
        let bad_value = "> 0 Struct([(\"0\", F32(1.0)]";
        let after_end = format!("{text}0 {hash}\n");
        let named = |line: &str| changed(&text, 4, &format!("{}\n{line}", lines[4]));
        let cases = [
            (changed(&text, 0, "gg-recording 8"), "replay.format", None),
            (changed(&text, 0, "(entities: {})"), "replay.format", None),
            (
                lines[..3].iter().map(|line| format!("{line}\n")).collect(),
                "replay.format",
                None,
            ),
            (changed(&text, 1, "app Other"), "replay.format", None),
            (changed(&text, 3, "layers Queuer"), "replay.format", None),
            (changed(&text, 4, "attach"), "replay.syntax", Some(5)),
            (changed(&text, 4, "seed 12"), "replay.syntax", Some(5)),
            (changed(&text, 5, "attach 00 00"), "replay.syntax", Some(6)),
            // A texture's name that isn't a list of it and its path.
            (
                named("named List([String(\"5f3c9a1e07b2d846\")])"),
                "replay.syntax",
                Some(6),
            ),
            (
                changed(&text, second, "named Bool(true)"),
                "replay.syntax",
                Some(second + 1),
            ),
            (
                changed(&text, second, &format!("2 {hash}")),
                "replay.syntax",
                Some(second + 1),
            ),
            (
                changed(&text, second, "1 tick"),
                "replay.syntax",
                Some(second + 1),
            ),
            (after_end, "replay.syntax", Some(lines.len() + 1)),
            (
                changed(&text, command, bad_value),
                "replay.syntax",
                Some(command + 1),
            ),
            (
                changed(&text, command, "> 2 Opaque"),
                "replay.bad_command",
                Some(command + 1),
            ),
            (
                changed(&text, command, "> 1 Opaque"),
                "replay.bad_command",
                Some(command + 1),
            ),
            (
                changed(&text, command, "> 0 UInt(1)"),
                "replay.bad_command",
                Some(command + 1),
            ),
        ];
        for (text, code, line) in cases {
            let replayed = Replay::new(two_layers(), &text).and_then(|mut replay| {
                while replay.step()?.is_some() {}
                Ok(())
            });
            let error = replayed.expect_err(&text);
            let message = error.to_string();
            assert_eq!(error.code(), code, "{message}\n{text}");
            if let Some(line) = line {
                assert!(message.starts_with(&format!("line {line}: ")), "{message}");
            }
        }
        // The value's column from its line's start, after `> 0 `.
        let alone = recording::text::Text::parse(&bad_value[4..], 1).expect_err("refused");
        let alone = alone.to_string();
        let column = |message: &str| {
            let column = message.split("column ").nth(1).expect("a column");
            column
                .split(':')
                .next()
                .expect("a number")
                .parse::<usize>()
                .expect("a number")
        };
        let text = changed(&text, command, bad_value);
        let error = Replay::new(two_layers(), &text).err().expect("refused");
        assert_eq!(column(&error.to_string()), column(&alone) + 4, "{error}");
    }

    /// A command a replay would refuse (such as an opaque part no value makes) stops the recording
    /// before it, failing with the layer, command and where the opaque part is. What was written
    /// before replays.
    #[test]
    fn a_command_that_wont_replay_stops_the_recording() {
        let placed = || {
            App::new("Placed")
                .layer(Turner(Rc::default()))
                .layer(Placer::default())
        };
        let mut app = placed();
        let thread = std::thread::current().id();
        let dir = Scratch::new(&format!("recording-placed-{thread:?}"));
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("placed.recording");
        app.stack
            .record(Recorder::create(&path, app.name).expect("created"));
        let mut cx = Context::new(None);
        app.stack.attach(&mut cx).expect("loaded");
        app.stack.tick(&mut cx, Tick { number: 0 });
        app.stack.detach(&mut cx);
        let error = app.stack.recorded().expect_err("stopped");
        let text = std::fs::read_to_string(&path).expect("written");
        assert_eq!(error.code(), "record.bad_command", "{error}");
        let message = error.to_string();
        for named in ["layer 1, Placer,", "(\"1\", Opaque)", "at `1`: it's opaque"] {
            assert!(message.contains(named), "{named}: {message}");
        }
        // The header and the attach's hash alone: no command, tick hash or end.
        let last = text.lines().last().unwrap_or_default();
        assert!(last.starts_with("attach "), "{text}");
        let replayed = replay_all(placed(), &text).map(|(ticks, _)| ticks);
        assert_eq!(replayed.map_err(|(_, error)| error.code()), Ok(0), "{text}");
    }

    /// A recording that can't be created (a missing directory) fails naming its file.
    #[test]
    fn a_recording_that_cant_be_created_fails_naming_its_file() {
        let dir = Scratch::new("recording-missing");
        let path = dir.0.join("run.recording");
        let error = Recorder::create(&path, "Missing")
            .err()
            .expect("no directory to make it in");
        assert_eq!(error.code(), "record.file", "{error}");
        assert!(error.to_string().contains("run.recording"), "{error}");
    }

    /// `--replay` checks every hash to the end, or stops where `--dump` says and writes the world.
    /// It fails where a hash differs or the named tick doesn't exist.
    #[test]
    fn a_replay_runs_to_its_end_or_its_dump() {
        let (text, ..) = recorded(two_layers(), &[Tick::DT; 4]);
        // What the run returns, whether it wrote the world where it stopped, and the next tick.
        let run = |text: &str, dump| {
            let mut replay = Replay::new(two_layers(), text).expect("attached");
            let mut out = Vec::new();
            let ran = replay.run(dump, &mut out).map_err(|error| error.code());
            let dumped = out == replay.world().dump().as_bytes();
            assert!(
                dumped || out.is_empty(),
                "{}",
                String::from_utf8_lossy(&out)
            );
            let next = replay.step().ok().flatten().map(|tick| tick.number);
            ((ran, dumped), next)
        };
        assert_eq!(run(&text, None), ((Ok(()), false), None));
        // Stopped after tick 1, it goes on from tick 2.
        assert_eq!(
            run(&text, Some(args::Dump::Tick(1))),
            ((Ok(()), true), Some(2))
        );
        assert_eq!(
            run(&text, Some(args::Dump::Attach)),
            ((Ok(()), true), Some(0))
        );
        assert_eq!(
            run(&text, Some(args::Dump::Tick(4))).0,
            (Err("replay.no_tick"), false)
        );
        let lines: Vec<&str> = text.lines().collect();
        let at = lines.iter().position(|line| line.starts_with("2 "));
        let at = at.expect("tick 2's hash");
        let layout = lines[at].rsplit(' ').next().expect("a layout");
        let wrong = changed(&text, at, &format!("2 0000000000000000 {layout}"));
        assert_eq!(run(&wrong, None).0, (Err("replay.diverged"), false));
        assert_eq!(run(&wrong, Some(args::Dump::Tick(1))).0, (Ok(()), true));
        assert_eq!(
            run(&wrong, Some(args::Dump::Tick(2))).0,
            (Err("replay.diverged"), true)
        );
    }
}
