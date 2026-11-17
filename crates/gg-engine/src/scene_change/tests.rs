use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::dirs::tests::Scratch;
use crate::ecs::WorldHash;
use crate::layer::{Commands, Stack};
use crate::package::{Files, Kind, Package, Writer};
use crate::recording::{Recorder, Replay};
use crate::reflect::Reflect;
use crate::{App, Context, EntityId, Layer, Tick};

/// A command of [`Levels`]'s: asks for the scene of this path from the asset root.
#[derive(Reflect)]
struct Go(String);

/// Loads `scenes/menu.ron` as it attaches, beside an entity of its own, and asks each tick for the
/// scenes its commands name.
#[derive(Default)]
struct Levels(Vec<Go>);

impl Layer for Levels {
    fn attach(&mut self, cx: &mut Context) {
        let root = cx.root.clone().expect("a root");
        let menu = cx.load_scene_file(root.join("scenes/menu.ron"));
        menu.unwrap_or_else(|error| panic!("{error:#}"));
        cx.world_mut().spawn(crate::Transform::IDENTITY);
    }

    fn tick(&mut self, world: &mut World, _: Tick) {
        for Go(scene) in self.0.drain(..) {
            world.change_scene(&scene);
        }
    }

    fn commands(&mut self) -> Option<&mut dyn Commands> {
        Some(&mut self.0)
    }
}

const STONE: &str = "textures/stone.png";
const MOSS: &str = "textures/moss.png";

fn scene(textures: &[&str]) -> String {
    let mut text = "(\n    entities: {\n".to_owned();
    for (at, texture) in textures.iter().enumerate() {
        text += &format!(
            "        \"{:016x}\": (\n            Sprite: (\n                texture: \
             Some(\"{texture}\"),\n            ),\n        ),\n",
            at + 1
        );
    }
    text + "    },\n)\n"
}

/// A project's asset root in `dir`: textures; the menu (a stone sprite); the level (moss and stone
/// sprites); a scene of a component no world registers; one of a missing model beside a sprite of
/// a texture loaded nowhere else.
fn project(dir: &Path) -> PathBuf {
    let root = dir.join("game");
    for folder in ["textures", "scenes"] {
        fs::create_dir_all(root.join(folder)).expect("a directory");
    }
    let png =
        crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
    for texture in [STONE, MOSS, "textures/lichen.png"] {
        fs::write(root.join(texture), &png).expect("a PNG");
    }
    fs::write(root.join("scenes/menu.ron"), scene(&[STONE])).expect("the menu");
    fs::write(root.join("scenes/level.ron"), scene(&[MOSS, STONE])).expect("the level");
    let broken = "(\n    entities: {\n        \"0000000000000001\": (\n            Nonesuch: \
                  (),\n        ),\n    },\n)\n";
    fs::write(root.join("scenes/broken.ron"), broken).expect("the broken scene");
    let modeled = "(\n    entities: {\n        \"0000000000000001\": (\n            ModelRoot: \
                   (\"models/gone.glb\"),\n        ),\n        \"0000000000000002\": (\n\
                   Sprite: (\n                texture: Some(\"textures/lichen.png\"),\n\
                   ),\n        ),\n    },\n)\n";
    fs::write(root.join("scenes/modeled.ron"), modeled).expect("the modeled scene");
    root
}

fn ids(world: &World) -> Vec<EntityId> {
    let mut ids: Vec<EntityId> = (world.query::<crate::Entity>())
        .map(|entity| world.id(entity).expect("alive"))
        .collect();
    ids.sort_unstable();
    ids
}

fn scene_ids(count: u64) -> Vec<EntityId> {
    (1..=count).filter_map(EntityId::new).collect()
}

fn tick(stack: &mut Stack, cx: &mut Context, number: u64, scene: Option<&str>) {
    if let Some(scene) = scene {
        let queue = stack.commands_of(0).expect("a queue");
        queue
            .push(&Go(scene.to_owned()).to_value())
            .expect("queued");
    }
    assert!(stack.tick(cx, Tick { number }));
}

