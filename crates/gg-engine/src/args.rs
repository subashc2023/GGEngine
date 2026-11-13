//! The command-line flags every app shares.

use std::ffi::OsString;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::time::Duration;

use gg_core::{Error, Result};
use gg_renderer::MAX_TEXTURE_SIZE;

#[derive(Debug, PartialEq)]
pub(crate) struct Args {
    pub(crate) headless: bool,
    /// Whether the run is embedded in the tool that started it, such as the editor: offscreen, in
    /// real time, its view published to the tool through the tool API, as it asks.
    pub(crate) embedded: bool,
    /// Whether the run ends once stdin ends, as it does as the supervisor that holds it, such as
    /// `gg run`, ends, however it ends.
    pub(crate) supervised: bool,
    /// Whether a headless run paces its ticks to real time, its frames a tick apart, rather than
    /// running a tick each frame as fast as it can.
    pub(crate) real_time: bool,
    pub(crate) frames: Option<u64>,
    /// A headless run's view, its width and height in pixels, if not 1280 by 720.
    pub(crate) size: Option<[u32; 2]>,
    /// The frame captured through RenderDoc, counting from 1.
    pub(crate) capture: Option<NonZeroU64>,
    pub(crate) gpu: Option<String>,
    pub(crate) validation: bool,
    pub(crate) vsync: bool,
    /// The most frames a second a windowed run renders, as
    /// [`Context::set_max_fps`](crate::Context::set_max_fps) says, if capped.
    pub(crate) max_fps: Option<NonZeroU32>,
    /// The threads the simulation's jobs run on, the main thread among them.
    pub(crate) threads: NonZeroUsize,
    /// Where the run is recorded: its seed, its layers' commands and the world's hash at each tick.
    pub(crate) record: Option<PathBuf>,
    /// The recording replayed, without a window or a GPU, in place of a run.
    pub(crate) replay: Option<PathBuf>,
    /// Where the replay stops, to write the world as it is there.
    pub(crate) dump: Option<Dump>,
    /// Whether each tick runs again, on a copy of the world taken as it started, a replay's too.
    pub(crate) sync_test: bool,
    /// Whether a development build serves the tool API.
    pub(crate) api: bool,
    /// The snapshot the run starts from, in place of the world its layers would attach.
    pub(crate) restore: Option<PathBuf>,
    /// Whether a run that doesn't start from a snapshot starts stopped, its world the world as
    /// edited, until the tool API's `play`.
    pub(crate) stopped: bool,
    /// The session the run hosts, joins, serves or connects to, if it's networked.
    pub(crate) net: Option<Net>,
    /// Whether the session's peers predict each other's inputs, rolling back where a prediction
    /// missed, rather than waiting for them.
    pub(crate) rollback: bool,
    /// What the links do to the packets the run receives, as a slow link would.
    pub(crate) link: Option<Conditions>,
    /// The folder of the mods the run runs, in place of those installed.
    pub(crate) mods: Option<PathBuf>,
    /// The game's package the run reads the files under its asset root from.
    pub(crate) package: Option<PathBuf>,
}

/// A networked session's end a run takes, and the address it listens at or connects to, as given:
/// a lockstep session's host or joiner, or a server or its client.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Net {
    Host(String),
    Join(String),
    Serve(String),
    Connect(String),
}

/// What `--link` has a peer's link do to what it receives, as a slow link would: hold each packet
/// `latency`, and up to `jitter` more, which may reorder them, and lose `loss` of them, from 0 up
/// to 1; and hold what comes on the session's stream `latency`, in order, losing none, as TCP does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Conditions {
    pub(crate) latency: Duration,
    pub(crate) jitter: Duration,
    pub(crate) loss: f64,
}

/// Where a replay stops to write the world: once its layers have attached, or after a tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Dump {
    Attach,
    Tick(u64),
}

/// The flags of a run that a replay, which renders nothing, records nothing and serves no tools,
/// refuses.
const NOT_REPLAYED: [&str; 22] = [
    "--record",
    "--restore",
    "--headless",
    "--real-time",
    "--embedded",
    "--frames",
    "--size",
    "--capture",
    "--gpu",
    "--validation",
    "--no-validation",
    "--vsync",
    "--max-fps",
    "--no-api",
    "--stopped",
    "--host",
    "--join",
    "--serve",
    "--connect",
    "--rollback",
    "--link",
    "--mods",
];

