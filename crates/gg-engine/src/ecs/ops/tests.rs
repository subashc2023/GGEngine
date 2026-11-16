use gg_core::math::{DVec3, Quat};
use gg_renderer::{Mipmaps, Texture, Textures};

use super::*;
use crate::ecs::hierarchy::tests::check_links;
use crate::ecs::id::tests::check_ids;
use crate::ecs::scene::tests::unchanged;
use crate::reflect::tests::{Everything, Handle, everything, same_snapshots, snapshot};
use crate::{Reflect, Sprite, Transform};

/// Opaque as a whole, like [`MeshDraw`](crate::MeshDraw): a world keeps one, no value makes one.
#[derive(Clone, Debug, PartialEq, crate::Component)]
#[reflect(opaque)]
pub(in crate::ecs) struct Sealed(pub(in crate::ecs) u32);

/// A seeded world's entities, the same each time:
/// - `r`, a root, with children `a` then `s`.
/// - `a`, with a sprite of its own texture and children `b`, `c`, `d`.
/// - `e`, `b`'s child.
/// - `f`, a root whose `Everything` targets `e` and keeps `Handle(99)`.
/// - `g`, a root that's `Sealed` alone.
pub(in crate::ecs) struct Fixture {
    pub(in crate::ecs) world: World,
    pub(in crate::ecs) r: Entity,
    pub(in crate::ecs) a: Entity,
    pub(in crate::ecs) s: Entity,
    pub(in crate::ecs) b: Entity,
    pub(in crate::ecs) c: Entity,
    pub(in crate::ecs) d: Entity,
    pub(in crate::ecs) e: Entity,
    pub(in crate::ecs) f: Entity,
    pub(in crate::ecs) g: Entity,
    pub(in crate::ecs) texture: Texture,
    /// Each entity's ID, which a dead entity's handle no longer gives.
    ids: Vec<(Entity, EntityId)>,
}

impl Fixture {
    pub(in crate::ecs) fn id(&self, entity: Entity) -> EntityId {
        let found = self.ids.iter().find(|&&(other, _)| other == entity);
        found.expect("one of the fixture's").1
    }
}

pub(in crate::ecs) fn fixture() -> Fixture {
    let mut world = World::seeded(0x0905);
    let registered = [
        world.register::<Transform>(),
        world.register::<Sprite>(),
        world.register::<Everything>(),
        world.register::<Sealed>(),
    ];
    registered
        .into_iter()
        .for_each(|done| done.expect("registered"));
    let texture = Textures::default().add("own", [1, 1], &[255, 0, 0, 255], Mipmaps::None);
    let texture = texture.expect("a texture");
    world
        .assets_mut()
        .add_texture(texture, "own.png".into(), "own.png".into());
    let at = |x: f32| Transform {
        translation: DVec3::new(f64::from(x), 0.0, 0.0),
        ..Transform::IDENTITY
    };
    let r = world.spawn(at(0.0));
    let sprite = Sprite {
        texture: Some(texture),
        ..Sprite::default()
    };
    let a = world.spawn((at(1.0), sprite));
    let s = world.spawn(at(2.0));
    let [b, c, d, e] = [3.0, 4.0, 5.0, 6.0].map(|x| world.spawn(at(x)));
    for (child, parent) in [(a, r), (s, r), (b, a), (c, a), (d, a), (e, b)] {
        world.set_parent(child, Some(parent)).expect("no cycle");
    }
    let f = world.spawn(Everything {
        target: Some(e),
        handle: Handle(99),
        ..everything()
    });
    let g = world.spawn(Sealed(5));
    let ids = [r, a, s, b, c, d, e, f, g].map(|entity| (entity, world.id(entity).expect("alive")));
    Fixture {
        world,
        r,
        a,
        s,
        b,
        c,
        d,
        e,
        f,
        g,
        texture,
        ids: ids.to_vec(),
    }
}

/// Every component of every entity, by entity then component, whatever the table.
pub(in crate::ecs) fn sorted(world: &World) -> Vec<(Entity, EntityId, String, Value)> {
    let mut all = snapshot(world);
    all.sort_by(|a, b| (a.0, &a.2).cmp(&(b.0, &b.2)));
    all
}

fn children(world: &World, entity: Entity) -> Vec<Entity> {
    world.children(entity).expect("alive").collect()
}

fn named(name: &'static str, value: Value) -> (Ident, Value) {
    (name.into(), value)
}

/// Applies `operation`, undoes, redoes and undoes it, checking the world against its state
/// before and after, by handle. Returns what redoes it.
fn round_trip(world: &mut World, mut operation: Operation) -> Inverse {
    let said = operation.to_string();
    let before = sorted(world);
    let inverse = world.apply(&mut operation);
    let mut inverse = inverse.unwrap_or_else(|error| panic!("{error}"));
    let after = sorted(world);
    for (step, expected) in [
        ("undone", &before),
        ("redone", &after),
        ("undone again", &before),
    ] {
        let stepped = world.undo(&mut inverse);
        stepped.unwrap_or_else(|error| panic!("{said} {step}: {error}"));
        assert!(same_snapshots(&sorted(world), expected), "{said} {step}");
        check_ids(world);
        check_links(world);
    }
    inverse
}

