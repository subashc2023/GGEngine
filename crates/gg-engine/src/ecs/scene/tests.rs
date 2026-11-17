use std::fs;

use gg_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};
use gg_renderer::{Mipmaps, Textures};

use super::{read_scene, write_scene};
use crate::dirs::tests::Scratch;
use crate::ecs::hierarchy::tests::check_links;
use crate::ecs::id::tests::check_ids;
use crate::ecs::id::{IdStream, placed};
use crate::ecs::{Log, Operation};
use crate::reflect::Value;
use crate::reflect::tests::{
    Everything, Form, Handle, Inner, Pair, Unit, everything, same, same_snapshots, snapshot,
};
use crate::{Component, Entity, EntityId, Parent, Reflect, Sampler, Sprite, Transform, World};

/// A grid's cell.
#[derive(Clone, Debug, PartialEq, crate::Component)]
struct GridCell {
    #[reflect(max = 1)]
    color: u32,
}

#[derive(Clone, Debug, PartialEq, crate::Component)]
struct Patrol {
    target: Option<Entity>,
    stops: Vec<Vec2>,
    mode: Mode,
}

#[derive(Clone, Debug, PartialEq, crate::Reflect)]
enum Mode {
    Idle,
    Loop { pause: f32 },
    Bounce(u8),
}

/// The golden scene, byte for byte.
const GOLDEN: &str = r#"(
    entities: {
        "2b7e151628aed2a6": (
            GridCell: (
                color: 1,
            ),
            Sprite: (
                color: (0.8, 0.2, 0.3, 1.0),
                texture: None,
                rect: [
                    (0.0, 0.0),
                    (1.0, 1.0),
                ],
                sampler: NearestClamp,
            ),
            Transform: (
                translation: (-0.9, 0.5, 0.2),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (0.18, 0.18, 1.0),
            ),
        ),
        "9f86d081884c7d65": (
            Parent: ("2b7e151628aed2a6"),
            Patrol: (
                target: Some("2b7e151628aed2a6"),
                stops: [],
                mode: Bounce(3),
            ),
            Transform: (
                translation: (0.75, 0.0, 0.01),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (0.3, 0.3, 1.0),
            ),
        ),
    },
)
"#;

/// The golden scene as a person might write it: comments, CRLFs, things out of order, a
/// reference before its entity, raw strings, a hex integer, integers for floats, no trailing
/// commas.
const LOOSE: &str = r##"// Written by hand.
(
  entities: {
    r"9f86d081884c7d65": (
      Transform: (scale: (0.3, 0.3, 1), rotation: (0, 0, 0, 1), translation: (0.75, 0, 0.01)),
      Patrol: (mode: Bounce(0x3), stops: [], target: Some(r#"2b7e151628aed2a6"#)),
      /* its parent */ Parent: ("2b7e151628aed2a6")
    ),
    "2b7e151628aed2a6": (
      Transform: (translation: (-0.9, 0.5, 0.2), rotation: (0.0, 0.0, 0.0, 1.0), scale: (0.18, 0.18, 1.0)),
      GridCell: (color: 1),
      Sprite: (sampler: NearestClamp, rect: [(0.0, 0.0), (1.0, 1.0)], color: (0.8, 0.2, 0.3, 1.0))
    )
  }
)"##;

/// A world that knows the golden scene's types and has free slots, the last freed `left`.
fn golden_world(seed: u64) -> World {
    let mut world = World::seeded(seed);
    world.register::<GridCell>().expect("registered");
    world.register::<Patrol>().expect("registered");
    world.register::<Sprite>().expect("registered");
    world.register::<Transform>().expect("registered");
    let [right, left] = [0, 1].map(|_| world.spawn(Transform::IDENTITY));
    world.spawn(GridCell { color: 0 });
    world.despawn(right).expect("alive");
    world.despawn(left).expect("alive");
    world
}

fn id(text: &str) -> EntityId {
    text.parse().expect("an ID")
}

/// Panics unless `world` matches `twin`: entities, components, IDs, row-change counts, and the
/// next entity and ID it draws.
pub(in crate::ecs) fn unchanged(world: &mut World, twin: &mut World, case: &str) {
    assert!(same_snapshots(&snapshot(world), &snapshot(twin)), "{case}");
    assert_eq!(world.version().1, twin.version().1, "{case}");
    let [drawn, twins] = [world, twin].map(|world| {
        let entity = world.spawn(());
        let id = world.id(entity);
        world.despawn(entity).expect("alive");
        (entity, id)
    });
    assert_eq!(drawn, twins, "{case}");
}

/// The line and column, from 1, where the last `marker` in `text` starts.
fn position(text: &str, marker: &str) -> (usize, usize) {
    let at = text
        .rfind(marker)
        .unwrap_or_else(|| panic!("`{marker}` in {text}"));
    let before = &text[..at];
    let line = before.matches('\n').count() + 1;
    let col = before.chars().rev().take_while(|&c| c != '\n').count() + 1;
    (line, col)
}

/// `text` with its last `from` replaced by `to`.
fn replace_last(text: &str, from: &str, to: &str) -> String {
    let at = text
        .rfind(from)
        .unwrap_or_else(|| panic!("`{from}` in the text"));
    format!("{}{to}{}", &text[..at], &text[at + from.len()..])
}

#[test]
fn a_scene_loads_and_saves_as_its_canonical_text() {
    let mut world = golden_world(1);
    let free: Vec<u32> = world.next_slots().take(2).map(|e| e.index).collect();
    let version = world.version();
    let roots = world
        .load_scene(GOLDEN)
        .unwrap_or_else(|error| panic!("{error}"));
    // So drawing sorts the rows again.
    assert_ne!(world.version(), version);
    let cell = world.entity(id("2b7e151628aed2a6")).expect("loaded");
    let mover = world.entity(id("9f86d081884c7d65")).expect("loaded");
    // In the two slots freed, the last freed first.
    assert_eq!([cell.index, mover.index], free[..]);
    assert_eq!(roots, [cell]);
    assert_eq!(world.get::<GridCell>(cell), Some(&GridCell { color: 1 }));
    let sprite = world.get::<Sprite>(cell).expect("a sprite");
    assert_eq!(sprite.color, Vec4::new(0.8, 0.2, 0.3, 1.0));
    assert_eq!(sprite.rect, [Vec2::ZERO, Vec2::ONE]);
    assert_eq!(sprite.sampler, Sampler::NearestClamp);
    assert_eq!(
        sprite.texture,
        Sprite::default().texture,
        "the white texture"
    );
    let transform = world.get::<Transform>(cell).expect("a transform");
    assert_eq!(transform.translation, DVec3::new(-0.9, 0.5, 0.2));
    assert_eq!(transform.rotation, Quat::IDENTITY);
    assert_eq!(transform.scale, Vec3::new(0.18, 0.18, 1.0));
    assert_eq!(world.get::<Parent>(mover), Some(&Parent(cell)));
    let patrol = Patrol {
        target: Some(cell),
        stops: Vec::new(),
        mode: Mode::Bounce(3),
    };
    assert_eq!(world.get::<Patrol>(mover), Some(&patrol));
    let children: Vec<Entity> = world.children(cell).expect("alive").collect();
    assert_eq!(children, [mover]);
    check_ids(&world);
    check_links(&world);
    let saved = world
        .save_scene(&roots)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(saved, GOLDEN);
    assert_eq!(world.save_scene(&[cell, mover]).expect("saved"), GOLDEN);
    assert_eq!(
        world.save_scene(&[]).expect("saved"),
        "(\n    entities: {},\n)\n"
    );
}

/// A scene parsed in a scratch world on another thread, then committed, leaves the world as a
/// load leaves a twin: entities in the load's slots, with references, parents, children and a
/// prefab instance's entities retargeted. Committed again, an ID the world has refuses it,
/// spawning nothing.
#[test]
fn a_scene_parsed_apart_commits_as_it_loads() {
    let (prefab, _) = prefab();
    let instance = r#"        "0a1b2c3d4e5f6071": (
            PrefabRoot: ("stand.prefab"),
            Transform: (
                translation: (2.0, 0.0, 0.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1.0, 1.0, 1.0),
            ),
        ),
    },
)
"#;
    let text = replace_last(GOLDEN, "    },\n)\n", instance);
    let [mut loaded, mut committed] = [0, 1].map(|_| {
        let mut world = golden_world(1);
        world.register::<crate::PrefabRoot>().expect("registered");
        let prefab = Some(prefab.as_str().into());
        let stand = "stand.prefab";
        world
            .assets_mut()
            .add_prefab(stand.into(), stand.into(), prefab);
        world
    });
    let roots = loaded.load_scene(&text).unwrap_or_else(|e| panic!("{e}"));
    let scratch = committed.scratch();
    let parsed = std::thread::scope(|scope| {
        let parsing = scope.spawn(|| scratch.parse_scene(&text));
        parsing.join().expect("parsed without a panic")
    });
    let parsed = parsed.unwrap_or_else(|e| panic!("{e}"));
    let committed_roots = committed.commit_scene(parsed).expect("committed");
    assert_eq!(committed_roots, roots, "in the slots the load gives");
    assert_eq!(committed.hash(), loaded.hash());
    check_ids(&committed);
    check_links(&committed);
    let cell = committed.entity(id("2b7e151628aed2a6")).expect("in");
    let mover = committed.entity(id("9f86d081884c7d65")).expect("in");
    assert_eq!(committed.get::<Parent>(mover), Some(&Parent(cell)));
    let patrols: Vec<(Entity, Option<Entity>)> = (committed.query::<(Entity, &Patrol)>())
        .map(|(entity, patrol)| (entity, patrol.target))
        .collect();
    assert_eq!(patrols.len(), 2, "the mover's and the instance's");
    for (entity, target) in patrols {
        let target = target.expect("a target");
        assert!(
            committed.id(target).is_some(),
            "{entity:?} patrols to {target:?}"
        );
        let up = |of| committed.get::<Parent>(of).map(|parent| parent.0);
        let kin = up(entity) == Some(target) || up(entity) == up(target);
        assert!(kin, "{entity:?} patrols to its parent or its sibling");
    }
    let again = committed.scratch().parse_scene(&text).expect("parsed");
    let (hash, version) = (committed.hash(), committed.version());
    let refused = committed
        .commit_scene(again)
        .expect_err("its IDs are taken");
    assert_eq!(refused.code(), "ecs.id_taken");
    assert_eq!((committed.hash(), committed.version()), (hash, version));
}

