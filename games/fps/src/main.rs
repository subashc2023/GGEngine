//! fps, a game made with GGEngine.
//!
//! A first-person shooter's skeleton. Its start scene, `scenes/main.ron`, holds a walled arena, a few
//! crates, targets, a sun and a sky, a camera and the player, a capsule that a character controller
//! moves. WASD walks, Space jumps, the mouse looks about while the cursor is grabbed, a click
//! grabs it and Escape lets it go, and a click while it's grabbed fires a ray from the eye that
//! removes the target it hits.

// A build without `dev`, as it ships, opens no console window on Windows: its JSON log still
// records the run, and what it prints still reaches a process it's piped to, as a tool's or CI's.
#![cfg_attr(not(feature = "dev"), windows_subsystem = "windows")]

use std::ops::ControlFlow;
use std::process::ExitCode;

use gg_engine::math::DVec3;
use gg_engine::prelude::*;
use gg_engine::{Camera, Cast, CharacterController3D};

fn main() -> ExitCode {
    gg_engine::run(app())
}

/// The game, with the engine's debug overlays: what `main` runs, and what its recordings replay.
fn app() -> App {
    let app = App::new("fps")
        .project(env!("CARGO_MANIFEST_DIR"))
        .source(env!("GG_SOURCE"));
    app.layer(Game::new()).debug_overlays()
}

/// The furthest the player looks up or down: 89°, short of straight, where yaw would spin.
const MAX_PITCH: f32 = 1.553_343;

/// How far a shot reaches, in units.
const RANGE: f32 = 200.0;

/// The player: walks where it looks, as its character controller moves it, and fires.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Player {
    /// Walking speed, in units a second.
    #[reflect(min = 0.0, default = 5.0)]
    speed: f32,
    /// How fast a jump leaves the ground, in units a second.
    #[reflect(min = 0.0, default = 5.0)]
    jump_speed: f32,
    /// How far a unit of the mouse's motion turns the view, in radians.
    #[reflect(min = 0.0, default = 0.0025)]
    sensitivity: f32,
    /// The eye's height above the capsule's center.
    #[reflect(default = 0.7)]
    eye: f32,
    /// As the last move set it, of length at most 1: across, then forward.
    heading: Vec2,
    /// The turn to its left about up, in radians, from facing -Z.
    yaw: f32,
    /// The turn up, in radians, no further than [`MAX_PITCH`] either way.
    #[reflect(min = -1.553_343, max = 1.553_343)]
    pitch: f32,
    /// How many shots it has fired.
    shots: u32,
    /// How many of them removed a target.
    hits: u32,
}

impl Player {
    /// Which way it faces, as a turn from facing -Z.
    fn look(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }
}

/// A thing a shot removes.
#[derive(Clone, Copy, Debug, PartialEq, Component)]
struct Target {
    /// What a hit on it is worth.
    #[reflect(default = 1)]
    points: u32,
}

/// What the keys and the mouse do, applied at the next tick. Recordings keep them.
#[derive(Clone, Debug, PartialEq, Reflect)]
enum Command {
    /// Walks the player along `heading`, across then forward, until the next.
    Move { heading: Vec2 },
    /// Turns the player's view by the mouse's motion, right and down.
    Look { by: Vec2 },
    /// Jumps, if the player stands.
    Jump,
    /// Fires a ray from the eye along the view.
    Fire,
}

/// What the keys and pads are bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Move,
    Jump,
}

/// The game's layer.
struct Game {
    actions: Actions<Act>,
    commands: Vec<Command>,
    /// Whether the mouse looks: from a click until Escape.
    grab: bool,
}