#[test]
fn each_operation_is_undone_and_redone_exactly() {
    let mut x = fixture();
    let [a, b, e, f, g] = [x.a, x.b, x.e, x.f, x.g].map(|entity| x.id(entity));
    let target = |entity| Value::Option(Some(Box::new(Value::Entity(entity))));
    let held = Everything {
        target: Some(x.c),
        count: 1,
        ..everything()
    };
    let held = held.to_value();
    for operation in [
        Operation::Create {
            entity: None,
            parent: Some(b),
            components: vec![
                named("Transform", Transform::IDENTITY.to_value()),
                named("Sprite", Sprite::default().to_value()),
                named("Everything", held.clone()),
            ],
        },
        Operation::Delete { entity: a },
        Operation::Duplicate {
            entity: a,
            copies: Vec::new(),
        },
        Operation::Add {
            entity: g,
            component: "Transform".into(),
            value: Transform::IDENTITY.to_value(),
        },
        Operation::Remove {
            entity: a,
            component: "Sprite".into(),
        },
        Operation::Set {
            entity: f,
            component: "Everything".into(),
            path: "target".to_owned(),
            value: target(x.c),
        },
        Operation::Set {
            entity: f,
            component: "Everything".into(),
            path: String::new(),
            value: held,
        },
        Operation::Reparent {
            entity: e,
            parent: Some(f),
        },
        Operation::Reparent {
            entity: a,
            parent: None,
        },
    ] {
        let redo = round_trip(&mut x.world, operation);
        x.world.release(redo);
    }
    // Opaque state, which no value reads, came back too.
    let sprite = x.world.get::<Sprite>(x.a).expect("a sprite");
    assert_eq!(sprite.texture, Some(x.texture));
    let all = x.world.get::<Everything>(x.f).expect("everything");
    assert_eq!(all.handle, Handle(99));
    assert_eq!(x.world.get::<Sealed>(x.g), Some(&Sealed(5)));
}

/// A duplicate copies an entity and its descendants, opaque state included, each under a new
/// ID.
/// - The entity's copy is its parent's last child; other copies sit under their parent's copy,
///   in the originals' order.
/// - A reference to a copied entity points at its copy; one outside stays.
/// - Undo is refused, changing nothing, once a copy has a foreign child or is gone.
#[test]
fn a_duplicate_copies_a_subtree_typed_under_ids_of_its_own() {
    let mut x = fixture();
    x.world.set_parent(x.f, Some(x.a)).expect("no cycle");
    x.world.set_parent(x.g, Some(x.f)).expect("no cycle");
    let mut duplicate = Operation::Duplicate {
        entity: x.id(x.a),
        copies: Vec::new(),
    };
    let mut inverse = x.world.apply(&mut duplicate).expect("applied");
    let Operation::Duplicate { copies, .. } = &duplicate else {
        unreachable!("applied as it was")
    };
    let copied = copies.iter().map(|&id| x.world.entity(id).expect("alive"));
    let copied: Vec<Entity> = copied.collect();
    // Each parent before its children.
    let [a, b, c, d, f, e, g] = copied[..] else {
        panic!("seven copies: {copies:?}");
    };
    assert_eq!(children(&x.world, x.r), [x.a, x.s, a]);
    assert_eq!(children(&x.world, a), [b, c, d, f]);
    assert_eq!(children(&x.world, b), [e]);
    assert_eq!(children(&x.world, f), [g]);
    for (copy, original) in [(a, x.a), (b, x.b), (c, x.c), (d, x.d), (e, x.e), (f, x.f)] {
        let at = x.world.get::<Transform>(copy);
        assert_eq!(at, x.world.get::<Transform>(original));
        assert_ne!(x.world.id(copy), Some(x.id(original)));
    }
    let sprite = x.world.get::<Sprite>(a).expect("copied");
    assert_eq!(sprite.texture, Some(x.texture), "opaque");
    assert_eq!(x.world.get::<Sealed>(g), Some(&Sealed(5)));
    let all = x.world.get::<Everything>(f).expect("copied");
    assert_eq!(all.target, Some(e), "its copy's");
    assert_eq!(all.handle, Handle(99));
    let all = x.world.get::<Everything>(x.f).expect("kept");
    assert_eq!(all.target, Some(x.e));
    check_ids(&x.world);
    check_links(&x.world);
    let mut again = Operation::Duplicate {
        entity: x.id(x.f),
        copies: Vec::new(),
    };
    let redo = x.world.apply(&mut again).expect("applied");
    let again = x.world.entity(again.entity().expect("written in"));
    let all = x.world.get::<Everything>(again.expect("alive"));
    assert_eq!(all.expect("copied").target, Some(x.e));
    x.world.release(redo);
    let extra = x.world.spawn(Transform::IDENTITY);
    x.world.set_parent(extra, Some(c)).expect("no cycle");
    for (code, case) in [
        ("ecs.has_children", "a child it didn't make"),
        ("ecs.no_entity", "a copy gone"),
    ] {
        if code == "ecs.no_entity" {
            x.world.despawn(extra).expect("alive");
            x.world.despawn(d).expect("alive");
        }
        let before = sorted(&x.world);
        let refused = x.world.undo(&mut inverse).expect_err(case);
        assert_eq!(refused.code(), code, "{refused}");
        assert!(same_snapshots(&sorted(&x.world), &before), "{case}");
    }
}