/// The help text, for the program called `program`.
pub(crate) fn usage(program: &str) -> String {
    format!(
        "Usage: {program} [options]

Options:
  --headless        Run without a window, rendering offscreen, a tick of the simulation a frame
  --embedded        Run without a window, rendering offscreen in real time for the tool that
                    started the run, such as the editor, which the tool API's `view` publishes the
                    view to and its `input` forwards input from: a frame once it has taken the last
  --supervised      End the run, as the tool API's `quit` does, once stdin ends, as it does as the
                    supervisor that holds it, such as `gg run`, ends, however it ends; a replay
                    ends at its recording's end alone
  --real-time       With --headless, step the simulation in real time, its frames a tick apart,
                    rather than a tick a frame as fast as frames run, so that a game watched
                    through the tool API plays at its own pace; --frames and --capture count
                    frames as they come, a tick's time apart or more. Not for --host or --join,
                    whose headless peers step in lockstep
  --frames <N>      Exit after N frames
  --size <WxH>      With --headless, render the view W by H pixels, such as `--size 320x180`,
                    rather than 1280 by 720
  --capture <N>     Capture frame N through RenderDoc, in a run under `renderdoccmd capture`
  --gpu <NAME>      Use the GPU whose name contains NAME, such as `--gpu amd`
  --validation      Run under Vulkan validation (the default in debug builds)
  --no-validation   Run without Vulkan validation (the default in release builds)
  --vsync           Present at the display's refresh rate, rather than uncapped
  --max-fps <N>     Render at most N frames a second in a window, such as `--max-fps 60`, in place
                    of the cap the title bar's settings keep in a development build; an embedded
                    run goes at its tool's pace, so an editor capped caps its game
  --threads <N>     Run the simulation's jobs on N threads, the main one among them (one per CPU
                    but the one the render thread has)
  --record <PATH>   Record the run to PATH: its seed, its layers' commands and its hash each tick
  --replay <PATH>   Replay the recording at PATH without a window or a GPU, checking each hash
  --dump <TICK>     With --replay, stop after tick TICK, or at `attach`, and print the world
  --sync-test       Run each tick again, a replay's too, on a copy of the world taken as it
                    started, with the commands it applied, as rollback runs ticks again, and end
                    the run where the copy and the world part, naming the field
  --restore <PATH>  Start from the snapshot at PATH, which the tool API's `snapshot` writes, where
                    its world was, rather than building the world afresh
  --no-api          Serve no tool API, which development builds serve for tools such as `gg`
  --stopped         Start stopped: the world the layers attach is the world as edited, which only
                    the tool API's edits, its `scene open` and `scene new`, and scene files' saves
                    change, until its `play`. A run given `--restore` goes on stopped or playing,
                    as the snapshot's run was
  --host <ADDRESS>  Host a lockstep session at ADDRESS, such as `0.0.0.0:7700`, as player 0
  --join <ADDRESS>  Join the lockstep session at ADDRESS as player 1, from the host's world
  --rollback        With --host or --join, of both peers: predict the other peer's inputs, 8 to 16
                    ticks on, as far as the link needs, and roll back where a prediction missed,
                    its ticks run again, with an input delay of 2 ticks rather than lockstep's 2 to
                    16, which follows the link
  --serve <ADDRESS> Serve the world at ADDRESS, such as `0.0.0.0:7700`, to the clients that connect,
                    as the server that owns it: without a window or a GPU, in real time, until
                    Ctrl+C, or --frames frames of a tick's time
  --connect <ADDRESS>
                    Play as a client of the server at ADDRESS, from its world, as the player it
                    gives this run, predicting the world, the other players drawn as it last had
                    them
  --link <MS,MS,%>  With --host, --join, --serve or --connect, have each link hold each packet this
                    run receives the first MS, and up to the second more, losing the percent of
                    them, such as `--link 100,20,5`, and hold what comes on its stream the first MS
  --mods <DIR>     Run the mods in the folder DIR, in place of those installed in the game's
                    per-user folder, which a windowed run runs
  --package <PATH>  Read the files under the asset root from the game's package at PATH, which
                    `tools/cook` writes, in place of the files themselves, as a build without
                    `dev` reads `<executable>.ggpk` beside its executable
  -h, --help        Print this help

Environment:
  RUST_LOG          The log filter, such as `debug` or `info,gg_gpu=trace`
  NO_COLOR          Turns the console's colors off

Exits 0 on success, 1 on an error, a validation message, a replay that goes another way or a tick
that goes another way run again, and 2 on bad arguments.
"
    )
}