fn authors(files: &[crate::scene_files::SceneFile]) -> Vec<&str> {
    files.iter().map(|file| file.author.as_str()).collect()
}

/// A scene asked for in a tick goes in as the tick returns, replacing every entity (the layer's
/// too) and loading the textures no layer had; the world keeps its serial. A missing one leaves
/// the world as it was and logs `scene.not_found`. Asking again restarts the level. The run's
/// recording replays to every hash, loading what the scene names at the tick its run did.
#[test]
fn a_scene_asked_for_in_a_tick_goes_in_as_it_returns_and_replays_to_its_hashes() {
    let logged = crate::testing::logged();
    let dir = Scratch::new("scene-change-replays");
    let root = project(&dir.0);
    let app = || App::new("Levels").root(&root).layer(Levels::default());
    let path = dir.0.join("run.recording");
    let mut run = app();
    run.stack
        .record(Recorder::create(&path, run.name).expect("created"));
    let mut cx = Context::seeded(1).root(root.clone());
    run.stack.attach(&mut cx).expect("loaded");
    assert_eq!(ids(cx.world()).len(), 2, "the menu's and the layer's own");
    let serial = cx.world().serial();
    tick(&mut run.stack, &mut cx, 0, None);
    assert!(cx.world().assets().texture(MOSS).is_none());
    assert_eq!(cx.world().scene(), None);

    tick(&mut run.stack, &mut cx, 1, Some("scenes/level.ron"));
    assert_eq!(ids(cx.world()), scene_ids(2), "the level's alone");
    assert!(cx.world().assets().texture(MOSS).is_some(), "loaded");
    assert_eq!(cx.world().scene_asked(), None);
    assert_eq!(cx.world().scene(), Some("scenes/level.ron"));
    assert_eq!(cx.world().serial(), serial, "the same world");
    assert_eq!(
        authors(&cx.scenes.files),
        ["scenes/level.ron"],
        "in the world"
    );
    let level = cx.world().hash();

    tick(&mut run.stack, &mut cx, 2, Some("scenes/gone.ron"));
    assert_eq!(cx.world().hash(), level, "as it was");
    tick(&mut run.stack, &mut cx, 3, Some("scenes/level.ron"));
    assert_eq!(cx.world().hash().content, level.content, "the level again");
    tick(&mut run.stack, &mut cx, 4, Some("scenes/menu.ron"));
    assert_eq!(ids(cx.world()), scene_ids(1));
    let ended = cx.world().hash();
    run.stack.detach(&mut cx);
    run.stack.recorded().expect("written");
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let count = |code: &str| codes.iter().filter(|logged| *logged == code).count();
    assert_eq!(count("scene.replaced"), 3, "{codes:?}");
    assert_eq!(count("scene.not_found"), 1, "{codes:?}");

    let text = fs::read_to_string(&path).expect("written");
    let mut replay = Replay::new(app(), &text).unwrap_or_else(|error| panic!("{error:#}"));
    let mut ran = 0;
    while let Some(tick) = replay.step().unwrap_or_else(|error| panic!("{error:#}")) {
        assert_eq!(tick.number, ran);
        ran += 1;
    }
    assert_eq!((ran, replay.world().hash()), (5, ended));
}