impl Game {
    fn new() -> Self {
        let mut actions = Actions::default();
        let bound = actions.bind_all([
            (Act::Move, Binding::WASD),
            (Act::Move, Binding::ARROWS),
            (Act::Move, Binding::stick(Stick::Left)),
            (Act::Jump, Binding::Button(Source::Key(Key::Space))),
            (
                Act::Jump,
                Binding::Button(Source::PadButton(PadButton::South)),
            ),
        ]);
        bound.expect("the bindings are each of one kind, of other sources");
        // What `gg actions` says each control does.
        let walk = "Walks the player on the level: [x, y], right and forward of where it faces by \
                    `Player.yaw` alone; 1 is `Player.speed` units a second";
        actions.describe(Act::Move, walk);
        let jump = "Jumps if the player stands, leaving the ground at `Player.jump_speed` units a \
                    second; a press in the air does nothing. Gravity is 9.81 units a second \
                    squared, times `CharacterController3D.gravity`";
        actions.describe(Act::Jump, jump);
        let look = "Turns the view while the cursor is grabbed, `Player.sensitivity` radians a \
                    unit: right turns right, lowering `Player.yaw`, and down looks down, lowering \
                    `Player.pitch`. The pitch stops at 89° either way";
        actions.describe_control(Control::Motion, look);
        let fire = format!(
            "A press grabs the cursor; once it's grabbed, each fires along the view from the eye, \
             removing the `Target` it hits first, up to {RANGE} units. Each shot counts in \
             `Player.shots`, and each hit in `Player.hits`"
        );
        let click = Control::Source(Source::Button(MouseButton::Left));
        actions.describe_control(click, fire);
        let escape = Control::Source(Source::Key(Key::Escape));
        actions.describe_control(escape, "Lets go of the cursor, so the mouse stops looking");
        Self {
            actions,
            commands: Vec::new(),
            grab: false,
        }
    }
}

/// Applies `command` to `world`.
fn apply(world: &mut World, command: &Command) {
    match *command {
        Command::Move { heading } => {
            for player in world.query_mut::<&mut Player>() {
                player.heading = heading;
            }
        }
        Command::Look { by } => {
            for player in world.query_mut::<&mut Player>() {
                // The mouse's right turns right, which is less yaw, and down looks down.
                player.yaw -= by.x * player.sensitivity;
                player.pitch =
                    (player.pitch - by.y * player.sensitivity).clamp(-MAX_PITCH, MAX_PITCH);
            }
        }
        Command::Jump => {
            for (player, controller) in world.query_mut::<(&Player, &mut CharacterController3D)>() {
                if controller.grounded {
                    controller.velocity.y = player.jump_speed;
                }
            }
        }
        Command::Fire => fire(world),
    }
}

/// Casts a ray from the player's eye along its view, and removes the target it hits first.
fn fire(world: &mut World) {
    let shot = world
        .query::<(Entity, &Player, &Transform)>()
        .next()
        .map(|(entity, player, at)| {
            let eye = at.translation + DVec3::Y * f64::from(player.eye);
            let cast = Cast {
                from: eye,
                toward: player.look() * Vec3::NEG_Z,
                max_distance: RANGE,
                exclude: Some(entity),
            };
            (entity, cast)
        });
    let Some((shooter, cast)) = shot else {
        return;
    };
    let hit = world.cast_ray(cast).map(|hit| hit.entity);
    let points = hit
        .and_then(|hit| world.get::<Target>(hit))
        .map(|t| t.points);
    if let (Some(hit), Some(points)) = (hit, points) {
        // A hit the world has since lost, as one removed this tick already, counts for nothing.
        if world.despawn(hit).is_err() {
            return;
        }
        info!(points, "Target hit");
    }
    if let Some(player) = world.get_mut::<Player>(shooter) {
        player.shots = player.shots.saturating_add(1);
        player.hits = player.hits.saturating_add(u32::from(points.is_some()));
    }
}

impl Layer for Game {
    /// Registers the components the start scene names.
    fn load(&mut self, cx: &mut Context) -> Result<(), Error> {
        cx.world_mut().register_all::<(Player, Target)>()
    }

