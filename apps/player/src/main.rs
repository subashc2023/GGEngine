//! The reference player: runs a game's package, which `tools/cook` writes, without the game's code,
//! its start scene and its Luau scripts, which act at its ticks, and its physics, as the
//! showcase's package runs; and the reference for how a game builds a player of its own. It runs
//! from the package `--package` names, or else `<executable>.ggpk` beside its executable,
//! `gg-player.ggpk` as it's built.

// Its builds that ship, release and shipping, show no console on Windows; its JSON log still
// records the run, and output piped to another process still arrives.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use gg_engine::{App, Context, Layer, Scripts, Tick, World, step_physics};

fn main() -> ExitCode {
    gg_engine::run(App::new("Player").from_package().layer(Player::default()))
}

/// The player's layer: as it loads, a VM loads every Luau script the package holds, in the order of
/// their paths; as it attaches, the package's start scene loads; and at each tick the scripts run,
/// then physics steps. It has no panels, nor components of a game's own: a scene that names a
/// component no script declares doesn't load. Anything that fails ends the run, with its error,
/// through [`Context::fail`].
#[derive(Default)]
struct Player {
    /// The package's scripts, once they've loaded.
    scripts: Option<Scripts>,
}

impl Layer for Player {
    fn load(&mut self, cx: &mut Context) {
        let scripts = Scripts::new().and_then(|mut scripts| {
            let files = cx.package_files().into_iter();
            let luau = files.filter(|path| {
                (path.extension()).is_some_and(|extension| extension.eq_ignore_ascii_case("luau"))
            });
            for path in luau {
                scripts.load(cx, path)?;
            }
            Ok(scripts)
        });
        match scripts {
            Ok(scripts) => self.scripts = Some(scripts),
            Err(error) => cx.fail(error),
        }
    }

    fn attach(&mut self, cx: &mut Context) {
        // A run that ends as it loads, as a script that doesn't load ends it, loads no scene, whose
        // components the script may have declared.
        if cx.exiting() {
            return;
        }
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        if let Some(scripts) = &mut self.scripts {
            scripts.run(world);
        }
        step_physics(world);
    }
}
