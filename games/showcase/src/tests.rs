use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gg_api::{Fetch, Project};
use gg_engine::Transform;
use gg_engine::math::{Quat, Vec3};
use gg_engine::reflect::Value;

use super::*;

/// The game's project's directory.
fn dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The game's project's manifest is canonical, writing back to its own bytes, and starts the game
/// in its welcome.
#[test]
fn the_project_is_canonical_and_starts_in_its_welcome() {
    let text = fs::read_to_string(dir().join(Project::FILE)).expect("the manifest");
    let project = Project::parse(&text).expect("the project reads");
    assert_eq!(project.text(), text);
    let start = dir().join("scenes").join("welcome.ron");
    assert_eq!(project.start_path(dir()), start);
}

/// Each texture, model and font under the game's asset root has its ID file beside it, committed
/// with it, each holding an ID of its own, and no ID file is left without its asset, so that every
/// checkout gives each asset the ID the repository holds, which its scenes name it by.
#[test]
fn every_asset_has_its_id_file_beside_it() {
    let project = Project::read(dir()).expect("the project reads");
    let found = gg_api::asset::scan(&project.assets_dir(dir()));
    assert!(
        found.assets.len() >= 16,
        "6 textures, a sky's among them, 9 models and a font"
    );
    let mut ids = HashSet::new();
    for asset in &found.assets {
        let path = &asset.path;
        match &asset.meta {
            Some(Ok(meta)) => {
                let id = meta.id;
                assert!(ids.insert(id), "{path}.meta holds {id}, another asset's ID");
            }
            Some(Err(failure)) => panic!("{path}'s ID file: {}: {failure}", failure.code),
            None => panic!(
                "{path} has no ID file, {path}.meta: a run that serves the tool API writes it; \
                 commit it beside its file"
            ),
        }
    }
    assert!(found.orphans.is_empty(), "{:?}", found.orphans);
    assert!(found.unread.is_empty(), "{:?}", found.unread);
}

/// The scene files, each in the scenes folder, by name.
fn scenes() -> Vec<PathBuf> {
    let listed = fs::read_dir(dir().join("scenes")).expect("the scenes folder");
    let mut scenes: Vec<PathBuf> = (listed.map(|entry| entry.expect("an entry").path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "ron"))
        .collect();
    scenes.sort();
    scenes
}

/// A context of the game's root, its scripts loaded, as the game's layer loads them, which
/// declares their components; and the layer.
fn loaded() -> (Context, Showcase) {
    let mut cx = Context::seeded(1).root(dir());
    let mut showcase = Showcase::new();
    showcase.load(&mut cx);
    (cx, showcase)
}

/// What scenes name the files the fetch list names that aren't there, as before `cargo gg fetch`
/// has fetched them, by: their paths, and the IDs the list gives them.
fn unfetched() -> Vec<String> {
    let fetch = Fetch::read(dir()).expect("the fetch list reads");
    let missing = fetch.files().filter(|file| !file.at(dir()).is_file());
    let names = missing.flat_map(|file| [Some(file.path), file.id.map(|id| id.to_string())]);
    names.flatten().collect()
}

/// What the fetch list fetches stays out of the repository: each folder it fetches into is one
/// the project's .gitignore leaves out.
#[test]
fn fetched_files_stay_out_of_the_repository() {
    let fetch = Fetch::read(dir()).expect("the fetch list reads");
    let ignore = fs::read_to_string(dir().join(".gitignore")).expect("the .gitignore");
    for source in &fetch.0 {
        let line = format!("/{}/", source.into);
        assert!(
            ignore.lines().any(|ignored| ignored == line),
            "{line} isn't in .gitignore"
        );
    }
}

/// Every scene loads, with the textures, models and font it names by their IDs or paths, holds a
/// camera the view sees through, and saves to its own bytes, so that a type of the game's or the
/// engine's, or a script's component, that changes under it fails here; but one naming a file the
/// fetch list fetches, before it's fetched.
#[test]
fn every_scene_loads_and_saves_as_it_is() {
    let scenes = scenes();
    assert_eq!(scenes.len(), 18, "{scenes:?}");
    let unfetched = unfetched();
    for path in scenes {
        let text = fs::read_to_string(&path).expect("the scene");
        if let Some(name) = (unfetched.iter()).find(|name| text.contains(&format!("\"{name}\""))) {
            println!(
                "{} isn't loaded: it names {name}, which `cargo gg fetch` fetches",
                path.display()
            );
            continue;
        }
        let (mut cx, _showcase) = loaded();
        let roots = cx.load_scene_file(&path);
        let roots = roots.unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        let primary = cx.world().query::<&gg_engine::Camera>();
        assert_eq!(
            primary.filter(|camera| camera.primary).count(),
            1,
            "{path:?}"
        );
        let saved = cx.world().save_scene(&roots).expect("saved");
        assert_eq!(saved, text, "{}", path.display());
    }
}