/// Refers to entities each way a value can, plus what a set refuses: a count above its max, and
/// a non-unit rotation in a variant with an entity.
#[derive(Clone, Debug, PartialEq, crate::Component)]
struct Links {
    to: Option<Entity>,
    all: Vec<Entity>,
    tie: Tie,
    #[reflect(max = 9)]
    count: u8,
}

#[derive(Clone, Debug, PartialEq, crate::Reflect)]
enum Tie {
    Turned { to: Entity, turn: Quat },
}

/// A duplicate retargets copies' references to originals at the copies, wherever held, without
/// setting the components again: the rest is copied bit for bit, even what a set would refuse.
#[test]
fn a_duplicate_points_references_at_the_copies_keeping_what_a_set_refuses() {
    let mut x = fixture();
    let drifted = Quat::from_xyzw(-0.0, 0.0, 0.6, 0.9);
    let links = Links {
        to: Some(x.e),
        all: vec![x.c, x.r],
        tie: Tie::Turned {
            to: x.d,
            turn: drifted,
        },
        count: 200,
    };
    x.world.insert(x.b, links.clone()).expect("alive");
    let mut duplicate = Operation::Duplicate {
        entity: x.id(x.a),
        copies: Vec::new(),
    };
    x.world.apply(&mut duplicate).expect("applied");
    let Operation::Duplicate { copies, .. } = &duplicate else {
        unreachable!("applied as it was")
    };
    let copied = copies.iter().map(|&id| x.world.entity(id).expect("alive"));
    let [_, b, c, d, e] = copied.collect::<Vec<Entity>>()[..] else {
        panic!("five copies: {copies:?}");
    };
    let copy = x.world.get::<Links>(b).expect("copied");
    let expected = Links {
        to: Some(e),
        all: vec![c, x.r],
        tie: Tie::Turned {
            to: d,
            turn: drifted,
        },
        count: 200,
    };
    assert_eq!(copy, &expected, "the outside one kept");
    let bits = |turn: Quat| turn.to_array().map(f32::to_bits);
    let Tie::Turned { turn, .. } = copy.tie;
    assert_eq!(bits(turn), bits(drifted));
    let original = x.world.get::<Links>(x.b);
    assert_eq!(original, Some(&links), "the original's kept");
    check_ids(&x.world);
    check_links(&x.world);
}

/// A duplicate's copies of an exclusive field are false: it stays true on the original alone.
#[test]
fn a_duplicates_exclusive_fields_are_false() {
    let (mut world, [lit, ..], _) = lamps();
    let mut duplicate = Operation::Duplicate {
        entity: world.id(lit).expect("alive"),
        copies: Vec::new(),
    };
    world.apply(&mut duplicate).expect("applied");
    let copy = world.entity(duplicate.entity().expect("written in"));
    let copy = world.get::<Lamp>(copy.expect("alive")).expect("copied");
    assert_eq!(
        copy,
        &Lamp {
            lit: false,
            watts: 40
        }
    );
    assert!(world.get::<Lamp>(lit).expect("kept").lit);
}

