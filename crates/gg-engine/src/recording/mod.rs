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

/// The engine, as a recording's `engine` line names it, and the tool API's session file: its
/// version and its build.
pub(crate) fn engine() -> String {
    format!("{} {BUILD}", env!("CARGO_PKG_VERSION"))
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
    /// Why the recording stopped, after which nothing more is written: the first write that failed,
    /// or the first command that wouldn't replay.
    failed: Option<Error>,
}

impl Recorder {
    /// Creates the file at `path`, in place of any there, for a recording of `app`.
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
        let hash = world.hash();
        writeln!(self.line, "attach {hash}").expect("a string takes any text");
        self.flush();
    }

    /// Writes the commands `commands` holds, the queue of the layer at `layer` from the bottom of
    /// the stack, of type `name`, as its tick is about to apply them, with the entities they hold
    /// by their IDs in `world`; but stops the recording before one that the queue's
    /// [`check`](Commands::check) says its replay would refuse, as one holding something opaque.
    pub(crate) fn queued(
        &mut self,
        layer: usize,
        name: &str,
        commands: &dyn Commands,
        world: &World,
    ) {
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

    /// Writes the world's hash after `tick`.
    pub(crate) fn ticked(&mut self, tick: Tick, world: &World) {
        self.line.clear();
        let hash = world.hash();
        writeln!(self.line, "{} {hash}", tick.number).expect("a string takes any text");
        self.flush();
    }

    /// Writes that the run has ended.
    pub(crate) fn end(&mut self) {
        self.line.clear();
        self.line.push_str("end\n");
        self.flush();
    }

    /// Writes the line, unless the recording has stopped.
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

    /// Fails once the recording has stopped, which ends at the last line written whole: with
    /// `record.file` once a write has failed, and with `record.bad_command`, naming the layer, the
    /// command and what its replay would refuse of it, once a layer has queued one that wouldn't
    /// replay.
    pub(crate) fn check(&self) -> Result<()> {
        match &self.failed {
            Some(error) => Err(Error::new(error.code(), error.to_string())),
            None => Ok(()),
        }
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

/// A hash as the recording's lines write it, as its `Display` does.
fn parse_hash(text: &str) -> Option<WorldHash> {
    let (content, layout) = text.split_once(' ')?;
    Some(WorldHash {
        content: hex16(content)?,
        layout: hex16(layout)?,
    })
}

/// The number `text` is as 16 hex digits, as a recording writes its seed and each half of a hash.
fn hex16(text: &str) -> Option<u64> {
    let digits = text.len() == 16 && text.bytes().all(|b| b.is_ascii_hexdigit());
    digits.then(|| u64::from_str_radix(text, 16).ok()).flatten()
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
        let seed = hex16(seed).ok_or_else(|| syntax(number, "a seed is 16 hex digits"))?;
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
        // So that its textures are named as the run's were.
        cx.root.clone_from(&app.root);
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
    /// isn't the one recorded, saying which part of it differs, with both hashes, and both engines'
    /// builds where another build recorded it, with the world as the tick left it.
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
        lines[at - 1] = format!("attach {}", replay.world().hash());
        while let Some(tick) = replay.run_tick()? {
            if let Some((_, at)) = replay.recording.ticks[tick.number as usize].hash {
                let hash = replay.world().hash();
                lines[at - 1] = format!("{} {hash}", tick.number);
            }
        }
        Ok(lines.iter().map(|line| format!("{line}\n")).collect())
    }

    /// Runs the replay as `--replay` does: checks the hash at the layers' attach, then runs each
    /// tick and checks its hash, to the last, or, with `dump`, to where it says, and writes the
    /// world there to `out`, stdout for `--replay`, as [`World::dump`] writes it.
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

    /// Writes the world to `out`, stdout for `--replay`, as [`World::dump`] writes it.
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
            let message = "can't read the recording";
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

    /// A command of [`Queuer`]'s: spawns an entity turned by this angle.
    #[derive(Reflect)]
    struct Spawn(f32);

    /// Queues a spawn as it attaches, and at every third update, which its next tick makes.
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

    /// A command of [`Placer`]'s: spawns this sprite, with a handle of the layer's own, which is
    /// opaque, so that no value makes it.
    #[derive(Reflect)]
    struct Place(Sprite, #[reflect(opaque)] Handle);

    /// Queues a sprite to place as it attaches, which its next tick spawns.
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

    /// What [`Pilot`]'s keys do.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Act {
        Push,
        Tap,
    }

    /// A command of [`Pilot`]'s: a change of one of its actions.
    #[derive(Reflect)]
    enum Control {
        Push(f32),
        Tap,
    }

    /// What [`Pilot`]'s commands leave: how it's pushed, how many ticks it was pushed at, and its
    /// taps.
    #[derive(Clone, Copy, Component)]
    struct Piloted {
        push: f32,
        pushed: u32,
        taps: u32,
    }

    /// Pushes its entity while the left or the right arrow is held, and counts Space's taps,
    /// queuing its actions' changes as its events come, and noting the tick each applies at.
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

    /// An app of a layer whose commands its updates queue, under one without.
    fn two_layers() -> App {
        let app = App::new("Recorded").layer(Queuer::default());
        app.layer(Turner(Rc::default()))
    }

    /// A run of `app` recorded as it goes, frames `frames` apart, as a windowed run steps them:
    /// its recording, the ticks it ran, and its world's hash as it ended.
    fn recorded(app: App, frames: &[Duration]) -> (String, u64, WorldHash) {
        let (text, ticks, world) = recorded_with(app, frames, |_, _, _| {});
        (text, ticks, world.hash())
    }

    /// As [`recorded`], with `between` sending the stack events before each frame, given the ticks
    /// run before it; and the world as it ended.
    fn recorded_with(
        mut app: App,
        frames: &[Duration],
        mut between: impl FnMut(&mut Stack, &mut Context, u64),
    ) -> (String, u64, World) {
        // The thread's own: the tests that record run at once, and a new scratch directory empties
        // one of its name.
        let thread = std::thread::current().id();
        let dir = Scratch::new(&format!("recording-{thread:?}"));
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("run.recording");
        let recorder = Recorder::create(&path, app.name).expect("created");
        app.stack.record(recorder);
        let mut cx = Context::new(None);
        app.stack.attach(&mut cx);
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

    /// A replay of `text` by `app` to its end: the ticks it ran and its world's hash, or the ticks
    /// it ran before the error it stopped at.
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

    /// `text` with its line `at`, from 0, in place of `line`.
    fn changed(text: &str, at: usize, line: &str) -> String {
        let mut lines: Vec<&str> = text.lines().collect();
        lines[at] = line;
        lines.iter().map(|line| format!("{line}\n")).collect()
    }

    /// A run recorded as it goes at a frame rate of its own, of a layer whose commands its updates
    /// queue, under one without, replays without a frame to the hashes it recorded, and to the
    /// world it ended with; a copy of it with a command changed, or a hash, goes another way at
    /// that tick; and a copy cut off partway through a line replays its whole lines.
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
        // The last command's tick, which its next hash is of.
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

    /// A layer's actions, fed key events between frames of a rate of their own, change at the
    /// first tick after the events come, as the commands its event queues: an axis is held at every
    /// tick until its release, from either key, a tap shorter than a frame applies once, and a
    /// repeat, a release without its press and a key not bound queue none; and the run replays to
    /// its hashes.
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
        // After how many ticks each event comes, and whether the layer queues a command for it:
        // not for Space's release, which it doesn't apply.
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

    /// A recording of an app without layers, as the player is, replays too.
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

    /// Each line a recording can get wrong is refused with its code, and, but for what's wrong with
    /// the recording as a whole, its line; and a value's column is the line's.
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
        let cases = [
            (changed(&text, 0, "gg-recording 2"), "replay.format", None),
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
        // The value's column, from the start of its line, after `> 0 `.
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

    /// A layer's command that its replay would refuse, as one holding something opaque, which no
    /// value makes, stops the recording before it, which fails, naming the layer, the command and
    /// where in it what's opaque is; what was written before it replays.
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
        app.stack.attach(&mut cx);
        app.stack.tick(&mut cx, Tick { number: 0 });
        app.stack.detach(&mut cx);
        let error = app.stack.recorded().expect_err("stopped");
        let text = std::fs::read_to_string(&path).expect("written");
        assert_eq!(error.code(), "record.bad_command", "{error}");
        let message = error.to_string();
        for named in ["layer 1, Placer,", "(\"1\", Opaque)", "at `1`: it's opaque"] {
            assert!(message.contains(named), "{named}: {message}");
        }
        // Its header and its attach's hash alone: neither the command, nor the tick's hash after
        // it, nor an end.
        let last = text.lines().last().unwrap_or_default();
        assert!(last.starts_with("attach "), "{text}");
        let replayed = replay_all(placed(), &text).map(|(ticks, _)| ticks);
        assert_eq!(replayed.map_err(|(_, error)| error.code()), Ok(0), "{text}");
    }

    /// A recording that can't be created, as in a directory that isn't there, fails naming its
    /// file.
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

    /// `--replay` checks every hash to the end, and stops where `--dump` says, writing the world
    /// there, failing where a hash goes another way, or where no tick is the one it names.
    #[test]
    fn a_replay_runs_to_its_end_or_its_dump() {
        let (text, ..) = recorded(two_layers(), &[Tick::DT; 4]);
        // What the run returns and whether it wrote the world where it stopped, and the next tick.
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
