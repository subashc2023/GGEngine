use gg_core::math::{Quat, Vec2, Vec3, Vec4};
use gg_renderer::{Mipmaps, Textures};

use crate::ecs::hierarchy::tests::check_links;
use crate::ecs::id::tests::check_ids;
use crate::ecs::id::{IdStream, placed};
use crate::reflect::Value;
use crate::reflect::tests::{
    Everything, Form, Handle, Inner, Pair, Unit, everything, same, same_snapshots, snapshot,
};
use crate::{
    Component, Entity, EntityId, Log, Operation, Parent, Reflect, Sampler, Sprite, Transform, World,
};

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

/// The scene the plan gives, byte for byte: its first entity's sprite has a texture, which isn't
/// saved.
const GOLDEN: &str = r#"(
    entities: {
        "2b7e151628aed2a6": (
            GridCell: (
                color: 1,
            ),
            Sprite: (
                color: (0.8, 0.2, 0.3, 1.0),
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

/// The golden scene as a person might write it: with comments and CRLFs, its entities, components
/// and fields out of order, a reference before its entity, raw strings, a hex integer, integers
/// for floats, and trailing commas left out.
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

/// A world that knows the golden scene's types, and has free slots, the last freed `left`.
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

/// Panics unless `world` is as `twin` is: its entities, their components and IDs, how many times
/// its rows have changed, and the next entity it spawns, and the ID it draws for it.
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
    let free: Vec<u32> = world.next_slots().take(3).map(|e| e.index).collect();
    let version = world.version();
    let roots = world
        .load_scene(GOLDEN)
        .unwrap_or_else(|error| panic!("{error}"));
    // So that drawing sorts the rows again.
    assert_ne!(world.version(), version);
    let cell = world.entity(id("2b7e151628aed2a6")).expect("loaded");
    let mover = world.entity(id("9f86d081884c7d65")).expect("loaded");
    // In the slots freed, the last freed first, and then a new one.
    assert_eq!([cell.index, mover.index], free[..2]);
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
    assert_eq!(transform.translation, Vec3::new(-0.9, 0.5, 0.2));
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
    // A texture that code sets isn't saved; nor are entities that aren't the roots' subtrees.
    let texture = Textures::default().add("first", [1, 1], &[0; 4], Mipmaps::None);
    world.get_mut::<Sprite>(cell).expect("a sprite").texture = texture.expect("a texture");
    assert_eq!(world.save_scene(&[cell, mover]).expect("saved"), GOLDEN);
    assert_eq!(
        world.save_scene(&[]).expect("saved"),
        "(\n    entities: {},\n)\n"
    );
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
            "                stops: [],\n",
            String::new(),
            "reflect.missing_field",
            "(\n                target",
            about("Patrol") + "field `stops` isn't given",
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
            "            Sprite: (\n                texture: 0,\n            ),\n            Parent:"
                .to_owned(),
            "reflect.opaque",
            "texture",
            about("Sprite") + "field `texture` is opaque",
        ),
        (
            "            ),\n        ),\n    },\n)",
            "            ),\n            Transform: (),\n        ),\n    },\n)".to_owned(),
            "scene.duplicate_component",
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
    // An ID the world has: here, from a scene loaded before.
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

/// Holds what's opaque in a list and an option, which a load can't make unless its base has it.
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

impl crate::Writable for Kept {}

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

/// `nodes` nodes, each the only kid of the one before, the last with `leaf`: the first is a
/// component's value, a level, its kids another, the next node another, and so on, so that it's
/// `2 * nodes` levels deep, and one more with a leaf.
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
    // Without what's opaque in them, they save.
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
    // Its handle from its base.
    let expected = Everything {
        target: Some(roots[0]),
        handle: Handle(7),
        ..all
    };
    assert!(same(&loaded.to_value(), &expected.to_value()), "{text}");
    assert_eq!(loaded.handle, Handle(7));
    assert_eq!(other.save_scene(&roots).expect("saved"), text);
    // A tuple of more than one part is a block, a part to a line.
    let pair = "\n                pair: (\n                    65535,\n                    -9223372036854775808,\n                ),\n";
    assert!(text.contains(pair), "{text}");
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
        // A soft hyphen, the grapheme joiner, zero-width characters, the word joiner, an invisible
        // operator, Mongolian's vowel separator and a Hangul filler, which draw nothing.
        "\u{ad}\u{34f}\u{200b}\u{200d}\u{2060}\u{2064}\u{180e}\u{3164}",
        // Variation selectors, an annotation anchor, a number sign, a beam, and tags.
        "❤\u{fe0f}\u{e0100}\u{fff9}\u{600}\u{1d173}\u{e0001}\u{e007f}",
        // What draws stays as it is: a combining mark, a no-break space and an emoji among them.
        "é e\u{301} \u{a0}😀",
    ];
    let numbers = Numbers {
        singles: singles.clone(),
        doubles: doubles.clone(),
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
    // Read as an f32 at once, never through an f64, whose rounding would give 1.
    let close = "(\n    entities: {\n        \"0000000000000001\": (\n            Numbers: (\n                \
         singles: [1.00000005960464477539062500000001],\n                doubles: [],\n                \
         texts: [],\n            ),\n        ),\n    },\n)\n";
    let roots = other.load_scene(close).expect("loaded");
    let loaded = &other.get::<Numbers>(roots[0]).expect("loaded").singles;
    assert_eq!(loaded[0].to_bits(), 1.0f32.next_up().to_bits());
}

/// The lines that differ between `a` and `b`: where they start, and how many there are of each.
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
            Value::F32(9.0),
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
    // An entity added is a block of its own, and changes nothing else.
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
    // Nor does a load take one deeper: the deepest leaf is the first written.
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
    // A scene saved by a world seeded as this one, which draws the ID the scene's entity has.
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

/// A prefab: a root with two children, the first patrolling to the second; and the IDs of its root
/// and children.
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
    // The golden world's three draws.
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
    // Placed beside the entities it was saved from, its references are to the copy's own.
    let (text, locals, mut beside) = prefab_and_world();
    let root = beside.place_prefab(&text, None).expect("placed");
    let kids: Vec<Entity> = beside.children(root).expect("alive").collect();
    let patrols = |kid: &&Entity| beside.get::<Patrol>(**kid).is_some();
    let patrolling = *kids.iter().find(patrols).expect("the patrolling kid");
    let target = *kids.iter().find(|kid| !patrols(kid)).expect("its sibling");
    let patrol = beside.get::<Patrol>(patrolling).expect("a patrol");
    assert_eq!(patrol.target, Some(target), "the copy's own sibling");
    assert_ne!(Some(target), beside.entity(locals[2]), "not the original");
    // Saved, the copy is entities as any other.
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
    // Two roots, and a dead entity, in each.
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
    // The entity that has the ID alive, and taken by a delete that keeps it for its undo.
    for deleted in [false, true] {
        let mut world = golden_world(19);
        let mut stream = IdStream::new(19);
        for _ in 0..3 {
            stream.draw();
        }
        // The ID the copy's root would take, which a scene loaded before has.
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