#[test]
fn a_delete_undone_gives_back_each_entity_under_its_handle() {
    let mut x = fixture();
    let taken = [x.e, x.b, x.c, x.d, x.a];
    let mut delete = Operation::Delete { entity: x.id(x.a) };
    let mut inverse = x.world.apply(&mut delete).expect("deleted");
    assert_eq!(inverse.kept(), taken.len());
    for gone in taken {
        assert!(!x.world.contains(gone), "{gone}");
        assert_eq!(x.world.entity(x.id(gone)), None, "{gone}");
    }
    assert_eq!(children(&x.world, x.r), [x.s]);
    // A reference into it dangles, as after a despawn: the world doesn't save.
    let roots = [x.r, x.f, x.g];
    let error = x.world.save_scene(&roots).expect_err("a reference dangles");
    assert_eq!(error.code(), "ecs.no_entity", "{error}");
    // Kept: its slots aren't reused by entities spawned meanwhile.
    let spawned: Vec<Entity> = (0..taken.len()).map(|_| x.world.spawn(())).collect();
    let elsewhere = |new: &Entity| taken.iter().all(|gone| gone.index != new.index);
    assert!(spawned.iter().all(elsewhere), "{spawned:?}");
    for new in spawned {
        x.world.despawn(new).expect("alive");
    }
    x.world.undo(&mut inverse).expect("undone");
    for gone in taken {
        assert_eq!(x.world.entity(x.id(gone)), Some(gone), "{gone}");
    }
    assert_eq!(children(&x.world, x.r), [x.s, x.a], "the root last");
    assert_eq!(children(&x.world, x.a), [x.b, x.c, x.d]);
    assert_eq!(children(&x.world, x.b), [x.e]);
    let all = x.world.get::<Everything>(x.f).expect("everything");
    assert_eq!(all.target, Some(x.e));
    x.world.save_scene(&roots).expect("whole again");
    let sprite = x.world.get::<Sprite>(x.a).expect("a sprite");
    assert_eq!(sprite.texture, Some(x.texture));
    check_ids(&x.world);
    check_links(&x.world);
    // Redone and dropped: gone for good, slots free at the next generation.
    x.world.undo(&mut inverse).expect("redone");
    x.world.release(inverse);
    let spawned: Vec<Entity> = (0..taken.len()).map(|_| x.world.spawn(())).collect();
    for gone in taken {
        assert!(!x.world.contains(gone), "{gone}");
        let generation = gone.generation.checked_add(1).expect("a generation to go");
        let next = Entity {
            index: gone.index,
            generation,
        };
        assert!(spawned.contains(&next), "{gone}'s slot: {spawned:?}");
    }
    check_ids(&x.world);
}

#[test]
fn an_add_builds_on_the_types_base() {
    let mut x = fixture();
    let mut add = Operation::Add {
        entity: x.id(x.s),
        component: "Sprite".into(),
        value: Sprite::default().to_value(),
    };
    x.world.apply(&mut add).expect("added");
    let sprite = x.world.get::<Sprite>(x.s).expect("a sprite");
    assert_eq!(
        sprite.texture,
        Sprite::default().texture,
        "a scene's sprite's"
    );
}

#[test]
fn a_refusal_changes_nothing() {
    let x = fixture();
    let [r, a, s, b, e, f] = [x.r, x.a, x.s, x.b, x.e, x.f].map(|entity| x.id(entity));
    let unknown = fresh();
    let transform = || named("Transform", Transform::IDENTITY.to_value());
    let create = |components| Operation::Create {
        entity: None,
        parent: None,
        components,
    };
    let too_many = Everything {
        count: 200,
        ..everything()
    };
    let set = |entity, path: &str, value| Operation::Set {
        entity,
        component: "Everything".into(),
        path: path.to_owned(),
        value,
    };
    let cases = [
        (Operation::Delete { entity: unknown }, "ecs.no_entity"),
        (
            Operation::Create {
                entity: Some(a),
                parent: None,
                components: Vec::new(),
            },
            "ecs.id_taken",
        ),
        (
            Operation::Create {
                entity: None,
                parent: Some(unknown),
                components: vec![transform()],
            },
            "ecs.no_entity",
        ),
        (
            create(vec![
                transform(),
                named("Velocity", Value::Struct(Vec::new())),
            ]),
            "reflect.unknown_type",
        ),
        (
            create(vec![transform(), named("Parent", Parent(x.r).to_value())]),
            "reflect.read_only",
        ),
        (
            create(vec![transform(), transform()]),
            "reflect.duplicate_component",
        ),
        (
            create(vec![transform(), named("Everything", too_many.to_value())]),
            "reflect.out_of_range",
        ),
        (
            create(vec![transform(), named("Sealed", Value::Opaque)]),
            "reflect.opaque",
        ),
        (
            Operation::Add {
                entity: a,
                component: "Transform".into(),
                value: transform().1,
            },
            "reflect.has_component",
        ),
        (
            Operation::Add {
                entity: s,
                component: "Sealed".into(),
                value: Value::Opaque,
            },
            "reflect.opaque",
        ),
        (
            Operation::Remove {
                entity: s,
                component: "Sprite".into(),
            },
            "reflect.missing_component",
        ),
        (
            Operation::Remove {
                entity: b,
                component: "Parent".into(),
            },
            "reflect.read_only",
        ),
        (set(f, "count", Value::UInt(200)), "reflect.out_of_range"),
        (set(unknown, "count", Value::UInt(1)), "ecs.no_entity"),
        (
            Operation::Reparent {
                entity: r,
                parent: Some(e),
            },
            "ecs.parent_cycle",
        ),
        (
            Operation::Reparent {
                entity: e,
                parent: Some(unknown),
            },
            "ecs.no_entity",
        ),
        (
            Operation::Duplicate {
                entity: unknown,
                copies: Vec::new(),
            },
            "ecs.no_entity",
        ),
        // `a`'s subtree is five.
        (
            Operation::Duplicate {
                entity: a,
                copies: vec![fresh()],
            },
            "ops.bad_copies",
        ),
        (
            Operation::Duplicate {
                entity: s,
                copies: vec![e],
            },
            "ecs.id_taken",
        ),
    ];
    for (mut operation, code) in cases {
        let (mut world, mut twin) = (fixture().world, fixture().world);
        let said = operation.to_string();
        let refused = world.apply(&mut operation).map(|_| ());
        let error = refused.expect_err(&said);
        assert_eq!(error.code(), code, "{said}: {error}");
        assert_eq!(operation.to_string(), said, "an ID written in");
        unchanged(&mut world, &mut twin, &said);
    }
    let mut unknown = create(vec![
        transform(),
        named("Velocity", Value::Struct(Vec::new())),
    ]);
    let error = fixture().world.apply(&mut unknown).map(|_| ());
    let error = error.expect_err("an unknown type");
    let says = "can't create an entity: there's no component type `Velocity`";
    assert!(error.to_string().starts_with(says), "{error}");
    let mut opaque = create(vec![named("Sealed", Value::Opaque)]);
    let error = fixture().world.apply(&mut opaque).map(|_| ());
    let error = error.expect_err("an opaque type");
    let says = "can't create an entity: at `Sealed`: ";
    assert!(error.to_string().starts_with(says), "{error}");
}

