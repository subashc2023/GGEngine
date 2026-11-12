//! The editor of the game's project it's started in, as [`game`] describes; started elsewhere, of
//! the project its [`hub`] picks. It takes the flags every app shares, and one of its own,
//! [`RELEASE_GAME`].

mod assets;
mod camera;
mod debugger;
mod file;
mod game;
mod handles;
mod hub;
mod look;
mod overlay;
mod rates;
mod scenes;
mod view;

use std::ffi::OsString;
use std::process::ExitCode;

use gg_engine::App;

/// The editor's own flag: its game built optimized, as `cargo gg run --release` builds it, so that
/// it runs as fast as a player's, without Vulkan's validation and without patches, each edit to its
/// code restarting it. The editor itself is built as it was.
const RELEASE_GAME: &str = "--release-game";

/// What `--help` adds, after the flags every app shares.
const USAGE: &str = "
Editor options:
  --release-game    Build and run the project's game optimized, under `cargo gg run --release`:
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

/// Whether `args`, those after the program's name, give [`RELEASE_GAME`], and the rest, in order,
/// for the engine.
fn own_flags(args: impl IntoIterator<Item = OsString>) -> (bool, Vec<OsString>) {
    let (own, rest): (Vec<_>, Vec<_>) = args.into_iter().partition(|arg| arg == RELEASE_GAME);
    (!own.is_empty(), rest)
}

/// The editor of the project it's started in, if it's started in one, its hub finding projects in
/// the engine's workspace, where it runs, or where its executable is; its game built optimized if
/// `release_game`.
fn app(release_game: bool) -> App {
    let dir = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    let workspace = hub::workspace(dir.as_deref(), exe.as_deref());
    let project = dir.and_then(|dir| gg_api::Project::find(&dir));
    game::app(project, workspace, release_game)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use gg_engine::Viewport;
    use gg_engine::math::DVec2;

    use crate::camera::Screen;
    use crate::handles::Reading;

    /// `--release-game` comes off the arguments wherever it is, the rest going to the engine as
    /// they were given; without it, all of them do.
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

    /// A view whose top left isn't the window's, at [`SCALE`] pixels a point: through the editor's
    /// camera as it starts, 10 units tall, the origin at its middle, (500, 350), and a unit 60
    /// pixels across.
    pub(crate) const VIEW: Viewport = Viewport {
        origin: [100, 50],
        size: [800, 600],
    };
    pub(crate) const SCALE: f32 = 1.5;

    /// What the editor reads of the window, [`VIEW`] at [`SCALE`] pixels a point, with the cursor
    /// at `at`, in its pixels, and neither Alt nor the command key held.
    pub(crate) fn reading(at: impl Into<DVec2>) -> Reading {
        Reading {
            screen: Screen {
                view: Some(VIEW),
                scale: SCALE,
                alt: false,
            },
            command: false,
            cursor: Some(at.into().to_array()),
        }
    }

    /// A probe's answer at pixel `at` of a view 800 by 600: a material's, its light as the view
    /// shows it, of entity Crate, with depth but, as an orthographic view's, no distance.
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