/// - A scene file reads back as written. A write replaces it through a temporary file beside
///   it, left behind neither on success nor on failure.
/// - A user's file named like such a temporary file is left alone.
/// - A read-only file is refused, on every OS, and kept.
/// - A write into a missing directory and a read of a missing file fail, naming the file.
#[test]
fn a_scene_file_is_written_whole() {
    let dir = Scratch::new("scene-file");
    fs::create_dir_all(&dir.0).expect("a directory");
    let path = dir.0.join("grid.ron");
    let draft = dir.0.join("grid.ron.new");
    fs::write(&draft, "a draft").expect("a draft");
    let listed = || {
        let entries = fs::read_dir(&dir.0).expect("listed");
        let mut names: Vec<_> = entries.map(|e| e.expect("an entry").file_name()).collect();
        names.sort();
        names
    };
    let empty = "(\n    entities: {},\n)\n";
    for text in [GOLDEN, empty] {
        write_scene(&path, text).unwrap_or_else(|error| panic!("{error:#}"));
        assert_eq!(read_scene(&path).ok().as_deref(), Some(text));
        assert_eq!(listed(), ["grid.ron", "grid.ron.new"]);
    }
    assert_eq!(fs::read_to_string(&draft).ok().as_deref(), Some("a draft"));
    // A directory: a file can't replace it.
    let taken = dir.0.join("taken");
    fs::create_dir(&taken).expect("a directory");
    let error = write_scene(&taken, GOLDEN).expect_err("a directory in the way");
    assert_eq!(error.code(), "scene.file", "{error:#}");
    assert_eq!(listed(), ["grid.ron", "grid.ron.new", "taken"]);
    let writable = fs::metadata(&path).expect("written").permissions();
    let mut read_only = writable.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&path, read_only).expect("made read only");
    let refused = write_scene(&path, GOLDEN);
    fs::set_permissions(&path, writable).expect("made writable again");
    let error = refused.expect_err("read only");
    assert_eq!(error.code(), "scene.file", "{error:#}");
    assert_eq!(read_scene(&path).ok().as_deref(), Some(empty));
    assert_eq!(listed(), ["grid.ron", "grid.ron.new", "taken"]);
    let missing = dir.0.join("missing").join("grid.ron");
    let named = missing.display().to_string();
    let errors = [
        write_scene(&missing, GOLDEN).expect_err("no directory"),
        read_scene(&missing).expect_err("no file"),
    ];
    for error in errors {
        assert_eq!(error.code(), "scene.file", "{error:#}");
        assert!(error.to_string().contains(&named), "{error}");
    }
}