/// A set of the field at `path` of `f`'s `Everything`.
fn set(x: &Fixture, path: &str, value: Value) -> Operation {
    Operation::Set {
        entity: x.id(x.f),
        component: "Everything".into(),
        path: path.to_owned(),
        value,
    }
}

/// An ID no entity of the fixture's has.
fn fresh() -> EntityId {
    "0123456789abcdef".parse().expect("an ID")
}

#[test]
fn an_undo_the_world_has_moved_past_is_refused_whole() {
    // Each: an operation, a change code makes after it, and the code refusing its undo.
    type Case = (
        &'static str,
        fn(&Fixture) -> Operation,
        fn(&mut Fixture),
        &'static str,
    );
    let cases: [Case; 7] = [
        (
            "a set, then a despawn",
            |x| set(x, "count", Value::UInt(1)),
            |x| x.world.despawn(x.f).expect("alive"),
            "ecs.no_entity",
        ),
        (
            "a create, then a child given to what it made",
            |_| Operation::Create {
                entity: Some(fresh()),
                parent: None,
                components: Vec::new(),
            },
            |x| {
                let made = x.world.entity(fresh()).expect("created");
                x.world.set_parent(x.g, Some(made)).expect("no cycle");
            },
            "ecs.has_children",
        ),
        (
            "a remove, then an insert",
            |x| Operation::Remove {
                entity: x.id(x.a),
                component: "Sprite".into(),
            },
            |x| x.world.insert(x.a, Sprite::default()).expect("alive"),
            "reflect.has_component",
        ),
        (
            "a delete, then a load of a descendant's ID",
            |x| Operation::Delete { entity: x.id(x.a) },
            |x| {
                let id = x.id(x.e);
                let text = format!("(\n    entities: {{\n        \"{id}\": (),\n    }},\n)\n");
                x.world.load_scene(&text).expect("loaded");
            },
            "ecs.id_taken",
        ),
        (
            "a delete, then a despawn of its parent",
            |x| Operation::Delete { entity: x.id(x.c) },
            |x| x.world.despawn(x.a).expect("alive"),
            "ecs.no_entity",
        ),
        (
            "a reparent, then one its undo would cycle",
            |x| Operation::Reparent {
                entity: x.id(x.e),
                parent: Some(x.id(x.f)),
            },
            |x| {
                x.world.set_parent(x.b, Some(x.e)).expect("no cycle");
            },
            "ecs.parent_cycle",
        ),
        (
            "a set of a list's element, then the list shortened",
            |x| set(x, "list[1]", Value::UInt(10)),
            |x| {
                let all = x.world.get_mut::<Everything>(x.f).expect("everything");
                all.list.truncate(1);
            },
            "reflect.no_field",
        ),
    ];
    for (case, operation, change, code) in cases {
        let mut x = fixture();
        let mut operation = operation(&x);
        let inverse = x.world.apply(&mut operation);
        let mut inverse = inverse.unwrap_or_else(|error| panic!("{case}: {error}"));
        change(&mut x);
        let (before, version) = (sorted(&x.world), x.world.version());
        // Twice: a refusal leaves the inverse as it was.
        for _ in 0..2 {
            let error = x.world.undo(&mut inverse).expect_err(case);
            assert_eq!(error.code(), code, "{case}: {error}");
            assert!(same_snapshots(&sorted(&x.world), &before), "{case}");
            assert_eq!(x.world.version(), version, "{case}");
        }
        x.world.release(inverse);
    }
}

#[test]
fn an_inverse_is_its_worlds_own() {
    let mut x = fixture();
    let mut delete = Operation::Delete { entity: x.id(x.a) };
    let mut inverse = x.world.apply(&mut delete).expect("deleted");
    let mut other = fixture();
    let before = sorted(&other.world);
    let error = other.world.undo(&mut inverse).expect_err("another world's");
    assert_eq!(error.code(), "ops.other_world", "{error}");
    // Dropped there, it frees none of that world's slots.
    other.world.release(inverse);
    assert!(same_snapshots(&sorted(&other.world), &before));
    let next = other.world.spawn(());
    assert!(
        other
            .ids
            .iter()
            .all(|(entity, _)| entity.index != next.index)
    );
    check_ids(&other.world);
}

