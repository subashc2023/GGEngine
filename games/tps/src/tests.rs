//! The game's tests.

use std::fs;
use std::path::Path;

use gg_engine::CharacterController3D;
use gg_engine::math::{DVec3, libm};

use super::*;

/// The game's project's directory.
fn dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A context with the game's start scene loaded, its roots, and the layer, as the game's `load`
/// makes them.
fn loaded() -> (Context, Game, Vec<Entity>) {
    let mut cx = Context::seeded(1).root(dir());
    let mut game = Game::new();
    game.load(&mut cx).expect("loaded");
    let path = dir().join("scenes/main.ron");
    let roots = cx.load_scene_file(path).expect("the scene loads");
    (cx, game, roots)
}

/// Runs `ticks` ticks of the layer's.
fn run(cx: &mut Context, game: &mut Game, ticks: u64) {
    for number in 0..ticks {
        game.tick(cx.world_mut(), Tick { number });
    }
}

/// The player's place.
fn player(world: &World) -> (Entity, DVec3) {
    let (entity, at, _) = world
        .query::<(Entity, &Transform, &Player)>()
        .next()
        .expect("a player");
    (entity, at.translation)
}

/// The start scene loads, and saves to its own bytes.
#[test]
fn the_scene_loads_and_saves_as_it_is() {
    let text = fs::read_to_string(dir().join("scenes/main.ron")).expect("the scene");
    let (cx, _, roots) = loaded();
    let saved = cx.world().save_scene(&roots).expect("saved");
    assert_eq!(saved, text);
}

/// A dodge rolls the player along its walk at `roll_speed`, and another before `roll_cooldown` is
/// up does nothing; once it's up, one rolls again.
#[test]
fn a_dodge_rolls_and_waits_out_its_cooldown() {
    let (mut cx, mut game, _) = loaded();
    // Lands on the floor first, then walks right.
    run(&mut cx, &mut game, 64);
    let heading = Vec2::new(1.0, 0.0);
    apply(cx.world_mut(), &Command::Steer { heading });
    apply(cx.world_mut(), &Command::Dodge);
    run(&mut cx, &mut game, 1);
    let (entity, _) = player(cx.world());
    let rolled = |cx: &Context| *cx.world().get::<Player>(entity).expect("a player");
    let rolling = rolled(&cx);
    assert!(
        rolling.rolling > 0.0 && rolling.roll_way.x > 0.99,
        "{:?}",
        rolling.roll_way
    );
    let velocity = cx
        .world()
        .get::<CharacterController3D>(entity)
        .expect("one")
        .velocity;
    assert!((velocity.x - rolling.roll_speed).abs() < 1e-4, "{velocity}");
    // Past the roll, inside the cooldown: a dodge is dropped, not kept for later.
    let ticks = |seconds: f32| (seconds / Tick::DT.as_secs_f32()).ceil() as u64;
    run(&mut cx, &mut game, ticks(rolling.roll_time));
    apply(cx.world_mut(), &Command::Dodge);
    run(&mut cx, &mut game, 1);
    assert_eq!(rolled(&cx).rolling, 0.0);
    run(&mut cx, &mut game, ticks(rolling.roll_cooldown));
    apply(cx.world_mut(), &Command::Dodge);
    run(&mut cx, &mut game, 1);
    assert!(rolled(&cx).rolling > 0.0);
}

/// Forward walks the player away from the camera, a jump lifts it, and the camera follows.
#[test]
fn the_player_walks_forward_jumps_and_the_camera_follows() {
    let (mut cx, mut game, _) = loaded();
    // Lands on the floor first, then walks forward for a second.
    run(&mut cx, &mut game, 64);
    let (entity, start) = player(cx.world());
    let heading = Vec2::new(0.0, 1.0);
    apply(cx.world_mut(), &Command::Steer { heading });
    run(&mut cx, &mut game, 64);
    let walked = player(cx.world()).1;
    assert!(
        walked.z < start.z - 5.0 && walked.x.abs() < 0.01,
        "{walked}"
    );
    apply(
        cx.world_mut(),
        &Command::Steer {
            heading: Vec2::ZERO,
        },
    );
    apply(cx.world_mut(), &Command::Jump);
    run(&mut cx, &mut game, 8);
    let jumping = player(cx.world()).1;
    assert!(jumping.y > walked.y + 0.5, "{jumping}");
    let grounded = |world: &World| {
        world
            .get::<CharacterController3D>(entity)
            .map(|c| c.grounded)
    };
    assert_eq!(grounded(cx.world()), Some(false));
    let (_, camera, _) = view(cx.world()).expect("a camera");
    let behind = (camera - jumping).length();
    assert!((2.0..8.0).contains(&behind), "{behind}");
}

