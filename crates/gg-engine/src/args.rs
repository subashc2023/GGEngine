//! The command-line flags every app shares.

use std::ffi::OsString;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use gg_core::{Error, Result};

#[derive(Debug, PartialEq)]
pub(crate) struct Args {
    pub(crate) headless: bool,
    pub(crate) frames: Option<u64>,
    pub(crate) gpu: Option<String>,
    pub(crate) validation: bool,
    pub(crate) vsync: bool,
    /// The threads the simulation's jobs run on, the main thread among them.
    pub(crate) threads: NonZeroUsize,
    /// Where the run is recorded: its seed, its layers' commands and the world's hash at each tick.
    pub(crate) record: Option<PathBuf>,
    /// The recording replayed, without a window or a GPU, in place of a run.
    pub(crate) replay: Option<PathBuf>,
    /// Where the replay stops, to write the world as it is there.
    pub(crate) dump: Option<Dump>,
}

/// Where a replay stops to write the world: once its layers have attached, or after a tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Dump {
    Attach,
    Tick(u64),
}

/// The flags of a run that a replay, which renders nothing and records nothing, refuses.
const NOT_REPLAYED: [&str; 7] = [
    "--record",
    "--headless",
    "--frames",
    "--gpu",
    "--validation",
    "--no-validation",
    "--vsync",
];

/// The help text, for the program called `program`.
pub(crate) fn usage(program: &str) -> String {
    format!(
        "Usage: {program} [options]

Options:
  --headless        Run without a window, rendering offscreen, a tick of the simulation a frame
  --frames <N>      Exit after N frames
  --gpu <NAME>      Use the GPU whose name contains NAME, such as `--gpu amd`
  --validation      Run under Vulkan validation (the default in debug builds)
  --no-validation   Run without Vulkan validation (the default in release builds)
  --vsync           Present at the display's refresh rate, rather than uncapped
  --threads <N>     Run the simulation's jobs on N threads, the main one among them (one per CPU
                    but the one the render thread has)
  --record <PATH>   Record the run to PATH: its seed, its layers' commands and its hash each tick
  --replay <PATH>   Replay the recording at PATH without a window or a GPU, checking each hash
  --dump <TICK>     With --replay, stop after tick TICK, or at `attach`, and print the world
  -h, --help        Print this help

Environment:
  RUST_LOG          The log filter, such as `debug` or `info,gg_gpu=trace`
  NO_COLOR          Turns the console's colors off

Exits 0 on success, 1 on an error, a validation message or a replay that goes another way, and 2
on bad arguments.
"
    )
}

/// Parses the arguments after the program name, or returns `None` for `--help`.
pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Option<Args>> {
    let mut parsed = Args {
        headless: false,
        frames: None,
        gpu: None,
        validation: cfg!(debug_assertions),
        vsync: false,
        threads: std::thread::available_parallelism()
            .ok()
            .and_then(|cpus| NonZeroUsize::new(cpus.get() - 1))
            .unwrap_or(NonZeroUsize::MIN),
        record: None,
        replay: None,
        dump: None,
    };
    // The first flag given that a replay refuses.
    let mut not_replayed = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|arg| bad(format!("{arg:?} isn't valid Unicode")))?;
        // `--frames=60` and `--frames 60` both work.
        let (flag, mut value) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        let mut value_of = |flag: &str| {
            value
                .take()
                .or_else(|| args.next().and_then(|value| value.into_string().ok()))
                .ok_or_else(|| bad(format!("`{flag}` needs a value")))
        };
        if NOT_REPLAYED.contains(&flag) {
            not_replayed.get_or_insert_with(|| flag.to_owned());
        }
        match flag {
            "-h" | "--help" => return Ok(None),
            "--headless" => parsed.headless = true,
            "--validation" => parsed.validation = true,
            "--no-validation" => parsed.validation = false,
            "--vsync" => parsed.vsync = true,
            "--frames" => {
                let frames = value_of(flag)?;
                let frames = frames.parse().map_err(|_| {
                    bad(format!(
                        "`--frames` takes a whole number of frames, not `{frames}`"
                    ))
                })?;
                parsed.frames = Some(frames);
            }
            "--threads" => {
                let threads = value_of(flag)?;
                parsed.threads = threads.parse().map_err(|_| {
                    bad(format!(
                        "`--threads` takes a number of threads, 1 or more, not `{threads}`"
                    ))
                })?;
            }
            "--gpu" => parsed.gpu = Some(value_of(flag)?),
            "--record" => parsed.record = Some(value_of(flag)?.into()),
            "--replay" => parsed.replay = Some(value_of(flag)?.into()),
            "--dump" => {
                let at = value_of(flag)?;
                let dump = match at.as_str() {
                    "attach" => Some(Dump::Attach),
                    tick => tick.parse().ok().map(Dump::Tick),
                };
                parsed.dump = Some(dump.ok_or_else(|| {
                    bad(format!(
                        "`--dump` takes a tick's number, or `attach`, not `{at}`"
                    ))
                })?);
            }
            _ => return Err(bad(format!("unknown argument `{arg}`"))),
        }
        if value.is_some() {
            return Err(bad(format!("`{flag}` takes no value")));
        }
    }
    if parsed.replay.is_some()
        && let Some(flag) = not_replayed
    {
        return Err(bad(format!(
            "`--replay` runs the recording's ticks alone, rendering and recording nothing, so it \
             takes no `{flag}`"
        )));
    }
    if parsed.dump.is_some() && parsed.replay.is_none() {
        return Err(bad(
            "`--dump` stops a replay, so it needs `--replay`".to_owned()
        ));
    }
    Ok(Some(parsed))
}