/// Each refusal leaves the world as it was and logs its code: a missing scene, naming it; a name
/// that isn't a path inside the asset root; a name in another case than its file's; a scene that
/// doesn't load, its name starting the message; one naming a model that doesn't load, which leaves
/// no stand-in for the model or the texture beside it.
#[test]
fn a_scene_that_isn_t_there_or_doesn_t_load_changes_nothing() {
    let logged = crate::testing::logged();
    let dir = Scratch::new("scene-change-refused");
    let root = project(&dir.0);
    let mut stack = Stack::default();
    stack.push_layer(Levels::default());
    let mut cx = Context::seeded(1).root(root);
    stack.attach(&mut cx).expect("loaded");
    let (hash, serial) = (cx.world().hash(), cx.world().serial());
    let refused = [
        ("scenes/gone.ron", "scene.not_found"),
        ("../game/scenes/menu.ron", "scene.bad_path"),
        ("scenes/./menu.ron", "scene.bad_path"),
        ("scenes\\menu.ron", "scene.bad_path"),
        ("scenes/me\nnu.ron", "scene.bad_path"),
        ("", "scene.bad_path"),
        ("Scenes/Level.ron", "scene.case"),
        ("scenes/broken.ron", "reflect.unknown_type"),
        ("scenes/modeled.ron", "asset.no_model"),
    ];
    for (number, (scene, code)) in (0..).zip(refused) {
        let _ = logged.codes.try_iter().count();
        tick(&mut stack, &mut cx, number, Some(scene));
        assert_eq!((cx.world().hash(), cx.world().serial()), (hash, serial));
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(
            codes.iter().any(|logged| logged == code),
            "{scene}: {codes:?}"
        );
        let error = cx.change_scene(scene).expect_err("refused");
        assert_eq!(error.code(), code);
        if code != "scene.bad_path" {
            assert!(error.to_string().contains(scene), "{error}");
        }
    }
    let assets = cx.world().assets();
    assert!(assets.model("models/gone.glb").is_none(), "no stand-in");
    assert!(
        assets.texture("textures/lichen.png").is_none(),
        "nor its texture"
    );
    let codes: Vec<String> = logged.codes.try_iter().collect();
    assert!(
        !codes.iter().any(|code| code == "asset.stand_in"),
        "{codes:?}"
    );
    stack.detach(&mut cx);
}

/// A run without an asset root has no scene to change to; a link under the root to a file outside
/// it is refused, as a package wouldn't hold it.
#[test]
fn a_scene_outside_the_asset_root_is_refused() {
    let mut cx = Context::seeded(1);
    let error = cx.change_scene("scenes/menu.ron").expect_err("no root");
    assert_eq!(error.code(), "scene.no_root");
    #[cfg(unix)]
    {
        let dir = Scratch::new("scene-change-link");
        let root = project(&dir.0);
        let outside = dir.0.join("outside.ron");
        fs::write(&outside, scene(&[])).expect("a scene");
        std::os::unix::fs::symlink(&outside, root.join("scenes/out.ron")).expect("a link");
        let mut cx = Context::seeded(1).root(root);
        let error = cx.change_scene("scenes/out.ron").expect_err("outside");
        assert_eq!(error.code(), "asset.outside_root");
    }
}

/// A request made outside a tick (a layer's attach) is dropped as the next tick starts, warning
/// `scene.not_in_tick`; the world stays as it was.
#[test]
fn a_scene_asked_for_outside_a_tick_is_dropped() {
    struct Eager;

    impl Layer for Eager {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().change_scene("scenes/level.ron");
        }
    }

    let logged = crate::testing::logged();
    let dir = Scratch::new("scene-change-untimely");
    let root = project(&dir.0);
    let mut stack = Stack::default();
    stack.push_layer(Eager);
    let mut cx = Context::seeded(1).root(root);
    stack.attach(&mut cx).expect("loaded");
    let hash = cx.world().hash();
    assert!(stack.tick(&mut cx, Tick { number: 0 }));
    assert_eq!(cx.world().hash(), hash);
    assert_eq!(cx.world().scene(), None);
    let codes: Vec<String> = logged.codes.try_iter().collect();
    assert!(
        codes.iter().any(|code| code == "scene.not_in_tick"),
        "{codes:?}"
    );
    stack.detach(&mut cx);
}

