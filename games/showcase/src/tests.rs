use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gg_api::Project;
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
        found.assets.len() >= 12,
        "5 textures, a sky's among them, 6 models and a font"
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

/// Every scene loads, with the textures, models and font it names by their IDs, holds a camera
/// the view sees through, and saves to its own bytes, so that a type of the game's or the
/// engine's, or a script's component, that changes under it fails here.
#[test]
fn every_scene_loads_and_saves_as_it_is() {
    let scenes = scenes();
    assert_eq!(scenes.len(), 11, "{scenes:?}");
    for path in scenes {
        let text = fs::read_to_string(&path).expect("the scene");
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
