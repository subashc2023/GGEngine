//! The command-line flags every app shares.

use std::ffi::OsString;

use gg_core::{Error, Result};

#[derive(Debug, PartialEq)]
pub(crate) struct Args {
    pub(crate) headless: bool,
    pub(crate) frames: Option<u64>,
    pub(crate) gpu: Option<String>,
    pub(crate) validation: bool,
    pub(crate) vsync: bool,
}

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
  -h, --help        Print this help

Environment:
  RUST_LOG          The log filter, such as `debug` or `info,gg_gpu=trace`
  NO_COLOR          Turns the console's colors off

Exits 0 on success, 1 on an error or a validation message, and 2 on bad arguments.
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
    };
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
            "--gpu" => parsed.gpu = Some(value_of(flag)?),
            _ => return Err(bad(format!("unknown argument `{arg}`"))),
        }
        if value.is_some() {
            return Err(bad(format!("`{flag}` takes no value")));
        }
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
            &["--headless=yes"],
        ] {
            let error = parse_strs(args).unwrap_err();
            assert_eq!(error.code(), "engine.args", "{args:?}: {error}");
        }
    }
}
