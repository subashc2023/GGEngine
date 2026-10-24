//! Recordings and replays: a run's seed and the commands its layers' ticks applied, with the world's
//! hash once the layers attach and after each tick; and a replay of one, which runs its ticks
//! again, without a window or a GPU, each layer's with the commands recorded for it, and names the
//! first hash that goes another way.
//!
//! A recording is text, a line at a time:
//!
//! ```text
//! gg-recording 1
//! app Sandbox
//! engine 0.1.0 debug
//! layers Sandbox PerfPanel Inspector
//! seed 3f8a6c2e9b1d4057
//! attach 58183b6352a09a44 fb695ce29daa8479
//! > 0 Variant("Swarm", [("0", UInt(10000))])
//! 0 8c35cd01914c7df5 fb695ce29daa8479
//! end
//! ```
//!
//! Its header names the format and its version, the app, the engine and its build, the layers from
//! the bottom up and the seed its world drew its IDs from; then come the world's hash once the
//! layers attached, its content then its layout, and each tick's commands, `>`, the index of the
//! layer whose tick applied them and their value as [`text`] writes it, before the tick's number
//! and the world's hash after it; and `end`, once the run ended.

pub(crate) mod text;

use std::fmt::{self, Write as _};
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use gg_core::{Error, Result};
use tracing::{error, info, warn};

use crate::args::Dump;
use crate::layer::Commands;
use crate::{App, BUILD, Context, Tick, World, WorldHash, perf};
use text::Text;

/// A recording's first line: its format, and the format's version, which a change to its lines or
/// to its values' text takes to the next.
const FORMAT: &str = "gg-recording 1";

/// The engine, as a recording's `engine` line names it: its version and its build.
fn engine() -> String {
    format!("{} {BUILD}", env!("CARGO_PKG_VERSION"))
}

/// A hash as a recording's lines write it, as `hashes.txt`'s do: its content, then its layout, 16
/// hex digits each.
struct Hashed(WorldHash);

impl fmt::Display for Hashed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x} {:016x}", self.0.content, self.0.layout)
    }
}

/// Writes a run's recording as it goes, each line to the file once it's whole, unbuffered, so that
/// a run that's killed, or that crashes, leaves every line before; a tick that panics leaves the
/// commands it was applying too.
pub(crate) struct Recorder {
    path: PathBuf,
    file: File,
    app: &'static str,
    /// The line being written, kept for its memory, so that a tick without commands allocates
    /// nothing.
    line: String,
    /// Why the first write that failed failed, after which nothing more is written.
    failed: Option<io::Error>,
}

impl Recorder {
    /// Creates the file at `path`, in place of any there, for a recording of `app`.
    ///
    /// Fails with `record.file` if it can't.
    pub(crate) fn create(path: &Path, app: &'static str) -> Result<Self> {
        let file = File::create(path).map_err(|error| {
            let message = format!("can't create the recording {}: {error}", path.display());
            Error::new("record.file", message).with_source(error)
        })?;
        Ok(Self {
            path: path.to_owned(),
            file,
            app,
            line: String::new(),
            failed: None,
        })
    }

    /// Writes the header: the app, the engine, `layers`, the names of the layers' types from the
    /// bottom up, and `seed`, where the stream the world draws its IDs from starts.
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

    /// Writes the world's hash once the layers have attached.
    pub(crate) fn attached(&mut self, world: &World) {
        self.line.clear();
        let hash = Hashed(world.hash());
        writeln!(self.line, "attach {hash}").expect("a string takes any text");
        self.flush();
    }

    /// Writes the commands `commands` holds, the queue of the layer at `layer` from the bottom of
    /// the stack, as its tick is about to apply them, with the entities they hold by their IDs in
    /// `world`.
    pub(crate) fn queued(&mut self, layer: usize, commands: &dyn Commands, world: &World) {
        for i in 0..commands.len() {
            self.line.clear();
            write!(self.line, "> {layer} ").expect("a string takes any text");
            text::write(&mut self.line, &commands.value(i), world);
            self.line.push('\n');
            self.flush();
        }
    }

