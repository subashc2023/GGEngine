use std::fs::{self, File};
use std::time::{Duration, SystemTime};

use gg_core::math::{DVec3, Vec3};

use super::*;
use crate::args::Net;
use crate::cells::CellState;
use crate::cells::tests::{id_of, look_at, scene as cells_scene, xs};
use crate::dirs::tests::Scratch;
use crate::ecs::Operation;
use crate::edits::Edit;
use crate::layer::{Offered, Stack};
use crate::net::{DELAY, Lockstep};
use crate::reflect::Value;
use crate::snapshot::{self, Run, Snapshot};
use crate::{Context, Entity, ModelNode, ModelRoot, Reflect, Tick, Transform, World};

/// Writes `text` at `path`, stamped `later` seconds past now, so a look sees it changed whatever
/// the clock's grain.
pub(crate) fn save(path: &Path, text: &str, later: u64) {
    fs::write(path, text).expect("written");
    let at = SystemTime::now() + Duration::from_secs(later);
    let file = File::options().write(true).open(path).expect("opened");
    file.set_modified(at).expect("stamped");
}

/// Looks at `cx`'s scene files at once, without waiting a [`LOOK`].
pub(crate) fn look_now(cx: &mut Context, stack: &mut Stack) {
    cx.scenes.looked = None;
    look(cx, stack);
}

/// Writes a project's manifest in `dir`: a game whose start scene `scenes/main.ron` holds `count`
/// entities with transforms, saved from a world of their own. Returns that world and their IDs.
pub(crate) fn project(dir: &Path, count: usize) -> (World, Vec<EntityId>) {
    fs::create_dir_all(dir.join("scenes")).expect("a directory");
    let project = gg_api::Project {
        game: "tested".to_owned(),
        assets: ".".to_owned(),
        scenes: vec!["scenes".to_owned()],
        start: "scenes/main.ron".to_owned(),
        relay: None,
        icon: None,
    };
    fs::write(dir.join(gg_api::Project::FILE), project.text()).expect("written");
    let mut file = World::seeded(1);
    let roots: Vec<Entity> = (0..count)
        .map(|_| file.spawn(Transform::IDENTITY))
        .collect();
    let text = file.save_scene(&roots).expect("saved");
    save(&dir.join("scenes").join("main.ron"), &text, 0);
    let ids = roots.iter().map(|&root| file.id(root).expect("alive"));
    let ids = ids.collect();
    (file, ids)
}

/// A context of the project in `dir`, its start scene loaded as a layer's attach does.
pub(crate) fn started(dir: &Path) -> Context {
    let mut cx = Context::seeded(2).project(dir).expect("the project reads");
    cx.load_start_scene().expect("loaded");
    cx
}

fn create() -> Operation {
    Operation::Create {
        entity: None,
        parent: None,
        components: vec![("Transform".into(), Transform::IDENTITY.to_value())],
    }
}

fn set_x(id: EntityId, x: f32) -> Operation {
    Operation::Set {
        entity: id,
        component: "Transform".into(),
        path: "translation.x".to_owned(),
        value: Value::F64(f64::from(x)),
    }
}

/// Applies `edit`, a client's, now through `stack`, as the tool API does; returns what it did.
fn client(cx: &mut Context, stack: &mut Stack, edit: Edit) -> Done {
    match stack.offer(cx, edit) {
        Offered::Now(done) => done.unwrap_or_else(|error| panic!("{error:#}")),
        Offered::Later(_) => panic!("applied now, outside a lockstep session"),
    }
}

/// Applies `operations` as client `author`'s edit ([`client`]); returns the IDs it made.
fn apply(
    cx: &mut Context,
    stack: &mut Stack,
    author: &str,
    operations: Vec<Operation>,
) -> Vec<EntityId> {
    let author = author.to_owned();
    match client(cx, stack, Edit::Apply { author, operations }) {
        Done::Applied(made) => made,
        Done::Stepped(_) => panic!("an apply applies"),
    }
}

fn owned_ids(cx: &Context) -> Vec<EntityId> {
    let owned = open_scene::owned(cx.world(), &cx.scenes.files, &cx.scenes.open);
    owned
        .iter()
        .map(|&entity| cx.world().id(entity).expect("alive"))
        .collect()
}

fn x(world: &World, id: EntityId) -> f64 {
    let entity = world.entity(id).expect("alive");
    world
        .get::<Transform>(entity)
        .expect("a transform")
        .translation
        .x
}

fn authors(cx: &Context) -> Vec<String> {
    let logs = cx.authors.logs(cx.world());
    logs.map(|(author, _)| author.to_owned()).collect()
}

fn sorted(mut ids: Vec<EntityId>) -> Vec<EntityId> {
    ids.sort_unstable();
    ids
}

/// A project's start scene loads as a watched scene file, its edits' author its path from the
/// asset root, and is the open scene; a game that isn't a project's has none.
#[test]
fn the_start_scene_loads_as_a_scene_file_of_its_project() {
    let dir = Scratch::new("start-scene");
    fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
    let project = gg_api::Project {
        game: "tested".to_owned(),
        assets: ".".to_owned(),
        scenes: vec!["scenes".to_owned()],
        start: "scenes/main.ron".to_owned(),
        relay: None,
        icon: None,
    };
    fs::write(dir.0.join(gg_api::Project::FILE), project.text()).expect("written");
    let mut file = crate::World::seeded(1);
    let a = file.spawn(Transform::IDENTITY);
    let path = dir.0.join("scenes").join("main.ron");
    fs::write(&path, file.save_scene(&[a]).expect("saved")).expect("written");
    let mut cx = Context::seeded(2)
        .project(&dir.0)
        .expect("the project reads");
    assert_eq!(cx.root.as_deref(), Some(dir.0.as_path()));
    assert_eq!(cx.load_start_scene().expect("loaded").len(), 1);
    let files = &cx.scenes.files;
    assert_eq!(files.len(), 1);
    assert_eq!(
        (&files[0].path, files[0].author.as_str()),
        (&path, "scenes/main.ron")
    );
    let open = cx.scenes.open.author.as_deref();
    assert_eq!(open, Some("scenes/main.ron"));
    let none = Context::seeded(3).load_start_scene().err();
    assert_eq!(none.map(|error| error.code()), Some("engine.no_project"));
}