/// Parses the arguments after the program name, or returns `None` for `--help`.
pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Option<Args>> {
    let mut parsed = Args {
        headless: false,
        embedded: false,
        supervised: false,
        real_time: false,
        frames: None,
        size: None,
        capture: None,
        gpu: None,
        validation: cfg!(debug_assertions),
        vsync: false,
        max_fps: None,
        threads: std::thread::available_parallelism()
            .ok()
            .and_then(|cpus| NonZeroUsize::new(cpus.get() - 1))
            .unwrap_or(NonZeroUsize::MIN),
        record: None,
        replay: None,
        dump: None,
        sync_test: false,
        api: true,
        restore: None,
        stopped: false,
        net: None,
        rollback: false,
        link: None,
        mods: None,
        package: None,
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
            "--embedded" => parsed.embedded = true,
            "--supervised" => parsed.supervised = true,
            "--real-time" => parsed.real_time = true,
            "--validation" => parsed.validation = true,
            "--no-validation" => parsed.validation = false,
            "--vsync" => parsed.vsync = true,
            "--no-api" => parsed.api = false,
            "--stopped" => parsed.stopped = true,
            "--sync-test" => parsed.sync_test = true,
            "--rollback" => parsed.rollback = true,
            "--link" => {
                let link = value_of(flag)?;
                parsed.link = Some(conditions(&link).ok_or_else(|| {
                    bad(format!(
                        "`--link` takes the milliseconds each packet is held, the most more, and \
                         the percent lost, from 0 to less than 100, such as `100,20,5`, not \
                         `{link}`"
                    ))
                })?);
            }
            "--frames" => {
                let frames = value_of(flag)?;
                let frames = frames.parse().map_err(|_| {
                    bad(format!(
                        "`--frames` takes a whole number of frames, not `{frames}`"
                    ))
                })?;
                parsed.frames = Some(frames);
            }
            "--size" => {
                let size = value_of(flag)?;
                parsed.size = Some(sides(&size).ok_or_else(|| {
                    bad(format!(
                        "`--size` takes the view's width and height in pixels, each from 1 to \
                         {MAX_TEXTURE_SIZE}, such as `320x180`, not `{size}`"
                    ))
                })?);
            }
            "--capture" => {
                let frame = value_of(flag)?;
                parsed.capture = Some(frame.parse().map_err(|_| {
                    bad(format!(
                        "`--capture` takes a frame's number, from 1, not `{frame}`"
                    ))
                })?);
            }
            "--max-fps" => {
                let max = value_of(flag)?;
                parsed.max_fps = Some(max.parse().map_err(|_| {
                    bad(format!(
                        "`--max-fps` takes a whole number of frames a second, 1 or more, not \
                         `{max}`"
                    ))
                })?);
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
            // The last given, which `gg run` gives after the game's own.
            "--restore" => parsed.restore = Some(value_of(flag)?.into()),
            "--host" | "--join" | "--serve" | "--connect" if parsed.net.is_some() => {
                return Err(bad(
                    "a run takes part in one networked session, so it takes one `--host`, \
                     `--join`, `--serve` or `--connect`"
                        .to_owned(),
                ));
            }
            "--host" => parsed.net = Some(Net::Host(value_of(flag)?)),
            "--join" => parsed.net = Some(Net::Join(value_of(flag)?)),
            "--serve" => parsed.net = Some(Net::Serve(value_of(flag)?)),
            "--connect" => parsed.net = Some(Net::Connect(value_of(flag)?)),
            "--mods" => parsed.mods = Some(value_of(flag)?.into()),
            "--package" => parsed.package = Some(value_of(flag)?.into()),
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
    if parsed.restore.is_some() && parsed.record.is_some() {
        return Err(bad(
            "`--restore` starts the run from a snapshot, which a recording, whose replay starts \
             from a seed, can't start from, so it takes no `--record`"
                .to_owned(),
        ));
    }
    if matches!(parsed.net, Some(Net::Join(_))) && parsed.record.is_some() {
        return Err(bad(
            "`--join` starts the run from the host's world, which a recording, whose replay starts \
             from a seed, can't start from, so it takes no `--record`"
                .to_owned(),
        ));
    }
    if matches!(parsed.net, Some(Net::Connect(_))) && parsed.record.is_some() {
        return Err(bad(
            "`--connect` starts the run from the server's world, and corrects it to the server's \
             after, which a recording, whose replay goes from a seed by its commands alone, can't \
             follow, so it takes no `--record`"
                .to_owned(),
        ));
    }
    if matches!(parsed.net, Some(Net::Serve(_))) {
        let renders = [
            (parsed.record.is_some(), "--record"),
            (parsed.embedded, "--embedded"),
            (parsed.capture.is_some(), "--capture"),
            (parsed.size.is_some(), "--size"),
            (parsed.gpu.is_some(), "--gpu"),
            (parsed.vsync, "--vsync"),
            (parsed.max_fps.is_some(), "--max-fps"),
        ];
        if let Some((_, flag)) = renders.iter().find(|(given, _)| *given) {
            let why = if *flag == "--record" {
                "applies its clients' inputs as they come, which a recording can't keep yet"
            } else {
                "renders nothing, without a GPU"
            };
            return Err(bad(format!("`--serve` {why}, so it takes no `{flag}`")));
        }
    }
    if parsed.embedded && parsed.headless {
        return Err(bad(
            "`--headless` and `--embedded` each run without a window, a tick a frame or in real \
             time, so a run takes one"
                .to_owned(),
        ));
    }
    if parsed.real_time && !parsed.headless {
        return Err(bad(
            "`--real-time` paces a headless run's ticks, as windowed and embedded runs step in \
             real time already, so it needs `--headless`"
                .to_owned(),
        ));
    }
    if parsed.real_time && matches!(parsed.net, Some(Net::Host(_) | Net::Join(_))) {
        return Err(bad(
            "`--real-time` steps a run by the clock, and a headless lockstep peer a tick a frame, \
             as its other peer's inputs come, so `--host` and `--join` take no `--real-time`"
                .to_owned(),
        ));
    }
    if parsed.size.is_some() && !parsed.headless {
        return Err(bad(
            "`--size` is of a headless run's view, as a window's view is the window's and an \
             embedded run's the size its tool asks, so it needs `--headless`"
                .to_owned(),
        ));
    }
    if parsed.max_fps.is_some() && (parsed.headless || parsed.embedded) {
        return Err(bad(
            "`--max-fps` caps a window's frames, as `--real-time` paces a headless run's and an \
             embedded run goes at its tool's pace, so it takes no `--headless` or `--embedded`"
                .to_owned(),
        ));
    }
    if parsed.embedded && !(cfg!(feature = "dev") && parsed.api) {
        return Err(bad(
            "`--embedded` publishes the view through the tool API, which only development builds \
             serve, and not with `--no-api`"
                .to_owned(),
        ));
    }
    if parsed.stopped && !(cfg!(feature = "dev") && parsed.api) {
        return Err(bad(
            "`--stopped` starts the run stopped until the tool API's `play`, which only \
             development builds serve, and not with `--no-api`"
                .to_owned(),
        ));
    }
    if parsed.stopped && parsed.record.is_some() {
        return Err(bad(
            "`--stopped` starts the run stopped, and the tool API's `stop` brings back the world \
             as edited between ticks, taking its ticks back, which a recording's replay can't \
             follow, so it takes no `--record`"
                .to_owned(),
        ));
    }
    if parsed.stopped && parsed.net.is_some() {
        return Err(bad(
            "`--stopped` starts the run stopped, but a networked session's runs play from start to \
             end, so that their worlds don't part, so it takes no `--host`, `--join`, `--serve` or \
             `--connect`"
                .to_owned(),
        ));
    }
    let peer = matches!(parsed.net, Some(Net::Host(_) | Net::Join(_)));
    if parsed.rollback && !peer {
        return Err(bad(
            "`--rollback` is of a lockstep session's peers, so it needs `--host` or `--join`"
                .to_owned(),
        ));
    }
    if parsed.link.is_some() && parsed.net.is_none() {
        return Err(bad(
            "`--link` is of a networked run's links, so it needs `--host`, `--join`, `--serve` or \
             `--connect`"
                .to_owned(),
        ));
    }
    if parsed.rollback && parsed.record.is_some() {
        return Err(bad(
            "`--rollback` runs ticks again as predictions miss, which a recording, written as \
             each tick runs, can't follow yet, so it takes no `--record`"
                .to_owned(),
        ));
    }
    if parsed.dump.is_some() && parsed.replay.is_none() {
        return Err(bad(
            "`--dump` stops a replay, so it needs `--replay`".to_owned()
        ));
    }
    if let (Some(capture), Some(frames)) = (parsed.capture, parsed.frames)
        && capture.get() > frames
    {
        return Err(bad(format!(
            "`--capture {capture}` names a frame past the last, as the run exits after \
             `--frames {frames}`"
        )));
    }
    Ok(Some(parsed))
}

fn bad(message: String) -> Error {
    Error::new("engine.args", message)
}

/// What `--size`'s `text` says, as `320x180` does: a width and a height in pixels, each from 1 to
/// the largest side a texture takes on every device.
fn sides(text: &str) -> Option<[u32; 2]> {
    let (width, height) = text.split_once('x')?;
    let side = |text: &str| {
        text.parse()
            .ok()
            .filter(|side| (1..=MAX_TEXTURE_SIZE).contains(side))
    };
    Some([side(width)?, side(height)?])
}

/// What `--link`'s `text` says, as `100,20,5` does: the whole milliseconds each packet is held,
/// the most more, and the percent lost, from 0 to less than 100, which would lose them all.
fn conditions(text: &str) -> Option<Conditions> {
    let mut parts = text.split(',');
    let mut millis = || parts.next()?.parse().ok().map(Duration::from_millis);
    let (latency, jitter) = (millis()?, millis()?);
    let percent: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(0.0..100.0).contains(&percent) {
        return None;
    }
    Some(Conditions {
        latency,
        jitter,
        loss: percent / 100.0,
    })
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
        assert_eq!(args.size, None);
        // A window's frames go uncapped unless capped.
        assert_eq!(args.max_fps, None);
        for args in [&["--max-fps", "60"][..], &["--max-fps=60"]] {
            let max_fps = parse_strs(args).unwrap().unwrap().max_fps;
            assert_eq!(max_fps, NonZeroU32::new(60), "{args:?}");
        }
        for (size, expected) in [("320x180", [320, 180]), ("1x8192", [1, 8192])] {
            let args = parse_strs(&["--headless", "--size", size]);
            assert_eq!(args.unwrap().unwrap().size, Some(expected), "{size}");
        }
        // A CPU for each but the render thread's.
        let cpus = std::thread::available_parallelism().unwrap().get();
        assert_eq!(
            parse_strs(&[]).unwrap().unwrap().threads.get(),
            cpus.saturating_sub(1).max(1)
        );
        for args in [&["--threads", "3"][..], &["--threads=3"]] {
            assert_eq!(parse_strs(args).unwrap().unwrap().threads.get(), 3);
        }
        // The last frame of a run of a set number of frames may be captured.
        for args in [
            &["--capture", "3"][..],
            &["--capture=3"],
            &["--frames=3", "--capture=3"],
        ] {
            let capture = parse_strs(args).unwrap().unwrap().capture;
            assert_eq!(capture, NonZeroU64::new(3), "{args:?}");
        }
        // A replay runs on as many threads as it's given: the ticks go the same way on any.
        let args = parse_strs(&["--replay", "run.recording", "--threads", "1"]);
        assert_eq!(args.unwrap().unwrap().threads, NonZeroUsize::MIN);
        let args = parse_strs(&["--record=run.recording", "--headless"])
            .unwrap()
            .unwrap();
        assert_eq!(args.record, Some(PathBuf::from("run.recording")));
        // Development builds serve the tool API unless told not to.
        assert!(args.api);
        // A headless run ticks as fast as it can unless it's paced to real time.
        assert!(!args.real_time);
        let paced = parse_strs(&["--headless", "--real-time"]).unwrap().unwrap();
        assert!(paced.headless && paced.real_time);
        // Counting its frames as they come.
        let paced = parse_strs(&["--headless", "--real-time", "--frames", "60"]);
        assert_eq!(paced.unwrap().unwrap().frames, Some(60));
        assert!(!parse_strs(&["--no-api"]).unwrap().unwrap().api);
        // A run embedded in a tool publishes its view through the API, which only they serve.
        let embedded = parse_strs(&["--embedded"]).map(|args| args.unwrap().embedded);
        let expected = if cfg!(feature = "dev") {
            Ok(true)
        } else {
            Err("engine.args")
        };
        assert_eq!(embedded.map_err(|error| error.code()), expected);
        // So does a run started stopped, which only the API plays; and given a snapshot, as `gg
        // run` gives one after the game's own arguments, it goes on as the snapshot's run was.
        let stopped = parse_strs(&["--stopped", "--restore", "a.snapshot"]);
        let stopped = stopped.map(|args| args.unwrap().stopped);
        assert_eq!(stopped.map_err(|error| error.code()), expected);
        // The last snapshot given, which `gg run` gives after the game's own arguments.
        let args = parse_strs(&["--restore", "a.snapshot", "--restore=b.snapshot"]);
        let restore = args.unwrap().unwrap().restore;
        assert_eq!(restore, Some(PathBuf::from("b.snapshot")));
        // A run ends as its supervisor does, which gives every game it runs `--supervised`, a
        // replay too, which ends at its end alone.
        assert!(!parse_strs(&[]).unwrap().unwrap().supervised);
        for args in [
            &["--supervised"][..],
            &["--headless", "--supervised"],
            &["--replay", "run.recording", "--supervised"],
        ] {
            assert!(parse_strs(args).unwrap().unwrap().supervised, "{args:?}");
        }
        let host = parse_strs(&["--host", "0.0.0.0:7700"]).unwrap().unwrap();
        assert_eq!(host.net, Some(Net::Host("0.0.0.0:7700".to_owned())));
        let join = parse_strs(&["--join=127.0.0.1:7700", "--restore", "a.snapshot"]);
        let join = join.unwrap().unwrap().net;
        assert_eq!(join, Some(Net::Join("127.0.0.1:7700".to_owned())));
        // Either peer predicts, over a link that holds and loses its packets.
        let args = [
            "--join",
            "127.0.0.1:7700",
            "--rollback",
            "--link=100,20,2.5",
        ];
        let join = parse_strs(&args).unwrap().unwrap();
        assert!(join.rollback);
        let link = Conditions {
            latency: Duration::from_millis(100),
            jitter: Duration::from_millis(20),
            loss: 0.025,
        };
        assert_eq!(join.link, Some(link));
        let host = parse_strs(&["--host", "0.0.0.0:7700"]).unwrap().unwrap();
        assert_eq!((host.rollback, host.link), (false, None));
        // A server and its client, each over a link of its own.
        let serve = parse_strs(&["--serve", "0.0.0.0:7700", "--link", "100,20,5"]);
        let serve = serve.unwrap().unwrap();
        assert_eq!(serve.net, Some(Net::Serve("0.0.0.0:7700".to_owned())));
        assert!(serve.link.is_some());
        let connect = parse_strs(&["--connect=127.0.0.1:7700", "--headless", "--link=100,20,5"]);
        let connect = connect.unwrap().unwrap();
        assert_eq!(connect.net, Some(Net::Connect("127.0.0.1:7700".to_owned())));
        assert!(connect.headless && connect.link.is_some());
        // A run's ticks run again, and a replay's.
        assert!(!parse_strs(&[]).unwrap().unwrap().sync_test);
        for args in [
            &["--sync-test"][..],
            &["--replay", "run.recording", "--sync-test"],
        ] {
            assert!(parse_strs(args).unwrap().unwrap().sync_test, "{args:?}");
        }
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
            let value = if flag == "--size" { "320x180" } else { "1" };
            let args = ["--replay", "run.recording", flag, value];
            let takes_value = [
                "--frames",
                "--size",
                "--capture",
                "--gpu",
                "--max-fps",
                "--record",
                "--restore",
                "--host",
                "--join",
                "--serve",
                "--connect",
                "--mods",
            ]
            .contains(&flag);
            let refused = if takes_value { &args[..] } else { &args[..3] };
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
            &["--capture"],
            &["--capture", "0"],
            &["--capture", "x"],
            &["--frames", "2", "--capture", "3"],
            &["--threads", "0"],
            &["--threads", "x"],
            &["--max-fps"],
            &["--max-fps", "0"],
            &["--max-fps", "-60"],
            &["--max-fps", "sixty"],
            &["--max-fps", "59.94"],
            &["--headless", "--max-fps", "60"],
            &["--embedded", "--max-fps", "60"],
            &["--size", "320x180"],
            &["--embedded", "--size", "320x180"],
            &["--headless", "--size"],
            &["--headless", "--size", "320"],
            &["--headless", "--size", "0x180"],
            &["--headless", "--size", "320x8193"],
            &["--headless", "--size", "320x180x1"],
            &["--headless", "--size", "-320x180"],
            &["--headless=yes"],
            &["--supervised=yes"],
            &["--dump", "attach"],
            &["--replay", "run.recording", "--dump", "-1"],
            &["--replay", "run.recording", "--dump", "first"],
            &["--restore", "run.snapshot", "--record", "run.recording"],
            &["--host", "0.0.0.0:7700", "--join", "127.0.0.1:7700"],
            &["--join", "127.0.0.1:7700", "--record", "run.recording"],
            &["--host"],
            &["--embedded", "--headless"],
            &["--real-time"],
            &["--embedded", "--real-time"],
            &["--real-time=yes", "--headless"],
            &["--headless", "--real-time", "--host", "0.0.0.0:7700"],
            &["--headless", "--real-time", "--join", "127.0.0.1:7700"],
            &["--embedded", "--no-api"],
            &["--stopped", "--no-api"],
            &["--stopped", "--record", "run.recording"],
            &["--stopped", "--host", "0.0.0.0:7700"],
            &["--stopped", "--join", "127.0.0.1:7700"],
            &["--rollback"],
            &["--link", "100,20,5"],
            &[
                "--host",
                "0.0.0.0:7700",
                "--rollback",
                "--record",
                "run.recording",
            ],
            &["--host", "0.0.0.0:7700", "--link", "100"],
            &["--host", "0.0.0.0:7700", "--link", "100,20"],
            &["--host", "0.0.0.0:7700", "--link", "100,20,5,1"],
            &["--host", "0.0.0.0:7700", "--link", "100,20,100"],
            &["--host", "0.0.0.0:7700", "--link", "100,20,-1"],
            &["--host", "0.0.0.0:7700", "--link", "100,-20,5"],
            &["--host", "0.0.0.0:7700", "--serve", "0.0.0.0:7701"],
            &["--serve", "0.0.0.0:7700", "--connect", "127.0.0.1:7700"],
            &["--serve"],
            &["--connect"],
            &["--serve", "0.0.0.0:7700", "--rollback"],
            &["--connect", "127.0.0.1:7700", "--rollback"],
            &["--serve", "0.0.0.0:7700", "--record", "run.recording"],
            &["--connect", "127.0.0.1:7700", "--record", "run.recording"],
            &["--serve", "0.0.0.0:7700", "--embedded"],
            &["--serve", "0.0.0.0:7700", "--capture", "3"],
            &["--serve", "0.0.0.0:7700", "--gpu", "amd"],
            &["--serve", "0.0.0.0:7700", "--vsync"],
            &["--serve", "0.0.0.0:7700", "--max-fps", "60"],
            &["--serve", "0.0.0.0:7700", "--headless", "--size", "320x180"],
            &["--stopped", "--serve", "0.0.0.0:7700"],
            &["--stopped", "--connect", "127.0.0.1:7700"],
        ] {
            let error = parse_strs(args).unwrap_err();
            assert_eq!(error.code(), "engine.args", "{args:?}: {error}");
        }
    }
}