    /// Writes the world's hash after `tick`.
    pub(crate) fn ticked(&mut self, tick: Tick, world: &World) {
        self.line.clear();
        let hash = Hashed(world.hash());
        writeln!(self.line, "{} {hash}", tick.number).expect("a string takes any text");
        self.flush();
    }

    /// Writes that the run has ended.
    pub(crate) fn end(&mut self) {
        self.line.clear();
        self.line.push_str("end\n");
        self.flush();
    }

    /// Writes the line, unless a write has failed before.
    fn flush(&mut self) {
        if self.failed.is_none()
            && let Err(error) = self.file.write_all(self.line.as_bytes())
        {
            self.failed = Some(error);
        }
    }

    /// Fails with `record.file` once a write has failed: the recording ends at the last line
    /// written whole.
    pub(crate) fn check(&self) -> Result<()> {
        let Some(error) = &self.failed else {
            return Ok(());
        };
        let path = self.path.display();
        let message = format!("can't write the recording {path}: {error}; it ends where it failed");
        Err(Error::new("record.file", message))
    }
}

/// A recording, as read.
struct Recording {
    app: String,
    /// The engine that recorded it, with its build.
    engine: String,
    /// The names of the layers' types, from the bottom up.
    layers: Vec<String>,
    seed: u64,
    /// The world's hash once the layers attached, and the line that gives it.
    attach: (WorldHash, usize),
    ticks: Vec<Ticked>,
    /// Whether it has its `end` line, which one whose run was cut short hasn't.
    complete: bool,
}

/// A tick of a recording: the commands its layers' ticks applied, in the order they did, and the
/// world's hash after it, with the line that gives it; none for the commands after the last hash
/// of a recording cut short as their tick ran.
#[derive(Default)]
struct Ticked {
    commands: Vec<Recorded>,
    hash: Option<(WorldHash, usize)>,
}

/// A command recorded: the layer's whose tick applied it, by its index from the bottom of the
/// stack, its value, and the line that gives it.
struct Recorded {
    layer: usize,
    value: Text,
    line: usize,
}

/// The lines of `text` that end in a line break, numbered from 1: all of them, but the last of a
/// recording whose writing stopped partway through it.
fn whole_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let lines = text
        .split_inclusive('\n')
        .map_while(|line| line.strip_suffix('\n'));
    lines.enumerate().map(|(i, line)| (i + 1, line))
}

/// The error that line `line` isn't one of a recording's.
fn syntax(line: usize, what: impl fmt::Display) -> Error {
    Error::new("replay.syntax", format!("line {line}: {what}"))
}

/// The error that the recording isn't one the app replays.
fn format_error(what: impl fmt::Display) -> Error {
    Error::new("replay.format", what.to_string())
}

/// A hash as the recording's lines write it.
fn parse_hash(text: &str) -> Option<WorldHash> {
    let (content, layout) = text.split_once(' ')?;
    let hex = |text: &str| {
        let digits = text.len() == 16 && text.bytes().all(|b| b.is_ascii_hexdigit());
        digits.then(|| u64::from_str_radix(text, 16).ok()).flatten()
    };
    Some(WorldHash {
        content: hex(content)?,
        layout: hex(layout)?,
    })
}