/// A saved scene file applies as an edit of its own, which an undo of its author takes back. One
/// that doesn't load changes nothing; the next save is compared with the file as last applied.
#[test]
fn a_scene_file_saved_applies_as_an_edit_of_its_own() {
    let dir = Scratch::new("scene-files");
    fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
    let path = dir.0.join("scenes").join("level.ron");
    let mut file = crate::World::seeded(1);
    let a = file.spawn(Transform::IDENTITY);
    let old = file.save_scene(&[a]).expect("saved");
    save(&path, &old, 0);
    let mut cx = Context::seeded(2).root(&dir.0);
    let roots = cx.load_scene_file(&path).expect("loaded");
    assert_eq!(roots.len(), 1);
    let mut stack = Stack::default();
    let look = |cx: &mut Context, stack: &mut Stack| {
        cx.scenes.looked = None;
        look(cx, stack);
    };
    // A first look sees it as loaded.
    look(&mut cx, &mut stack);
    assert_eq!(cx.authors.logs(cx.world()).count(), 0);
    let moved = Transform {
        translation: DVec3::new(1.0, 2.0, 3.0),
        ..Transform::IDENTITY
    };
    file.insert(a, moved).expect("alive");
    let new = file.save_scene(&[a]).expect("saved");
    save(&path, &new, 2);
    look(&mut cx, &mut stack);
    let at = |cx: &Context| *cx.world().get::<Transform>(roots[0]).expect("one");
    assert_eq!(at(&cx), moved);
    let logs: Vec<&str> = cx
        .authors
        .logs(cx.world())
        .map(|(author, _)| author)
        .collect();
    assert_eq!(logs, ["scenes/level.ron"]);
    assert_eq!(cx.scenes.files[0].text, new);
    let undo = Edit::Undo {
        author: "scenes/level.ron".to_owned(),
    };
    stack.edit(&mut cx, undo).expect("undone");
    assert_eq!(at(&cx), Transform::IDENTITY);
    // Broken, it changes nothing; fixed, it's compared with the file as last applied, which
    // the undo didn't change, so the world stays as it is but for what the save changes.
    save(&path, "(entities: {", 4);
    look(&mut cx, &mut stack);
    assert_eq!(at(&cx), Transform::IDENTITY);
    assert_eq!(cx.scenes.files[0].text, new);
    let turned = Transform {
        scale: Vec3::splat(2.0),
        ..moved
    };
    file.insert(a, turned).expect("alive");
    save(&path, &file.save_scene(&[a]).expect("saved"), 6);
    look(&mut cx, &mut stack);
    assert_eq!(
        at(&cx),
        Transform {
            scale: Vec3::splat(2.0),
            ..Transform::IDENTITY
        }
    );
}

/// A scene file saved while a copy plays applies to the copy, and after `stop` to the world as
/// edited too (which holds the file as last applied there), as the file's own edit.
#[test]
fn a_scene_file_saved_while_a_copy_plays_applies_to_both_worlds() {
    let dir = Scratch::new("scene-files-play");
    fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
    let path = dir.0.join("scenes").join("level.ron");
    let mut file = crate::World::seeded(1);
    let a = file.spawn(Transform::IDENTITY);
    save(&path, &file.save_scene(&[a]).expect("saved"), 0);
    let mut cx = Context::seeded(2).root(&dir.0);
    let roots = cx.load_scene_file(&path).expect("loaded");
    let mut stack = Stack::default();
    let look = |cx: &mut Context, stack: &mut Stack| {
        cx.scenes.looked = None;
        look(cx, stack);
    };
    look(&mut cx, &mut stack);
    stack.play(&mut cx, 0);
    let moved = Transform {
        translation: DVec3::new(1.0, 2.0, 3.0),
        ..Transform::IDENTITY
    };
    file.insert(a, moved).expect("alive");
    save(&path, &file.save_scene(&[a]).expect("saved"), 2);
    look(&mut cx, &mut stack);
    let at = |cx: &Context| *cx.world().get::<Transform>(roots[0]).expect("one");
    assert_eq!(at(&cx), moved, "the copy");
    assert_eq!(stack.stop(&mut cx), Some(0));
    assert_eq!(
        at(&cx),
        Transform::IDENTITY,
        "the world as edited, as it was"
    );
    look(&mut cx, &mut stack);
    assert_eq!(at(&cx), moved, "the world as edited");
    let logs = cx.authors.logs(cx.world());
    let authors: Vec<&str> = logs.map(|(author, _)| author).collect();
    assert_eq!(authors, ["scenes/level.ron"]);
}

/// In a lockstep session, whose saves apply at a tick to come, a file put back before its save
/// applies is a save too, and the world ends as the file does.
#[test]
fn a_file_put_back_before_its_save_applies_applies_as_put_back() {
    let dir = Scratch::new("scene-files-lockstep");
    fs::create_dir_all(dir.0.join("scenes")).expect("a directory");
    let path = dir.0.join("scenes").join("level.ron");
    let mut file = crate::World::seeded(1);
    let a = file.spawn(Transform::IDENTITY);
    let old = file.save_scene(&[a]).expect("saved");
    save(&path, &old, 0);
    let mut cx = Context::seeded(2).root(&dir.0);
    let roots = cx.load_scene_file(&path).expect("loaded");
    let mut stack = Stack::default();
    let net = Net::Host(crate::args::Listen::at("127.0.0.1:0"));
    let code = crate::net::TESTED;
    stack.lockstep(Lockstep::start(&net, code, "Tested", crate::net::BUILD, &[]).expect("hosting"));
    let look = |cx: &mut Context, stack: &mut Stack| {
        cx.scenes.looked = None;
        look(cx, stack);
    };
    look(&mut cx, &mut stack);
    let moved = Transform {
        translation: DVec3::new(1.0, 2.0, 3.0),
        ..Transform::IDENTITY
    };
    file.insert(a, moved).expect("alive");
    save(&path, &file.save_scene(&[a]).expect("saved"), 2);
    look(&mut cx, &mut stack);
    save(&path, &old, 4);
    look(&mut cx, &mut stack);
    for number in 0..=DELAY {
        assert!(stack.tick(&mut cx, Tick { number }));
    }
    assert_eq!(
        *cx.world().get::<Transform>(roots[0]).expect("one"),
        Transform::IDENTITY
    );
    assert_eq!(cx.scenes.files[0].text, old);
}

/// The open scene owns its file's entities and those clients make into it, not the game's, nor
/// another scene file's. An entity its file's save deleted stays owned; an undo restores it.
#[test]
fn the_open_scene_owns_its_files_entities_and_what_clients_make_into_it() {
    let dir = Scratch::new("open-scene-owns");
    let (mut file, ids) = project(&dir.0, 2);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    let other = dir.0.join("scenes").join("other.ron");
    let mut theirs = World::seeded(3);
    let c = theirs.spawn(Transform::IDENTITY);
    save(&other, &theirs.save_scene(&[c]).expect("saved"), 0);
    cx.load_scene_file(&other).expect("loaded");
    cx.world_mut().spawn(Transform::IDENTITY);
    let made = apply(&mut cx, &mut stack, "agent", vec![create()]);
    let mut owns = sorted([&ids[..], &made[..]].concat());
    assert_eq!(owned_ids(&cx), owns);
    assert_eq!(cx.scenes.open.made, made);
    look_now(&mut cx, &mut stack);
    theirs.spawn(Transform::IDENTITY);
    let all: Vec<Entity> = theirs.query::<Entity>().collect();
    save(&other, &theirs.save_scene(&all).expect("saved"), 2);
    look_now(&mut cx, &mut stack);
    assert_eq!(
        cx.world().query::<&Transform>().count(),
        6,
        "the other file's save applied"
    );
    assert_eq!(owned_ids(&cx), owns, "nor its create");
    let [a, b] = [ids[0], ids[1]].map(|id| file.entity(id).expect("alive"));
    file.despawn(b).expect("alive");
    save(
        &dir.0.join("scenes").join("main.ron"),
        &file.save_scene(&[a]).expect("saved"),
        4,
    );
    look_now(&mut cx, &mut stack);
    owns.retain(|&id| id != ids[1]);
    assert_eq!(owned_ids(&cx), owns, "b deleted by its file's save");
    let undo = Edit::Undo {
        author: "scenes/main.ron".to_owned(),
    };
    assert!(matches!(
        client(&mut cx, &mut stack, undo),
        Done::Stepped(Some(_))
    ));
    assert_eq!(owned_ids(&cx), sorted([&ids[..], &made[..]].concat()));
}