/// A lamp: operations keep at most one lit.
#[derive(Clone, Debug, PartialEq, crate::Reflect)]
pub(in crate::ecs) struct Lamp {
    lit: bool,
    watts: u32,
}

impl crate::Component for Lamp {
    fn exclusive() -> Option<&'static str> {
        Some("lit")
    }
}

impl crate::Writable for Lamp {}

/// Three lamps, the first two lit by code, and an entity without one.
pub(in crate::ecs) fn lamps() -> (World, [Entity; 3], Entity) {
    let mut world = World::seeded(0x1a3b);
    let lamps = [true, true, false].map(|lit| world.spawn(Lamp { lit, watts: 40 }));
    let bare = world.spawn(());
    (world, lamps, bare)
}

/// Whether each of `lamps` is lit.
pub(in crate::ecs) fn lit(world: &World, lamps: [Entity; 3]) -> [bool; 3] {
    lamps.map(|lamp| world.get::<Lamp>(lamp).expect("a lamp").lit)
}

/// A create, add or set that lights a lamp puts every other out, and undoes and redoes as one
/// operation. One that puts a lamp out, or sets another field, puts none out.
#[test]
fn an_operation_that_makes_an_exclusive_field_true_makes_it_false_on_every_other_entity() {
    let (mut world, lamps, bare) = lamps();
    let [a, _, c] = lamps.map(|lamp| world.id(lamp).expect("alive"));
    let bare = world.id(bare).expect("alive");
    let lamp = Lamp {
        lit: true,
        watts: 75,
    };
    let set = |entity, path: &str, value| Operation::Set {
        entity,
        component: "Lamp".into(),
        path: path.to_owned(),
        value,
    };
    for (operation, after) in [
        (set(c, "lit", Value::Bool(true)), [false, false, true]),
        (set(c, "", lamp.to_value()), [false, false, true]),
        (set(a, "lit", Value::Bool(true)), [true, false, false]),
        (
            Operation::Create {
                entity: None,
                parent: None,
                components: vec![named("Lamp", lamp.to_value())],
            },
            [false, false, false],
        ),
        (
            Operation::Add {
                entity: bare,
                component: "Lamp".into(),
                value: lamp.to_value(),
            },
            [false, false, false],
        ),
        (set(a, "lit", Value::Bool(false)), [false, true, false]),
        (set(c, "watts", Value::UInt(60)), [true, true, false]),
    ] {
        let said = operation.to_string();
        let mut redo = round_trip(&mut world, operation);
        assert_eq!(lit(&world, lamps), [true, true, false], "{said}");
        world.undo(&mut redo).expect("redone");
        assert_eq!(lit(&world, lamps), after, "{said}");
        world.undo(&mut redo).expect("undone");
        world.release(redo);
    }
}

/// An undo that would relight a lamp since gone or no longer a lamp is refused, changing nothing.
#[test]
fn an_undo_that_would_set_an_exclusive_field_again_where_it_cant_is_refused_whole() {
    type Case = (&'static str, fn(&mut World, Entity), &'static str);
    let cases: [Case; 2] = [
        (
            "a despawn",
            |world, lamp| world.despawn(lamp).expect("alive"),
            "ecs.no_entity",
        ),
        (
            "a remove",
            |world, lamp| assert!(world.remove::<Lamp>(lamp).expect("alive").is_some()),
            "reflect.missing_component",
        ),
    ];
    for (case, change, code) in cases {
        let (mut world, lamps, _) = lamps();
        let mut light = Operation::Set {
            entity: world.id(lamps[2]).expect("alive"),
            component: "Lamp".into(),
            path: "lit".to_owned(),
            value: Value::Bool(true),
        };
        let mut inverse = world.apply(&mut light).expect("lit");
        change(&mut world, lamps[0]);
        let (before, version) = (sorted(&world), world.version());
        // Twice: a refusal leaves the inverse as it was.
        for _ in 0..2 {
            let error = world.undo(&mut inverse).expect_err(case);
            assert_eq!(error.code(), code, "{case}: {error}");
            assert!(same_snapshots(&sorted(&world), &before), "{case}");
            assert_eq!(world.version(), version, "{case}");
        }
        world.release(inverse);
    }
}

/// An exclusive field the component lacks.
#[derive(Clone, Debug, PartialEq, crate::Reflect)]
struct Unlit {
    watts: u32,
}

impl crate::Component for Unlit {
    fn exclusive() -> Option<&'static str> {
        Some("lit")
    }
}

/// An exclusive field that isn't a `bool`.
#[derive(Clone, Debug, PartialEq, crate::Reflect)]
struct Dimmed {
    lit: u32,
}

