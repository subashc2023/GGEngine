//! The editor of the project it's started in, as [`game`] describes, or else of the one its [`hub`]
//! picks. It takes the flags every app shares, and [`RELEASE_GAME`].

mod assets;
mod camera;
mod debugger;
mod file;
mod game;
mod handles;
mod hub;
mod look;
mod overlay;
mod scenes;
mod shortcuts;
mod view;

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitCode};

use gg_engine::App;

/// The editor's own flag, as [`USAGE`] says. It changes only the game's build.
const RELEASE_GAME: &str = "--release-game";

/// What `--help` adds, after the flags every app shares.
const USAGE: &str = "
Editor options:
  --release-game    Build and run the project's game optimized, under `gg run --release`:
                    as fast as a player's, without Vulkan validation, each edit to its code
                    restarting it rather than patching it
";

fn main() -> ExitCode {
    let (release_game, args) = own_flags(std::env::args_os().skip(1));
    let help = args.iter().any(|arg| arg == "-h" || arg == "--help");
    let status = gg_engine::run_with(app(release_game), args);
    if help && status == ExitCode::SUCCESS {
        print!("{USAGE}");
    }
    status
}

/// Whether `args`, after the program's name, give [`RELEASE_GAME`], and the rest, in order, for the
/// engine.
fn own_flags(args: impl IntoIterator<Item = OsString>) -> (bool, Vec<OsString>) {
    let (own, rest): (Vec<_>, Vec<_>) = args.into_iter().partition(|arg| arg == RELEASE_GAME);
    (!own.is_empty(), rest)
}

/// The editor of the project it's started in, if any. Its hub finds projects in the engine's
/// workspace: where it runs, or where its executable is.
fn app(release_game: bool) -> App {
    let dir = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    let workspace = hub::workspace(dir.as_deref(), exe.as_deref());
    let project = dir.and_then(|dir| gg_api::Project::find(&dir));
    game::app(project, workspace, release_game)
}

/// The engine's checkout, two folders above the editor.
fn engine() -> &'static Path {
    let editor = Path::new(env!("CARGO_MANIFEST_DIR"));
    (editor.ancestors().nth(2)).expect("the editor is at apps/editor in the engine")
}

/// Cargo running the [`engine`]'s gg by its manifest, in `dir`, whose cargo config and toolchain
/// apply; gg's arguments follow. It is `cargo gg`, which no alias gives in a game's own workspace.
/// Cargo is `CARGO`'s, which cargo sets for what it runs, else the `PATH`'s.
fn gg(dir: &Path) -> Command {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.args(["run", "--quiet", "--manifest-path"]);
    command.arg(engine().join("tools").join("gg").join("Cargo.toml"));
    command.arg("--").current_dir(dir);
    command
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use gg_engine::Viewport;
    use gg_engine::math::DVec2;

    use crate::camera::Screen;
    use crate::handles::Reading;

    /// `--release-game` comes off the arguments wherever it is; the rest go to the engine as given.
    #[test]
    fn the_editor_takes_its_own_flag_and_leaves_the_engine_the_rest() {
        let args = |args: &[&str]| args.iter().map(OsString::from).collect::<Vec<_>>();
        let given = args(&["--max-fps", "60", "--release-game", "--gpu=nvidia"]);
        assert_eq!(
            super::own_flags(given),
            (true, args(&["--max-fps", "60", "--gpu=nvidia"]))
        );
        let given = args(&["--max-fps", "60"]);
        assert_eq!(super::own_flags(given.clone()), (false, given));
    }

    /// gg runs in the directory given, from the engine's gg manifest, wherever the directory is.
    #[test]
    fn gg_runs_from_the_engines_manifest_in_the_directory_given() {
        let dir = std::env::temp_dir();
        let gg = super::gg(&dir);
        let args: Vec<_> = gg.get_args().collect();
        assert_eq!(args[..3], ["run", "--quiet", "--manifest-path"]);
        assert_eq!(args[4..], ["--"]);
        let manifest = std::fs::read_to_string(args[3]).expect("gg's manifest reads");
        assert_eq!(manifest.lines().nth(1), Some("name = \"gg\""), "{manifest}");
        assert_eq!(gg.get_current_dir(), Some(dir.as_path()));
    }

    /// A view offset from the window's top left, at [`SCALE`] pixels a point. The editor's starting
    /// camera shows 10 units tall, the origin at its middle (500, 350), a unit 60 pixels across.
    pub(crate) const VIEW: Viewport = Viewport {
        origin: [100, 50],
        size: [800, 600],
    };
    pub(crate) const SCALE: f32 = 1.5;

    /// [`VIEW`] as the editor reads the window, the cursor at pixel `at`, no modifier held.
    pub(crate) fn reading(at: impl Into<DVec2>) -> Reading {
        Reading {
            screen: Screen {
                view: Some(VIEW),
                scale: SCALE,
                alt: false,
            },
            command: false,
            shift: false,
            cursor: Some(at.into().to_array()),
        }
    }

    /// A probe's answer at pixel `at` of an 800 by 600 view: a material's light as shown, of entity
    /// Crate, with depth but no distance, as an orthographic view's.
    pub(crate) fn probe(at: [f64; 2]) -> gg_api::probe::Probe {
        gg_api::probe::Probe {
            at,
            size: [800, 600],
            ticks: 9,
            base_color: Some([0.5, 0.25, 1.0]),
            normal: Some([0.0, 0.0, 1.0]),
            surface_normal: Some([0.0, 0.0, 1.0]),
            metallic: Some(0.25),
            roughness: Some(0.75),
            occlusion: Some(1.0),
            emission: Some([0.0; 3]),
            specular_color: Some([0.155, 0.0925, 0.28]),
            direct_light: Some([12, 30, 40]),
            sky_light: Some([1, 2, 3]),
            reflection: Some([0, 0, 0]),
            diffuse_light: Some([10, 25, 35]),
            specular_light: Some([4, 6, 8]),
            shown: Some([20, 40, 60]),
            depth: Some(1.0 / 4096.0),
            distance: None,
            entity: gg_api::Picked {
                id: Some("2b7e151628aed2a6".to_owned()),
                name: Some("Crate".to_owned()),
            },
            quantized: String::new(),
        }
    }
}
