//! The reference player: runs a game's package, which `tools/cook` writes, without the game's code.
//! The start scene, Luau scripts, animation, navigation and physics run, as the showcase's do. It's
//! also the reference for a game's own player.
//!
//! The package is the one `--package` names, else `<executable>.ggpk` beside the executable
//! (`gg-player.ggpk` as built).

// Release and shipping builds show no console on Windows. The JSON log still records the run, and
// output piped to another process still arrives.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use gg_engine::prelude::*;

fn main() -> ExitCode {
    gg_engine::run(App::new("Player").from_package().layer(Player::default()))
}

/// The player's layer. Scripts load in path order. It has no panels, nor components of a game's
/// own: a scene naming a component no script declares doesn't load. A failure ends the run.
#[derive(Default)]
struct Player {
    /// Once loaded.
    scripts: Option<Scripts>,
}

impl Layer for Player {
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        let mut scripts = Scripts::new()?;
        let files = cx.package_files().into_iter();
        let luau = files.filter(|path| {
            (path.extension()).is_some_and(|extension| extension.eq_ignore_ascii_case("luau"))
        });
        for path in luau {
            scripts.load(cx, path)?;
        }
        self.scripts = Some(scripts);
        Ok(())
    }

    fn attach(&mut self, cx: &mut Context) {
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        if let Some(scripts) = &mut self.scripts {
            scripts.run(world);
        }
        step_animation(world);
        step_navigation(world);
        step_physics(world);
    }
}