impl crate::Component for Dimmed {
    fn exclusive() -> Option<&'static str> {
        Some("lit")
    }
}

#[test]
fn a_components_exclusive_field_is_a_bool_of_its_own() {
    let mut world = World::default();
    let described = world.types().iter().len();
    for registered in [world.register::<Unlit>(), world.register::<Dimmed>()] {
        let error = registered.expect_err("refused");
        assert_eq!(error.code(), "ecs.bad_exclusive", "{error}");
    }
    assert_eq!(world.component_types().count(), 1, "`Parent` alone");
    // Nor are their descriptions kept: they'd take their names.
    assert_eq!(world.types().iter().len(), described);
    world.register::<Lamp>().expect("registered");
}

/// A delete-clearing field that isn't an `Option<Entity>`.
#[derive(Clone, Debug, PartialEq, crate::Reflect)]
struct Leash {
    to: Option<Entity>,
    length: u32,
}

impl crate::Component for Leash {
    fn cleared() -> &'static [&'static str] {
        &["to", "length"]
    }
}

#[test]
fn a_field_a_delete_clears_is_an_optional_entity_of_its_own() {
    let mut world = World::default();
    let error = world.register::<Leash>().expect_err("refused");
    assert_eq!(error.code(), "ecs.bad_cleared", "{error}");
}

/// An element of [`Slots`]' list: a number and an opaque handle.
#[derive(Clone, Debug, PartialEq, crate::Reflect)]
struct Slot {
    at: u32,
    #[reflect(opaque)]
    handle: Handle,
}

/// A list of slots, and a form that may hold a handle.
#[derive(Clone, Debug, PartialEq, crate::Reflect)]
struct Slots {
    slots: Vec<Slot>,
    form: crate::reflect::tests::Form,
}

impl crate::Component for Slots {
    fn base() -> Option<Self> {
        Some(Self {
            slots: Vec::new(),
            form: crate::reflect::tests::Form::Point,
        })
    }
}

impl crate::Writable for Slots {}

/// - A set that takes an element out of a list leaves each later element its own opaque state,
///   not its predecessor's; one that puts an element back (as an undo does) does too.
/// - Undo gives back the element taken out, state included, and the variant switched from,
///   whose state the new variant lacks. Redo takes them out again.
#[test]
fn opaque_state_stays_with_its_element_and_comes_back_with_an_undo() {
    use crate::reflect::tests::Form;
    let mut world = World::seeded(0x5107);
    world.register::<Slots>().expect("registered");
    let slot = |at, handle| Slot {
        at,
        handle: Handle(handle),
    };
    let held = Slots {
        slots: vec![slot(1, 10), slot(2, 20), slot(3, 30)],
        form: Form::Held(Handle(7)),
    };
    let entity = world.spawn(held.clone());
    let id = world.id(entity).expect("alive");
    let set = |path: &str, value: Value| Operation::Set {
        entity: id,
        component: "Slots".into(),
        path: path.to_owned(),
        value,
    };
    let get = |world: &World| world.get::<Slots>(entity).expect("slots").clone();
    let slots = |slots: &[Slot]| Slots {
        slots: slots.to_vec(),
        form: Form::Held(Handle(7)),
    };
    let shrunk = vec![slot(1, 10), slot(3, 30)];
    let value = slots(&shrunk).to_value();
    let field = |value: &Value| match value {
        Value::Struct(fields) => fields[0].1.clone(),
        _ => unreachable!("a struct's"),
    };
    let mut inverse = world.apply(&mut set("slots", field(&value))).expect("set");
    assert_eq!(get(&world), slots(&shrunk));
    let mut first = world
        .apply(&mut set("", slots(&[slot(3, 30)]).to_value()))
        .expect("set whole");
    assert_eq!(get(&world), slots(&[slot(3, 30)]));
    world.undo(&mut first).expect("undone");
    assert_eq!(get(&world), slots(&shrunk));
    world.release(first);
    world.undo(&mut inverse).expect("undone");
    assert_eq!(get(&world), held);
    world.undo(&mut inverse).expect("redone");
    assert_eq!(get(&world), slots(&shrunk));
    world.undo(&mut inverse).expect("undone again");
    assert_eq!(get(&world), held);
    world.release(inverse);
    // One put in leaves later ones their state but can't make its own.
    let grown = slots(&[slot(1, 10), slot(9, 0), slot(2, 20), slot(3, 30)]);
    let Err(error) = world.apply(&mut set("slots", field(&grown.to_value()))) else {
        panic!("an opaque handle made");
    };
    assert_eq!(error.code(), "reflect.opaque", "{error}");
    assert_eq!(get(&world), held);
    // The variant switched from comes back with its handle, though the world changed since.
    let mut switched = world
        .apply(&mut set("form", Form::Point.to_value()))
        .expect("switched");
    assert_eq!(get(&world).form, Form::Point);
    let mut other = world
        .apply(&mut set("slots[0].at", Value::UInt(5)))
        .expect("set");
    world.release(other);
    world.undo(&mut switched).expect("undone");
    let back = get(&world);
    assert_eq!(back.form, Form::Held(Handle(7)));
    assert_eq!(back.slots[0], slot(5, 10), "the rest as it is now");
    world.undo(&mut switched).expect("redone");
    assert_eq!(get(&world).form, Form::Point);
    world.undo(&mut switched).expect("undone again");
    assert_eq!(get(&world).form, Form::Held(Handle(7)));
    world.release(switched);
    // A set of a field holding nothing opaque keeps no copy: its undo needs none.
    other = world
        .apply(&mut set("slots[1].at", Value::UInt(6)))
        .expect("set");
    assert!(!other.keeps_before());
    world.release(other);
}