/// Written through a link, a scene replaces the file it names, keeping its permissions; the
/// link stays a link.
#[cfg(unix)]
#[test]
fn a_scene_file_is_written_through_a_link_keeping_its_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = Scratch::new("scene-link");
    fs::create_dir_all(&dir.0).expect("a directory");
    let shared = dir.0.join("shared.ron");
    fs::write(&shared, "old").expect("written");
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o664)).expect("group-writable");
    let link = dir.0.join("level.ron");
    symlink(&shared, &link).expect("a link");
    write_scene(&link, GOLDEN).unwrap_or_else(|error| panic!("{error:#}"));
    let linked = fs::symlink_metadata(&link).expect("there");
    assert!(linked.file_type().is_symlink());
    assert_eq!(read_scene(&shared).ok().as_deref(), Some(GOLDEN));
    let mode = fs::metadata(&shared).expect("there").permissions().mode();
    assert_eq!(mode & 0o777, 0o664);
}

#[test]
fn text_written_by_hand_loads_and_saves_canonically() {
    let loose = LOOSE.replace('\n', "\r\n");
    let mut world = golden_world(2);
    let roots = world
        .load_scene(&loose)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(world.save_scene(&roots).expect("saved"), GOLDEN);
    check_ids(&world);
    check_links(&world);
}

/// - Exactly the given entities save: an entity whose parent isn't given is a root; one
///   referring to an ungiven entity is refused.
/// - A hand-written text is the engine's own in canonical form.
/// - A text's IDs read from its syntax alone, whatever types it names.
#[cfg(feature = "dev")]
#[test]
fn exactly_the_entities_given_save_and_a_texts_ids_read_from_its_syntax() {
    let mut world = golden_world(3);
    world.load_scene(GOLDEN).expect("loaded");
    let [cell, mover] = ["2b7e151628aed2a6", "9f86d081884c7d65"].map(id);
    let [cell, mover] = [cell, mover].map(|id| world.entity(id).expect("loaded"));
    let kid = world.spawn(Transform::IDENTITY);
    world.set_parent(kid, Some(mover)).expect("no cycle");
    assert_ne!(
        world.save_scene(&[cell]).expect("saved"),
        GOLDEN,
        "with the kid"
    );
    assert_eq!(world.save_entities(&[mover, cell]).expect("saved"), GOLDEN);
    let alone = world.save_entities(&[kid]).expect("saved");
    assert!(!alone.contains("Parent"), "a root: {alone}");
    let outside = world
        .save_entities(&[mover])
        .expect_err("its target isn't saved");
    assert_eq!(outside.code(), "scene.outside", "{outside}");
    let loose = world
        .scene_canonical(LOOSE)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(loose, GOLDEN);
    let ids = [id("2b7e151628aed2a6"), id("9f86d081884c7d65")];
    for text in [GOLDEN, LOOSE, GOLDEN.replace("GridCell", "Gone").as_str()] {
        assert_eq!(World::scene_ids(text).expect("read"), ids, "{text}");
    }
    for (text, code) in [
        ("(entities: {", "scene.syntax"),
        ("#![enable(implicit_some)]\n(entities: {})", "scene.syntax"),
        ("(entities: {\"nope\": ()})", "ecs.bad_id"),
    ] {
        let error = World::scene_ids(text).expect_err(text);
        assert_eq!(error.code(), code, "{text}: {error}");
    }
}

/// A scene's worlds' cells' folders read from its syntax alone, in ID order, whatever types it
/// names. A `Cells` without its folder gives none.
#[cfg(feature = "dev")]
#[test]
fn a_scenes_cells_folders_read_from_its_syntax() {
    let text = r#"(
    entities: {
        "00000000000000b0": (
            Cells: (
                folder: "scenes/b",
                size: 8.0,
            ),
            Gone: (),
        ),
        "00000000000000a0": (
            Cells: (
                folder: "scenes/a",
            ),
        ),
        "00000000000000c0": (
            Cells: (
                size: 8.0,
            ),
        ),
        "00000000000000d0": (
            Name: ("none"),
        ),
    },
)
"#;
    let folders = super::cells_folders(text).expect("read");
    assert_eq!(folders, ["scenes/a", "scenes/b"]);
    for text in ["(entities: {", "#![enable(implicit_some)]\n(entities: {})"] {
        let error = super::cells_folders(text).expect_err(text);
        assert_eq!(error.code(), "scene.syntax", "{text}: {error}");
    }
}

