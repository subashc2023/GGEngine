//! The showcase: a scene for each of what the engine draws and simulates so far, in its scenes
//! folder, which the editor's Scenes panel opens, each with a camera of its own: `welcome.ron`,
//! where it starts; sprites, tinted, textured, cut from a sheet, tiled, sampled crisp or smooth,
//! flipped and blended (`sprites.ron`); sprites played through a sheet's cells, a coin spinning and
//! numbered tiles, looping, back and forth and once, at their own rates and flipped
//! (`flipbooks.ron`, its sheet `textures/flipbook.png`, written by a script outside the
//! repository); fields eased by tweens, a dot along a track for each curve, a size, a turn, a color
//! and an alpha, and a crate a script pops (`tweens.ron`); circles, discs and rings, crisp and
//! faded (`shapes.ron`);
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
//! gg fetch` has fetched them, Sponza under that sky and a sun (`sponza.ron`), and walked through
//! by a character that the keys or a pad move, a camera following it, Sponza's triangles what it
//! collides with, among crates and balls it pushes (`walk.ron`), and Amazon
//! Lumberyard Bistro, its materials of specular and glossiness, its images DDS files of BC7, lit by
//! its own sun and 96 point lights under that sky (`bistro.ron`), and 16 of it, 4 by 4, each with
//! its own lights, its 16 suns' light exposed 4 stops down, from the same street of one of the
//! four in the middle (`bistro_grid.ron`). Its
//! Luau scripts, `scripts/spin.luau` and `scripts/bob.luau`,
//! turn and bob what they're on at each tick, `scripts/walk.luau` walks a character the way the
//! keys or a pad say, W and S or the arrows or the left stick forward and back, turning by A and D,
//! jumping by Space or South, `scripts/follow.luau` follows it with a camera, casting back
//! from it so that walls don't come between them, and `scripts/pulse.luau` pops what it's on every
//! so often, a tween easing it back; and animation and physics step, so that each scene moves as
//! it plays, in the editor from its Play, and in the reference player, which runs the game's
//! package without its code, and so without the keys' walking.

// A build without `dev`, as it ships, shows no console on Windows; its JSON log still records the
// run, and output piped to another process still arrives.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;

use gg_engine::math::{Vec2, Vec3};
use gg_engine::reflect::Value;
use gg_engine::{
    Actions, App, Binding, Change, Commands, Context, Event, Key, Layer, PadButton, Reflect,
    Scripts, Source, Stick, Tick, World, step_animation, step_physics,
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
    let app = app.layer(Showcase::new()).overlay(gg_engine::PerfPanel);
    app.overlay(gg_engine::Inspector::default())
}

/// The game's scripts, by their paths from its directory: each declares the component it is.
const SCRIPTS: [&str; 5] = [
    "scripts/bob.luau",
    "scripts/follow.luau",
    "scripts/pulse.luau",
    "scripts/spin.luau",
    "scripts/walk.luau",
];

/// What the keys and a pad do: walk the walkers, `walk.luau`'s, and have them jump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// W, A, S and D, the arrows and the left stick.
    Walk,
    /// Space and South.
    Jump,
}

/// What the keys and a pad do, queued for the next tick, which a recording keeps.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Has every walker go this way, until the next: across to turn, and up to go forward.
    Walk { heading: Vec2 },
    /// Has every walker jump, if it stands.
    Jump,
}

/// The game's layer: it loads the scripts as it loads, which declares their components, so that
/// its scenes, the Inspector's Add Component and the tool API know them before any entity has one;
/// loads the start scene as it attaches; and at each tick applies what the keys did to its
/// scenes' walkers, then runs the scripts, steps animation and then physics. A script or the start scene that
/// doesn't load ends the run with its error, through [`Context::fail`].
struct Showcase {
    scripts: Scripts,
    actions: Actions<Act>,
    commands: Vec<Command>,
}

impl Showcase {
    fn new() -> Self {
        let mut actions = Actions::default();
        let arrows = Binding::Axis2 {
            x: [Key::ArrowLeft, Key::ArrowRight].map(Source::Key),
            y: [Key::ArrowDown, Key::ArrowUp].map(Source::Key),
        };
        let bound = [
            (Act::Walk, Binding::WASD),
            (Act::Walk, arrows),
            (Act::Walk, Binding::stick(Stick::Left)),
            (Act::Jump, Binding::Button(Source::Key(Key::Space))),
            (
                Act::Jump,
                Binding::Button(Source::PadButton(PadButton::South)),
            ),
        ]
        .into_iter()
        .try_for_each(|(action, binding)| actions.bind(action, binding));
        bound.expect("the game's bindings are each of one kind, of other sources");
        Self {
            scripts: Scripts::new().expect("Luau starts"),
            actions,
            commands: Vec::new(),
        }
    }
}

/// Applies `command` to each of `world`'s walkers, the entities with `walk.luau`'s `Walk`: its
/// heading, with forward along -Z, or its jump.
fn walk(world: &mut World, command: &Command) {
    let walkers: Vec<_> = match world.entities(Some("Walk")) {
        Ok(walkers) => walkers.collect(),
        Err(_) => return,
    };
    let (field, value) = match *command {
        Command::Walk { heading } => (
            "heading",
            Value::Vec3(Vec3::new(heading.x, 0.0, -heading.y)),
        ),
        Command::Jump => ("jump", Value::Bool(true)),
    };
    for walker in walkers {
        // A walker whose script holds no such field, as one edited to drop it, goes without.
        let _ = world.set_field(walker, "Walk", field, &value);
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

    /// Lets go of what may have been held as the world was taken.
    fn restored(&mut self, _: &mut Context) {
        let heading = Vec2::ZERO;
        self.commands.push(Command::Walk { heading });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.commands.drain(..) {
            walk(world, &command);
        }
        self.scripts.run(world);
        step_animation(world);
        step_physics(world);
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    fn event(&mut self, _: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, rather than at the next update, so that the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| match change {
            Change::Axis2(Act::Walk, heading) => commands.push(Command::Walk { heading }),
            Change::Pressed(Act::Jump) => commands.push(Command::Jump),
            _ => {}
        });
        ControlFlow::Continue(())
    }
}