/// The mouse turns the rig, and a shot from its middle removes the target it hits and scores.
#[test]
fn a_shot_removes_the_target_it_hits() {
    let (mut cx, mut game, _) = loaded();
    run(&mut cx, &mut game, 4);
    let targets = |world: &World| world.query::<&Target>().count();
    assert_eq!(targets(cx.world()), 5);
    // Aimed at the sky, nothing is hit.
    apply(
        cx.world_mut(),
        &Command::Look {
            by: Vec2::new(0.0, -400.0),
        },
    );
    run(&mut cx, &mut game, 2);
    apply(cx.world_mut(), &Command::Fire);
    assert_eq!(targets(cx.world()), 5);
    // Aimed at the red target, from where the camera is as the rig turns toward it.
    let (aimed, at) = {
        let world = cx.world();
        let (entity, at, _) = (world.query::<(Entity, &Transform, &Target)>())
            .find(|(_, _, target)| target.points == 3)
            .expect("the red target");
        (entity, at.translation)
    };
    for rig in cx.world_mut().query_mut::<&mut Rig>() {
        rig.shoulder = 0.0;
        rig.aim_shoulder = 0.0;
    }
    for _ in 0..4 {
        let (_, from, _) = view(cx.world()).expect("a camera");
        let way = (at - from).as_vec3().normalize();
        for rig in cx.world_mut().query_mut::<&mut Rig>() {
            rig.yaw = libm::atan2f(-way.x, -way.z);
            rig.pitch = libm::asinf(way.y);
        }
        run(&mut cx, &mut game, 1);
    }
    apply(cx.world_mut(), &Command::Fire);
    // Knocked, not gone: it tumbles, and goes after TOPPLE seconds.
    assert_eq!(targets(cx.world()), 5);
    let fallen = *cx.world().get::<Target>(aimed).expect("it still tumbles");
    assert!(fallen.fall > 0.0);
    let body = *cx.world().get::<RigidBody3D>(aimed).expect("a body");
    assert_eq!(body.body_type, BodyType::Dynamic);
    // A second shot at it scores nothing more.
    apply(cx.world_mut(), &Command::Fire);
    assert_eq!(
        cx.world().query::<&Score>().next().expect("a score").hits,
        1
    );
    run(
        &mut cx,
        &mut game,
        (TOPPLE / Tick::DT.as_secs_f32()).ceil() as u64 + 2,
    );
    assert!(!cx.world().contains(aimed));
    assert_eq!(targets(cx.world()), 4);
    let score = *cx.world().query::<&Score>().next().expect("a score");
    assert_eq!(score.hits, 1);
    assert_eq!(score.points, 3);
}

/// A click in the view grabs the cursor and fires nothing; Escape lets it go.
#[test]
fn a_click_grabs_the_cursor_and_escape_lets_go() {
    let (mut cx, mut game, _) = loaded();
    let click = Event::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    };
    let _ = game.event(&mut cx, &click);
    game.update(&mut cx, Duration::from_millis(16));
    assert!(cx.cursor_grabbed());
    assert!(game.commands.is_empty(), "{:?}", game.commands);
    let _ = game.event(&mut cx, &click);
    assert_eq!(game.commands, [Command::Fire]);
    let escape = Event::Key {
        key: Key::Escape,
        pressed: true,
        repeat: false,
    };
    let _ = game.event(&mut cx, &escape);
    game.update(&mut cx, Duration::from_millis(16));
    assert!(!cx.cursor_grabbed());
}