#[test]
fn a_refused_scene_changes_nothing_and_says_where() {
    let about = |path: &str| format!("can't load `{path}` of entity 9f86d081884c7d65: ");
    let cases = [
        (
            "stops: [],\n",
            "stops: []\n".to_owned(),
            "scene.syntax",
            "mode: Bounce",
            "Expected comma".to_owned(),
        ),
        (
            "    },\n)\n",
            "    },\n)\nx\n".to_owned(),
            "scene.syntax",
            "x\n",
            "Non-whitespace trailing characters".to_owned(),
        ),
        (
            "(\n    entities",
            "// Not #![enable(implicit_some)], which is a comment.\n#![type = \"#\"] \
             #![enable(implicit_some)]\n(\n    entities"
                .to_owned(),
            "scene.syntax",
            "#![enable",
            "a scene enables none of RON's extensions".to_owned(),
        ),
        (
            "stops:",
            "stopz:".to_owned(),
            "reflect.no_field",
            "stopz",
            about("Patrol")
                + "there's no field `stopz`; the fields are `target`, `stops` and `mode`",
        ),
        (
            "Parent: (\"2b7e151628aed2a6\")",
            "Parent: ()".to_owned(),
            "reflect.missing_field",
            "(),\n            Patrol",
            about("Parent") + "field `0` isn't given, and has no default to start from",
        ),
        (
            "stops: [],\n",
            "stops: [],\n                stops: [],\n".to_owned(),
            "reflect.duplicate_field",
            "stops",
            about("Patrol") + "field `stops` is given twice",
        ),
        (
            "Bounce(3)",
            "Bounse(3)".to_owned(),
            "reflect.no_variant",
            "Bounse",
            about("Patrol.mode")
                + "there's no variant `Bounse`; the variants are `Idle`, `Loop` and `Bounce`",
        ),
        (
            "Bounce(3)",
            "Bounce(300)".to_owned(),
            "reflect.out_of_range",
            "300",
            about("Patrol.mode.0") + "300 doesn't fit u8, from 0 to 255",
        ),
        (
            "            Parent:",
            "            GridCell: (\n                color: 2,\n            ),\n            Parent:"
                .to_owned(),
            "reflect.out_of_range",
            "2,\n            ),\n            Parent",
            about("GridCell.color") + "2 is above the field's max, 1",
        ),
        (
            "scale: (0.3, 0.3, 1.0)",
            "scale: (0.3, 0.3)".to_owned(),
            "reflect.wrong_length",
            "(0.3, 0.3)",
            about("Transform.scale") + "a Vec3 has 3 components, but it's given 2",
        ),
        (
            "scale: (0.3, 0.3, 1.0)",
            "scale: (0.3, 0.3, 1.0, 2.0)".to_owned(),
            "reflect.wrong_length",
            "2.0)",
            about("Transform.scale") + "a Vec3 has 3 components, but it's given more",
        ),
        (
            "translation: (0.75, 0.0, 0.01)",
            "translation: (0.75, false, 0.01)".to_owned(),
            "scene.syntax",
            "false",
            "Expected float".to_owned(),
        ),
        (
            "rotation: (0.0, 0.0, 0.0, 1.0)",
            "rotation: (0.0, 0.0, 0.0, 2.0)".to_owned(),
            "reflect.not_normalized",
            "(0.0, 0.0, 0.0, 2.0)",
            about("Transform.rotation") + "a rotation is a unit quaternion",
        ),
        (
            "Patrol:",
            "Patrolling:".to_owned(),
            "reflect.unknown_type",
            "Patrolling",
            about("Patrolling")
                + "there's no component type `Patrolling`; the component types are `Parent`, \
                   `GridCell`, `Patrol`, `Sprite` and `Transform`; a game registers its types \
                   with `World::register` before it loads a scene of them",
        ),
        (
            "            Parent:",
            "            Sprite: (\n                texture: Some(\"nowhere.png\"),\n            ),\n            \
             Parent:"
                .to_owned(),
            "asset.no_texture",
            "\"nowhere.png\"",
            about("Sprite.texture") + "no texture loaded has the ID or path `nowhere.png`",
        ),
        (
            "            ),\n        ),\n    },\n)",
            "            ),\n            Transform: (),\n        ),\n    },\n)".to_owned(),
            "reflect.duplicate_component",
            "Transform: ()",
            about("Transform") + "it's given twice, but an entity has one of each component",
        ),
        (
            "\"9f86d081884c7d65\": (",
            "\"9F86D081884C7D65\": (".to_owned(),
            "ecs.bad_id",
            "\"9F86",
            "`9F86D081884C7D65` isn't an entity's ID".to_owned(),
        ),
        (
            "Parent: (\"2b7e151628aed2a6\")",
            "Parent: (\"2b7e\")".to_owned(),
            "ecs.bad_id",
            "\"2b7e\")",
            about("Parent.0") + "`2b7e` isn't an entity's ID",
        ),
        (
            "\"9f86d081884c7d65\": (",
            "\"2b7e151628aed2a6\": (".to_owned(),
            "scene.duplicate_id",
            "\"2b7e151628aed2a6\": (",
            "entity 2b7e151628aed2a6 is given twice".to_owned(),
        ),
        (
            "target: Some(\"2b7e151628aed2a6\")",
            "target: Some(\"0123456789abcdef\")".to_owned(),
            "scene.unknown_id",
            "\"0123456789abcdef\"",
            about("Patrol.target") + "the scene has no entity 0123456789abcdef",
        ),
        (
            "Parent: (\"2b7e151628aed2a6\")",
            "Parent: (\"9f86d081884c7d65\")".to_owned(),
            "ecs.parent_cycle",
            "(\"9f86d081884c7d65\")",
            about("Parent") + "it makes the entity its own ancestor",
        ),
    ];
    let mut world = golden_world(3);
    let mut twin = golden_world(3);
    for (from, to, code, marker, says) in cases {
        let text = replace_last(GOLDEN, from, &to);
        let error = world.load_scene(&text).expect_err(&to);
        let message = error.to_string();
        assert_eq!(error.code(), code, "{message}");
        let (line, col) = position(&text, marker);
        let says = format!("{line}:{col}: {says}");
        assert!(message.starts_with(&says), "{message}\nisn't\n{says}");
        unchanged(&mut world, &mut twin, &message);
    }
    // An ID the world already has, from an earlier scene load.
    let taken = "(\n    entities: {\n        \"9f86d081884c7d65\": (),\n    },\n)\n";
    for world in [&mut world, &mut twin] {
        world.load_scene(taken).expect("loaded");
    }
    let error = world.load_scene(GOLDEN).expect_err("an ID taken");
    assert_eq!(error.code(), "ecs.id_taken", "{error}");
    let (line, col) = position(GOLDEN, "\"9f86d081884c7d65\": (");
    let says = format!("{line}:{col}: entity 9f86d081884c7d65 is in the world already");
    assert!(error.to_string().starts_with(&says), "{error}");
    unchanged(&mut world, &mut twin, "an ID taken");
}

/// - A scene replaces the world's entities as a load into a cleared world does, byte for byte.
/// - A refused scene changes nothing, the world's serial included.
/// - The scene the world was saved to, refused by a load for its IDs, replaces it under them.
#[test]
fn a_scene_replaces_the_worlds_entities_all_or_nothing() {
    let mut world = golden_world(20);
    world.spawn(Transform::IDENTITY);
    let before: Vec<Entity> = world.entities(None).expect("every entity").collect();
    let mut cleared = world.clone();
    let serial = world.serial();
    let roots = world
        .replace_scene(GOLDEN)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_ne!(world.serial(), serial);
    cleared.clear();
    let loaded = cleared.load_scene(GOLDEN).expect("loaded");
    assert_eq!(roots, loaded, "the same handles");
    assert_eq!(world.save_scene(&roots).expect("saved"), GOLDEN);
    assert_eq!(cleared.save_scene(&loaded).expect("saved"), GOLDEN);
    assert!(before.iter().all(|&entity| !world.contains(entity)));
    unchanged(
        &mut world,
        &mut cleared,
        "replaced, and loaded into the world cleared",
    );
    check_ids(&world);
    check_links(&world);
    for (from, to, code) in [
        ("stops: [],\n", "stops: []\n", "scene.syntax"),
        ("Patrol:", "Patrolling:", "reflect.unknown_type"),
    ] {
        let text = replace_last(GOLDEN, from, to);
        let serial = world.serial();
        let error = world.replace_scene(&text).expect_err(to);
        assert_eq!(error.code(), code, "{error}");
        let mut empty = world.clone();
        empty.clear();
        let expected = empty.load_scene(&text).expect_err(to);
        assert_eq!(error.to_string(), expected.to_string());
        assert_eq!(world.serial(), serial, "{error}");
        assert!(roots.iter().all(|&root| world.contains(root)), "{error}");
        assert_eq!(world.save_scene(&roots).expect("saved"), GOLDEN, "{error}");
        unchanged(&mut world, &mut cleared, &error.to_string());
    }
    let saved = world.save_scene(&roots).expect("saved");
    let error = world.load_scene(&saved).expect_err("its IDs");
    assert_eq!(error.code(), "ecs.id_taken", "{error}");
    let again = world.replace_scene(&saved).expect("replaced");
    assert_eq!(world.id(again[0]), Some(id("2b7e151628aed2a6")));
    assert_eq!(world.save_scene(&again).expect("saved"), saved);
    assert!(
        roots.iter().all(|&root| !world.contains(root)),
        "the old handles"
    );
    check_ids(&world);
    check_links(&world);
}