/// A file save that deletes an entity deletes its child and a game-made child. An undo of the
/// file's author restores all three: the scene owns the two its file held, not the game's, and a
/// save writes them as they were, without it.
#[test]
fn an_undo_of_its_files_save_brings_back_what_it_deleted_owned() {
    let dir = Scratch::new("open-scene-undo-save");
    let (mut file, ids) = project(&dir.0, 1);
    let b = file.entity(ids[0]).expect("alive");
    let [a, a1] = [0, 1].map(|_| file.spawn(Transform::IDENTITY));
    file.set_parent(a1, Some(a)).expect("no cycle");
    let main = dir.0.join("scenes").join("main.ron");
    save(&main, &file.save_scene(&[a, b]).expect("saved"), 0);
    let [a_id, a1_id] = [a, a1].map(|entity| file.id(entity).expect("alive"));
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    look_now(&mut cx, &mut stack);
    let world = cx.world_mut();
    let game = world.spawn(Transform::IDENTITY);
    let a_in = world.entity(a_id).expect("loaded");
    world.set_parent(game, Some(a_in)).expect("no cycle");
    let game = world.id(game).expect("alive");
    save(&main, &file.save_scene(&[b]).expect("saved"), 2);
    look_now(&mut cx, &mut stack);
    for id in [a_id, a1_id, game] {
        assert!(cx.world().entity(id).is_none(), "{id} deleted");
    }
    assert_eq!(owned_ids(&cx), [ids[0]]);
    let undo = Edit::Undo {
        author: "scenes/main.ron".to_owned(),
    };
    client(&mut cx, &mut stack, undo);
    for id in [a_id, a1_id, game] {
        assert!(cx.world().entity(id).is_some(), "{id} back");
    }
    assert_eq!(owned_ids(&cx), sorted(vec![ids[0], a_id, a1_id]));
    open_scene::save(&mut cx, None).expect("saved");
    let mut loaded = World::seeded(5);
    loaded.register::<Transform>().expect("registered");
    loaded
        .load_scene(&crate::ecs::read_scene(&main).expect("read"))
        .expect("loaded");
    let [a_at, a1_at] = [a_id, a1_id].map(|id| loaded.entity(id).expect("saved"));
    assert_eq!(loaded.parent(a1_at).expect("alive"), Some(a_at));
    assert!(loaded.entity(game).is_none(), "the game's");
}

/// An entity a client deleted, which its file's save then took out of the text, comes back owned
/// when the client undoes the delete, so a save doesn't leave it out.
#[test]
fn what_its_files_save_takes_out_while_gone_comes_back_owned() {
    let dir = Scratch::new("open-scene-undo-gone");
    let (file, ids) = project(&dir.0, 1);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    look_now(&mut cx, &mut stack);
    let delete = Operation::Delete { entity: ids[0] };
    apply(&mut cx, &mut stack, "agent", vec![delete]);
    let main = dir.0.join("scenes").join("main.ron");
    save(&main, &file.save_scene(&[]).expect("saved"), 2);
    look_now(&mut cx, &mut stack);
    let undo = Edit::Undo {
        author: "agent".to_owned(),
    };
    client(&mut cx, &mut stack, undo);
    assert!(cx.world().entity(ids[0]).is_some(), "back");
    assert_eq!(owned_ids(&cx), [ids[0]]);
}

/// A save writes exactly the entities the open scene owns to its file, leaving out a game-made
/// child, and changes no world. The watch's next look applies nothing of it, and nothing is
/// unsaved after. A gone owned entity stays owned; an undo restores it.
#[test]
fn a_save_writes_what_the_open_scene_owns_which_the_watch_applies_nothing_of() {
    let logged = crate::testing::logged();
    let dir = Scratch::new("open-scene-save");
    let (_, ids) = project(&dir.0, 1);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    let a = cx.world().entity(ids[0]).expect("loaded");
    let world = cx.world_mut();
    let game = world.spawn(Transform::IDENTITY);
    let kid = world.spawn(Transform::IDENTITY);
    world.set_parent(kid, Some(a)).expect("no cycle");
    let made = apply(
        &mut cx,
        &mut stack,
        "agent",
        vec![create(), set_x(ids[0], 5.0)],
    );
    assert!(open_scene::unsaved(&mut cx, 0));
    let hash = cx.world().hash();
    open_scene::save(&mut cx, None).expect("saved");
    assert_eq!(cx.world().hash(), hash, "no world changed");
    let path = dir.0.join("scenes").join("main.ron");
    let text = crate::ecs::read_scene(&path).expect("read");
    let d = cx.world().entity(made[0]).expect("alive");
    assert_eq!(text, cx.world().save_entities(&[a, d]).expect("saved"));
    assert_eq!(cx.scenes.files[0].text, text);
    let mut loaded = World::seeded(5);
    loaded.register::<Transform>().expect("registered");
    loaded.load_scene(&text).expect("loaded");
    assert_eq!(
        (loaded.query::<Entity>().count(), x(&loaded, ids[0])),
        (2, 5.0)
    );
    for left in [game, kid] {
        let id = cx.world().id(left).expect("alive");
        assert!(loaded.entity(id).is_none(), "{id}, the game's");
    }
    assert!(!open_scene::unsaved(&mut cx, 0));
    look_now(&mut cx, &mut stack);
    assert_eq!(authors(&cx), ["agent"], "no edit of the file's");
    let codes: Vec<String> = logged.codes.try_iter().collect();
    assert!(codes.iter().any(|code| code == "scene.saved"), "{codes:?}");
    assert!(
        !codes.iter().any(|code| code == "scene.applied"),
        "{codes:?}"
    );
    // d gone, its file's save leaves it out but owns it still, and an undo brings it back.
    apply(
        &mut cx,
        &mut stack,
        "agent",
        vec![Operation::Delete { entity: made[0] }],
    );
    open_scene::save(&mut cx, None).expect("saved");
    assert_eq!(cx.scenes.open.made, made);
    assert!(!open_scene::unsaved(&mut cx, 0));
    let undo = Edit::Undo {
        author: "agent".to_owned(),
    };
    client(&mut cx, &mut stack, undo);
    assert_eq!(owned_ids(&cx), sorted([ids[0], made[0]].to_vec()));
    assert!(open_scene::unsaved(&mut cx, 0));
}