    fn attach(&mut self, cx: &mut Context) {
        // Watched as the game runs, in development builds, which apply its saves. One that doesn't
        // load ends the run with its error.
        if let Err(error) = cx.load_start_scene() {
            cx.fail(error);
        }
    }

    /// Lets go of what was held as the world was taken, as when `gg run` restarts the game.
    fn restored(&mut self, _: &mut Context) {
        let heading = Vec2::ZERO;
        self.commands.push(Command::Move { heading });
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for command in self.commands.drain(..) {
            apply(world, &command);
        }
        // Walks where it looks, keeping the fall or jump the controller has.
        for (player, controller) in world.query_mut::<(&Player, &mut CharacterController3D)>() {
            let turn = Quat::from_rotation_y(player.yaw);
            let across = turn * Vec3::new(player.heading.x, 0.0, -player.heading.y) * player.speed;
            controller.velocity = Vec3::new(across.x, controller.velocity.y, across.z);
        }
        step_physics(world);
        // The camera sits at the eye, as the step left the player.
        let eye = world
            .query::<(&Player, &Transform)>()
            .next()
            .map(|(player, at)| {
                let height = DVec3::Y * f64::from(player.eye);
                (at.translation + height, player.look())
            });
        if let Some((translation, rotation)) = eye {
            for (_, at) in world.query_mut::<(&Camera, &mut Transform)>() {
                at.translation = translation;
                at.rotation = rotation;
            }
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.commands)
    }

    /// What `gg actions` lists, and `gg input` presses by name.
    fn actions(&self) -> Controls {
        self.actions.controls()
    }