/// Holds opaque state in a list and an option; a load can't make it unless its base has it.
#[derive(Clone, Debug, PartialEq, crate::Reflect)]
struct Kept {
    sealed: Vec<Sealed>,
    maybe: Option<Sealed>,
}

#[derive(Clone, Debug, PartialEq, crate::Reflect)]
#[reflect(opaque)]
struct Sealed(u32);

impl Component for Kept {
    fn base() -> Option<Self> {
        Some(Self {
            sealed: vec![Sealed(1)],
            maybe: Some(Sealed(2)),
        })
    }
}

impl crate::ecs::Writable for Kept {}

/// A field named as no scene names one.
#[derive(Clone, crate::Component)]
struct Measure {
    größe: f32,
}

/// Named as one of RON's words.
#[derive(Clone, crate::Component)]
#[reflect(name = "None")]
struct Nothing;

/// A tree, as deep as it's made.
#[derive(Clone, Debug, PartialEq, crate::Component)]
struct Node {
    kids: Vec<Node>,
    leaf: Option<u8>,
}

/// `nodes` nodes, each the only kid of the previous, the last with `leaf`. A node's value and
/// its kids are each a level: `2 * nodes` deep, plus one with a leaf.
fn chain(nodes: usize, leaf: Option<u8>) -> Node {
    let mut node = Node {
        kids: Vec::new(),
        leaf,
    };
    for _ in 1..nodes {
        node = Node {
            kids: vec![node],
            leaf: None,
        };
    }
    node
}

#[test]
fn a_save_refuses_what_a_load_would_and_says_where() {
    let mut world = golden_world(4);
    let other = world.spawn(Transform::IDENTITY);
    let dead = world.spawn(());
    world.despawn(dead).expect("alive");
    let error = world.save_scene(&[other, dead]).expect_err("a dead root");
    assert_eq!(error.code(), "ecs.no_entity", "{error}");
    let patrol = |target| Patrol {
        target: Some(target),
        stops: Vec::new(),
        mode: Mode::Idle,
    };
    let mut cases: Vec<(Entity, &str, &str)> = vec![
        (world.spawn(patrol(dead)), "ecs.no_entity", "Patrol.target"),
        (world.spawn(patrol(other)), "scene.outside", "Patrol.target"),
        (
            world.spawn(Kept {
                sealed: vec![Sealed(3)],
                maybe: None,
            }),
            "reflect.opaque",
            "Kept.sealed[0]",
        ),
        (
            world.spawn(Kept {
                sealed: Vec::new(),
                maybe: Some(Sealed(4)),
            }),
            "reflect.opaque",
            "Kept.maybe",
        ),
        (
            world.spawn(Everything {
                form: Form::Held(Handle(5)),
                ..everything()
            }),
            "reflect.opaque",
            "Everything.form.0",
        ),
        (
            world.spawn(Measure { größe: 1.0 }),
            "scene.bad_name",
            "Measure.größe",
        ),
        (world.spawn(Nothing), "scene.bad_name", "None"),
        (
            world.spawn(GridCell { color: 2 }),
            "reflect.out_of_range",
            "GridCell.color",
        ),
        (world.spawn(chain(32, Some(1))), "scene.too_deep", "Node"),
    ];
    let turned = world.spawn(Transform::IDENTITY);
    world
        .get_mut::<Transform>(turned)
        .expect("a transform")
        .rotation = Quat::from_xyzw(0.0, 0.0, 0.0, 2.0);
    cases.push((turned, "reflect.not_normalized", "Transform.rotation"));
    for (entity, code, path) in cases {
        let error = world.save_scene(&[entity]).expect_err(path);
        let message = error.to_string();
        assert_eq!(error.code(), code, "{message}");
        let id = world.id(entity).expect("alive");
        let says = format!("can't save `{path}");
        assert!(message.starts_with(&says), "{message}");
        assert!(
            message.contains(&format!("` of entity {id}: ")),
            "{message}"
        );
    }
    // Without opaque state in them, they save.
    let kept = world.spawn(Kept {
        sealed: Vec::new(),
        maybe: None,
    });
    world.save_scene(&[kept]).expect("saved");
}

#[test]
fn every_kind_of_value_comes_back_bit_for_bit() {
    let mut world = World::seeded(5);
    let entity = world.spawn(());
    let all = Everything {
        on: false,
        count: 100,
        byte: 255,
        weight: f64::MAX,
        angle: -3.2,
        tint: Vec4::new(f32::NAN, -f32::NAN, f32::INFINITY, -0.0),
        offset: Vec3::new(f32::from_bits(1), f32::MIN_POSITIVE, f32::MAX),
        turn: Quat::from_rotation_y(2.0),
        target: Some(entity),
        inner: Inner {
            level: -1,
            name: "\"quoted\"\n".to_owned(),
        },
        form: Form::Box {
            size: Vec2::new(-0.0, 1e-7),
            turn: f32::NEG_INFINITY,
        },
        maybe: Some(Inner {
            level: 1,
            name: "é".to_owned(),
        }),
        list: vec![0, 10, 7],
        rect: [Vec2::NEG_ONE, Vec2::new(0.1, 0.2)],
        pair: Pair(u16::MAX, i64::MIN),
        unit: Unit,
        handle: Handle(99),
    };
    world.insert(entity, all.clone()).expect("alive");
    let text = world
        .save_scene(&[entity])
        .unwrap_or_else(|error| panic!("{error}"));
    let mut other = World::seeded(6);
    other.register::<Everything>().expect("registered");
    let roots = other
        .load_scene(&text)
        .unwrap_or_else(|error| panic!("{error}"));
    let loaded = other.get::<Everything>(roots[0]).expect("loaded");
    let expected = Everything {
        target: Some(roots[0]),
        handle: Handle(7),
        ..all
    };
    assert!(same(&loaded.to_value(), &expected.to_value()), "{text}");
    assert_eq!(loaded.handle, Handle(7));
    assert_eq!(other.save_scene(&roots).expect("saved"), text);
    let pair = "\n                pair: (\n                    65535,\n                    -9223372036854775808,\n                ),\n";
    assert!(text.contains(pair), "{text}");
}