impl Recording {
    /// Reads `text`, every line of it but a last one without its line break, which a run killed as
    /// it wrote the line leaves.
    ///
    /// Fails with `replay.format` for text that isn't a recording, is of another version of the
    /// format, or ends in its header; and, saying at which line, with `replay.syntax` for a line
    /// that isn't one of a recording's, a value that isn't one, or a tick out of order, and with
    /// `replay.bad_command` for a command of a layer the recording hasn't.
    fn parse(text: &str) -> Result<Self> {
        let mut lines = whole_lines(text);
        match lines.next() {
            Some((_, FORMAT)) => {}
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
        let mut header = |key: &str| {
            let (number, line) = lines.next().ok_or_else(|| {
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
        let app = header("app")?.1.to_owned();
        let engine = header("engine")?.1.to_owned();
        // None, for an app without layers, as the player is.
        let layers = header("layers")?.1.split_whitespace();
        let layers: Vec<String> = layers.map(str::to_owned).collect();
        let (number, seed) = header("seed")?;
        let hex = seed.len() == 16 && seed.bytes().all(|b| b.is_ascii_hexdigit());
        let seed = hex.then(|| u64::from_str_radix(seed, 16).ok()).flatten();
        let seed = seed.ok_or_else(|| syntax(number, "a seed is 16 hex digits"))?;
        let (number, attach) = header("attach")?;
        let attach =
            parse_hash(attach).ok_or_else(|| syntax(number, "a hash is two of 16 hex digits"))?;
        let mut recording = Self {
            app,
            engine,
            layers,
            seed,
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
            } else if let Some(command) = line.strip_prefix("> ") {
                ticked.commands.push(recording.command(number, command)?);
            } else {
                let tick = line.split_once(' ').and_then(|(tick, hash)| {
                    Some((tick.parse::<usize>().ok()?, parse_hash(hash)?))
                });
                let Some((tick, hash)) = tick else {
                    return Err(syntax(
                        number,
                        format!(
                            "expected a command, `> <layer> <value>`, a tick's hash, `<tick> \
                             <content> <layout>`, or `end`; found `{line}`"
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
        // A tick cut short, whose commands run again unchecked, so that what stopped it stops the
        // replay too.
        if !ticked.commands.is_empty() {
            recording.ticks.push(ticked);
        }
        Ok(recording)
    }

    /// The command line `number`, `command` after its `> `, gives.
    fn command(&self, number: usize, command: &str) -> Result<Recorded> {
        let (layer, value) = command.split_once(' ').unwrap_or((command, ""));
        // Where the value starts in the line, after `> `, the layer and a space.
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

/// A recording, run again: its app's layers attached in a world from its seed, without a window or
/// a GPU, and its ticks run alone, with no update, UI or event between them, each layer's with the
/// commands recorded for it in place of what it queued itself, each checked against the world's
/// hash recorded after it. Dropped, it detaches the layers.
#[doc(hidden)]
pub struct Replay {
    app: App,
    cx: Context,
    recording: Recording,
    /// How many of its ticks have run.
    ran: usize,
}

impl Replay {
    /// Reads `text`, a recording of `app`, and attaches `app`'s layers in a world from its seed,
    /// checking the world's hash once they have.
    ///
    /// Fails with `replay.format` for text that isn't a recording, is of another version of the
    /// format, ends in its header, or is of another app or other layers; and, saying at which line,
    /// with `replay.syntax` for a line that isn't one of a recording's, with `replay.bad_command`
    /// for a command of a layer the recording hasn't, and with `replay.diverged`, as
    /// [`step`](Self::step) does, if the hash isn't the one recorded.
    pub fn new(app: App, text: &str) -> Result<Self> {
        let replay = Self::attach(app, text)?;
        replay.check(None)?;
        Ok(replay)
    }

    /// As [`new`](Self::new), without checking the world's hash.
    fn attach(mut app: App, text: &str) -> Result<Self> {
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
        app.stack.attach(&mut cx);
        Ok(Self {
            app,
            cx,
            recording,
            ran: 0,
        })
    }

    /// Runs the recording's next tick, and returns it; or `None` past its last. Each layer's tick
    /// gets the commands recorded for it, read against the world as its tick finds it, and the
    /// world's hash after the tick is checked against the one recorded, but for commands after the
    /// last hash of a recording cut short.
    ///
    /// Fails, saying at which line, with `replay.bad_command` for a command recorded for a layer
    /// that queues none, or that its layer refuses; and with `replay.diverged` if the world's hash
    /// isn't the one recorded, saying which part of it differs, and with both hashes and both
    /// engines' builds, with the world as the tick left it.
    pub fn step(&mut self) -> Result<Option<Tick>> {
        let Some(tick) = self.run_tick()? else {
            return Ok(None);
        };
        self.check(Some(tick))?;
        Ok(Some(tick))
    }

    /// As [`step`](Self::step), without checking the world's hash.
    fn run_tick(&mut self) -> Result<Option<Tick>> {
        let Some(ticked) = self.recording.ticks.get(self.ran) else {
            return Ok(None);
        };
        let tick = Tick {
            number: self.ran as u64,
        };
        self.ran += 1;
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

    /// Checks the world's hash against the one recorded once the layers attached, for `None`, or
    /// after `tick`, if one was.
    fn check(&self, tick: Option<Tick>) -> Result<()> {
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
        let (recorded, replayed) = (Hashed(recorded), Hashed(hash));
        let mut message = format!(
            "line {line}: the world's {part} another way {at}: the recording has `{recorded}`, and \
             the replay `{replayed}`"
        );
        let here = engine();
        if self.recording.engine != here {
            let there = &self.recording.engine;
            message += &format!("; engine {there} recorded it, and engine {here} replays it");
        }
        Err(Error::new("replay.diverged", message))
    }

    /// The world, as the last tick run left it, or the layers' attach before the first.
    pub fn world(&self) -> &World {
        self.cx.world()
    }

    /// The recording `text`, of `app`, with the hashes this build gives in place of those it
    /// recorded, at its attach and after each tick: for a change meant to change the simulation,
    /// or what the hash reads. A last line without its line break is left out.
    ///
    /// Fails as [`new`](Self::new) and [`step`](Self::step) do, but for a hash that goes another
    /// way.
    pub fn rewrite(app: App, text: &str) -> Result<String> {
        let mut replay = Self::attach(app, text)?;
        let mut lines: Vec<String> = whole_lines(text).map(|(_, line)| line.to_owned()).collect();
        let (_, at) = replay.recording.attach;
        lines[at - 1] = format!("attach {}", Hashed(replay.world().hash()));
        while let Some(tick) = replay.run_tick()? {
            if let Some((_, at)) = replay.recording.ticks[tick.number as usize].hash {
                let hash = Hashed(replay.world().hash());
                lines[at - 1] = format!("{} {hash}", tick.number);
            }
        }
        Ok(lines.iter().map(|line| format!("{line}\n")).collect())
    }

    /// Runs the replay as `--replay` does: checks the hash at the layers' attach, then runs each
    /// tick and checks its hash, to the last, or, with `dump`, to where it says, and writes the
    /// world there to stdout, as [`World::dump`] writes it.
    pub(crate) fn run(&mut self, dump: Option<Dump>) -> Result<()> {
        let attached = self.check(None);
        if dump == Some(Dump::Attach) {
            self.dump()?;
            return attached;
        }
        attached?;
        while let Some(tick) = self.run_tick()? {
            let checked = self.check(Some(tick));
            if dump == Some(Dump::Tick(tick.number)) {
                self.dump()?;
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

    /// Writes the world to stdout, as [`World::dump`] writes it.
    fn dump(&self) -> Result<()> {
        let mut stdout = io::stdout().lock();
        let written = stdout.write_all(self.world().dump().as_bytes());
        written.and_then(|()| stdout.flush()).map_err(|error| {
            let message = format!("can't write the world to stdout: {error}");
            Error::new("replay.stdout", message).with_source(error)
        })
    }
}

impl Drop for Replay {
    fn drop(&mut self) {
        self.app.stack.detach(&mut self.cx);
    }
}

/// Replays the recording at `path`, of `app`, as `--replay` does: to its end, or, with `dump`, to
/// where it says, writing the world there to stdout; and returns the exit status, 0 if the world's
/// hash was the one recorded wherever it was checked.
pub(crate) fn replay(app: App, path: &Path, dump: Option<Dump>) -> ExitCode {
    let started = Instant::now();
    let failed = |error: Error, hint: &str| {
        let shown = path.display();
        error!(code = error.code(), path = %shown, "{shown}: {error:#}{hint}");
        ExitCode::FAILURE
    };
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let message = format!("can't read the recording: {error}");
            return failed(Error::new("replay.file", message).with_source(error), "");
        }
    };
    // Its whole lines alone, since the last, cut off as it was written, may end partway through a
    // character.
    let whole = bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    let text = match std::str::from_utf8(&bytes[..whole]) {
        Ok(text) => text,
        Err(error) => {
            let message = format!("the recording isn't UTF-8 text: {error}");
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
    match replay.run(dump) {
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
