//! The showcase: a scene for each of what the engine draws and simulates so far, in its scenes
//! folder, which the editor's Scenes panel opens, each with a camera of its own: `welcome.ron`,
//! where it starts; sprites, tinted, textured, cut from a sheet, tiled, sampled crisp or smooth,
//! flipped and blended (`sprites.ron`); circles, discs and rings, crisp and faded (`shapes.ron`);
//! text in the engine's font and the project's, aligned, sized, colored and turned (`text.ron`); a
//! hierarchy of transforms, each child carried by its parent (`hierarchy.ron`); 2D physics, bodies
//! dynamic, kinematic and static, boxes and circles, bouncing and sliding (`physics.ron`); and glTF
//! models, unlit, textured and of more than one node, through a camera with perspective
//! (`models.ron`). Its scripts, [`Spin`] and [`Bob`], move what they're on at each tick, and 2D
//! physics steps, so that each scene moves as it plays, in the editor from its Play.

use std::f32::consts::TAU;
use std::process::ExitCode;

use gg_engine::math::{Quat, Vec3, libm};
use gg_engine::{
    App, Component, Context, Entity, Layer, Script, Tick, Transform, World, run_scripts,
    step_physics,
};

#[cfg(test)]
mod tests;

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's Performance and Inspector panels: what `main` runs, and what its
/// recordings replay.
fn app() -> App {
    let app = App::new("Showcase").project(env!("CARGO_MANIFEST_DIR"));
    let app = app.layer(Showcase).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// A script that turns its entity about `axis`, in its parent's space, `speed` radians a second,
/// counterclockwise as the axis points at the viewer; about Z, out of the view, without an axis.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Spin {
    axis: Vec3,
    #[reflect(degrees)]
    speed: f32,
}

impl Script for Spin {
    fn tick(entity: Entity, world: &mut World, _: Tick) {
        let Some(&Spin { axis, speed }) = world.get::<Spin>(entity) else {
            return;
        };
        let axis = axis.try_normalize().unwrap_or(Vec3::Z);
        let turn = Quat::from_axis_angle(axis, speed * Tick::DT.as_secs_f32());
        if let Some(transform) = world.get_mut::<Transform>(entity) {
            transform.rotation = (turn * transform.rotation).normalize();
        }
    }
}

/// A script that moves its entity up and down, along its parent's Y, as far as `height` either
/// way of where it starts, once every `period` seconds of the simulation's time.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Bob {
    #[reflect(min = 0.0)]
    height: f32,
    #[reflect(min = 0.1, default = 2.0)]
    period: f32,
}

impl Script for Bob {
    fn tick(entity: Entity, world: &mut World, tick: Tick) {
        let Some(&Bob { height, period }) = world.get::<Bob>(entity) else {
            return;
        };
        let at = |time: f32| height * libm::sinf(TAU * time / period.max(0.1));
        let (time, dt) = (tick.time().as_secs_f32(), Tick::DT.as_secs_f32());
        if let Some(transform) = world.get_mut::<Transform>(entity) {
            transform.translation.y += at(time) - at(time - dt);
        }
    }
}

/// The game's layer: it loads the start scene as it attaches, and at each tick runs the scripts
/// and steps 2D physics.
struct Showcase;

impl Layer for Showcase {
    fn load(&mut self, cx: &mut Context) {
        register(cx.world_mut());
    }

    fn attach(&mut self, cx: &mut Context) {
        // Watched as the game runs, in development builds, which apply its saves.
        let loaded = cx.load_start_scene();
        loaded.unwrap_or_else(|error| panic!("{error:#}"));
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        run_scripts::<Spin>(world, tick);
        run_scripts::<Bob>(world, tick);
        step_physics(world);
    }
}

/// Registers the game's scripts with `world`, so that its scenes, the Inspector's Add Component
/// and the tool API know them before any entity has one.
fn register(world: &mut World) {
    let registered = world
        .register::<Spin>()
        .and_then(|()| world.register::<Bob>());
    registered.expect("the game's scripts describe themselves");
}