/// A translation far from the origin saves as the shortest numbers that read back to its `f64`s
/// and loads to their bits; `f32` would round 1,000,000.1 to 1,000,000.125.
#[test]
fn a_far_translation_comes_back_bit_for_bit() {
    let mut world = World::seeded(11);
    let far = DVec3::new(1.0e6 + 0.1, -0.0, -1.0e12 - 0.5);
    let entity = world.spawn(Transform {
        translation: far,
        ..Transform::IDENTITY
    });
    let text = world.save_scene(&[entity]).expect("saved");
    let written = "translation: (1000000.1, -0.0, -1000000000000.5),";
    assert!(text.lines().any(|line| line.trim() == written), "{text}");
    let mut other = World::seeded(12);
    other.register::<Transform>().expect("registered");
    let roots = other.load_scene(&text).expect("loaded");
    let loaded = other.get::<Transform>(roots[0]).expect("loaded");
    let bits = |v: DVec3| v.to_array().map(f64::to_bits);
    assert_eq!(bits(loaded.translation), bits(far));
}

#[derive(Clone, Debug, PartialEq, crate::Component)]
struct Numbers {
    singles: Vec<f32>,
    doubles: Vec<f64>,
    texts: Vec<String>,
}

#[test]
fn floats_and_strings_are_written_as_they_read_back() {
    let singles = vec![
        0.0,
        -0.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        -f32::NAN,
        f32::from_bits(1),
        f32::MIN_POSITIVE,
        f32::MAX,
        0.1,
        1e20,
        16_777_217.0,
    ];
    let doubles = vec![
        0.0,
        -0.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        -f64::NAN,
        f64::from_bits(1),
        f64::MIN_POSITIVE,
        f64::MAX,
        0.1,
        1e20,
        16_777_217.0,
    ];
    let texts = [
        "",
        "plain",
        "\\ \" \n \r \t",
        "\u{0}\u{1f}\u{7f}\u{85}\u{9f}",
        "\u{61c}\u{200e}\u{200f}\u{2028}\u{2029}\u{202a}\u{202e}\u{2066}\u{2069}\u{feff}",
        // Draw nothing: a soft hyphen, the grapheme joiner, zero-width characters, the word
        // joiner, an invisible operator, Mongolian's vowel separator, a Hangul filler.
        "\u{ad}\u{34f}\u{200b}\u{200d}\u{2060}\u{2064}\u{180e}\u{3164}",
        // Variation selectors, an annotation anchor, a number sign, a beam, and tags.
        "❤\u{fe0f}\u{e0100}\u{fff9}\u{600}\u{1d173}\u{e0001}\u{e007f}",
        // What draws stays as is: a combining mark, a no-break space, an emoji.
        "é e\u{301} \u{a0}😀",
    ];
    let numbers = Numbers {
        singles,
        doubles,
        texts: texts.map(str::to_owned).to_vec(),
    };
    let mut world = World::seeded(7);
    let entity = world.spawn(numbers.clone());
    let text = world.save_scene(&[entity]).expect("saved");
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    for written in [
        "0.0,",
        "-0.0,",
        "inf,",
        "-inf,",
        "NaN,",
        "-NaN,",
        "0.000000000000000000000000000000000000000000001,",
        "0.000000000000000000000000000000000000011754944,",
        "340282350000000000000000000000000000000.0,",
        "0.1,",
        "100000000000000000000.0,",
        "16777216.0,",
        "16777217.0,",
        r#""","#,
        r#""plain","#,
        r#""\\ \" \n \r \t","#,
        r#""\u{0}\u{1f}\u{7f}\u{85}\u{9f}","#,
        r#""\u{61c}\u{200e}\u{200f}\u{2028}\u{2029}\u{202a}\u{202e}\u{2066}\u{2069}\u{feff}","#,
        r#""\u{ad}\u{34f}\u{200b}\u{200d}\u{2060}\u{2064}\u{180e}\u{3164}","#,
        r#""❤\u{fe0f}\u{e0100}\u{fff9}\u{600}\u{1d173}\u{e0001}\u{e007f}","#,
        "\"é e\u{301} \u{a0}😀\",",
    ] {
        assert!(lines.contains(&written), "{written} in\n{text}");
    }
    let mut other = World::seeded(8);
    other.register::<Numbers>().expect("registered");
    let roots = other
        .load_scene(&text)
        .unwrap_or_else(|error| panic!("{error}"));
    let loaded = other.get::<Numbers>(roots[0]).expect("loaded");
    assert!(same(&loaded.to_value(), &numbers.to_value()), "{text}");
    // A NaN keeps its sign, but not its payload.
    let mut nans = World::seeded(9);
    let payloads = Numbers {
        singles: vec![f32::from_bits(0x7fc0_1234), f32::from_bits(0xffc0_0002)],
        doubles: Vec::new(),
        texts: Vec::new(),
    };
    let entity = nans.spawn(payloads);
    let text = nans.save_scene(&[entity]).expect("saved");
    let mut other = World::seeded(10);
    other.register::<Numbers>().expect("registered");
    let roots = other.load_scene(&text).expect("loaded");
    let loaded = &other.get::<Numbers>(roots[0]).expect("loaded").singles;
    let bits: Vec<u32> = loaded.iter().map(|x| x.to_bits()).collect();
    assert_eq!(bits, [f32::NAN, -f32::NAN].map(f32::to_bits));
    // Read as an f32 directly: through an f64, rounding would give 1.
    let close = "(\n    entities: {\n        \"0000000000000001\": (\n            Numbers: (\n                \
         singles: [1.00000005960464477539062500000001],\n                doubles: [],\n                \
         texts: [],\n            ),\n        ),\n    },\n)\n";
    let roots = other.load_scene(close).expect("loaded");
    let loaded = &other.get::<Numbers>(roots[0]).expect("loaded").singles;
    assert_eq!(loaded[0].to_bits(), 1.0f32.next_up().to_bits());
}

/// The lines that differ between `a` and `b`: where they start and how many each has.
fn changed(a: &str, b: &str) -> (usize, usize, usize) {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let start = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    let rest = a.len().min(b.len()) - start;
    let end = a.iter().rev().zip(b.iter().rev()).take(rest);
    let end = end.take_while(|(a, b)| a == b).count();
    (start + 1, a.len() - start - end, b.len() - start - end)
}

#[derive(Clone, Debug, PartialEq, crate::Component)]
struct Wrapped(Settings);

#[derive(Clone, Debug, PartialEq, crate::Reflect)]
struct Settings {
    level: u32,
    limit: Option<u32>,
}