/// A server's session loads nothing as it plays: a scene naming a texture no layer loaded is
/// refused with `net.no_scene_change`, and goes in once a layer has loaded it.
#[test]
fn a_server_s_session_changes_only_to_a_scene_of_what_is_loaded() {
    let dir = Scratch::new("scene-change-served");
    let root = project(&dir.0);
    let mut cx = Context::seeded(1).root(root.clone());
    cx.served = true;
    let menu = cx.load_scene_file(root.join("scenes/menu.ron"));
    menu.unwrap_or_else(|error| panic!("{error:#}"));
    let error = cx.change_scene("scenes/level.ron").expect_err("refused");
    assert_eq!(error.code(), "net.no_scene_change");
    assert!(error.to_string().contains(MOSS), "{error}");
    assert!(cx.world().assets().texture(MOSS).is_none(), "loaded none");
    cx.add_texture("moss", root.join(MOSS)).expect("a texture");
    assert_eq!(cx.change_scene("scenes/level.ron").ok(), Some(2));
    assert_eq!(ids(cx.world()), scene_ids(2));
}

/// A scene naming a texture that doesn't load goes in anyway, with a stand-in.
#[test]
fn a_texture_a_scene_names_that_doesn_t_load_stands_in() {
    let logged = crate::testing::logged();
    let dir = Scratch::new("scene-change-stand-in");
    let root = project(&dir.0);
    fs::write(root.join("scenes/lost.ron"), scene(&["textures/lost.png"])).expect("a scene");
    let mut cx = Context::seeded(1).root(root);
    assert_eq!(cx.change_scene("scenes/lost.ron").ok(), Some(1));
    assert!(cx.world().assets().texture("textures/lost.png").is_some());
    let codes: Vec<String> = logged.codes.try_iter().collect();
    assert!(
        codes.iter().any(|code| code == "asset.stand_in"),
        "{codes:?}"
    );
}

/// A scene's text is read once a run: a change after its file changes on disk puts in the text
/// read first, as a tick run again, a replay and a lockstep peer would.
#[test]
fn a_scene_is_read_once_a_run() {
    let dir = Scratch::new("scene-change-once");
    let root = project(&dir.0);
    let mut cx = Context::seeded(1).root(root.clone());
    assert_eq!(cx.change_scene("scenes/level.ron").ok(), Some(2));
    fs::write(root.join("scenes/level.ron"), scene(&[STONE])).expect("written");
    assert_eq!(cx.change_scene("scenes/level.ron").ok(), Some(2), "as read");
}

/// A snapshot taken once a scene has gone in holds its entities, loaded textures, file and kept
/// texts. A run restored from it is back in the scene, hashes alike, and changes scene as the
/// original would.
#[test]
fn a_run_restored_from_a_snapshot_comes_back_in_the_scene() {
    use crate::snapshot::{Of, Run, Snapshot};

    let dir = Scratch::new("scene-change-snapshot");
    let root = project(&dir.0);
    let mut stack = Stack::default();
    stack.push_layer(Levels::default());
    let mut cx = Context::seeded(1).root(root.clone());
    stack.attach(&mut cx).expect("loaded");
    tick(&mut stack, &mut cx, 0, Some("scenes/level.ron"));
    assert_eq!(authors(&cx.scenes.files), ["scenes/level.ron"]);
    let of = Of {
        world: cx.world(),
        scenes: &cx.scenes.files,
        cached: &cx.scenes.cached,
        open: &cx.scenes.open,
        scripts: &[],
        mods: &[],
        seen: None,
    };
    let text = crate::snapshot::write(&of, &Run::Playing, "Tested", &[], 1).expect("written");
    let snapshot = Snapshot::parse(text, "a test".to_owned(), "Tested").expect("read");
    assert_eq!(authors(&snapshot.scenes), ["scenes/level.ron"]);
    assert_eq!(
        authors(&snapshot.cached),
        ["scenes/menu.ron", "scenes/level.ron"]
    );

    let mut restored = Context::seeded(2).root(root);
    restored.load_restored(snapshot.added());
    snapshot
        .restore(restored.world_mut(), &[])
        .expect("restored");
    assert_eq!(restored.world().hash(), cx.world().hash());
    assert_eq!(restored.world().scene(), Some("scenes/level.ron"));
    assert!(restored.world().assets().texture(MOSS).is_some());
    stack.detach(&mut cx);
}

