//! `gg`, the command-line tool for GGEngine's games, as ARCHITECTURE.md's Agents and tools
//! describes: it builds a game's development build and runs it, supervising it; runs headless
//! checks; talks to running games through the tool API; and makes new games. Inside a game's
//! project, it builds the project's game and reaches the project's games alone. It prints what it
//! finds as JSON, a line at a time, for agents and scripts as much as for people.

mod build;
mod classify;
mod dx;
mod game;
mod mcp;
mod new;
mod patch;
mod run;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use gg_api::{Failure, Project};
use serde_json::{Value, json};

use game::Pick;
use run::{Check, Run};

const USAGE: &str = "\
Usage: gg <command> [options]

In a game's project, the directory that holds its `project.ron` beside its `Cargo.toml`, and those
below it, or in the project that `--project <PATH>` names, by its directory or its `project.ron`,
gg builds the project's game, runs cargo in the project's directory, and talks to the project's
games alone; a manifest that doesn't read is an error. Elsewhere, it works on the whole workspace.

Build and run:
  run [PACKAGE] [--release] [--no-patch] [--hang <SECONDS>] [--snapshot <SECONDS>] [--peers <N>]
      [--embedded] [-- <ARGS>...]
      Build PACKAGE's development build, the project's game without one, and run a copy of it with
      ARGS, printing a line as it starts, as it serves the tool API, and as it ends: `exited`, or
      `crashed` or `hung`, with the errors its log holds. A run whose frames stop for `--hang`
      seconds (10), while it waits for nothing the user does, has hung, and is ended. It's
      snapshotted every `--snapshot` seconds (5) from its first frame, `snapshot_failed` saying why
      they fail, and started again from its last snapshot after a crash or a hang, and from one
      taken then as PACKAGE is rebuilt (`rebuilt`). One that ends before its first snapshot waits
      for a rebuild (`waiting`); one that refuses its snapshot starts afresh (`refused`). `run` ends
      as the game comes to its own end, as its window closes, `quit` asks or its frames run out,
      whatever its status; as one that has never served the API ends; and as it ends at all with
      `--snapshot 0` or ARGS with `--record`, `--replay` or `--no-api`, which turn restarts off.
      With dx, Dioxus's tool, on the PATH, of the release of the subsecond PACKAGE builds with,
      the game is patched as its code changes, unless `--no-patch`, `--release`, restarts' being
      off or a library beside PACKAGE's binary, which no patch reaches, turns patches off
      (`patches_off` says why): `run` builds it with `dx build --fat-binary` and watches PACKAGE's
      modules, the workspace's packages it builds with, and the workspace's manifest and lock file
      (`watching`). An edit inside function bodies of PACKAGE's modules is built as a patch with
      `dx tools hotpatch`, which the game applies between ticks (`patched`, or `patch_failed` with
      rustc's errors, the game running on); any other change rebuilds it with dx, saying which file
      and why (`rebuilding`, then `rebuilt` or `rebuild_failed`): one to a type, even one declared
      in a body, or to code that has run as the game started, `main` and a layer's `load` and
      `restored`, and any change while the last rebuild has failed. A game that waits for a
      rebuild gets one at any change, and one started again is patched with the edits since its
      build at once. With `--peers 2`, it runs the two peers of a lockstep session from one build,
      the host given `--host` and the joiner `--join` at a free port of the loopback, each
      `started` naming its `player`: patched together, and stopped and started again together as
      either ends or the game is rebuilt. With `--embedded`, for the tool that started gg, such as
      the editor, the game is given `--embedded`, running offscreen for the tool, which shows its
      view, and the run ends cleanly, stopping the game, once gg's stdin closes (`closed`), as the
      tool ends, a build under way finishing first. A second `run` of PACKAGE while one runs is
      refused (`gg.running`).
  check [PACKAGE] [--release] [--frames <N>] [--timeout <SECONDS>] [-- <ARGS>...]
      Build PACKAGE, the project's game without one, and run it headless for N frames (60) with
      ARGS, and print what it did: its exit status, its warnings and errors, and its frame times.
      One that runs longer than SECONDS (600) is ended.
      Building the project's game, `run` and `check` first check that it's the package beside the
      project's manifest (`gg.project`).

Make a game:
  new <NAME>
      Write a new game in the workspace, in `games/NAME`, writing over nothing: package NAME, whose
      binary is `gg-NAME`, a development build of the engine's; its project, whose start scene is
      `scenes/main.ron`, empty; and its main module, whose layer loads the start scene. Prints a
      line naming its directory and files.

Talk to a running game, through its tool API:
  sessions                       List the running games
  status                         Its ticks, whether it's paused or stopped, its frames, entities and
                                 view, and its open scene's file and whether it has unsaved edits
  pause                          Pause its simulation
  resume                         Run its simulation in real time again
  play                           Play a copy of its world as edited in its place, until `stop`, if
                                 it's stopped
  stop                           Stop it: no tick runs, and its world is the world as edited, which
                                 only edits, scene files' saves, `scene open` and `scene new`
                                 change; while a copy plays, bring the world as edited back, what
                                 the copy did, its edits among them, going; recorded runs and
                                 lockstep peers refuse it
  step [N]                       Run N ticks (1), and stay paused; refused while it's stopped
  scene open <PATH> [--discard]  Open the scene file at PATH, in one of the project's scenes
                                 folders, in place of the open scene: its entities in place of the
                                 scene's, the game's own kept; refused while the scene has unsaved
                                 edits, unless `--discard`; every author's edits start afresh
  scene save [PATH]              Save the open scene's entities, never those the game's code made,
                                 to its file, or to PATH, its file from then on
  scene new [--discard]          Delete the open scene's entities, leaving it untitled; refused as
                                 `scene open` is
  capture [--own] [PATH]         Write its view as a PNG at PATH, or in `captures` beside it: as
                                 it's shown, the look included, or with `--own`, as the game shows
                                 it on its own
  look [<LOOK>]                  Its look, the view's settings any client sets, the last to set a
                                 part winning; with LOOK, a JSON object of parts as the API's `look`
                                 takes them, `viewpoint`, `outline`, `grid` or `colliders`, those set
                                 first, each `null` giving it back to the game
  pick <X> <Y>                   The ID and name of the entity its view shows at pixel X, Y from its
                                 top left, as a capture has it, the look included; `null` for none
  snapshot [PATH]                Write its world as a snapshot at PATH, or in `snapshots` beside it
  entities [--with <COMPONENT>] [--skip <N>] [--take <N>]
                                 List its entities, those with COMPONENT, N at a time (100)
  entity <ID>                    An entity and its components' values
  get <ID> <COMPONENT> [PATH]    A component's value, or its field at PATH, such as `translation.x`
  hash                           Its world's hash
  assets [--kind <KIND>] [--skip <N>] [--take <N>]
                                 List its textures, models and fonts, those of KIND, `texture`,
                                 `model` or `font`, N at a time (100), by their paths from its
                                 asset root, each with its ID, whether a layer has added it, and
                                 what's wrong with its ID file, and the ID files left without their
                                 assets; the game, but a lockstep session's joiner, whose host
                                 does, writes each one's ID in a file beside it, `<file>.meta`, as
                                 it starts and as it finds new files: commit it with its file. A
                                 file renamed or moved alone keeps its ID, the game moving its ID
                                 file after it, unless it's edited in the same step. A file of a
                                 texture, a model, a font or a sound the game has added, edited,
                                 reloads in it, as a shader's does, the nodes its `ModelRoot`s
                                 spawn with it, but in a recorded run or a lockstep session, where
                                 a model's nodes and materials change as it next starts
  apply <OPERATIONS>             Apply OPERATIONS, a JSON array of the API's operations, as one edit:
                                 all or none, between ticks, which `undo` takes back
  set <ID> <COMPONENT> <PATH> <VALUE>
                                 Set a field, such as `translation.x`, or a whole component for an
                                 empty PATH, to VALUE: JSON, or else a string, as one edit
  undo [--author <NAME>]         Undo gg's last edit, or author NAME's, such as a scene file's path,
                                 `scenes/grid.ron`, printing it, or `null` if there's none
  redo [--author <NAME>]         Redo the last edit gg, or NAME, undid
  edits [--author <NAME>]        Each author's edits, or NAME's, those to undo and those to redo
  logs [--level <LEVEL>] [--follow]
                                 Its log's events at LEVEL (`warn`) and above, of the last 10,000 it
                                 keeps, each its JSON log's line; with `--follow`, then each as it's
                                 logged, until it ends
  quit                           End it cleanly, as closing its window does, unasked
  call <METHOD> [<PARAMS>]       Send METHOD with PARAMS, as JSON, and print what it answers

Serve agents:
  mcp                            Serve the tool API to an agent that speaks MCP, on stdin and stdout:
                                 a tool for each command that talks to a game but `quit`, `call`,
                                 `logs --follow` and the PATHs of `capture` and `snapshot`, with
                                 `scene`'s PATH from the game's asset root, which reaches the game
                                 the options choose at each call, as the agent, its MCP client's
                                 name, the author of the edits it applies

Options for talking to a game:
  --pid <PID>           The game in process PID, when more than one runs
  --app <NAME>          The game whose app is NAME, when more than one runs
  --timeout <SECONDS>   How long to wait for its answer (30)

Options for every command but `new`:
  --project <PATH>      Work in the project PATH names, by its directory or its `project.ron`,
                        rather than in the one gg is in

Each result is a line of JSON on stdout; `logs` prints a line for each of the game's events, and
`run` one for each of its own. An error is a line on stderr, {\"error\":{\"code\":...,\"message\":...}}. Exits 0 on success,
1 on an error, a check that failed or a run that didn't end cleanly, and 2 on bad arguments.
";

/// The project gg works in, as `--project` names it or gg finds it: its manifest's path, absolute,
/// in whose directory cargo runs, and the game it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    pub(crate) manifest: PathBuf,
    pub(crate) game: String,
}

impl Scope {
    /// The project's directory.
    pub(crate) fn dir(&self) -> &Path {
        self.manifest.parent().unwrap_or(Path::new("."))
    }
}

/// What `gg` was asked to do.
enum Command {
    Help,
    Run(Run),
    Check(Check),
    New(String),
    /// The running games, those of the project whose manifest it names alone.
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
    Mcp(Pick),
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
            // Bad arguments, or else a project's manifest that doesn't read, an error.
            return ExitCode::from(if failure.code == "gg.args" { 2 } else { 1 });
        }
    };
    let done = match command {
        Command::Help => {
            print(USAGE.trim_end());
            Ok(true)
        }
        Command::Run(run) => run::run(&run),
        Command::Check(check) => run::check(&check),
        Command::New(name) => new::new(&name).map(|()| true),
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
        Command::Mcp(pick) => mcp::serve(pick).map(|()| true),
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

/// Prints `line` on stdout, ignoring a stdout that's gone, as when piped into `head`.
fn print(line: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

/// Prints `failure` on stderr, as a line of JSON.
fn report(failure: &Failure) {
    let error = json!({"error": {"code": failure.code, "message": failure.message}});
    let _ = writeln!(std::io::stderr().lock(), "{error}");
}

fn bad(message: String) -> Failure {
    Failure::new(
        "gg.args",
        format!("{message}; `gg help` lists the commands"),
    )
}

/// `path`, given for the game to write at, made absolute, as the game's working directory may not
/// be gg's.
///
/// Fails with `gg.args` if gg can't tell where it is.
fn absolute(path: &str) -> Result<PathBuf, Failure> {
    std::path::absolute(Path::new(path))
        .map_err(|error| bad(format!("can't tell where `{path}` is: {error}")))
}

/// The flags that take a value: `--name value` or `--name=value`.
const VALUED: [&str; 14] = [
    "--project",
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
];

/// The arguments after a command's name: what isn't a flag, the flags, each with its value if it
/// takes one, and what follows `--`, for the game.
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

    /// Takes flag `name`, returning whether it was given.
    fn switch(&mut self, name: &str) -> bool {
        self.take(name).is_some()
    }

    /// Takes flag `name`'s value, if it was given.
    fn value(&mut self, name: &str) -> Option<String> {
        self.take(name).flatten()
    }

    /// Takes flag `name`'s value as a `T`, or `default` if it wasn't given.
    fn number<T: FromStr>(&mut self, name: &str, default: T) -> Result<T, Failure> {
        match self.value(name) {
            Some(value) => value
                .parse()
                .map_err(|_| bad(format!("`{name}` takes a whole number, not `{value}`"))),
            None => Ok(default),
        }
    }

    /// Takes flag `name`'s value as a number of seconds, or `default`'s if it wasn't given.
    fn seconds(&mut self, name: &str, default: u64) -> Result<Duration, Failure> {
        self.number(name, default).map(Duration::from_secs)
    }

    fn take(&mut self, name: &str) -> Option<Option<String>> {
        let at = self.flags.iter().position(|(given, _)| given == name)?;
        Some(self.flags.remove(at).1)
    }

    /// The options for talking to a game, in the project gg works in, as [`scope`](Self::scope)
    /// finds it.
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
            project: self.scope()?.map(|scope| scope.manifest),
        })
    }

    /// The project gg works in: the one `--project` names, by its directory or its manifest, or
    /// else the one the working directory is in, as gg-api's `Project::find` finds it; `None`
    /// outside one.
    ///
    /// Fails with `gg.args` if gg can't tell where the working directory, or `--project`'s path,
    /// is; and with `project.read` or `project.bad` for a manifest that doesn't read, as
    /// `Project::read` does, rather than working on the whole workspace.
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

    /// The package `run` or `check` builds, `command`'s: the one given, or else the game of the
    /// project gg works in, `scope`.
    ///
    /// Fails with `gg.args` if neither is.
    fn package(&mut self, command: &str, scope: Option<&Scope>) -> Result<String, Failure> {
        let given = self
            .positional(0, 1, &format!("{command} [PACKAGE]"))?
            .pop();
        let package = given.or_else(|| Some(scope?.game.clone()));
        package.ok_or_else(|| bad(format!("outside a project, `gg {command}` takes a PACKAGE")))
    }

    /// The positional arguments, checking that there are at least `least` and at most `most`, as
    /// `usage` shows them.
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

    /// Checks that command `command` was given nothing it doesn't take.
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