#[test]
fn a_field_set_changes_only_its_lines() {
    let mut world = golden_world(11);
    let roots = world.load_scene(GOLDEN).expect("loaded");
    let cell = world.entity(id("2b7e151628aed2a6")).expect("loaded");
    let mover = world.entity(id("9f86d081884c7d65")).expect("loaded");
    let wrapped = Wrapped(Settings {
        level: 1,
        limit: None,
    });
    world.insert(mover, wrapped).expect("alive");
    let before = world.save_scene(&roots).expect("saved");
    let stop = Value::List(vec![Value::Vec2(Vec2::new(1.0, 2.0))]);
    let pause = vec![("pause".into(), Value::F32(0.5))];
    let limit = Value::Option(Some(Box::new(Value::UInt(5))));
    for (entity, component, path, value, at, lines) in [
        (cell, "GridCell", "color", Value::UInt(0), "color: 1", 1),
        (
            cell,
            "Transform",
            "translation.y",
            Value::F64(9.0),
            "translation: (-0.9",
            1,
        ),
        (
            cell,
            "Sprite",
            "rect[1].x",
            Value::F32(0.5),
            "(1.0, 1.0),",
            1,
        ),
        (
            cell,
            "Sprite",
            "sampler",
            Value::Variant("LinearRepeat".into(), Vec::new()),
            "sampler",
            1,
        ),
        (mover, "Patrol", "stops", stop, "stops", 3),
        (
            mover,
            "Patrol",
            "mode",
            Value::Variant("Loop".into(), pause),
            "mode",
            3,
        ),
        (mover, "Wrapped", "0.limit", limit, "limit", 1),
    ] {
        let old = world
            .set_field(entity, component, path, &value)
            .expect(path);
        let after = world.save_scene(&roots).expect("saved");
        let (start, removed, added) = changed(&before, &after);
        assert_eq!(start, position(&before, at).0, "{path}:\n{after}");
        assert_eq!((removed, added), (1, lines), "{path}:\n{after}");
        world
            .set_field(entity, component, path, &old)
            .expect("set back");
    }
    assert_eq!(world.save_scene(&roots).expect("saved"), before);
    let added = world.spawn(Transform::IDENTITY);
    let after = world.save_scene(&[cell, added]).expect("saved");
    let (start, removed, lines) = changed(&before, &after);
    assert_eq!((removed, lines), (0, 7), "{after}");
    let id = world.id(added).expect("alive");
    let first = after.lines().nth(start - 1).expect("a line");
    assert_eq!(first, format!("        \"{id}\": ("), "{after}");
}

#[test]
fn values_nest_64_levels_deep() {
    let mut world = World::seeded(12);
    let deep = world.spawn(chain(32, None));
    let text = world.save_scene(&[deep]).expect("64 levels");
    let mut other = World::seeded(13);
    other.register::<Node>().expect("registered");
    let roots = other
        .load_scene(&text)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(other.get::<Node>(roots[0]), Some(&chain(32, None)));
    let deeper = world.spawn(chain(32, Some(1)));
    let error = world.save_scene(&[deeper]).expect_err("65 levels");
    assert_eq!(error.code(), "scene.too_deep", "{error}");
    // Nor does a load go deeper: the deepest leaf is written first.
    let deeper = text.replacen("leaf: None", "leaf: Some(1)", 1);
    let mut other = World::seeded(13);
    other.register::<Node>().expect("registered");
    let error = other.load_scene(&deeper).expect_err("65 levels");
    assert_eq!(error.code(), "scene.too_deep", "{error}");
    let (line, col) = position(&deeper, "1)");
    assert!(
        error.to_string().starts_with(&format!("{line}:{col}: ")),
        "{error}"
    );
}

#[test]
fn a_draw_that_meets_a_loaded_id_reseeds() {
    // A scene from an identically seeded world, which draws the scene entity's ID.
    let mut first = World::seeded(14);
    let saved = first.spawn(Transform::IDENTITY);
    let text = first.save_scene(&[saved]).expect("saved");
    let mut world = World::seeded(14);
    world.register::<Transform>().expect("registered");
    world.load_scene(&text).expect("loaded");
    let spawned = world.spawn(());
    let mut stream = IdStream::new(14);
    assert_eq!(Some(stream.draw()), first.id(saved));
    stream.reseed();
    assert_eq!(world.id(spawned), Some(stream.draw()));
    check_ids(&world);
}

/// A prefab: a root with two children, the first patrolling to the second; and their IDs.
fn prefab() -> (String, [EntityId; 3]) {
    let (text, ids, _) = prefab_and_world();
    (text, ids)
}

/// [`prefab`], and the world it was saved from, whose entities have its IDs.
fn prefab_and_world() -> (String, [EntityId; 3], World) {
    let mut world = World::seeded(15);
    let [root, first, second] = [0, 1, 2].map(|_| world.spawn(Transform::IDENTITY));
    for child in [first, second] {
        world.set_parent(child, Some(root)).expect("no cycle");
    }
    let patrol = Patrol {
        target: Some(second),
        stops: Vec::new(),
        mode: Mode::Idle,
    };
    world.insert(first, patrol).expect("alive");
    let text = world.save_scene(&[root]).expect("saved");
    let ids = [root, first, second].map(|entity| world.id(entity).expect("alive"));
    (text, ids, world)
}

#[test]
fn a_prefab_is_placed_as_a_copy_with_ids_of_its_own() {
    let (text, locals) = prefab();
    let mut world = golden_world(16);
    let mut stream = IdStream::new(16);
    for _ in 0..3 {
        stream.draw();
    }
    let [first, second, older] = [0, 1, 2].map(|_| world.spawn(Transform::IDENTITY));
    world.set_parent(older, Some(first)).expect("no cycle");
    for _ in 0..3 {
        stream.draw();
    }
    let mut copies = Vec::new();
    for parent in [first, second] {
        let root = world.place_prefab(&text, Some(parent)).expect("placed");
        let instance = stream.draw();
        let ids = locals.map(|local| placed(instance, local).expect("not zero"));
        assert_eq!(world.id(root), Some(ids[0]));
        let mut kids: Vec<EntityId> = ids[1..].to_vec();
        kids.sort();
        let listed = world.children(root).expect("alive");
        let listed: Vec<EntityId> = listed.filter_map(|kid| world.id(kid)).collect();
        assert_eq!(listed, kids, "in the order of their IDs");
        let patrolling = world.entity(ids[1]).expect("placed");
        let target = world
            .get::<Patrol>(patrolling)
            .and_then(|patrol| patrol.target);
        assert_eq!(target, world.entity(ids[2]), "its own sibling");
        let last = world.children(parent).expect("alive").last();
        assert_eq!(last, Some(root));
        copies.push(ids);
    }
    assert_ne!(copies[0], copies[1]);
    check_ids(&world);
    check_links(&world);
    // Placed beside its originals, its references point to the copy's own.
    let (text, locals, mut beside) = prefab_and_world();
    let root = beside.place_prefab(&text, None).expect("placed");
    let kids: Vec<Entity> = beside.children(root).expect("alive").collect();
    let patrols = |kid: &&Entity| beside.get::<Patrol>(**kid).is_some();
    let patrolling = *kids.iter().find(patrols).expect("the patrolling kid");
    let target = *kids.iter().find(|kid| !patrols(kid)).expect("its sibling");
    let patrol = beside.get::<Patrol>(patrolling).expect("a patrol");
    assert_eq!(patrol.target, Some(target), "the copy's own sibling");
    assert_ne!(Some(target), beside.entity(locals[2]), "not the original");
    let saved = world.save_scene(&[first]).expect("saved");
    let mut other = golden_world(17);
    let roots = other.load_scene(&saved).expect("loaded");
    assert_eq!(other.id(roots[0]), world.id(first));
    for id in copies[0] {
        assert!(other.entity(id).is_some(), "{id}");
    }
    check_links(&other);
}