/// Under a SyncTest, a tick asking for a scene runs again on a copy of the world, the scene going
/// in there too, and leaves the copy as it left the world.
#[test]
fn a_scene_change_runs_again_alike_under_a_sync_test() {
    let dir = Scratch::new("scene-change-sync-test");
    let root = project(&dir.0);
    let mut stack = Stack::default();
    stack.push_layer(Levels::default());
    stack.sync_test();
    let mut cx = Context::seeded(1).root(root);
    stack.attach(&mut cx).expect("loaded");
    tick(&mut stack, &mut cx, 0, Some("scenes/level.ron"));
    tick(&mut stack, &mut cx, 1, Some("scenes/menu.ron"));
    assert!(!cx.exiting());
    assert_eq!(stack.sync_test_mut().map(|sync| sync.matched()), Some(2));
    assert_eq!(ids(cx.world()), scene_ids(1));
    stack.detach(&mut cx);
    assert!(!stack.failed());
}

/// Authors' edits stay in their logs as a scene goes in, but one applied before refuses to undo
/// (`ops.scene_changed`): a restarted level put another entity under its ID, which stays as the
/// scene gave it, and the edit stays logged. One applied after undoes. The scene's file becomes
/// the world's scene file and the project's open scene. A save of a scene file kept for changes
/// puts its text in at the next change, though the file isn't in the world.
#[cfg(feature = "dev")]
#[test]
fn edits_from_before_a_change_refuse_to_undo_and_a_save_reaches_the_next_change() {
    use crate::Transform;
    use crate::edits::Edit;
    use crate::reflect::Value;

    let logged = crate::testing::logged();
    let dir = Scratch::new("scene-change-edits");
    let root = project(&dir.0);
    let placed = "(\n    entities: {\n        \"0000000000000001\": (\n            Transform: \
                  (),\n        ),\n    },\n)\n";
    fs::write(root.join("scenes/placed.ron"), placed).expect("a scene");
    let mut stack = Stack::default();
    stack.push_layer(Levels::default());
    let mut cx = Context::seeded(1).root(root.clone());
    cx.start = Some(root.join("scenes/menu.ron"));
    stack.attach(&mut cx).expect("loaded");
    assert_eq!(cx.scenes.open.author.as_deref(), Some("scenes/menu.ron"));
    tick(&mut stack, &mut cx, 0, Some("scenes/placed.ron"));
    let one = EntityId::new(1).expect("an ID");
    let set = |x: f32| Edit::Apply {
        author: "agent".to_owned(),
        operations: vec![crate::ecs::Operation::Set {
            entity: one,
            component: "Transform".into(),
            path: "translation.x".to_owned(),
            value: Value::F64(f64::from(x)),
        }],
    };
    let x = |cx: &Context| {
        let entity = cx.world().entity(one).expect("alive");
        let transform = cx.world().get::<Transform>(entity);
        transform.expect("placed").translation.x
    };
    stack.edit(&mut cx, set(5.0)).expect("applied");
    assert_eq!(x(&cx), 5.0);

    // The level restarts: the entity under ID 1 is the scene's anew.
    tick(&mut stack, &mut cx, 1, Some("scenes/placed.ron"));
    assert_eq!(x(&cx), 0.0);
    assert_eq!(cx.authors.entries(cx.world()), 1, "kept");
    stack.edit(&mut cx, set(2.0)).expect("applied");
    let undo = || Edit::Undo {
        author: "agent".to_owned(),
    };
    stack.edit(&mut cx, undo()).expect("undone");
    assert_eq!(x(&cx), 0.0, "the edit after the change undone");
    let refused = stack.edit(&mut cx, undo()).expect_err("refused");
    assert_eq!(refused.code(), "ops.scene_changed");
    assert_eq!(x(&cx), 0.0, "the scene's entity left as it is");
    assert_eq!(cx.authors.entries(cx.world()), 2, "the edit stays");
    assert_eq!(cx.scenes.open.author.as_deref(), Some("scenes/placed.ron"));
    assert_eq!(authors(&cx.scenes.files), ["scenes/placed.ron"]);
    let codes: Vec<String> = logged.codes.try_iter().collect();
    let replaced = codes.iter().filter(|code| *code == "scene.replaced");
    assert_eq!(replaced.count(), 2, "{codes:?}");

    // The menu's file saved, not in the world now: the save keeps only its text.
    let saved = scene(&[MOSS, MOSS, STONE]);
    let apply = &mut |cx: &mut Context, edit| stack.edit(cx, edit);
    let done = crate::scene_files::apply_save(&mut cx, apply, "scenes/menu.ron", saved);
    done.expect("kept");
    assert_eq!(ids(cx.world()), scene_ids(1), "the placed scene's still");
    tick(&mut stack, &mut cx, 2, Some("scenes/menu.ron"));
    assert_eq!(ids(cx.world()), scene_ids(3), "as saved");
    stack.detach(&mut cx);
}