/// Spin turns its entity about its axis, about Z without one, a quarter turn in a second at 90°
/// a second, and about the axis it's given, made a unit; and Bob moves it up and back, at its top
/// a quarter of its period in, and where it started once the period's done, keeping the time it
/// has run.
#[test]
fn the_scripts_turn_and_bob_what_theyre_on() {
    let (mut cx, mut showcase) = loaded();
    let quarter = std::f32::consts::FRAC_PI_2;
    let world = cx.world_mut();
    let mut with = |component: &str, fields: &[(&str, Value)]| {
        let entity = world.spawn(Transform::IDENTITY);
        world
            .add_default(entity, component)
            .expect("its script loaded");
        for (field, value) in fields {
            let set = world.set_field(entity, component, field, value);
            set.unwrap_or_else(|error| panic!("{error:#}"));
        }
        entity
    };
    let spun = with("Spin", &[("speed", Value::F64(quarter.into()))]);
    let tilted = with(
        "Spin",
        &[
            ("axis", Value::Vec3(Vec3::new(0.0, 2.0, 0.0))),
            ("speed", Value::F64(quarter.into())),
        ],
    );
    let bobbed = with("Bob", &[("height", Value::F64(0.5))]);
    let mut ticks = (0..).map(|number| Tick { number });
    let mut run = |world: &mut World, count: usize| {
        for tick in ticks.by_ref().take(count) {
            showcase.tick(world, tick);
        }
    };
    let transform = |world: &World, entity| *world.get::<Transform>(entity).expect("placed");
    run(cx.world_mut(), 32);
    assert!((transform(cx.world(), bobbed).translation.y - 0.5).abs() < 1e-4);
    run(cx.world_mut(), 32);
    for (entity, turn) in [
        (spun, Quat::from_rotation_z(quarter)),
        (tilted, Quat::from_rotation_y(quarter)),
    ] {
        let turned = transform(cx.world(), entity).rotation;
        assert!(turned.angle_between(turn) < 1e-3, "{turned:?}");
    }
    run(cx.world_mut(), 64);
    assert!(transform(cx.world(), bobbed).translation.y.abs() < 1e-4);
    let time = cx
        .world()
        .get_field(bobbed, "Bob", "time")
        .expect("its time");
    assert_eq!(time, Value::F64(2.0));
}

/// Pulse pops what it's on to its size once its seconds have passed, and a tween eases it back to
/// its rest, springing past it as it settles, and there at its end.
#[test]
fn a_pulse_pops_and_a_tween_eases_it_back() {
    let (mut cx, mut showcase) = loaded();
    let world = cx.world_mut();
    let popped = world.spawn(Transform::IDENTITY);
    world
        .add_default(popped, "Pulse")
        .expect("its script loaded");
    let mut ticks = (0..).map(|number| Tick { number });
    let mut run = |showcase: &mut Showcase, world: &mut World| {
        let tick = ticks.next().expect("ticks without end");
        showcase.tick(world, tick);
        world.get::<Transform>(popped).expect("placed").scale
    };
    // A second and a half, 96 ticks, before it pops.
    for _ in 0..95 {
        assert_eq!(run(&mut showcase, cx.world_mut()), Vec3::ONE);
    }
    let popped_to = run(&mut showcase, cx.world_mut());
    assert!(popped_to.x > 1.0 && popped_to.x < 1.5, "{popped_to}");
    let settling: Vec<Vec3> = (0..52)
        .map(|_| run(&mut showcase, cx.world_mut()))
        .collect();
    let least = settling
        .iter()
        .map(|scale| scale.x)
        .fold(f32::INFINITY, f32::min);
    assert!(least < 1.0, "springs past its rest: {least}");
    assert_eq!(settling.last(), Some(&Vec3::ONE));
}