/// A save is refused, saving nothing, for a file the watch hasn't applied, an untitled scene
/// without a path, and another scene file's path. To a path, it saves there; that file is open
/// from then on and the old one unwatched.
#[test]
fn a_save_is_refused_what_would_lose_an_edit_and_saves_as_another_file() {
    let dir = Scratch::new("open-scene-save-as");
    let (mut file, ids) = project(&dir.0, 1);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    let main = dir.0.join("scenes").join("main.ron");
    let other = dir.0.join("scenes").join("other.ron");
    save(&other, "(entities: {})\n", 0);
    cx.load_scene_file(&other).expect("loaded");
    look_now(&mut cx, &mut stack);
    // An agent's edit to the file, which the watch hasn't looked at.
    let a = file.entity(ids[0]).expect("alive");
    file.insert(
        a,
        Transform {
            translation: DVec3::X,
            ..Transform::IDENTITY
        },
    )
    .expect("alive");
    let edited = file.save_scene(&[a]).expect("saved");
    save(&main, &edited, 2);
    let refused = open_scene::save(&mut cx, None).expect_err("an edit unapplied");
    assert_eq!(refused.code(), "scene.changed", "{refused}");
    assert!(
        refused.to_string().contains(&main.display().to_string()),
        "{refused}"
    );
    assert_eq!(crate::ecs::read_scene(&main).expect("read"), edited, "kept");
    look_now(&mut cx, &mut stack);
    assert_eq!(x(cx.world(), ids[0]), 1.0);
    let resolve = |cx: &Context, path: &str| open_scene::resolve(cx, Path::new(path), false);
    let to = resolve(&cx, "scenes/other.ron").expect("in the scenes folder");
    let watched = open_scene::save(&mut cx, Some(to)).expect_err("another's");
    assert_eq!(watched.code(), "scene.watched", "{watched}");
    let to = resolve(&cx, "scenes/copy.ron").expect("in the scenes folder");
    open_scene::save(&mut cx, Some(to)).expect("saved as");
    let copy = dir.0.join("scenes").join("copy.ron");
    assert_eq!(crate::ecs::read_scene(&copy).expect("read"), edited);
    assert_eq!(open_scene::title(&cx), Some("scenes/copy.ron"));
    let watching: Vec<&str> = cx
        .scenes
        .files
        .iter()
        .map(|file| file.author.as_str())
        .collect();
    assert_eq!(watching, ["scenes/copy.ron", "scenes/other.ron"]);
    assert!(!open_scene::unsaved(&mut cx, 0));
    // An untitled scene saves to a path alone.
    open_scene::new(&mut cx, &mut stack).expect("emptied");
    let untitled = open_scene::save(&mut cx, None).expect_err("untitled");
    assert_eq!(untitled.code(), "scene.untitled", "{untitled}");
}

/// While a copy plays, a save writes the world as edited, held aside, and the copy's scene files
/// follow; neither world applies it, then or once `stop` brings the world as edited back.
#[test]
fn a_save_while_a_copy_plays_writes_the_world_as_edited() {
    let dir = Scratch::new("open-scene-save-playing");
    let (_, ids) = project(&dir.0, 1);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    look_now(&mut cx, &mut stack);
    apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 3.0)]);
    stack.play(&mut cx, 0);
    apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 7.0)]);
    open_scene::save(&mut cx, None).expect("saved");
    let text = crate::ecs::read_scene(&dir.0.join("scenes").join("main.ron")).expect("read");
    let mut loaded = World::seeded(5);
    loaded.register::<Transform>().expect("registered");
    loaded.load_scene(&text).expect("loaded");
    assert_eq!(x(&loaded, ids[0]), 3.0, "the world as edited's");
    assert_eq!(cx.scenes.files[0].text, text, "the copy's file follows");
    look_now(&mut cx, &mut stack);
    assert_eq!(x(cx.world(), ids[0]), 7.0, "the copy, as it was");
    assert_eq!(stack.stop(&mut cx), Some(0));
    look_now(&mut cx, &mut stack);
    assert_eq!(x(cx.world(), ids[0]), 3.0);
    assert_eq!(authors(&cx), ["agent"], "no edit of the file's");
}

/// New is refused while the open scene has unsaved edits, unless discarded. It deletes the
/// scene's entities, keeping the game's own (a child of theirs becomes a root), and leaves the
/// scene untitled, its file unwatched and unchanged. Every author's history starts afresh, and
/// the layers are told. Untitled, the scene is unsaved once it owns an entity.
#[test]
fn new_empties_the_open_scene_and_starts_history_afresh() {
    let dir = Scratch::new("open-scene-new");
    let (_, ids) = project(&dir.0, 2);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    let restored = std::rc::Rc::new(std::cell::Cell::new(0));
    stack.push_layer(Counted(restored.clone()));
    let a = cx.world().entity(ids[0]).expect("loaded");
    let world = cx.world_mut();
    let game = world.spawn(Transform::IDENTITY);
    let kid = world.spawn(Transform::IDENTITY);
    world.set_parent(kid, Some(a)).expect("no cycle");
    let made = apply(&mut cx, &mut stack, "agent", vec![create()]);
    let refused = open_scene::check_unsaved(&mut cx, 0, false).expect_err("unsaved");
    assert_eq!(refused.code(), "scene.unsaved", "{refused}");
    open_scene::check_unsaved(&mut cx, 0, true).expect("discarded");
    let main = dir.0.join("scenes").join("main.ron");
    let text = crate::ecs::read_scene(&main).expect("read");
    let serial = cx.world().serial();
    open_scene::new(&mut cx, &mut stack).expect("emptied");
    let world = cx.world();
    for id in [ids[0], ids[1], made[0]] {
        assert!(world.entity(id).is_none(), "{id}, the scene's");
    }
    assert!(world.contains(game) && world.contains(kid), "the game's");
    assert_eq!(world.parent(kid).expect("alive"), None);
    assert_ne!(world.serial(), serial);
    assert!(authors(&cx).is_empty());
    assert_eq!(restored.get(), 1);
    let undo = Edit::Undo {
        author: "agent".to_owned(),
    };
    assert_eq!(client(&mut cx, &mut stack, undo), Done::Stepped(None));
    assert_eq!((open_scene::title(&cx), cx.scenes.files.len()), (None, 0));
    assert_eq!(
        crate::ecs::read_scene(&main).expect("read"),
        text,
        "on disk as it was"
    );
    assert!(!open_scene::unsaved(&mut cx, 0));
    apply(&mut cx, &mut stack, "agent", vec![create()]);
    assert!(open_scene::unsaved(&mut cx, 0));
}

/// Counts the calls of its `restored`.
struct Counted(std::rc::Rc<std::cell::Cell<u32>>);

impl crate::Layer for Counted {
    fn restored(&mut self, _: &mut Context) {
        self.0.set(self.0.get() + 1);
    }
}