#[test]
fn a_prefab_refused_changes_nothing() {
    let (text, locals) = prefab();
    let mut world = golden_world(18);
    let mut twin = golden_world(18);
    let prepare = |world: &mut World| {
        let roots = [0, 1].map(|_| world.spawn(Transform::IDENTITY));
        let dead = world.spawn(());
        world.despawn(dead).expect("alive");
        let second = roots
            .map(|root| world.id(root).expect("alive"))
            .into_iter()
            .max();
        (world.save_scene(&roots).expect("saved"), second, dead)
    };
    let (two, second, dead) = prepare(&mut world);
    prepare(&mut twin);
    let second = second.expect("two roots");
    let target = format!("Some(\"{}\")", locals[2]);
    let outside = text.replace(&target, "Some(\"0123456789abcdef\")");
    let empty = "(\n    entities: {},\n)\n".to_owned();
    for (text, parent, code, marker) in [
        (&two, None, "scene.prefab_roots", format!("\"{second}\"")),
        (&empty, None, "scene.prefab_roots", "{}".to_owned()),
        (&text, Some(dead), "ecs.no_entity", String::new()),
        (
            &outside,
            None,
            "scene.unknown_id",
            "\"0123456789abcdef\"".to_owned(),
        ),
    ] {
        let error = world.place_prefab(text, parent).expect_err(code);
        assert_eq!(error.code(), code, "{error}");
        if !marker.is_empty() {
            let (line, col) = position(text, &marker);
            let says = format!("{line}:{col}: ");
            assert!(error.to_string().starts_with(&says), "{error}");
        }
        unchanged(&mut world, &mut twin, &error.to_string());
    }
}

#[test]
fn a_copy_that_would_have_a_living_or_kept_entitys_id_is_another() {
    let (text, locals) = prefab();
    // The entity with the ID is alive, then taken by a delete that keeps it for its undo.
    for deleted in [false, true] {
        let mut world = golden_world(19);
        let mut stream = IdStream::new(19);
        for _ in 0..3 {
            stream.draw();
        }
        // The ID the copy's root would take, which an earlier scene load has.
        let instance = stream.draw();
        let taken = placed(instance, locals[0]).expect("not zero");
        let scene = format!("(\n    entities: {{\n        \"{taken}\": (),\n    }},\n)\n");
        let loaded = world.load_scene(&scene).expect("loaded")[0];
        let mut log = Log::default();
        if deleted {
            let delete = Operation::Delete { entity: taken };
            log.apply(&mut world, delete).expect("deleted");
        }
        let root = world.place_prefab(&text, None).expect("placed");
        stream.reseed();
        let instance = stream.draw();
        assert_eq!(world.id(root), placed(instance, locals[0]), "{deleted}");
        if deleted {
            assert!(log.undo(&mut world).expect("undone"));
            assert_eq!(world.entity(taken), Some(loaded));
        }
        check_ids(&world);
    }
}

/// - A sprite's texture saves as its name, plus its ID where it has one.
/// - A load finds the texture by that ID in a world that has it, or by a file path (a
///   hand-written scene may name one); it saves again by name.
/// - An unknown name refuses the scene; a nameless texture refuses the save.
#[test]
fn a_sprite_saves_its_texture_by_its_name() {
    let mut textures = Textures::default();
    let mut add = |name| textures.add(name, [1, 1], &[0; 4], Mipmaps::None);
    let [logo, unnamed] = ["logo", "unnamed"].map(|name| add(name).expect("a texture"));
    let named = |world: &mut World| {
        let (path, id) = ("assets/logo.png".into(), "5f3c9a1e07b2d846".into());
        world.assets_mut().add_texture(logo, path, id);
    };
    let mut world = golden_world(4);
    named(&mut world);
    let shown = Sprite {
        texture: Some(logo),
        ..Sprite::default()
    };
    let entity = world.spawn(shown);
    let text = world.save_scene(&[entity]).expect("saved");
    let by_id = "texture: Some(\"5f3c9a1e07b2d846\"),";
    assert!(text.contains(by_id), "{text}");
    let by_path = text.replace(by_id, "texture: Some(\"assets/logo.png\"),");
    for given in [&text, &by_path] {
        let mut other = golden_world(5);
        named(&mut other);
        let loaded = other.load_scene(given).expect("loaded")[0];
        assert_eq!(other.get::<Sprite>(loaded), Some(&shown));
        assert_eq!(other.save_scene(&[loaded]).expect("saved"), text);
    }
    let refused = golden_world(6)
        .load_scene(&text)
        .expect_err("no such texture");
    assert_eq!(refused.code(), "asset.no_texture", "{refused}");
    world.get_mut::<Sprite>(entity).expect("a sprite").texture = Some(unnamed);
    let refused = world.save_scene(&[entity]).expect_err("no name");
    assert_eq!(refused.code(), "asset.unnamed", "{refused}");
}

/// A field a scene leaves out (as one saved before the field existed) takes its default; the
/// scene saves again with it.
#[test]
fn a_field_a_scene_leaves_out_takes_its_default() {
    let older = GOLDEN
        .replacen("                texture: None,\n", "", 1)
        .replacen("                stops: [],\n", "", 1);
    let mut world = golden_world(7);
    let roots = world.load_scene(&older).expect("loaded");
    let mover = world.entity(id("9f86d081884c7d65")).expect("loaded");
    assert_eq!(
        world.get::<Patrol>(mover).map(|patrol| patrol.stops.len()),
        Some(0)
    );
    assert_eq!(world.save_scene(&roots).expect("saved"), GOLDEN);
}