/// A form that may hold a handle, a list of slots, a slot and a turn; code changes all freely.
#[derive(Clone, Debug, PartialEq, crate::Reflect)]
struct Rigged {
    form: crate::reflect::tests::Form,
    slots: Vec<Slot>,
    cursor: Slot,
    turn: Quat,
}

impl crate::Component for Rigged {
    fn base() -> Option<Self> {
        None
    }
}

impl crate::Writable for Rigged {}

/// Undoing a set that switched away from a variant with opaque state restores that field alone,
/// state included. The rest of the component stays as code left it: a swapped-in handle, a
/// grown list of elements no value could make, a non-unit turn a set would refuse. A set keeps
/// the component for its undo only where it may drop opaque state: a variant switch or a list
/// of another length.
#[test]
fn an_undo_from_the_component_kept_puts_back_its_field_alone() {
    use crate::reflect::tests::Form;
    let mut world = World::seeded(0x5108);
    world.register::<Rigged>().expect("registered");
    let slot = |at, handle| Slot {
        at,
        handle: Handle(handle),
    };
    let entity = world.spawn(Rigged {
        form: Form::Held(Handle(7)),
        slots: vec![slot(1, 10)],
        cursor: slot(0, 50),
        turn: Quat::IDENTITY,
    });
    let id = world.id(entity).expect("alive");
    let set = |path: &str, value: Value| Operation::Set {
        entity: id,
        component: "Rigged".into(),
        path: path.to_owned(),
        value,
    };
    let mut switched = world
        .apply(&mut set("form", Form::Point.to_value()))
        .expect("switched");
    assert!(switched.keeps_before());
    let rigged = world.get_mut::<Rigged>(entity).expect("rigged");
    rigged.cursor.handle = Handle(77);
    rigged.slots.push(slot(2, 20));
    rigged.turn = Quat::from_xyzw(0.0, 0.0, 0.0, 2.0);
    world.undo(&mut switched).expect("undone");
    let rigged = world.get::<Rigged>(entity).expect("rigged");
    assert_eq!(rigged.form, Form::Held(Handle(7)));
    assert_eq!(rigged.cursor, slot(0, 77), "the handle code swapped in");
    assert_eq!(
        rigged.slots,
        [slot(1, 10), slot(2, 20)],
        "the list code grew"
    );
    assert_eq!(rigged.turn, Quat::from_xyzw(0.0, 0.0, 0.0, 2.0));
    world.undo(&mut switched).expect("redone");
    assert_eq!(
        world.get::<Rigged>(entity).expect("rigged").form,
        Form::Point
    );
    world.release(switched);
    // Same length and variants: nothing opaque can go, so nothing's kept.
    let slots = vec![slot(3, 0), slot(4, 0)];
    let same = |slots: &[Slot]| Value::List(slots.iter().map(Reflect::to_value).collect());
    let mut kept = world.apply(&mut set("slots", same(&slots))).expect("set");
    assert!(!kept.keeps_before());
    world.release(kept);
    let cursor = slot(5, 0).to_value();
    kept = world.apply(&mut set("cursor", cursor)).expect("set");
    assert!(!kept.keeps_before());
    world.release(kept);
    let rigged = world.get::<Rigged>(entity).expect("rigged");
    assert_eq!(rigged.slots, [slot(3, 10), slot(4, 20)]);
    assert_eq!(rigged.cursor, slot(5, 77));
    // Shorter, or a variant switched: kept.
    kept = world
        .apply(&mut set("slots", same(&slots[..1])))
        .expect("set");
    assert!(kept.keeps_before());
    world.release(kept);
    kept = world
        .apply(&mut set("form", Form::Circle(1.0).to_value()))
        .expect("set");
    assert!(
        !kept.keeps_before(),
        "switched from a variant holding nothing opaque"
    );
    world.release(kept);
    // In a list of another length, a removed element's state passes to the next: the values
    // can't tell them apart.
    let list = vec![slot(1, 10), slot(2, 20), slot(3, 30)];
    let set_to = list.with_value(&same(&[slot(1, 0), slot(9, 0)]));
    assert_eq!(set_to.expect("set"), [slot(1, 10), slot(9, 20)]);
}