/// Open is refused, changing nothing, for undiscarded unsaved edits, another scene file's path, a
/// file that doesn't load, and one giving a game entity's ID. Otherwise it replaces the open
/// scene's entities with the file's, keeping the game's own; the file is watched from then on,
/// the old one not, and history starts afresh. Opening the open scene's file again reverts it.
#[test]
fn open_puts_a_files_entities_in_place_of_the_open_scenes() {
    let dir = Scratch::new("open-scene-open");
    let (_, ids) = project(&dir.0, 1);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    let scenes = dir.0.join("scenes");
    let mut level = World::seeded(4);
    let [p, q] = [0, 1].map(|_| level.spawn(Transform::IDENTITY));
    level.set_parent(q, Some(p)).expect("no cycle");
    save(
        &scenes.join("level2.ron"),
        &level.save_scene(&[p]).expect("saved"),
        0,
    );
    let [p, q] = [p, q].map(|entity| level.id(entity).expect("alive"));
    let other = scenes.join("other.ron");
    save(&other, "(entities: {})\n", 0);
    cx.load_scene_file(&other).expect("loaded");
    let game = cx.world_mut().spawn(Transform::IDENTITY);
    let game_id = cx.world().id(game).expect("alive");
    save(&scenes.join("broken.ron"), "(entities: {", 0);
    let taken = format!("(entities: {{\"{game_id}\": ()}})\n");
    save(&scenes.join("taken.ron"), &taken, 0);
    apply(&mut cx, &mut stack, "agent", vec![create()]);
    let resolve = |cx: &Context, path: &str| open_scene::resolve(cx, Path::new(path), true);
    let (hash, edits) = (cx.world().hash(), cx.authors.logs(cx.world()).count());
    for (path, discard, code) in [
        ("scenes/level2.ron", false, "scene.unsaved"),
        ("scenes/other.ron", true, "scene.watched"),
        ("scenes/broken.ron", true, "scene.syntax"),
        ("scenes/taken.ron", true, "ecs.id_taken"),
    ] {
        let file = resolve(&cx, path).expect("in the scenes folder");
        let refused = open_scene::check_open(&mut cx, file, 0, discard).err();
        assert_eq!(refused.map(|error| error.code()), Some(code), "{path}");
        assert_eq!(cx.world().hash(), hash, "{path}");
        assert_eq!(cx.authors.logs(cx.world()).count(), edits, "{path}");
    }
    let file = resolve(&cx, "scenes/level2.ron").expect("in the scenes folder");
    let opening = open_scene::check_open(&mut cx, file, 0, true).expect("opens");
    open_scene::open(&mut cx, &mut stack, opening).expect("opened");
    let world = cx.world();
    assert!(world.entity(ids[0]).is_none() && world.contains(game));
    let q_in = world.entity(q).expect("made");
    assert_eq!(world.parent(q_in).expect("alive"), world.entity(p));
    assert_eq!(open_scene::title(&cx), Some("scenes/level2.ron"));
    let watching: Vec<&str> = cx
        .scenes
        .files
        .iter()
        .map(|file| file.author.as_str())
        .collect();
    assert_eq!(watching, ["scenes/other.ron", "scenes/level2.ron"]);
    assert!(authors(&cx).is_empty());
    assert!(!open_scene::unsaved(&mut cx, 0));
    // Watched: its save applies, as its own edit.
    look_now(&mut cx, &mut stack);
    let moved = Transform {
        translation: DVec3::X,
        ..Transform::IDENTITY
    };
    level
        .insert(level.entity(p).expect("alive"), moved)
        .expect("alive");
    let roots = [level.entity(p).expect("alive")];
    save(
        &scenes.join("level2.ron"),
        &level.save_scene(&roots).expect("saved"),
        2,
    );
    look_now(&mut cx, &mut stack);
    assert_eq!(
        (x(cx.world(), p), authors(&cx)),
        (1.0, vec!["scenes/level2.ron".to_owned()])
    );
    // Opened again, it reverts.
    apply(&mut cx, &mut stack, "agent", vec![set_x(q, 9.0)]);
    let file = resolve(&cx, "scenes/level2.ron").expect("in the scenes folder");
    let opening = open_scene::check_open(&mut cx, file, 0, true).expect("opens");
    open_scene::open(&mut cx, &mut stack, opening).expect("opened");
    assert_eq!((x(cx.world(), p), x(cx.world(), q)), (1.0, 0.0));
}

/// Open and new delete a model root with its nodes, which scenes don't save, and whatever code
/// put under them, as a change of its model does. Its file, opened again, respawns the nodes
/// under the same IDs; a file may give an entity a node's ID, which the node frees as it goes.
#[test]
fn open_and_new_take_a_models_nodes_with_its_root() {
    let dir = Scratch::new("open-scene-model");
    let (_, ids) = project(&dir.0, 1);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    let (model, _) = crate::model::tests::added(&mut cx, "m", crate::model::tests::imported());
    // First in ID order, so that its delete comes before those of what the scene owns.
    let root_id = EntityId::new(1).expect("not zero");
    let create = Operation::Create {
        entity: Some(root_id),
        parent: None,
        components: vec![("ModelRoot".into(), ModelRoot(model).to_value())],
    };
    apply(&mut cx, &mut stack, "agent", vec![create]);
    open_scene::save(&mut cx, None).expect("saved");
    let nodes = |world: &World| {
        let nodes = world.query::<(Entity, &ModelNode)>();
        let ids = nodes.map(|(node, _)| world.id(node).expect("alive"));
        sorted(ids.collect())
    };
    let spawned = nodes(cx.world());
    assert_eq!(spawned.len(), 4);
    let world = cx.world_mut();
    let node = world.entity(spawned[0]).expect("alive");
    let a = world.entity(ids[0]).expect("loaded");
    world.set_parent(a, Some(node)).expect("code may");
    let held = world.spawn(Transform::IDENTITY);
    world.set_parent(held, Some(node)).expect("code may");

    let open = |cx: &mut Context, stack: &mut Stack, path: &str| {
        let file = open_scene::resolve(cx, Path::new(path), true);
        let file = file.expect("in the scenes folder");
        let opening = open_scene::check_open(cx, file, 0, true).expect("opens");
        open_scene::open(cx, stack, opening).expect("opened");
    };
    open(&mut cx, &mut stack, "scenes/main.ron");
    let world = cx.world();
    assert_eq!(nodes(world), spawned, "spawned again, under the same IDs");
    let a = world.entity(ids[0]).expect("the file's");
    assert_eq!(world.parent(a).expect("alive"), None, "as its file has it");
    assert!(!world.contains(held), "gone with the node it was under");
    assert!(!open_scene::unsaved(&mut cx, 0));

    let node = format!("(entities: {{\"{}\": ()}})\n", spawned[0]);
    save(&dir.0.join("scenes").join("node.ron"), &node, 0);
    open(&mut cx, &mut stack, "scenes/node.ron");
    let world = cx.world();
    let entity = world.entity(spawned[0]).expect("the file's");
    assert!(world.get::<ModelNode>(entity).is_none());
    assert_eq!((world.entity(root_id), nodes(world)), (None, vec![]));
    open(&mut cx, &mut stack, "scenes/main.ron");

    open_scene::new(&mut cx, &mut stack).expect("emptied");
    let world = cx.world();
    assert_eq!((world.entity(root_id), world.entity(ids[0])), (None, None));
    assert_eq!(nodes(world), []);
}

/// A path names a scene file from the asset root, relative or absolute, in its folder's own
/// form. Refused: a path outside the scenes folders, a file to open that isn't there, a
/// directory to save in that isn't, and any path in a game that isn't a project's.
#[test]
fn a_path_names_a_scene_file_in_the_projects_scenes_folders() {
    let dir = Scratch::new("open-scene-paths");
    project(&dir.0, 1);
    let cx = started(&dir.0);
    let main = dir.0.join("scenes").join("main.ron");
    let resolve = |path: &Path, existing| open_scene::resolve(&cx, path, existing);
    let found = (main.clone(), "scenes/main.ron".to_owned());
    assert_eq!(
        resolve(Path::new("scenes/main.ron"), true).expect("found"),
        found
    );
    assert_eq!(resolve(&main, true).expect("found"), found);
    let dotted = dir
        .0
        .join("scenes")
        .join("..")
        .join("scenes")
        .join("new.ron");
    let new = (
        dir.0.join("scenes").join("new.ron"),
        "scenes/new.ron".to_owned(),
    );
    assert_eq!(resolve(&dotted, false).expect("to save as"), new);
    fs::write(dir.0.join("outside.ron"), "(entities: {})\n").expect("written");
    for (path, existing, code) in [
        ("outside.ron", true, "scene.bad_path"),
        ("scenes/../outside.ron", false, "scene.bad_path"),
        ("scenes/none.ron", true, "scene.file"),
        ("scenes/none/new.ron", false, "scene.file"),
    ] {
        let refused = resolve(Path::new(path), existing).err();
        assert_eq!(refused.map(|error| error.code()), Some(code), "{path}");
    }
    let none = open_scene::resolve(&Context::seeded(3), &main, true).err();
    assert_eq!(none.map(|error| error.code()), Some("engine.no_project"));
}

