//! `gg-cook`, the offline asset cooker: writes a game's package from its project, as the `cook`
//! library says, printing what it holds as JSON, a line at a time, as `gg` prints what it finds.

use std::io::Write as _;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use gg_engine::cook::Kind;
use serde_json::json;

const USAGE: &str = "\
Usage: gg-cook <PROJECT> <PACKAGE>

Cooks the game's project in the directory PROJECT, which holds its `project.ron`, into the package
at PACKAGE, which a build of the game without `dev` runs from as `<executable>.ggpk` beside its
executable, or as `--package <PATH>` names it: the files under its asset root that its runs load,
textures compressed to BC7, models' meshes optimized with levels of detail and their images
compressed, fonts baked into their fields, shaders compiled to SPIR-V, the rest as they are, and
each asset's ID. Prints a line of JSON for each file the package holds, its path, kind, ID and
size, then one for the package; and an error as a line of JSON on stderr.

Exits 0 once the package is written, 1 on an error, and 2 on bad arguments.";

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print(USAGE);
        return ExitCode::SUCCESS;
    }
    let [project, package] = <[_; 2]>::try_from(args).unwrap_or_else(|_| {
        let error = json!({"error": {"code": "cook.args", "message": "gg-cook takes a project's directory and a package's path; `gg-cook --help` says more"}});
        let _ = writeln!(std::io::stderr().lock(), "{error}");
        std::process::exit(2);
    });
    let (project, package) = (PathBuf::from(project), PathBuf::from(package));
    let start = Instant::now();
    // Fonts bake on the job system's threads, as a run's do.
    let threads = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    let cooked = gg_engine::start_jobs(threads).and_then(|()| cook::cook(&project, &package));
    let cooked = match cooked {
        Ok(cooked) => cooked,
        Err(error) => {
            let error = json!({"error": {"code": error.code(), "message": format!("{error:#}")}});
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            return ExitCode::FAILURE;
        }
    };
    let mut bytes = 0;
    for cooked in &cooked {
        bytes += cooked.bytes;
        let kind = match cooked.kind {
            Kind::File => "file",
            Kind::Font => "font",
            Kind::Shader => "shader",
            Kind::Texture => "texture",
            Kind::Model => "model",
        };
        let id = cooked.id.map(|id| format!("{id:016x}"));
        let line = json!({"cooked": cooked.path, "kind": kind, "id": id, "bytes": cooked.bytes});
        print(&line.to_string());
    }
    let ms = start.elapsed().as_millis() as u64;
    let line = json!({"package": package, "files": cooked.len(), "bytes": bytes, "ms": ms});
    print(&line.to_string());
    ExitCode::SUCCESS
}

/// Prints `line` on stdout, ignoring a stdout that's gone, as when piped into `head`.
fn print(line: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}
