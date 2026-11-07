//! The showcase: a scene for each of what the engine draws and simulates so far, in its scenes
//! folder, which the editor's Scenes panel opens, each with a camera of its own: `welcome.ron`,
//! where it starts; sprites, tinted, textured, cut from a sheet, tiled, sampled crisp or smooth,
//! flipped and blended (`sprites.ron`); circles, discs and rings, crisp and faded (`shapes.ron`);
//! text in the engine's font and the project's, aligned, sized, colored and turned (`text.ron`); a
//! hierarchy of transforms, each child carried by its parent (`hierarchy.ron`); 2D physics, bodies
//! dynamic, kinematic and static, boxes and circles, bouncing and sliding (`physics.ron`); glTF
//! models, unlit, textured and of more than one node, through a camera with perspective
//! (`models.ron`); colors from a quarter as bright as white to 16 times, which the world's
//! environment tonemaps (`hdr.ron`); glTF's metallic-roughness materials, plastic, metal,
//! glowing and tiled with every map, lit by a sun, a point light and a spot (`lights.ron`); the
//! same lit by a sky alone, Poly Haven's Kloofendal 48d Partly Cloudy Puresky at 1K, CC0, which
//! shows behind them (`sky.ron`); models casting a sun's shadows (`shadows.ron`); 3D physics,
//! boxes, balls and capsules, dynamic, kinematic and static, tumbling down a ramp, bouncing and
//! swept about within walls (`physics3d.ron`); and, once `cargo
//! gg fetch` has fetched them, Sponza under that sky and a sun (`sponza.ron`), and Amazon
//! Lumberyard Bistro, its materials of specular and glossiness, its images DDS files of BC7, lit by
//! its own sun and 96 point lights under that sky (`bistro.ron`), and 16 of it, 4 by 4, each with
//! its own lights, its 16 suns' light exposed 4 stops down, from the same street of one of the
//! four in the middle (`bistro_grid.ron`). Its
//! Luau scripts, `scripts/spin.luau` and `scripts/bob.luau`,
//! turn and bob what they're on at each tick, and physics steps, so that each scene moves as it
//! plays, in the editor from its Play, and in the reference player, which runs the game's package
//! without its code.

// A build without `dev`, as it ships, shows no console on Windows; its JSON log still records the
// run, and output piped to another process still arrives.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::path::Path;
use std::process::ExitCode;

use gg_engine::{App, Context, Layer, Scripts, Tick, World, step_physics};

#[cfg(test)]
mod tests;

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's Performance and Inspector panels: what `main` runs, and what its
/// recordings replay.
fn app() -> App {
    let app = App::new("Showcase").project(env!("CARGO_MANIFEST_DIR"));
    let app = app.layer(Showcase::new()).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// The game's scripts, by their paths from its directory: each declares the component it is.
const SCRIPTS: [&str; 2] = ["scripts/bob.luau", "scripts/spin.luau"];

/// The game's layer: it loads the scripts as it loads, which declares their components, so that
/// its scenes, the Inspector's Add Component and the tool API know them before any entity has one;
/// loads the start scene as it attaches; and at each tick runs the scripts and steps physics. A
/// script or the start scene that doesn't load ends the run with its error, through
/// [`Context::fail`].
struct Showcase {
    scripts: Scripts,
}

impl Showcase {
    fn new() -> Self {
        Self {
            scripts: Scripts::new().expect("Luau starts"),
        }
    }
}

impl Layer for Showcase {
    fn load(&mut self, cx: &mut Context) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Each is watched as the game runs, in development builds, which reload it as it's saved.
        for script in SCRIPTS {
            if let Err(error) = self.scripts.load(cx, root.join(script)) {
                cx.fail(error);
                return;
            }
        }
    }

    fn attach(&mut self, cx: &mut Context) {
        // A run that ends as it loads loads no scene, whose components the scripts declare.
        if cx.exiting() {
            return;
        }
        // Watched as the game runs, in development builds, which apply its saves.
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        self.scripts.run(world);
        step_physics(world);
    }
}
