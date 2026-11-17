use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gg_api::{Fetch, Project};
use gg_engine::Transform;
use gg_engine::math::{DVec3, Quat, Vec3};
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

/// Each texture, model, font and prefab under the game's asset root has its ID file beside it,
/// committed with it, each holding an ID of its own, and no ID file is left without its asset, so
/// that every checkout gives each asset the ID the repository holds, which its scenes name it by.
#[test]
fn every_asset_has_its_id_file_beside_it() {
    let project = Project::read(dir()).expect("the project reads");
    let found = gg_api::asset::scan(&project.assets_dir(dir()));
    assert!(
        found.assets.len() >= 20,
        "7 textures, a sky's and the Fox's among them, 10 models, a font, 2 prefabs and a UI"
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
    assert_eq!(scenes.len(), 44, "{scenes:?}");
    let unfetched = unfetched();
    // One context for them all, each scene loaded into an empty world: what a scene names, by ID
    // or path, it finds whether or not a scene before loaded it. The prefabs' own tests load
    // theirs afresh.
    let (mut cx, _showcase) = loaded();
    for path in scenes {
        let text = fs::read_to_string(&path).expect("the scene");
        if let Some(name) = (unfetched.iter()).find(|name| text.contains(&format!("\"{name}\""))) {
            println!(
                "{} isn't loaded: it names {name}, which `cargo gg fetch` fetches",
                path.display()
            );
            continue;
        }
        cx.world_mut().clear();
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

/// The world of cells' scene names the folder its cells are in, each of which is a cell's file, on
/// the 8 by 8 grid of cells from the origin along x and z, and loads, beside the others, as cells
/// come in, and saves to its own bytes.
#[test]
fn every_cell_of_the_world_loads_and_saves_as_it_is() {
    let world = fs::read_to_string(dir().join("scenes/cells.ron")).expect("the world");
    assert!(world.contains("folder: \"scenes/cells\""), "{world}");
    let listed = fs::read_dir(dir().join("scenes/cells")).expect("the cells' folder");
    let mut names: Vec<String> = (listed.map(|entry| entry.expect("an entry").file_name()))
        .map(|name| name.into_string().expect("a name"))
        .collect();
    names.sort();
    let mut grid: Vec<String> = (0..8)
        .flat_map(|x| (0..8).map(move |z| format!("{x}_0_{z}.ron")))
        .collect();
    grid.sort();
    assert_eq!(names, grid);
    let (mut cx, _showcase) = loaded();
    for name in names {
        let path = dir().join("scenes/cells").join(&name);
        let text = fs::read_to_string(&path).expect("the cell");
        let roots = cx.load_scene_file(&path);
        let roots = roots.unwrap_or_else(|error| panic!("{name}: {error:#}"));
        let saved = cx.world().save_scene(&roots).expect("saved");
        assert_eq!(saved, text, "{name}");
    }
}

/// The world's scene names the folder of its cells, which holds no file, and the game makes each
/// cell as a scene file would hold it: each loads, with what it names, and saves to its own bytes,
/// each entity's ID its own, at the world's corners and between them.
#[test]
fn the_world_s_made_cells_load_and_save_as_they_are() {
    let world = fs::read_to_string(dir().join("scenes/world.ron")).expect("the world");
    assert!(
        world.contains(&format!("folder: \"{WORLD_FOLDER}\"")),
        "{world}"
    );
    assert!(!dir().join(WORLD_FOLDER).exists(), "made, not filed");
    let (mut cx, _showcase) = loaded();
    // Which names what they name, as a server's session loads nothing as it plays.
    cx.load_scene_file(dir().join("scenes/world.ron"))
        .expect("loaded");
    let mut ids = HashSet::new();
    for [x, z] in [[0, 0], [1, 0], [0, 1], [488, 311], [976, 976]] {
        let text = world_cell(Cell(IVec3::new(x, 0, z)));
        let roots = cx.world_mut().load_scene(&text);
        let roots = roots.unwrap_or_else(|error| panic!("{x}_0_{z}: {error:#}"));
        assert_eq!(roots.len(), 5, "{text}");
        ids.extend(
            roots
                .iter()
                .map(|&root| cx.world().id(root).expect("alive")),
        );
        let saved = cx.world().save_scene(&roots).expect("saved");
        assert_eq!(saved, text, "{x}_0_{z}");
    }
    assert_eq!(ids.len(), 25, "each its own");
}

/// The crossing's recording, `tests/references/crossing.recording`, of a run of the world, its
/// fliers crossing 1,000 km of cells the game makes as they come near, replays without a GPU to
/// the hashes it recorded, in a debug build as in a release one and on every platform.
/// `GG_UPDATE_REFERENCES=1` writes its hashes again, after a change meant to change the
/// simulation or what the hash reads; a change to a command's type that its lines no longer read
/// as, or to how the game makes its cells, needs a new recording: a run under `cargo gg run
/// --release -- --headless --real-time --record tests/references/crossing.recording`, taken from
/// the welcome to the world by Page Down twice, through `cargo gg input`, and quit through `cargo
/// gg quit` some 2,160 ticks after the world's cells came in, both fliers across.
#[test]
fn the_crossing_replays_to_its_hashes() {
    let path = dir().join("tests/references/crossing.recording");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("can't read {}: {error}", path.display()));
    if std::env::var_os("GG_UPDATE_REFERENCES").is_some_and(|value| value == "1") {
        let rewritten = gg_engine::recording::Replay::rewrite(app(), &text);
        fs::write(&path, rewritten.unwrap_or_else(|error| panic!("{error:#}"))).expect("written");
        println!("wrote {}", path.display());
        return;
    }
    assert!(text.ends_with("\nend\n"), "the recording has its end");
    let mut replay = gg_engine::recording::Replay::new(app(), &text);
    let replay = replay.as_mut().unwrap_or_else(|error| panic!("{error:#}"));
    let mut ticks = 0;
    // Each flier's cells, at the ends of its way: the east one's starting near the origin and
    // ending 976 cells east, the west one's the other way round, 976 cells south of it.
    let near = |world: &World, x: i32, z: i32| {
        let mut cells = world.cells_in();
        cells.any(|cell| (cell.0.x - x).abs() <= 2 && (cell.0.z - z).abs() <= 2)
    };
    let ends = |world: &World| {
        let [started, ended] =
            [[0, 976], [976, 0]].map(|[east, west]| near(world, east, 0) && near(world, west, 976));
        (started, ended)
    };
    let (mut started, mut ended) = (false, false);
    while let Some(tick) = replay.step().unwrap_or_else(|error| panic!("{error:#}")) {
        ticks = tick.number + 1;
        let (start, end) = ends(replay.world());
        started |= start;
        ended |= started && end;
    }
    let hashes = text
        .lines()
        .filter(|line| line.starts_with(|c: char| c.is_ascii_digit()));
    assert_eq!(ticks, hashes.count() as u64);
    assert!(
        started && ended,
        "the fliers crossed the world, each its way"
    );
}

/// Each prefab under the game's asset root loads, as the scene of one subtree it is, and saves to
/// its own bytes.
#[test]
fn every_prefab_loads_and_saves_as_it_is() {
    for name in ["lantern", "lamppost"] {
        let path = dir().join("prefabs").join(format!("{name}.prefab"));
        let text = fs::read_to_string(&path).expect("the prefab");
        let (mut cx, _showcase) = loaded();
        cx.add_prefab(name, &path).expect("added");
        let roots = cx.world_mut().load_scene(&text);
        let roots = roots.unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        assert_eq!(roots.len(), 1, "{}", path.display());
        let saved = cx.world().save_scene(&roots).expect("saved");
        assert_eq!(saved, text, "{}", path.display());
    }
}

/// The prefabs scene's lanterns are the prefab's, each with what it changes itself, the
/// lamppost's two among them, its right one's flame blue, and a caption of the prefab's on the
/// one that changes nothing.
#[test]
fn the_prefabs_scene_links_its_lanterns() {
    use gg_engine::{Name, PrefabRoot, Sprite, Text};

    let (mut cx, _showcase) = loaded();
    let path = dir().join("scenes").join("prefabs.ron");
    cx.load_scene_file(&path).expect("loaded");
    let world = cx.world();
    let lanterns = world.query::<(gg_engine::Entity, &Name)>();
    let lanterns = lanterns.filter(|(_, name)| name.0 == "Lantern");
    let lanterns: Vec<gg_engine::Entity> = lanterns.map(|(entity, _)| entity).collect();
    assert_eq!(lanterns.len(), 6, "four in the scene, two in the lamppost");
    for &lantern in &lanterns {
        assert!(world.get::<PrefabRoot>(lantern).is_some());
    }
    let named = |name: &str| {
        let found = world.query::<(gg_engine::Entity, &Name)>();
        let found = found
            .filter(|(_, named)| named.0 == name)
            .map(|(entity, _)| entity);
        found.collect::<Vec<_>>()
    };
    let colors: Vec<[f32; 4]> = (named("Flame").into_iter())
        .map(|flame| {
            world
                .get::<Sprite>(flame)
                .expect("a sprite")
                .color
                .to_array()
        })
        .collect();
    let orange = [1.0, 0.62, 0.18, 1.0];
    assert_eq!(colors.iter().filter(|&&color| color == orange).count(), 4);
    assert!(colors.contains(&[0.3, 1.0, 0.45, 1.0]), "the green flame");
    assert!(
        colors.contains(&[0.3, 0.6, 1.0, 1.0]),
        "the lamppost's blue flame"
    );
    let captions: Vec<String> = (named("Caption").into_iter())
        .map(|caption| world.get::<Text>(caption).expect("a text").text.clone())
        .collect();
    assert_eq!(captions.iter().filter(|text| *text == "LANTERN").count(), 1);
    assert!(captions.contains(&"NESTED".to_owned()), "{captions:?}");
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
        translation: DVec3::new(0.0, -0.5, 0.0),
        scale: Vec3::new(40.0, 1.0, 40.0),
        ..Transform::IDENTITY
    };
    world.spawn((floor, fixed, BoxCollider3D::default()));
    let at = Transform {
        translation: DVec3::new(0.0, 1.02, 0.0),
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
    let want = DVec3::new(walked.x, walked.y + 2.0, walked.z + 5.0);
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
        translation: transform(cx.world(), walker).translation + (behind * 2.0).as_dvec3(),
        rotation: transform(cx.world(), walker).rotation,
        scale: Vec3::new(6.0, 8.0, 0.5),
    };
    cx.world_mut()
        .spawn((wall, fixed, BoxCollider3D::default()));
    run(&mut showcase, cx.world_mut(), 2);
    let near = transform(cx.world(), camera).translation;
    let walker_at = transform(cx.world(), walker).translation;
    let off = near - walker_at - DVec3::Y * 2.0;
    assert!(off.length() < 1.75, "in front of the wall: {off}");
}

/// The joints scene, played: its bridge's planks stay on their hinges; its rope keeps its ball no
/// further than its length, its tether drawn from the hook to the ball; the door swings out one
/// way, then, as its swing turns its motor around through the joint's fields, the other; and of
/// the balls fired at the thin plate, the marbles, which have continuous collision, never pass it,
/// where some of the orange balls, which don't, do.
#[test]
fn the_joints_scene_holds_its_bodies_and_stops_only_whats_swept() {
    use gg_engine::{Joint3D, Name};

    let (mut cx, mut showcase) = loaded();
    let path = dir().join("scenes").join("joints.ron");
    cx.load_scene_file(&path).expect("loaded");
    let named = |world: &World, name: &str| -> Vec<Entity> {
        let found = world.query::<(Entity, &Name)>();
        let found = found.filter(|(_, named)| named.0 == name);
        found.map(|(entity, _)| entity).collect()
    };
    let world = cx.world_mut();
    let [marbles, balls, hinges] =
        ["Marble, continuous", "Ball", "Bridge hinge"].map(|name| named(world, name));
    assert_eq!((marbles.len(), balls.len(), hinges.len()), (3, 3, 10));
    let [rope] = named(world, "Rope")[..] else {
        panic!("one rope");
    };
    let [door] = named(world, "Door")[..] else {
        panic!("one door");
    };
    let transform = |world: &World, entity| *world.get::<Transform>(entity).expect("placed");
    let anchored = |world: &World, body: Option<Entity>, anchor: Vec3| {
        let body = body.expect("a body");
        transform(world, body)
            .matrix()
            .transform_point3(anchor.as_dvec3())
    };
    let door_turned = |world: &World| {
        let x = transform(world, door).rotation * Vec3::X;
        gg_engine::math::libm::atan2f(-x.z, x.x)
    };
    let (mut passed, mut swung) = (0, [0.0f32; 2]);
    for number in 0..320 {
        let world = cx.world_mut();
        showcase.tick(world, Tick { number });
        for &marble in &marbles {
            let x = transform(world, marble).translation.x;
            assert!(x < 5.2, "a marble passed the plate at tick {number}: {x}");
        }
        passed += balls
            .iter()
            .filter(|&&ball| transform(world, ball).translation.x > 5.3)
            .count();
        let roped = *world.get::<Joint3D>(rope).expect("a joint");
        let ends = [
            anchored(world, roped.body_a, roped.anchor_a),
            anchored(world, roped.body_b, roped.anchor_b),
        ];
        assert!(
            ends[0].distance(ends[1]) < 1.73,
            "past the rope's length at tick {number}"
        );
        let drawn = transform(world, rope);
        assert!(drawn.translation.distance((ends[0] + ends[1]) * 0.5) < 1e-4);
        // The swing turns the door's motor around every 2.5 seconds, 160 ticks.
        if number == 150 || number == 310 {
            swung[usize::from(number == 310)] = door_turned(world);
        }
    }
    assert!(passed > 0, "no orange ball passed the plate");
    assert!(swung[0] > 1.0 && swung[1] < -1.0, "{swung:?}");
    let world = cx.world();
    for hinge in hinges {
        let joint = *world.get::<Joint3D>(hinge).expect("a joint");
        let ends = [
            anchored(world, joint.body_a, joint.anchor_a),
            anchored(world, joint.body_b, joint.anchor_b),
        ];
        assert!(
            ends[0].distance(ends[1]) < 0.02,
            "a plank off its hinge: {ends:?}"
        );
    }
}

/// A chime rings once its seconds have passed, its tone placed on its entity, its circle flashing
/// full and thinning back to its ring; and again each time they pass once more.
#[test]
fn a_chime_rings_from_its_entity_and_flashes() {
    let (mut cx, mut showcase) = loaded();
    let world = cx.world_mut();
    let ring = gg_engine::Circle {
        thickness: RING,
        ..gg_engine::Circle::default()
    };
    let chime = Chime {
        tone: Tone::Pluck,
        every: 1.0,
        time: 0.0,
    };
    let bell = world.spawn((Transform::IDENTITY, ring, chime));
    let tones = showcase.assets.as_ref().expect("loaded").tones;
    let mut rang = Vec::new();
    for number in 0..192 {
        let world = cx.world_mut();
        showcase.tick(world, Tick { number });
        // The queue, which the engine would play and empty once each tick has run, grows here.
        if world.sounds_queued().count() > rang.len() {
            let queued: Vec<_> = world.sounds_queued().skip(rang.len()).collect();
            assert_eq!(queued, [(tones[1], Place::On(bell))]);
            rang.push(number);
        }
        let thickness = world
            .get::<gg_engine::Circle>(bell)
            .expect("its circle")
            .thickness;
        let since = rang.last().map(|rang| number - rang);
        match since {
            Some(0) => assert!(thickness > 0.9, "flashed: {thickness}"),
            Some(since) if since >= 38 => assert_eq!(thickness, RING),
            _ => assert!((RING..=1.0).contains(&thickness)),
        }
    }
    // Every second, 64 ticks, the first at the end of the first.
    assert_eq!(rang, [63, 127, 191]);
}

/// A burster raises its emitter's count of bursts once its seconds have passed, after its wait,
/// and again each time they pass once more, which the frames after burst it by, as
/// `particles.ron`'s sparks burst.
#[test]
fn a_burster_bursts_its_emitter_every_so_often() {
    let (mut cx, mut showcase) = loaded();
    let world = cx.world_mut();
    let burster = Burster {
        every: 1.0,
        time: 0.5,
    };
    let sparks = world.spawn((Transform::IDENTITY, burster, ParticleEmitter::default()));
    let mut raised = Vec::new();
    let mut counted = 0;
    for number in 0..160 {
        showcase.tick(world, Tick { number });
        let bursts = world
            .get::<ParticleEmitter>(sparks)
            .expect("its emitter")
            .bursts;
        if bursts != counted {
            assert_eq!(bursts, counted + 1, "once at a time");
            counted = bursts;
            raised.push(number);
        }
    }
    // Half a second in, a second apart: at the ends of ticks 31 and 95, of 64 a second.
    assert_eq!(raised, [31, 95, 159]);
}

/// A HUD counts its ticks and the clicks on its button, which reach it as commands; and a plasma's
/// entity gets its draw with the showcase's shader at a tick, again as its fields change.
#[test]
fn a_hud_counts_ticks_and_clicks_and_a_plasma_draws() {
    let (mut cx, mut showcase) = loaded();
    let world = cx.world_mut();
    let hud = world.spawn(Hud {
        ticks: 0,
        clicks: 0,
    });
    let plasma = Plasma {
        tint: Vec4::ONE,
        cells: 1.0,
        speed: 1.0,
    };
    let square = world.spawn((Transform::IDENTITY, plasma));
    assert!(world.get::<MeshDraw>(square).is_none());
    showcase.commands.extend([Command::Click, Command::Click]);
    for number in 0..3 {
        showcase.tick(world, Tick { number });
    }
    let counted = *world.get::<Hud>(hud).expect("the HUD");
    assert_eq!((counted.ticks, counted.clicks), (3, 2));
    let drawn = *world.get::<MeshDraw>(square).expect("drawn");
    world.get_mut::<Plasma>(square).expect("its plasma").speed = 2.0;
    showcase.tick(world, Tick { number: 3 });
    assert_ne!(world.get::<MeshDraw>(square), Some(&drawn));
}

/// The menu's UI takes input while a menu is in the world, and none without one: a key moves its
/// focus, past its disabled button, and Enter presses the button with the focus, and Escape asks
/// for back, each queuing a command that the next tick records on the menu, as it counts its
/// ticks.
#[test]
fn the_menus_presses_reach_the_world_as_commands() {
    let (mut cx, mut showcase) = loaded();
    let ui = showcase.assets.as_ref().expect("loaded").menu;
    let frame = Duration::from_millis(16);
    let key = |key| Event::Key {
        key,
        pressed: true,
        repeat: false,
    };
    // The arrows walk the walkers too, which queues commands of their own.
    let presses = |showcase: &Showcase| -> Vec<Command> {
        let presses = showcase.commands.iter();
        let presses = presses.filter(|command| matches!(command, Command::Press { .. }));
        presses.cloned().collect()
    };
    showcase.update(&mut cx, frame);
    let _ = showcase.event(&mut cx, &key(Key::ArrowDown));
    let _ = showcase.event(&mut cx, &key(Key::Enter));
    assert_eq!(presses(&showcase), [], "no menu, no input");
    assert_eq!(
        ui.focused(&mut cx).as_deref(),
        Some("new_game"),
        "the file's"
    );

    let scene = dir().join("scenes").join("menu.ron");
    cx.load_scene_file(&scene).expect("the scene loads");
    showcase.update(&mut cx, frame);
    let _ = showcase.event(&mut cx, &key(Key::ArrowUp));
    assert_eq!(
        ui.focused(&mut cx).as_deref(),
        Some("new_game"),
        "past the disabled one"
    );
    let _ = showcase.event(&mut cx, &key(Key::ArrowDown));
    assert_eq!(ui.focused(&mut cx).as_deref(), Some("options"));
    let _ = showcase.event(&mut cx, &key(Key::Enter));
    let _ = showcase.event(&mut cx, &key(Key::Escape));
    let press = |button: &str| Command::Press {
        button: button.to_owned(),
    };
    assert_eq!(presses(&showcase), [press("options"), press("back")]);
    showcase.tick(cx.world_mut(), Tick { number: 0 });
    let menu = cx.world().query::<&Menu>().next().cloned();
    let menu = menu.expect("the scene's menu");
    assert_eq!((menu.ticks, menu.presses, &*menu.pressed), (1, 2, "back"));
    showcase.update(&mut cx, frame);
}

/// Page Down and Page Up step through the scenes in the order of their names, from the scene the
/// world's entities came from, the start scene before any change has gone in, each step queuing a
/// command that the next tick asks for the scene with: a step whose change hasn't gone in steps
/// from where the world still is.
#[test]
fn the_page_keys_step_through_the_scenes() {
    let (mut cx, mut showcase) = loaded();
    let names: Vec<String> = scenes()
        .iter()
        .map(|path| format!("scenes/{}", path.file_name().expect("a name").display()))
        .collect();
    assert_eq!(showcase.scenes, names);
    let key = |key| Event::Key {
        key,
        pressed: true,
        repeat: false,
    };
    let mut step = |cx: &mut Context, page| {
        showcase.commands.clear();
        let _ = showcase.event(cx, &key(page));
        showcase.tick(cx.world_mut(), Tick { number: 0 });
        cx.world().scene_asked().map(str::to_owned)
    };
    let start = names.iter().position(|name| name == START).expect("listed");
    let after = names[(start + 1) % names.len()].clone();
    assert_eq!(step(&mut cx, Key::PageDown), Some(after));
    assert_eq!(step(&mut cx, Key::PageUp), Some(names[start - 1].clone()));
}

/// The widgets' UI takes input while widgets are in the world, and none without them: the right
/// arrow steps its slider, whose label follows it as it's drawn, and the down arrow and Enter flip
/// its toggle, each change queuing a command that the next tick records on the widgets.
#[test]
fn the_widgets_changes_reach_the_world_as_commands() {
    let (mut cx, mut showcase) = loaded();
    let ui = showcase.assets.as_ref().expect("loaded").widgets;
    let frame = Duration::from_millis(16);
    let key = |key| Event::Key {
        key,
        pressed: true,
        repeat: false,
    };
    // The arrows walk the walkers too, which queues commands of their own.
    let changes = |showcase: &Showcase| -> Vec<Command> {
        let changes = showcase.commands.iter();
        let changes = changes.filter(|command| matches!(command, Command::Change { .. }));
        changes.cloned().collect()
    };
    showcase.update(&mut cx, frame);
    let _ = showcase.event(&mut cx, &key(Key::ArrowRight));
    assert_eq!(changes(&showcase), [], "no widgets, no input");

    let scene = dir().join("scenes").join("widgets.ron");
    cx.load_scene_file(&scene).expect("the scene loads");
    showcase.update(&mut cx, frame);
    for pressed in [Key::ArrowRight, Key::ArrowDown, Key::Enter] {
        let _ = showcase.event(&mut cx, &key(pressed));
    }
    let change = |widget: &str, value: &str| Command::Change {
        widget: widget.to_owned(),
        value: value.to_owned(),
    };
    let expected = [change("volume", "0.75"), change("subtitles", "off")];
    assert_eq!(changes(&showcase), expected);
    showcase.tick(cx.world_mut(), Tick { number: 0 });
    let widgets = cx.world().query::<&Widgets>().next().cloned();
    let widgets = widgets.expect("the scene's widgets");
    assert_eq!((widgets.changes, &*widgets.changed), (2, "subtitles off"));
    showcase.update(&mut cx, frame);
    let Ok(UiValue::Number(volume)) = ui.value(&mut cx, "volume") else {
        panic!("the slider's value");
    };
    assert!((volume - 0.75).abs() < 1e-5, "{volume}");
}

/// A click in the view grabs the cursor while the world has a look, and Escape lets go; the mouse's
/// motion while it's grabbed, and only then, queues one turn a tick, adding up, which turns the
/// look's camera to its right and up, level, no further up than 89°; and a world without a look
/// lets go.
#[test]
fn a_look_turns_its_camera_by_the_mouse_while_the_cursor_is_grabbed() {
    let (mut cx, mut showcase) = loaded();
    let look = Look {
        yaw: 0.0,
        pitch: 0.0,
        sensitivity: 0.0025,
    };
    let camera = cx.world_mut().spawn((Transform::IDENTITY, look));
    let frame = Duration::from_millis(16);
    let motion = |x, y| Event::MouseMotion { x, y };
    let _ = showcase.event(&mut cx, &motion(100.0, 0.0));
    assert!(showcase.commands.is_empty(), "not grabbed: no turn");
    let click = Event::MouseButton {
        button: MouseButton::Left,
        pressed: true,
    };
    let _ = showcase.event(&mut cx, &click);
    showcase.update(&mut cx, frame);
    assert!(cx.cursor_grabbed());
    // A quarter turn to the right, in two moves, and a little up.
    for event in [motion(314.159_27, -40.0), motion(314.159_27, 0.0)] {
        let _ = showcase.event(&mut cx, &event);
    }
    assert_eq!(showcase.commands.len(), 1, "{:?}", showcase.commands);
    showcase.tick(cx.world_mut(), Tick { number: 0 });
    let turned = cx
        .world()
        .get::<Transform>(camera)
        .expect("the camera")
        .rotation;
    let forward = turned * Vec3::NEG_Z;
    assert!(
        (forward - Vec3::new(1.0, 0.1, 0.0).normalize()).length() < 1e-3,
        "{forward}"
    );
    let right = turned * Vec3::X;
    assert!(right.y.abs() < 1e-6, "level: {right}");
    // Far up, no further than 89°.
    let _ = showcase.event(&mut cx, &motion(0.0, -10_000.0));
    showcase.tick(cx.world_mut(), Tick { number: 1 });
    let looked = *cx.world().get::<Look>(camera).expect("the look");
    assert_eq!(looked.pitch, MAX_PITCH);
    let escape = Event::Key {
        key: Key::Escape,
        pressed: true,
        repeat: false,
    };
    let _ = showcase.event(&mut cx, &escape);
    showcase.update(&mut cx, frame);
    assert!(!cx.cursor_grabbed(), "let go at Escape");
    let _ = showcase.event(&mut cx, &click);
    showcase.update(&mut cx, frame);
    assert!(cx.cursor_grabbed());
    cx.world_mut().despawn(camera).expect("despawned");
    showcase.update(&mut cx, frame);
    assert!(!cx.cursor_grabbed(), "nothing to look with");
}

/// The music plays while a jukebox is in the world, its bus at the jukebox's volume, and stops once
/// it's gone.
#[test]
fn a_jukebox_plays_the_music_while_its_there() {
    let (mut cx, mut showcase) = loaded();
    let music = showcase.assets.as_ref().expect("loaded").music;
    let frame = Duration::from_millis(16);
    showcase.update(&mut cx, frame);
    assert_eq!(cx.music_playing(), None);
    let jukebox = cx.world_mut().spawn(Jukebox { volume: 0.5 });
    showcase.update(&mut cx, frame);
    assert_eq!(cx.music_playing(), Some(music));
    assert_eq!(cx.volume(Bus::Music), 0.5);
    cx.world_mut().despawn(jukebox).expect("despawned");
    showcase.update(&mut cx, frame);
    assert_eq!(cx.music_playing(), None);
}

/// The states scene's fox stands surveying, then walks round, Walk alone at a walking speed, which
/// Pace sets through `gg.set_param`, jogs on Walk and Run blended in step, and at its look stands
/// and plays Look once before it walks on; and its label says each state and speed.
#[test]
fn the_states_scene_walks_its_fox_through_its_states() {
    use gg_engine::{AnimationStates, Animator, Text};

    let (mut cx, mut showcase) = loaded();
    let scene = dir().join("scenes").join("states.ron");
    cx.load_scene_file(&scene).expect("the scene loads");
    let world = cx.world_mut();
    let fox = world
        .query::<(gg_engine::Entity, &AnimationStates)>()
        .next();
    let fox = fox.expect("a fox with a state machine").0;
    let label = world.query::<(gg_engine::Entity, &StateLabel)>().next();
    let label = label.expect("a label").0;
    let mut ticks = (0..).map(|number| Tick { number });
    let mut run = |showcase: &mut Showcase, world: &mut World, seconds: f32| {
        for _ in 0..(seconds * 64.0) as usize {
            showcase.tick(world, ticks.next().expect("ticks without end"));
        }
    };
    let state = |world: &World| {
        let machine = world.get::<AnimationStates>(fox).expect("its machine");
        machine.state.clone()
    };
    let plays = |world: &World| -> Vec<(String, f32)> {
        let Animator(plays) = world.get::<Animator>(fox).expect("playing");
        plays
            .iter()
            .map(|play| (play.clip.clone(), play.weight))
            .collect()
    };
    let said = |world: &World| world.get::<Text>(label).expect("a text").text.clone();
    run(&mut showcase, world, 1.0);
    assert_eq!(state(world), "Idle");
    assert_eq!(plays(world), [("Survey".to_owned(), 1.0)]);
    assert_eq!(said(world), "Idle  ·  speed 0.0");
    let start = world.get::<Transform>(fox).expect("placed").translation;
    // Walking, three and a half seconds in.
    run(&mut showcase, world, 2.5);
    assert_eq!(state(world), "Move");
    assert_eq!(plays(world), [("Walk".to_owned(), 1.0)]);
    assert_eq!(said(world), "Move  ·  speed 1.6");
    let moved = world.get::<Transform>(fox).expect("placed").translation;
    assert!(moved.distance(start) > 1.0, "{start} to {moved}");
    // Looking, from four and a half seconds in, until its share of Look has played.
    run(&mut showcase, world, 1.25);
    assert_eq!(state(world), "Look");
    assert_eq!(said(world), "Look  ·  speed 0.0");
    run(&mut showcase, world, 2.0);
    assert_eq!(state(world), "Move");
    // Jogging, Walk and Run blended in step, eight and a half seconds in, the round's seven.
    run(&mut showcase, world, 1.75);
    let jogging = plays(world);
    let weights: Vec<f32> = jogging.iter().map(|(_, weight)| *weight).collect();
    assert_eq!(jogging.len(), 2, "{jogging:?}");
    assert!((weights[0] + weights[1] - 1.0).abs() < 1e-6, "{jogging:?}");
    let Animator(played) = world.get::<Animator>(fox).expect("playing");
    let lengths = |clip: &str| {
        if clip == "Walk" {
            0.708_333_3
        } else {
            1.158_333_3
        }
    };
    let phases: Vec<f32> = (played.iter())
        .map(|play| play.time / lengths(&play.clip))
        .collect();
    assert!((phases[0] - phases[1]).abs() < 1e-4, "in step: {phases:?}");
}

/// Moods cross-fades its fox to its model's next clip every so often, through `gg.play`: from
/// Survey to Walk once its three seconds have passed, Walk's weight easing in over half a second as
/// Survey's eases out and leaves; and the fox's joints move as its clips play.
#[test]
fn a_mood_cross_fades_its_fox_to_its_next_clip() {
    let (mut cx, mut showcase) = loaded();
    let scene = dir().join("scenes").join("skeletal.ron");
    cx.load_scene_file(&scene).expect("the scene loads");
    let world = cx.world_mut();
    let moody = world.entities(Some("Moods")).expect("declared").next();
    let fox = moody.expect("a fox with moods");
    let plays = |world: &World| -> Vec<(String, f32)> {
        let gg_engine::Animator(plays) = world.get::<gg_engine::Animator>(fox).expect("playing");
        plays
            .iter()
            .map(|play| (play.clip.clone(), play.weight))
            .collect()
    };
    let mut ticks = (0..).map(|number| Tick { number });
    let mut run = |showcase: &mut Showcase, world: &mut World, count: usize| {
        for _ in 0..count {
            showcase.tick(world, ticks.next().expect("ticks without end"));
        }
    };
    let joints = |world: &World| -> Vec<Transform> {
        let nodes = world.query::<(&gg_engine::ModelNode, &Transform)>();
        nodes.map(|(_, transform)| *transform).collect()
    };
    let before = joints(world);
    run(&mut showcase, world, 191);
    assert_eq!(plays(world), [("Survey".to_owned(), 1.0)]);
    assert_ne!(joints(world), before, "the joints move");
    run(&mut showcase, world, 1);
    run(&mut showcase, world, 1);
    let fading = plays(world);
    assert_eq!(fading[0].0, "Survey");
    assert_eq!(fading[1], ("Walk".to_owned(), 2.0 / 32.0), "{fading:?}");
    run(&mut showcase, world, 31);
    assert_eq!(plays(world), [("Walk".to_owned(), 1.0)]);
}

/// The navigation scene bakes its navmesh, and its agents walk their tours, each sent on by Tour to
/// its stops in turn as it arrives at each, the platform up the ramp among them, round each other.
#[test]
fn the_navigation_scenes_agents_walk_their_tours() {
    use gg_engine::{NavAgent, Navigation};

    let (mut cx, mut showcase) = loaded();
    let scene = dir().join("scenes").join("navigation.ron");
    cx.load_scene_file(&scene).expect("the scene loads");
    let world = cx.world_mut();
    let touring: Vec<gg_engine::Entity> = world.entities(Some("Tour")).expect("declared").collect();
    assert_eq!(touring.len(), 12);
    let next = |world: &World, agent| match world.get_field(agent, "Tour", "next") {
        Ok(Value::F64(next)) => next,
        other => panic!("{other:?}"),
    };
    let mut stops = vec![0; touring.len()];
    let mut was: Vec<f64> = touring.iter().map(|&agent| next(world, agent)).collect();
    let mut highest = f32::NEG_INFINITY;
    for number in 0..1920 {
        showcase.tick(world, Tick { number });
        for (i, &agent) in touring.iter().enumerate() {
            let now = next(world, agent);
            if now != was[i] {
                stops[i] += 1;
                was[i] = now;
            }
            let at = world.get::<Transform>(agent).expect("placed").translation;
            highest = highest.max(at.y as f32);
        }
    }
    assert!(world.resource::<Navigation>().is_some(), "baked");
    let walked = touring.iter().filter(|&&agent| {
        let agent = world.get::<NavAgent>(agent).expect("an agent");
        !agent.corners.is_empty()
    });
    assert_eq!(walked.count(), touring.len(), "each has a path");
    // Its first stop set off for, then at least two more reached in thirty seconds.
    assert!(stops.iter().all(|&set| set >= 3), "{stops:?}");
    assert!(highest > 1.9, "up on the platform: {highest}");
}