/// A run from a game's package (a build without `dev`) reads the scene from it though no file is
/// on disk, and loads the texture it names by ID from there. A name in another case than the
/// package's is refused.
#[test]
fn a_run_from_a_package_changes_to_a_scene_it_holds() {
    use crate::image::tests::ktx2;

    let dir = Scratch::new("scene-change-package");
    // Never made: everything loads from the package.
    let root = dir.0.join("game");
    let block = ktx2(ktx2::Format::BC7_SRGB_BLOCK, [1, 1], &[vec![0; 16]], true);
    let mut writer = Writer::new(".", Some("scenes/menu.ron"), None);
    let (id, named) = (0x5f3c_9a1e_07b2_d846, "5f3c9a1e07b2d846");
    writer
        .add(MOSS, Some(id), Kind::Texture, block)
        .expect("added");
    for (path, text) in [
        ("scenes/menu.ron", scene(&[])),
        ("scenes/level.ron", scene(&[named])),
    ] {
        let added = writer.add(path, None, Kind::File, text.into_bytes());
        added.expect("added");
    }
    let path = dir.0.join("game.ggpk");
    fs::create_dir_all(&dir.0).expect("a directory");
    writer.write(&path).expect("written");
    let mut cx = Context::seeded(1);
    cx.root = Some(root.clone());
    cx.files = Files::package(Arc::new(Package::open(&path).expect("opened")), root);
    let mut stack = Stack::default();
    stack.push_layer(Levels::default());
    stack.attach(&mut cx).expect("loaded");
    assert!(cx.world().assets().texture(named).is_none());
    tick(&mut stack, &mut cx, 0, Some("scenes/level.ron"));
    assert_eq!(ids(cx.world()), scene_ids(1));
    assert!(cx.world().assets().texture(named).is_some(), "by its ID");
    let error = cx
        .change_scene("scenes/gone.ron")
        .expect_err("not packaged");
    assert_eq!(error.code(), "scene.not_found");
    assert!(error.to_string().contains("package"), "{error}");
    let error = cx
        .change_scene("scenes/Level.ron")
        .expect_err("another case");
    assert_eq!(error.code(), "scene.case");
    stack.detach(&mut cx);
}

/// A level put in again hashes as the first time, whatever the world held between.
#[test]
fn a_level_restarted_hashes_as_it_started() {
    let dir = Scratch::new("scene-change-restart");
    let root = project(&dir.0);
    let mut cx = Context::seeded(1).root(root);
    cx.change_scene("scenes/level.ron").expect("in");
    let first: WorldHash = cx.world().hash();
    cx.world_mut().spawn(crate::Transform::IDENTITY);
    cx.change_scene("scenes/level.ron").expect("in again");
    assert_eq!(cx.world().hash().content, first.content);
}