fn bad(message: String) -> Error {
    Error::new("engine.args", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Option<Args>> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn parses_shared_flags_in_both_forms() {
        let args = parse_strs(&[
            "--headless",
            "--frames",
            "60",
            "--gpu=AMD Radeon",
            "--vsync",
        ])
        .unwrap()
        .unwrap();
        assert!(args.headless);
        assert!(args.vsync);
        assert_eq!(args.frames, Some(60));
        assert_eq!(args.gpu.as_deref(), Some("AMD Radeon"));
        assert_eq!(
            parse_strs(&["--frames=60"]).unwrap().unwrap().frames,
            Some(60)
        );
        // A CPU for each but the render thread's.
        let cpus = std::thread::available_parallelism().unwrap().get();
        assert_eq!(
            parse_strs(&[]).unwrap().unwrap().threads.get(),
            cpus.saturating_sub(1).max(1)
        );
        for args in [&["--threads", "3"][..], &["--threads=3"]] {
            assert_eq!(parse_strs(args).unwrap().unwrap().threads.get(), 3);
        }
        // A replay runs on as many threads as it's given: the ticks go the same way on any.
        let args = parse_strs(&["--replay", "run.recording", "--threads", "1"]);
        assert_eq!(args.unwrap().unwrap().threads, NonZeroUsize::MIN);
        let args = parse_strs(&["--record=run.recording", "--headless"])
            .unwrap()
            .unwrap();
        assert_eq!(args.record, Some(PathBuf::from("run.recording")));
        for (dump, at) in [("attach", Dump::Attach), ("300", Dump::Tick(300))] {
            let args = parse_strs(&["--replay", "run.recording", "--dump", dump]);
            let args = args.unwrap().unwrap();
            assert_eq!(args.replay, Some(PathBuf::from("run.recording")));
            assert_eq!(args.dump, Some(at));
        }
    }

    #[test]
    fn a_replay_refuses_the_flags_of_a_run() {
        for flag in NOT_REPLAYED {
            let args = ["--replay", "run.recording", flag, "1"];
            let refused = if flag == "--frames" || flag == "--gpu" || flag == "--record" {
                &args[..]
            } else {
                &args[..3]
            };
            let error = parse_strs(refused).unwrap_err();
            assert_eq!(error.code(), "engine.args", "{flag}: {error}");
            assert!(error.to_string().contains(&format!("`{flag}`")), "{error}");
        }
    }

    #[test]
    fn help_stops_parsing() {
        assert_eq!(parse_strs(&["--help", "--bogus"]).unwrap(), None);
    }

    #[test]
    fn rejects_bad_arguments_with_a_code() {
        for args in [
            &["--bogus"][..],
            &["--frames"],
            &["--frames", "sixty"],
            &["--threads", "0"],
            &["--threads", "x"],
            &["--headless=yes"],
            &["--dump", "attach"],
            &["--replay", "run.recording", "--dump", "-1"],
            &["--replay", "run.recording", "--dump", "first"],
        ] {
            let error = parse_strs(args).unwrap_err();
            assert_eq!(error.code(), "engine.args", "{args:?}: {error}");
        }
    }
}