/// A playing copy has a copy of the open scene; what clients make into it goes with it at
/// `stop`. A snapshot keeps the open scene of the run's world and the held edited world; one
/// written before open scenes existed opens the start scene's file.
#[test]
fn the_open_scene_lasts_across_play_stop_and_a_snapshots_restore() {
    let dir = Scratch::new("open-scene-kept");
    project(&dir.0, 1);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    let made = apply(&mut cx, &mut stack, "agent", vec![create()]);
    let edited = cx.scenes.open.clone();
    stack.play(&mut cx, 0);
    assert_eq!(cx.scenes.open, edited);
    let copied = apply(&mut cx, &mut stack, "agent", vec![create()]);
    let played = cx.scenes.open.clone();
    assert_eq!(played.made, sorted([&made[..], &copied[..]].concat()));
    let run = Run::Held {
        world: Box::new(cx.dev.edited.as_ref().expect("held").world.clone()),
        scenes: cx.scenes.files.clone(),
        open: edited.clone(),
        ticks: 0,
    };
    let of = snapshot::Of {
        world: cx.world(),
        scenes: &cx.scenes.files,
        cached: &cx.scenes.cached,
        open: &played,
        scripts: &[],
        mods: &[],
        seen: None,
    };
    let text = snapshot::write(&of, &run, "Tested", &[], 3).expect("written");
    assert_eq!(stack.stop(&mut cx), Some(0));
    assert_eq!(cx.scenes.open, edited);
    let restore = |text: &str| {
        let mut restored = Context::seeded(6)
            .project(&dir.0)
            .expect("the project reads");
        let snapshot = Snapshot::parse(text.to_owned(), "kept".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");
        let restoring = Stack::default().restore(&mut restored, &snapshot, true);
        restoring.expect("restored");
        restored
    };
    let restored = restore(&text);
    assert_eq!(restored.scenes.open, played);
    assert_eq!(
        restored.dev.edited.as_ref().map(|held| &held.open),
        Some(&edited)
    );
    let older: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with("open "))
        .collect();
    let restored = restore(&format!("{}\n", older.join("\n")));
    let start = Open {
        author: Some("scenes/main.ron".to_owned()),
        made: Vec::new(),
        edited: false,
    };
    assert_eq!(restored.scenes.open, start);
    assert_eq!(
        restored.dev.edited.as_ref().map(|held| &held.open),
        Some(&start)
    );
}

/// Unsaved is compared in canonical form, so a hand-written file isn't unsaved as loaded; it's
/// found again after an edit or another tick and kept otherwise; and a file whose text as last
/// applied no longer loads is unsaved.
#[test]
fn unsaved_compares_canonically_and_is_kept_until_the_world_as_edited_changes() {
    let dir = Scratch::new("open-scene-unsaved");
    let (_, ids) = project(&dir.0, 1);
    let main = dir.0.join("scenes").join("main.ron");
    let id = ids[0];
    let transform = "Transform: (scale: (1, 1, 1), rotation: (0, 0, 0, 1), translation: (0, 0, 0))";
    let loose = format!("// Written by hand.\n(entities: {{\"{id}\": ({transform})}})");
    save(&main, &loose, 0);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    assert!(!open_scene::unsaved(&mut cx, 0), "as loaded");
    apply(&mut cx, &mut stack, "agent", vec![set_x(id, 1.0)]);
    assert!(open_scene::unsaved(&mut cx, 0), "found again after an edit");
    let entity = cx.world().entity(id).expect("alive");
    cx.world_mut()
        .insert(entity, Transform::IDENTITY)
        .expect("alive");
    assert!(open_scene::unsaved(&mut cx, 0), "kept");
    assert!(
        !open_scene::unsaved(&mut cx, 1),
        "found again at another tick"
    );
    cx.scenes.files[0].text = format!("(entities: {{\"{id}\": (Gone: ())}})\n");
    cx.scenes.changed();
    assert!(
        open_scene::unsaved(&mut cx, 1),
        "a text that no longer loads"
    );
}
/// A tick's change makes the open scene unsaved but not edited; an author's apply makes it
/// edited. A script reload, a renewal, another scene file's save, and a play and its stop leave
/// it as found, the copy's edits going with it. A save, the open scene's file applying, and New
/// clear it.
#[test]
fn edited_is_set_by_authors_edits_alone_and_cleared_by_saves() {
    let dir = Scratch::new("open-scene-edited");
    let (_, ids) = project(&dir.0, 1);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    assert!(!cx.scenes.open.edited, "as loaded");
    // What a tick's physics does: the world changes, with no edit.
    let entity = cx.world().entity(ids[0]).expect("alive");
    cx.world_mut()
        .insert(
            entity,
            Transform {
                translation: [3.0, 0.0, 0.0].into(),
                ..Transform::IDENTITY
            },
        )
        .expect("alive");
    cx.scenes.changed();
    assert!(open_scene::unsaved(&mut cx, 1), "unsaved");
    assert!(!cx.scenes.open.edited, "but no author's edit");
    apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 5.0)]);
    assert!(cx.scenes.open.edited, "an author's apply");
    // Edits of no author of the open scene's.
    let script = Edit::Script {
        path: "scripts/none.luau".to_owned(),
        text: "return 1".to_owned(),
    };
    let _ = cx.apply_edit(script);
    cx.apply_edit(Edit::Renew).expect("renewed");
    let other = "(entities: {\n})\n".to_owned();
    let path = dir.0.join("scenes/other.ron");
    cx.scenes.loaded(SceneFile::new(
        path,
        "scenes/other.ron".to_owned(),
        other.clone(),
    ));
    let saved = super::apply_save(
        &mut cx,
        &mut |cx, edit| cx.apply_edit(edit),
        "scenes/other.ron",
        other,
    );
    saved.expect("applied");
    assert!(cx.scenes.open.edited, "kept by others' edits");
    // A play and its stop bring back the world as edited's, whatever the copy's edits did.
    stack.play(&mut cx, 1);
    stack.stop(&mut cx);
    assert!(cx.scenes.open.edited, "kept across a play");
    open_scene::save(&mut cx, None).expect("saved");
    assert!(!cx.scenes.open.edited, "a save clears it");
    stack.play(&mut cx, 1);
    apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 7.0)]);
    assert!(
        cx.dev
            .edited
            .as_ref()
            .is_some_and(|edited| !edited.open.edited)
    );
    stack.stop(&mut cx);
    assert!(!cx.scenes.open.edited, "the copy's edit went with it");
    apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 6.0)]);
    assert!(cx.scenes.open.edited);
    let text = cx.scenes.files[0].text.clone();
    let applied = super::apply_save(
        &mut cx,
        &mut |cx, edit| cx.apply_edit(edit),
        "scenes/main.ron",
        text,
    );
    applied.expect("applied");
    assert!(!cx.scenes.open.edited, "its file applied");
    apply(&mut cx, &mut stack, "agent", vec![set_x(ids[0], 8.0)]);
    open_scene::new(&mut cx, &mut stack).expect("emptied");
    assert!(!cx.scenes.open.edited, "New clears it");
}