/// The command `args`, the arguments after `gg`, give, in the project gg works in, as
/// [`Parsed::scope`] finds it.
///
/// Fails with `gg.args` for arguments that aren't a command's, and as `Parsed::scope` does.
fn parse(args: &[String]) -> Result<Command, Failure> {
    let Some((name, args)) = args.split_first() else {
        return Ok(Command::Help);
    };
    let mut parsed = Parsed::new(args)?;
    let call = |parsed: &mut Parsed, method: &str, params: Value| -> Result<Command, Failure> {
        Ok(Command::Call {
            pick: parsed.pick()?,
            method: method.to_owned(),
            params,
        })
    };
    let command = match name.as_str() {
        "help" | "-h" | "--help" => Command::Help,
        "run" => {
            let project = parsed.scope()?;
            let run = Run {
                package: parsed.package("run", project.as_ref())?,
                project,
                release: parsed.switch("--release"),
                patches: !parsed.switch("--no-patch"),
                hang: parsed.seconds("--hang", 10)?,
                snapshot: parsed.seconds("--snapshot", 5)?,
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
                && let Some(flag) = ["--host", "--join", "--record"]
                    .into_iter()
                    .find(|&flag| run.given(flag))
            {
                return Err(bad(format!(
                    "`--peers 2` gives the peers their `--host` and `--join` itself, and a joiner \
                     records nothing, so the game's arguments take no `{flag}`"
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
        "new" => {
            let name = parsed.positional(1, 1, "new <NAME>")?.remove(0);
            if new::project(&name).check().is_err() {
                return Err(bad(format!(
                    "`gg new` takes a package's name, of ASCII letters, digits, `-` and `_`, not \
                     starting with a digit, not `{name}`"
                )));
            }
            Command::New(name)
        }
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
        method @ ("status" | "pause" | "resume" | "play" | "stop" | "hash" | "quit") => {
            parsed.positional(0, 0, method)?;
            call(&mut parsed, method, Value::Null)?
        }
        "step" => {
            let ticks = parsed.positional(0, 1, "step [N]")?;
            let ticks = match ticks.first() {
                Some(ticks) => ticks.parse::<u64>().map_err(|_| {
                    bad(format!("`gg step` takes a number of ticks, not `{ticks}`"))
                })?,
                None => 1,
            };
            call(&mut parsed, "step", json!({"ticks": ticks}))?
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
            call(&mut parsed, "scene", params)?
        }
        "capture" => {
            let path = parsed.positional(0, 1, "capture [--own] [PATH]")?;
            let mut params = json!({});
            if let Some(path) = path.first() {
                params["path"] = json!(absolute(path)?);
            }
            if parsed.switch("--own") {
                params["own"] = json!(true);
            }
            call(&mut parsed, "capture", params)?
        }
        "snapshot" => {
            let path = parsed.positional(0, 1, "snapshot [PATH]")?;
            let params = match path.first() {
                Some(path) => json!({"path": absolute(path)?}),
                None => json!({}),
            };
            call(&mut parsed, "snapshot", params)?
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
            call(&mut parsed, "look", look)?
        }
        "pick" => {
            let given = parsed.positional(2, 2, "pick <X> <Y>")?;
            let at = given.iter().map(|given| {
                let at = given.parse::<f64>().ok().filter(|at| at.is_finite());
                at.ok_or_else(|| {
                    bad(format!(
                        "`gg pick` takes a pixel's X and Y, numbers, not `{given}`"
                    ))
                })
            });
            let at = at.collect::<Result<Vec<f64>, Failure>>()?;
            call(&mut parsed, "pick", json!({"at": at}))?
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
                &mut parsed,
                "entities",
                json!({"with": with, "skip": skip, "take": take}),
            )?
        }
        "assets" => {
            parsed.positional(0, 0, "assets [--kind <KIND>] [--skip <N>] [--take <N>]")?;
            // The game refuses a kind there's none of, as it does a component.
            let kind = parsed.value("--kind");
            let skip: usize = parsed.number("--skip", 0)?;
            let take: usize = parsed.number("--take", 100)?;
            call(
                &mut parsed,
                "assets",
                json!({"kind": kind, "skip": skip, "take": take}),
            )?
        }
        "entity" => {
            let id = parsed.positional(1, 1, "entity <ID>")?.remove(0);
            call(&mut parsed, "entity", json!({"id": id}))?
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
                &mut parsed,
                "get",
                json!({"id": id, "component": component, "path": path}),
            )?
        }
        "apply" => {
            let given = parsed.positional(1, 1, "apply <OPERATIONS>")?;
            let operations: Value = serde_json::from_str(&given[0])
                .map_err(|error| bad(format!("OPERATIONS isn't JSON: {error}")))?;
            if !operations.is_array() {
                return Err(bad("OPERATIONS is a JSON array of operations".to_owned()));
            }
            call(&mut parsed, "apply", json!({"operations": operations}))?
        }
        "set" => {
            let given = parsed.positional(4, 4, "set <ID> <COMPONENT> <PATH> <VALUE>")?;
            let value =
                serde_json::from_str(&given[3]).unwrap_or_else(|_| Value::String(given[3].clone()));
            let set =
                json!({"id": given[0], "component": given[1], "path": given[2], "value": value});
            call(&mut parsed, "apply", json!({"operations": [{"set": set}]}))?
        }
        method @ ("undo" | "redo" | "edits") => {
            parsed.positional(0, 0, &format!("{method} [--author <NAME>]"))?;
            let author = parsed.value("--author");
            call(&mut parsed, method, json!({"author": author}))?
        }
        "mcp" => {
            parsed.positional(0, 0, "mcp")?;
            // Its MCP client's name, once it says, and else its own.
            let pick = parsed.pick()?;
            Command::Mcp(Pick {
                client: "mcp".to_owned(),
                ..pick
            })
        }
        "call" => {
            let mut given = parsed.positional(1, 2, "call <METHOD> [<PARAMS>]")?;
            let params = match given.get(1) {
                Some(params) => serde_json::from_str(params)
                    .map_err(|error| bad(format!("PARAMS isn't JSON: {error}")))?,
                None => Value::Null,
            };
            let method = given.remove(0);
            call(&mut parsed, &method, params)?
        }
        other => return Err(bad(format!("there's no command `{other}`"))),
    };
    parsed.finish(name)?;
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<Command, Failure> {
        let args: Vec<String> = args.iter().map(|&arg| arg.to_owned()).collect();
        parse(&args)
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
                Duration::from_secs(30),
                Duration::from_secs(5),
                vec!["--vsync".to_owned()]
            )
        );
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
        // Tests run in the package's directory, which is in no project.
        assert_eq!(run.project, None);
        let shards = std::path::absolute("../../games/shards").unwrap();
        for project in ["../../games/shards", "../../games/shards/project.ron"] {
            let Ok(Command::Run(run)) = parsed(&["run", "--project", project]) else {
                panic!("a run");
            };
            assert_eq!(run.package, "shards", "{project}");
            let manifest = shards.join(Project::FILE);
            assert_eq!(run.project.map(|scope| scope.manifest), Some(manifest));
        }
        let Ok(Command::Check(check)) =
            parsed(&["check", "sandbox", "--project=../../games/shards"])
        else {
            panic!("a check");
        };
        assert_eq!(
            (check.package.as_str(), check.project.is_some()),
            ("sandbox", true)
        );
        let Ok(Command::Call { pick, .. }) = parsed(&["status", "--project", "../../games/shards"])
        else {
            panic!("a call");
        };
        assert_eq!(pick.project, Some(shards.join(Project::FILE)));
        let Ok(Command::New(name)) = parsed(&["new", "probe"]) else {
            panic!("a new game");
        };
        assert_eq!(name, "probe");
        let snapshot = std::path::absolute("world.snapshot").unwrap();
        let png = std::path::absolute("view.png").unwrap();
        let level = std::path::absolute("scenes/level2.ron").unwrap();
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
            (&["look"], json!({})),
            (
                &["look", r#"{"grid":"xz","outline":null}"#],
                json!({"grid": "xz", "outline": null}),
            ),
            // A pixel outside the view too, which the game answers with none.
            (&["pick", "3", "-4.5"], json!({"at": [3.0, -4.5]})),
            (&["assets"], json!({"kind": null, "skip": 0, "take": 100})),
            (
                &["assets", "--kind", "model", "--take", "5"],
                json!({"kind": "model", "skip": 0, "take": 5}),
            ),
        ] {
            let Ok(Command::Call { method, params, .. }) = parsed(args) else {
                panic!("a call");
            };
            assert_eq!((method.as_str(), params), (args[0], want), "{args:?}");
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
            &["run"],
            &["run", "a", "b"],
            &["run", "a", "--snapshot", "often"],
            &["run", "a", "--peers", "3"],
            &["run", "a", "--peers", "0"],
            &["run", "a", "--peers", "2", "--", "--host", "0.0.0.0:7700"],
            &["run", "a", "--peers", "2", "--", "--record=run.recording"],
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
            &["look", "{not json"],
            &["look", "[]"],
            &["look", "{}", "{}"],
            &["pick", "1"],
            &["pick", "1", "2", "3"],
            &["pick", "left", "2"],
            &["pick", "1", "NaN"],
            &["status", "--frames", "2"],
            &["status", "extra"],
            &["step", "many"],
            &["entities", "--take"],
            &["assets", "x"],
            &["logs", "--level", "loud"],
            &["call", "status", "{not json"],
            &["status", "--release=yes"],
            &["status", "--", "x"],
            &["check"],
            &["new"],
            &["new", "2probe"],
            &["new", "a", "b"],
            &["new", "probe", "--project", "../../games/shards"],
        ] {
            let error = parsed(args).err().map(|error| error.code);
            assert_eq!(error.as_deref(), Some("gg.args"), "{args:?}");
        }
        // A project whose manifest doesn't read is an error, which isn't the arguments'.
        let error = parsed(&["status", "--project", "src"]).err();
        assert_eq!(
            error.map(|error| error.code).as_deref(),
            Some("project.read")
        );
    }
}
