use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use gg_api::Project;

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
    assert!(found.assets.len() >= 9, "4 textures, 4 models and a font");
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

/// Every scene loads, with the textures, models and font it names by their IDs, holds a camera
/// the view sees through, and saves to its own bytes, so that a type of the game's or the
/// engine's that changes under it fails here.
#[test]
fn every_scene_loads_and_saves_as_it_is() {
    let scenes = scenes();
    assert_eq!(scenes.len(), 7, "{scenes:?}");
    for path in scenes {
        let text = fs::read_to_string(&path).expect("the scene");
        let mut cx = Context::seeded(1).root(dir());
        register(cx.world_mut());
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
/// a second; and Bob moves it up and back, at its top a quarter of its period in, and where it
/// started once the period's done.
#[test]
fn the_scripts_turn_and_bob_what_theyre_on() {
    let mut world = World::default();
    register(&mut world);
    let quarter = std::f32::consts::FRAC_PI_2;
    let spun = world.spawn((
        Transform::IDENTITY,
        Spin {
            axis: Vec3::ZERO,
            speed: quarter,
        },
    ));
    let bobbed = world.spawn((
        Transform::IDENTITY,
        Bob {
            height: 0.5,
            period: 2.0,
        },
    ));
    let mut ticks = (0..).map(|number| Tick { number });
    let mut run = |world: &mut World, count: usize| {
        for tick in ticks.by_ref().take(count) {
            Showcase.tick(world, tick);
        }
    };
    let transform = |world: &World, entity| *world.get::<Transform>(entity).expect("placed");
    run(&mut world, 32);
    assert!((transform(&world, bobbed).translation.y - 0.5).abs() < 1e-4);
    run(&mut world, 32);
    let turned = transform(&world, spun).rotation;
    assert!(
        turned.angle_between(Quat::from_rotation_z(quarter)) < 1e-3,
        "{turned:?}"
    );
    run(&mut world, 64);
    assert!(transform(&world, bobbed).translation.y.abs() < 1e-4);
}