/// The world's own entity in [`world_project`]'s start scene, beside its `Cells`.
const OWN: u64 = 0xb0;

/// Like [`project`], but the start scene is a world of cells in `scenes/cells`, 10 units on a
/// side, with its own entity [`OWN`] at x 100 beside its `Cells`. Cells 0 to 2 along x each hold an
/// entity ([`id_of`]) at their middle, cell 100 one at x 1005. Returns the scenes folder.
fn world_project(dir: &Path) -> PathBuf {
    project(dir, 0);
    let scenes = dir.join("scenes");
    fs::create_dir_all(scenes.join("cells")).expect("a directory");
    let main = format!(
        "(\n    entities: {{\n        \"00000000000000a0\": (\n            Cells: (\n                \
             folder: \"scenes/cells\",\n                size: 10.0,\n            ),\n        ),\n{}",
        &cells_scene(&[(OWN, 100.0)])["(\n    entities: {\n".len()..]
    );
    save(&scenes.join("main.ron"), &main, 0);
    for cell in [0, 1, 2, 100] {
        let x = match cell {
            100 => 1005.0,
            cell => cell as f32 * 10.0 + 5.0,
        };
        let text = cells_scene(&[(id_of(cell).get(), x)]);
        save(&scenes.join(format!("cells/{cell}_0_0.ron")), &text, 0);
    }
    scenes
}

/// Brings in `cx`'s world's cells near x 15, as the look's viewpoint does while the run is
/// stopped: cells 0 to 2.
fn bring_cells_in(cx: &mut Context) {
    look_at(cx, 15.0);
    cx.stream_stopped();
    cx.stream_stopped();
    assert_eq!(xs(cx.world()), [0, 1, 2]);
}

fn file_ids(path: &Path) -> Vec<u64> {
    let text = crate::ecs::read_scene(path).expect("read");
    let ids = World::scene_ids(&text).expect("a scene");
    ids.into_iter().map(EntityId::get).collect()
}

/// A world's save writes each changed cell's file: each streamed root, and a duplicate of one,
/// in its cell's file without `Streamed`; the world's file without them, but for one whose
/// `Streamed` was taken away. An unchanged cell's file isn't written. Unsaved follows.
#[test]
fn a_world_s_save_writes_each_cell_s_file_as_its_entities_stand() {
    let dir = Scratch::new("open-scene-world-save");
    let scenes = world_project(&dir.0);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    // Attached, as a run's is, whose world changes between ticks through the engine alone.
    stack.attach(&mut cx).expect("loaded");
    bring_cells_in(&mut cx);
    assert!(!open_scene::unsaved(&mut cx, 0), "as loaded");
    let cell = |x: i32| scenes.join(format!("cells/{x}_0_0.ron"));
    let far = crate::ecs::read_scene(&cell(100)).expect("read");
    let operations = vec![set_x(id_of(1), 16.0), set_x(id_of(0), 25.0)];
    apply(&mut cx, &mut stack, "agent", operations);
    assert!(open_scene::unsaved(&mut cx, 0));
    open_scene::save(&mut cx, None).expect("saved");
    assert!(!open_scene::unsaved(&mut cx, 0), "saved");
    let [zero, one, two] = [0, 1, 2].map(|x| id_of(x).get());
    assert_eq!(file_ids(&cell(0)), [0; 0], "emptied");
    assert_eq!(file_ids(&cell(1)), [one]);
    assert_eq!(file_ids(&cell(2)), [zero, two], "moved in");
    assert_eq!(file_ids(&scenes.join("main.ron")), [0xa0, OWN]);
    let text = crate::ecs::read_scene(&cell(1)).expect("read");
    assert!(
        text.contains("16.0") && !text.contains("Streamed"),
        "{text}"
    );
    assert_eq!(
        crate::ecs::read_scene(&cell(100)).expect("read"),
        far,
        "out"
    );
    let duplicate = Operation::Duplicate {
        entity: id_of(1),
        copies: Vec::new(),
    };
    let copy = apply(&mut cx, &mut stack, "agent", vec![duplicate])[0].get();
    let unstreamed = Operation::Remove {
        entity: id_of(2),
        component: "Streamed".into(),
    };
    apply(&mut cx, &mut stack, "agent", vec![unstreamed]);
    open_scene::save(&mut cx, None).expect("saved");
    let mut held = vec![one, copy];
    held.sort_unstable();
    assert_eq!(file_ids(&cell(1)), held, "the duplicate in its cell's");
    assert_eq!(file_ids(&cell(2)), [zero]);
    assert_eq!(file_ids(&scenes.join("main.ron")), [two, 0xa0, OWN]);
    assert!(!open_scene::unsaved(&mut cx, 0));
    // Into cell 3, which has no file: it comes in, and the save makes its file, which holds
    // what the cell does from then on.
    apply(&mut cx, &mut stack, "agent", vec![set_x(id_of(1), 35.0)]);
    cx.stream_stopped();
    cx.stream_stopped();
    assert_eq!(xs(cx.world()), [0, 1, 2, 3]);
    open_scene::save(&mut cx, None).expect("saved");
    assert_eq!(file_ids(&cell(3)), [one]);
    assert!(!open_scene::unsaved(&mut cx, 0), "its file's");
    open_scene::save(&mut cx, None).expect("saved again");
}

/// Made cells come in around the look's viewpoint while the run is stopped and go out as they
/// came. A save writes a made cell's file only once it changed, and that file stands for it from
/// then on. A root moved into a made cell that hasn't come in is refused until it has.
#[test]
fn a_world_s_save_writes_a_made_cell_s_file_once_it_changes() {
    use crate::cells::Cell;
    use gg_core::math::IVec3;

    let dir = Scratch::new("open-scene-world-made");
    let scenes = world_project(&dir.0);
    let mut cx = started(&dir.0);
    let [from, to] = [3, 100].map(|x| Cell(IVec3::new(x, 0, 0)));
    cx.make_cells("scenes/cells", from, to, |cell| {
        let x = cell.0.x;
        cells_scene(&[(0x100 + x as u64, x as f32 * 10.0 + 5.0)])
    });
    let mut stack = Stack::default();
    stack.attach(&mut cx).expect("loaded");
    let look = |cx: &mut Context, x: f32| {
        look_at(cx, x);
        cx.stream_stopped();
        cx.stream_stopped();
    };
    look(&mut cx, 15.0);
    // Within the look's radius of 512.
    assert_eq!(xs(cx.world()), (0..=52).collect::<Vec<_>>());
    assert!(!open_scene::unsaved(&mut cx, 0), "as made");
    let cell = |x: i32| scenes.join(format!("cells/{x}_0_0.ron"));
    let four = EntityId::new(0x104).expect("an ID");
    apply(&mut cx, &mut stack, "agent", vec![set_x(four, 46.0)]);
    assert!(open_scene::unsaved(&mut cx, 0));
    open_scene::save(&mut cx, None).expect("saved");
    assert!(!open_scene::unsaved(&mut cx, 0), "saved");
    assert_eq!(file_ids(&cell(4)), [0x104]);
    assert!(!cell(3).exists() && !cell(5).exists(), "as made");
    let state = cx.world().resource::<CellState>().expect("streamed");
    let saved = Cell(IVec3::new(4, 0, 0));
    assert!(
        !state.came_as.contains_key(&saved),
        "it came as the file no longer has it"
    );
    assert!(state.came_as.contains_key(&Cell(IVec3::new(5, 0, 0))));
    let list = cx.cells_read(true, 0).world.expect("a world of cells");
    let five = list.cells.iter().find(|at| at.cell == [5, 0, 0]);
    let five = five.expect("listed, as it's in");
    assert!(five.made && !five.file, "{five:?}");

    look(&mut cx, 5000.0);
    assert_eq!(xs(cx.world()), [0; 0], "each as it came, or as saved");
    look(&mut cx, 15.0);
    let at = |cx: &Context| {
        let world = cx.world();
        let four = world.entity(four).expect("in");
        world.get::<Transform>(four).expect("placed").translation.x
    };
    assert_eq!(at(&cx), 46.0, "as its file has it");

    // Into cell 80, made, which hasn't come in.
    apply(&mut cx, &mut stack, "agent", vec![set_x(id_of(0), 805.0)]);
    let refused = open_scene::save(&mut cx, None).expect_err("refused");
    assert_eq!(refused.code(), "scene.cell_out", "{refused:#}");
    assert!(!cell(80).exists());
    cx.stream_stopped();
    cx.stream_stopped();
    open_scene::save(&mut cx, None).expect("saved once it's in");
    assert_eq!(file_ids(&cell(80)), [id_of(0).get(), 0x150]);
}

