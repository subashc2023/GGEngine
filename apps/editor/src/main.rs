//! The editor of the game's project it's started in, as [`game`] describes; started elsewhere, of
//! the project its [`hub`] picks.

mod assets;
mod camera;
mod file;
mod game;
mod handles;
mod hub;
mod look;
mod overlay;
mod scenes;
mod view;

use std::process::ExitCode;

use gg_engine::App;

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The editor of the project it's started in, if it's started in one, its hub finding projects in
/// the engine's workspace, where it runs, or where its executable is.
fn app() -> App {
    let dir = std::env::current_dir().ok();
    let exe = std::env::current_exe().ok();
    let workspace = hub::workspace(dir.as_deref(), exe.as_deref());
    game::app(dir.and_then(|dir| gg_api::Project::find(&dir)), workspace)
}

#[cfg(test)]
mod tests {
    use gg_engine::Viewport;
    use gg_engine::math::DVec2;

    use crate::camera::Screen;
    use crate::handles::Reading;

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
}