/// Walk walks its character the way the keys say, forward along its -Z, turning about Y, and
/// jumps as the jump key is pressed, once, if it stands; Follow keeps its camera behind and above
/// the character, looking the way it faces, and nearer where a wall comes between them.
#[test]
fn the_walker_walks_turns_and_jumps_and_its_camera_follows() {
    use gg_engine::{
        BodyType, BoxCollider3D, CapsuleCollider3D, CharacterController3D, RigidBody3D,
    };

    let (mut cx, mut showcase) = loaded();
    let world = cx.world_mut();
    let fixed = RigidBody3D {
        body_type: BodyType::Static,
        ..RigidBody3D::default()
    };
    let floor = Transform {
        translation: Vec3::new(0.0, -0.5, 0.0),
        scale: Vec3::new(40.0, 1.0, 40.0),
        ..Transform::IDENTITY
    };
    world.spawn((floor, fixed, BoxCollider3D::default()));
    let at = Transform {
        translation: Vec3::new(0.0, 1.02, 0.0),
        ..Transform::IDENTITY
    };
    let moved = (
        at,
        CharacterController3D::default(),
        CapsuleCollider3D::default(),
    );
    let walker = world.spawn(moved);
    world
        .add_default(walker, "Walk")
        .expect("its script loaded");
    let camera = world.spawn(Transform::IDENTITY);
    world
        .add_default(camera, "Follow")
        .expect("its script loaded");
    let target = Value::Option(Some(Box::new(Value::Entity(walker))));
    world
        .set_field(camera, "Follow", "target", &target)
        .expect("an entity");
    let mut ticks = (0..).map(|number| Tick { number });
    let mut run = |showcase: &mut Showcase, world: &mut World, count: usize| {
        for tick in ticks.by_ref().take(count) {
            showcase.tick(world, tick);
        }
    };
    let transform = |world: &World, entity| *world.get::<Transform>(entity).expect("placed");
    let controller = |world: &World| {
        *world
            .get::<CharacterController3D>(walker)
            .expect("a character")
    };
    run(&mut showcase, cx.world_mut(), 16);
    let start = transform(cx.world(), walker).translation;
    // A second forward, at 4 units a second.
    let heading = Vec2::new(0.0, 1.0);
    showcase.commands.push(Command::Walk { heading });
    run(&mut showcase, cx.world_mut(), 64);
    let walked = transform(cx.world(), walker).translation;
    assert!(
        walked.z < start.z - 3.7 && (walked.x - start.x).abs() < 0.01,
        "{walked}"
    );
    // The camera 5 behind, along +Z, and 2 above, as the last tick found the walker.
    let followed = transform(cx.world(), camera);
    let want = Vec3::new(walked.x, walked.y + 2.0, walked.z + 5.0);
    assert!(followed.translation.distance(want) < 0.1, "{followed:?}");
    // Half a second turning right, still, then a jump.
    let heading = Vec2::new(1.0, 0.0);
    showcase.commands.push(Command::Walk { heading });
    run(&mut showcase, cx.world_mut(), 32);
    let turned = transform(cx.world(), walker).rotation;
    assert!(
        turned.angle_between(Quat::from_rotation_y(-1.25)) < 1e-3,
        "{turned}"
    );
    let heading = Vec2::ZERO;
    showcase.commands.push(Command::Walk { heading });
    showcase.commands.push(Command::Jump);
    run(&mut showcase, cx.world_mut(), 8);
    let jumped = controller(cx.world());
    assert!(!jumped.grounded && jumped.velocity.y > 2.0, "{jumped:?}");
    let jump = cx
        .world()
        .get_field(walker, "Walk", "jump")
        .expect("a field");
    assert_eq!(jump, Value::Bool(false), "taken");
    run(&mut showcase, cx.world_mut(), 96);
    assert!(controller(cx.world()).grounded, "landed");
    // A wall 2 behind it, the way the camera is: the camera comes in front of it.
    let behind = transform(cx.world(), walker).rotation * Vec3::Z;
    let wall = Transform {
        translation: transform(cx.world(), walker).translation + behind * 2.0,
        rotation: transform(cx.world(), walker).rotation,
        scale: Vec3::new(6.0, 8.0, 0.5),
    };
    cx.world_mut()
        .spawn((wall, fixed, BoxCollider3D::default()));
    run(&mut showcase, cx.world_mut(), 2);
    let near = transform(cx.world(), camera).translation;
    let walker_at = transform(cx.world(), walker).translation;
    let off = near - walker_at - Vec3::Y * 2.0;
    assert!(off.length() < 1.75, "in front of the wall: {off}");
}