    /// Grabs the cursor while the mouse looks, and draws the crosshair.
    fn update(&mut self, cx: &mut Context, _: std::time::Duration) {
        cx.set_cursor_grabbed(self.grab);
        let mut canvas = cx.canvas([Align::Center; 2]);
        let sprite = Sprite {
            color: Vec4::new(1.0, 1.0, 1.0, 0.85),
            ..Sprite::default()
        };
        for size in [Vec3::new(26.0, 3.0, 1.0), Vec3::new(3.0, 26.0, 1.0)] {
            canvas.draw_sprite(Mat4::from_scale(size), sprite);
        }
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        // Queued now, so the next tick applies them.
        let commands = &mut self.commands;
        self.actions.event(event, |change| match change {
            Change::Axis2(Act::Move, heading) => commands.push(Command::Move { heading }),
            Change::Pressed(Act::Jump) => commands.push(Command::Jump),
            _ => {}
        });
        match *event {
            Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            } if !cx.pointer_over_ui() => {
                // The click that grabs the cursor doesn't fire.
                if self.grab {
                    commands.push(Command::Fire);
                }
                self.grab = true;
            }
            Event::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => self.grab = false,
            // Each tick turns the view once, by the motion since the tick before.
            Event::MouseMotion { x, y } if self.grab => {
                let by = Vec2::new(x as f32, y as f32);
                match commands.last_mut() {
                    Some(Command::Look { by: queued }) => *queued += by,
                    _ => commands.push(Command::Look { by }),
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use gg_engine::{BodyType, BoxCollider3D, CapsuleCollider3D, RigidBody3D, SphereCollider3D};

    use super::*;

    fn fixed() -> RigidBody3D {
        RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        }
    }

    /// A floor, a player 10 units from a target straight ahead, and a camera.
    fn arena() -> (World, Entity, Entity) {
        let mut world = World::default();
        world
            .register_all::<(Player, Target)>()
            .expect("they register");
        let floor = Transform {
            translation: DVec3::new(0.0, -0.5, 0.0),
            scale: Vec3::new(40.0, 1.0, 40.0),
            ..Transform::IDENTITY
        };
        world.spawn((floor, fixed(), BoxCollider3D::default()));
        let at = |z: f64, y: f64| Transform {
            translation: DVec3::new(0.0, y, z),
            ..Transform::IDENTITY
        };
        let player = Player {
            speed: 5.0,
            jump_speed: 5.0,
            sensitivity: 0.0025,
            eye: 0.7,
            heading: Vec2::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            shots: 0,
            hits: 0,
        };
        let player = world.spawn((
            at(5.0, 1.0),
            player,
            CharacterController3D::default(),
            CapsuleCollider3D {
                height: 1.8,
                radius: 0.35,
                ..CapsuleCollider3D::default()
            },
        ));
        let target = world.spawn((
            at(-5.0, 1.7),
            Target { points: 1 },
            fixed(),
            SphereCollider3D::default(),
        ));
        let camera = Camera {
            projection: Projection::Perspective {
                fov: 1.2,
                near: 0.1,
            },
            primary: true,
        };
        world.spawn((Transform::IDENTITY, camera));
        (world, player, target)
    }

    fn run(game: &mut Game, world: &mut World, ticks: u64) {
        for number in 0..ticks {
            game.tick(world, Tick { number });
        }
    }

    /// The start scene saves to its own bytes, so a `gg scene save` changes only what was edited.
    #[test]
    fn the_scene_saves_as_it_is() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut cx = Context::seeded(1).root(dir);
        Game::new().load(&mut cx).expect("loaded");
        let path = dir.join("scenes/main.ron");
        let text = std::fs::read_to_string(&path).expect("the scene");
        let roots = cx.load_scene_file(path).expect("the scene loads");
        let saved = cx.world().save_scene(&roots).expect("saved");
        assert_eq!(saved, text);
    }

    #[test]
    fn walking_looking_jumping_and_the_camera_follow_the_player() {
        let (mut world, player, _) = arena();
        let mut game = Game::new();
        run(&mut game, &mut world, 30);
        let start = world.get::<Transform>(player).expect("placed").translation;
        let walk = Command::Move {
            heading: Vec2::new(0.0, 1.0),
        };
        game.commands.push(walk);
        run(&mut game, &mut world, 30);
        let walked = world.get::<Transform>(player).expect("placed").translation;
        assert!(
            walked.z < start.z - 2.0 && walked.x.abs() < 0.01,
            "{walked}"
        );
        // A quarter turn left, then forward goes along -X.
        let by = Vec2::new(-std::f32::consts::FRAC_PI_2 / 0.0025, 0.0);
        game.commands.push(Command::Look { by });
        run(&mut game, &mut world, 30);
        let turned = world.get::<Transform>(player).expect("placed").translation;
        assert!(turned.x < walked.x - 2.0, "{turned}");
        game.commands.push(Command::Move {
            heading: Vec2::ZERO,
        });
        game.commands.push(Command::Jump);
        run(&mut game, &mut world, 10);
        let up = world.get::<Transform>(player).expect("placed").translation;
        assert!(up.y > turned.y + 0.5, "{up}");
        let (at, _) = world
            .query::<(&Transform, &Camera)>()
            .next()
            .expect("a camera");
        assert!((at.translation - up - DVec3::Y * 0.7).length() < 1e-6);
    }

    #[test]
    fn a_shot_removes_the_target_it_hits_and_only_that() {
        let (mut world, player, target) = arena();
        let mut game = Game::new();
        run(&mut game, &mut world, 10);
        // Aimed away from it, nothing is hit.
        let by = Vec2::new(400.0, 0.0);
        game.commands.push(Command::Look { by });
        game.commands.push(Command::Fire);
        run(&mut game, &mut world, 1);
        assert!(world.contains(target));
        let by = Vec2::new(-400.0, 0.0);
        game.commands.push(Command::Look { by });
        game.commands.push(Command::Fire);
        run(&mut game, &mut world, 1);
        assert!(!world.contains(target));
        let shooter = world.get::<Player>(player).expect("a player");
        assert_eq!((shooter.shots, shooter.hits), (2, 1));
    }
}
