//! `gg`, the command-line tool for GGEngine's games, as ARCHITECTURE.md's Agents and tools
//! describes: it builds a game's development build and runs it, supervising it; runs headless
//! checks; and talks to running games through the tool API. It prints what it finds as JSON, a line
//! at a time, for agents and scripts as much as for people.

mod build;
mod game;
mod run;

use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use gg_api::Failure;
use serde_json::{Value, json};

use game::Pick;
use run::{Check, Run};

const USAGE: &str = "\
Usage: gg <command> [options]

Build and run:
  run <PACKAGE> [--release] [--hang <SECONDS>] [-- <ARGS>...]
      Build PACKAGE's development build and run it with ARGS until it ends, printing a line as it
      starts, as it serves the tool API, and as it ends: `exited`, or `crashed` or `hung`, with the
      errors its log holds. A run whose frames stop for SECONDS (10), while it waits for nothing
      the user does, has hung, and is ended.
  check <PACKAGE> [--release] [--frames <N>] [--timeout <SECONDS>] [-- <ARGS>...]
      Build PACKAGE and run it headless for N frames (60) with ARGS, and print what it did: its exit
      status, its warnings and errors, and its frame times. One that runs longer than SECONDS
      (600) is ended.

Talk to a running game, through its tool API:
  sessions                       List the running games
  status                         Its ticks, whether it's paused, its frames, entities and view
  pause                          Pause its simulation
  resume                         Run its simulation in real time again
  step [N]                       Run N ticks (1), and stay paused
  capture [PATH]                 Write its view as a PNG at PATH, or in `captures` beside it
  entities [--with <COMPONENT>] [--skip <N>] [--take <N>]
                                 List its entities, those with COMPONENT, N at a time (100)
  entity <ID>                    An entity and its components' values
  get <ID> <COMPONENT> [PATH]    A component's value, or its field at PATH, such as `translation.x`
  hash                           Its world's hash
  logs [--level <LEVEL>]         Its log's events at LEVEL (`warn`) and above
  quit                           End it cleanly, as closing its window does, unasked
  call <METHOD> [<PARAMS>]       Send METHOD with PARAMS, as JSON, and print what it answers

Options for talking to a game:
  --pid <PID>           The game in process PID, when more than one runs
  --app <NAME>          The game whose app is NAME, when more than one runs
  --timeout <SECONDS>   How long to wait for its answer (30)

Each result is a line of JSON on stdout; `logs` prints the log's lines, and `run` a line for each
event. An error is a line on stderr, {\"error\":{\"code\":...,\"message\":...}}. Exits 0 on success,
1 on an error, a check that failed or a run that didn't end cleanly, and 2 on bad arguments.
";

/// What `gg` was asked to do.
enum Command {
    Help,
    Run(Run),
    Check(Check),
    Sessions,
    Logs {
        pick: Pick,
        level: u8,
    },
    Call {
        pick: Pick,
        method: String,
        params: Value,
    },
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
            return ExitCode::from(2);
        }
    };
    let done = match command {
        Command::Help => {
            print(USAGE.trim_end());
            Ok(true)
        }
        Command::Run(run) => run::run(&run),
        Command::Check(check) => run::check(&check),
        Command::Sessions => game::sessions().map(|sessions| {
            print(&sessions.to_string());
            true
        }),
        Command::Logs { pick, level } => game::logs(&pick, level).map(|()| true),
        Command::Call {
            pick,
            method,
            params,
        } => game::call(&pick, &method, params).map(|result| {
            print(result.get());
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

/// The flags that take a value: `--name value` or `--name=value`.
const VALUED: [&str; 9] = [
    "--pid",
    "--app",
    "--timeout",
    "--hang",
    "--frames",
    "--with",
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

    /// The options for talking to a game.
    fn pick(&mut self) -> Result<Pick, Failure> {
        Ok(Pick {
            pid: self
                .value("--pid")
                .map(|pid| pid.parse())
                .transpose()
                .map_err(|_| bad("`--pid` takes a process's ID, a whole number".to_owned()))?,
            app: self.value("--app"),
            timeout: self.seconds("--timeout", 30)?,
        })
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

/// The command `args`, the arguments after `gg`, give.
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
            let package = parsed.positional(1, 1, "run <PACKAGE>")?.remove(0);
            Command::Run(Run {
                package,
                release: parsed.switch("--release"),
                hang: parsed.seconds("--hang", 10)?,
                args: std::mem::take(&mut parsed.rest),
            })
        }
        "check" => {
            let package = parsed.positional(1, 1, "check <PACKAGE>")?.remove(0);
            Command::Check(Check {
                package,
                release: parsed.switch("--release"),
                frames: parsed.number("--frames", 60)?,
                timeout: parsed.seconds("--timeout", 600)?,
                args: std::mem::take(&mut parsed.rest),
            })
        }
        "sessions" => {
            parsed.positional(0, 0, "sessions")?;
            Command::Sessions
        }
        "logs" => {
            parsed.positional(0, 0, "logs [--level <LEVEL>]")?;
            let level = parsed.value("--level").unwrap_or_else(|| "warn".to_owned());
            let level = game::rank(&level.to_ascii_uppercase()).ok_or_else(|| {
                bad(format!(
                    "`--level` is `error`, `warn`, `info`, `debug` or `trace`, not `{level}`"
                ))
            })?;
            Command::Logs {
                pick: parsed.pick()?,
                level,
            }
        }
        method @ ("status" | "pause" | "resume" | "hash" | "quit") => {
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
        "capture" => {
            let path = parsed.positional(0, 1, "capture [PATH]")?;
            let params = match path.first() {
                // The game's working directory may not be this one.
                Some(path) => {
                    let path = std::path::absolute(Path::new(path))
                        .map_err(|error| bad(format!("can't tell where `{path}` is: {error}")))?;
                    json!({"path": path})
                }
                None => json!({}),
            };
            call(&mut parsed, "capture", params)?
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
        assert_eq!((run.package.as_str(), run.release), ("sandbox", false));
        assert_eq!(
            (run.hang, run.args),
            (Duration::from_secs(30), vec!["--vsync".to_owned()])
        );
        let Ok(Command::Logs { level, .. }) = parsed(&["logs", "--level", "info"]) else {
            panic!("logs");
        };
        assert_eq!(Some(level), game::rank("INFO"));
    }

    #[test]
    fn bad_arguments_are_refused_with_a_code() {
        for args in [
            &["nonsense"][..],
            &["run"],
            &["run", "a", "b"],
            &["status", "--frames", "2"],
            &["status", "extra"],
            &["step", "many"],
            &["entities", "--take"],
            &["logs", "--level", "loud"],
            &["call", "status", "{not json"],
            &["status", "--release=yes"],
            &["status", "--", "x"],
        ] {
            let error = parsed(args).err().map(|error| error.code);
            assert_eq!(error.as_deref(), Some("gg.args"), "{args:?}");
        }
    }
}