/// A world's save is refused, writing nothing, for a cell's file saved since it last applied, and
/// a root moved into a cell whose file hasn't come in, until it has; unsaved meanwhile.
#[test]
fn a_world_s_save_is_refused_what_would_lose_a_cell_s_file() {
    let dir = Scratch::new("open-scene-world-refused");
    let scenes = world_project(&dir.0);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    bring_cells_in(&mut cx);
    let main = scenes.join("main.ron");
    let text = crate::ecs::read_scene(&main).expect("read");
    let one = scenes.join("cells/1_0_0.ron");
    let theirs = cells_scene(&[(id_of(1).get(), 17.0)]);
    save(&one, &theirs, 2);
    apply(&mut cx, &mut stack, "agent", vec![set_x(id_of(1), 16.0)]);
    let refused = open_scene::save(&mut cx, None).expect_err("refused");
    assert_eq!(refused.code(), "scene.changed", "{refused:#}");
    assert_eq!(crate::ecs::read_scene(&one).expect("read"), theirs);
    assert_eq!(
        crate::ecs::read_scene(&main).expect("read"),
        text,
        "nothing written"
    );
    save(&one, &cells_scene(&[(id_of(1).get(), 15.0)]), 4);
    apply(&mut cx, &mut stack, "agent", vec![set_x(id_of(0), 1004.0)]);
    let refused = open_scene::save(&mut cx, None).expect_err("refused");
    assert_eq!(refused.code(), "scene.cell_out", "{refused:#}");
    assert!(open_scene::unsaved(&mut cx, 0));
    // One frame notes it coming, the next brings it in.
    cx.stream_stopped();
    cx.stream_stopped();
    assert_eq!(xs(cx.world()), [0, 1, 2, 100], "the root's cell came in");
    open_scene::save(&mut cx, None).expect("saved");
    let held = [id_of(0).get(), id_of(100).get()];
    assert_eq!(file_ids(&scenes.join("cells/100_0_0.ron")), held);
}

/// New and open from a world leave none of its cells' entities, and its cells' state empty.
#[test]
fn new_and_open_from_a_world_leave_none_of_its_cells() {
    let dir = Scratch::new("open-scene-world-new");
    world_project(&dir.0);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    bring_cells_in(&mut cx);
    let streamed = |cx: &Context| cx.world().query::<&crate::Streamed>().count();
    assert_eq!(streamed(&cx), 3);
    open_scene::new(&mut cx, &mut stack).expect("emptied");
    assert_eq!(streamed(&cx), 0);
    let state = cx.world().resource::<CellState>();
    assert!(state.is_none_or(CellState::is_empty));
    let main = open_scene::resolve(&cx, Path::new("scenes/main.ron"), true).expect("found");
    let opening = open_scene::check_open(&mut cx, main, 0, true).expect("opens");
    open_scene::open(&mut cx, &mut stack, opening).expect("opened");
    bring_cells_in(&mut cx);
    let main = open_scene::resolve(&cx, Path::new("scenes/main.ron"), true).expect("found");
    let opening = open_scene::check_open(&mut cx, main, 0, true).expect("opens again");
    open_scene::open(&mut cx, &mut stack, opening).expect("opened again");
    assert_eq!(streamed(&cx), 0, "until they come in");
    assert!(xs(cx.world()).is_empty());
}

/// A saved cell file applies to its cell in like the open scene's file: an edit of its own, which
/// an undo takes back, a root it adds streaming. A cell file made since brings its cell in. One
/// saved while a copy plays applies to the edited world after `stop`.
#[test]
fn a_cell_s_file_saved_applies_to_its_cell_in() {
    let dir = Scratch::new("open-scene-world-watch");
    let scenes = world_project(&dir.0);
    let mut cx = started(&dir.0);
    let mut stack = Stack::default();
    bring_cells_in(&mut cx);
    look_now(&mut cx, &mut stack);
    let one = scenes.join("cells/1_0_0.ron");
    let added = 0x20;
    save(
        &one,
        &cells_scene(&[(id_of(1).get(), 17.0), (added, 12.0)]),
        2,
    );
    look_now(&mut cx, &mut stack);
    assert_eq!(x(cx.world(), id_of(1)), 17.0);
    let added = EntityId::new(added).expect("an ID");
    let entity = cx.world().entity(added).expect("added");
    assert!(cx.world().get::<crate::Streamed>(entity).is_some());
    assert!(!open_scene::unsaved(&mut cx, 0), "as its file has it");
    let undo = Edit::Undo {
        author: "scenes/cells/1_0_0.ron".to_owned(),
    };
    client(&mut cx, &mut stack, undo);
    assert_eq!(x(cx.world(), id_of(1)), 15.0, "taken back");
    assert!(cx.world().entity(added).is_none());
    let three = scenes.join("cells/3_0_0.ron");
    save(&three, &cells_scene(&[(id_of(3).get(), 35.0)]), 2);
    look_now(&mut cx, &mut stack);
    cx.stream_stopped();
    cx.stream_stopped();
    assert_eq!(xs(cx.world()), [0, 1, 2, 3], "its file made since");
    stack.play(&mut cx, 0);
    save(&one, &cells_scene(&[(id_of(1).get(), 18.0)]), 4);
    look_now(&mut cx, &mut stack);
    assert_eq!(x(cx.world(), id_of(1)), 18.0, "the copy");
    assert_eq!(stack.stop(&mut cx), Some(0));
    assert_eq!(
        x(cx.world(), id_of(1)),
        15.0,
        "the world as edited, as it was"
    );
    look_now(&mut cx, &mut stack);
    assert_eq!(x(cx.world(), id_of(1)), 18.0, "the world as edited");
}
