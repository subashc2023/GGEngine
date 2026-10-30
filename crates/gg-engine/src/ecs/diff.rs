//! What tells two worlds apart, as their hashes' content does: the first entity, component and
//! field where they differ, which names where a replay went another way; and a world written a
//! field a line, so that the dumps of two machines' worlds diff to the field where they part.

use super::{Entity, EntityId, World};
use crate::recording::text;
use crate::reflect::{self, Value};

/// Where two worlds first differ, as [`World::difference`] finds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Difference {
    /// The entity, by its ID.
    pub entity: EntityId,
    /// Its component's type, by name; `None` where one world alone has the entity.
    pub component: Option<String>,
    /// The field's path in the component, as [`World::get_field`] reads it: empty for the whole
    /// component, as where one world alone has it.
    pub path: String,
    /// The field's value in the world `difference` was called on; `None` where it hasn't the
    /// entity, or its component.
    pub ours: Option<Value>,
    /// The field's value in the other world, as `ours` is.
    pub theirs: Option<Value>,
}

impl World {
    /// The first place where this world's content and `other`'s differ, as their hashes'
    /// [`content`](crate::WorldHash::content) tells them apart: the entities in the order of their
    /// IDs, each one's components in the order of their types' names, and each component's fields
    /// in the order its type declares them, down through structs, lists, arrays and vectors;
    /// options, variants and quaternions are compared whole, as paths take them. Floats are
    /// compared by their bits, every NaN as one and -0.0 apart from 0.0, an entity held by its ID
    /// in each world, or as none if it's dead there, and what's opaque as the same. An entity in one
    /// world alone differs, even without components, which the content leaves to the layout.
    /// `None` where nothing differs, though the layouts may: where the worlds hold their entities,
    /// and where their streams of IDs are.
    ///
    /// It reads the worlds by name, and allocates, for a replay that has gone another way.
    pub fn difference(&self, other: &World) -> Option<Difference> {
        let _zone = gg_core::zone!();
        let mut ids: Vec<EntityId> = ids(self).chain(ids(other)).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().find_map(|id| differs(id, self, other))
    }

    /// The world's content as text, a line for each field, as [`difference`](Self::difference)
    /// walks them: the entity's ID, the component's type and the field's path, joined as an error
    /// names them, such as `Sprite.color.x`, and the value, as a recording writes it, with every NaN
    /// as one, as the hash takes them; and a line of its ID alone for an entity without components.
    /// Two machines' dumps of worlds that should be the same, diffed, give the field where they part.
    pub(crate) fn dump(&self) -> String {
        let _zone = gg_core::zone!();
        let mut ids: Vec<EntityId> = ids(self).collect();
        ids.sort_unstable();
        let mut out = String::new();
        for id in ids {
            let entity = self.entity(id).expect("a living entity's ID");
            let mut names: Vec<&str> = components(self, entity).collect();
            if names.is_empty() {
                out.push_str(&format!("{id}\n"));
            }
            names.sort_unstable();
            for name in names {
                let value = self.get_field(entity, name, "");
                let value = value.expect("a component reads whole");
                leaves(&mut out, self, &format!("{id} {name}"), &value);
            }
        }
        out
    }
}

/// The IDs of `world`'s entities, in no order.
fn ids(world: &World) -> impl Iterator<Item = EntityId> + '_ {
    let ids = world.query::<Entity>().map(|entity| world.id(entity));
    ids.map(|id| id.expect("a living entity has an ID"))
}

/// The names of the types of `entity`'s components, which is alive.
fn components(world: &World, entity: Entity) -> impl Iterator<Item = &str> {
    world.components_of(entity).expect("the entity is alive")
}

/// Where the entity whose ID is `id`, which one world at least has, first differs between `ours`
/// and `theirs`.
fn differs(id: EntityId, ours: &World, theirs: &World) -> Option<Difference> {
    let at = |component: Option<&str>, path: String, ours, theirs| {
        let component = component.map(str::to_owned);
        Some(Difference {
            entity: id,
            component,
            path,
            ours,
            theirs,
        })
    };
    let (Some(a), Some(b)) = (ours.entity(id), theirs.entity(id)) else {
        return at(None, String::new(), None, None);
    };
    let mut names: Vec<&str> = components(ours, a).chain(components(theirs, b)).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        match (ours.get_field(a, name, ""), theirs.get_field(b, name, "")) {
            (Ok(x), Ok(y)) => {
                if let Some((path, x, y)) = first(&x, ours, &y, theirs, String::new()) {
                    return at(Some(name), path, Some(x), Some(y));
                }
            }
            (x, y) => return at(Some(name), String::new(), x.ok(), y.ok()),
        }
    }
    None
}

/// The path, from `path`, of the first part where `a`, in `ours`, and `b`, in `theirs`, differ,
/// and their values there.
fn first(
    a: &Value,
    ours: &World,
    b: &Value,
    theirs: &World,
    path: String,
) -> Option<(String, Value, Value)> {
    let floats = |a: &[f32], b: &[f32]| {
        let mut parts = reflect::COMPONENTS.into_iter().zip(a.iter().zip(b));
        let (name, (&x, &y)) = parts.find(|(_, (x, y))| !same_f32(**x, **y))?;
        Some((reflect::join(&path, name), Value::F32(x), Value::F32(y)))
    };
    match (a, b) {
        (Value::Struct(x), Value::Struct(y)) if same_names(x, y) => {
            let mut fields = x.iter().zip(y);
            fields.find_map(|((name, x), (_, y))| {
                first(x, ours, y, theirs, reflect::join(&path, name))
            })
        }
        (Value::List(x), Value::List(y)) if x.len() == y.len() => {
            let mut items = x.iter().zip(y).enumerate();
            items.find_map(|(i, (x, y))| first(x, ours, y, theirs, format!("{path}[{i}]")))
        }
        (Value::Vec2(x), Value::Vec2(y)) => floats(x.as_ref(), y.as_ref()),
        (Value::Vec3(x), Value::Vec3(y)) => floats(x.as_ref(), y.as_ref()),
        (Value::Vec4(x), Value::Vec4(y)) => floats(x.as_ref(), y.as_ref()),
        _ => (!same(a, ours, b, theirs)).then(|| (path, a.clone(), b.clone())),
    }
}

/// Whether `a`, in `ours`, and `b`, in `theirs`, are the same, as the world's hash tells values
/// apart.
fn same(a: &Value, ours: &World, b: &Value, theirs: &World) -> bool {
    let floats = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(&x, &y)| same_f32(x, y));
    let fields = |x: &[(reflect::Ident, Value)], y: &[(reflect::Ident, Value)]| {
        let mut both = x.iter().zip(y);
        same_names(x, y) && both.all(|((_, x), (_, y))| same(x, ours, y, theirs))
    };
    match (a, b) {
        (&Value::F32(x), &Value::F32(y)) => same_f32(x, y),
        (&Value::F64(x), &Value::F64(y)) => x.to_bits() == y.to_bits() || x.is_nan() && y.is_nan(),
        (Value::Vec2(x), Value::Vec2(y)) => floats(x.as_ref(), y.as_ref()),
        (Value::Vec3(x), Value::Vec3(y)) => floats(x.as_ref(), y.as_ref()),
        (Value::Vec4(x), Value::Vec4(y)) => floats(x.as_ref(), y.as_ref()),
        (Value::Quat(x), Value::Quat(y)) => floats(&x.to_array(), &y.to_array()),
        (&Value::Entity(x), &Value::Entity(y)) => ours.id(x) == theirs.id(y),
        (&Value::Texture(x), &Value::Texture(y)) => {
            ours.assets().path(x) == theirs.assets().path(y)
        }
        (Value::Option(x), Value::Option(y)) => match (x, y) {
            (Some(x), Some(y)) => same(x, ours, y, theirs),
            (x, y) => x.is_none() && y.is_none(),
        },
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, ours, y, theirs))
        }
        (Value::Struct(x), Value::Struct(y)) => fields(x, y),
        (Value::Variant(m, x), Value::Variant(n, y)) => m == n && fields(x, y),
        // Booleans, integers, strings, what's opaque, and values of other kinds.
        _ => a == b,
    }
}

/// By their bits, every NaN as one, and -0.0 apart from 0.0, as the hash takes them.
fn same_f32(x: f32, y: f32) -> bool {
    x.to_bits() == y.to_bits() || x.is_nan() && y.is_nan()
}

/// Whether two structs' or variants' fields are the same fields, in the same order.
fn same_names(x: &[(reflect::Ident, Value)], y: &[(reflect::Ident, Value)]) -> bool {
    x.len() == y.len() && x.iter().zip(y).all(|((m, _), (n, _))| m == n)
}

/// Writes a line to `out` for each field of `value`, at `at`, as [`World::dump`] does.
fn leaves(out: &mut String, world: &World, at: &str, value: &Value) {
    let floats = |out: &mut String, xs: &[f32]| {
        for (name, &x) in reflect::COMPONENTS.into_iter().zip(xs) {
            leaves(out, world, &reflect::join(at, name), &Value::F32(x));
        }
    };
    match value {
        Value::Struct(fields) if !fields.is_empty() => {
            for (name, value) in fields {
                leaves(out, world, &reflect::join(at, name), value);
            }
        }
        Value::List(items) if !items.is_empty() => {
            for (i, item) in items.iter().enumerate() {
                leaves(out, world, &format!("{at}[{i}]"), item);
            }
        }
        Value::Vec2(v) => floats(out, v.as_ref()),
        Value::Vec3(v) => floats(out, v.as_ref()),
        Value::Vec4(v) => floats(out, v.as_ref()),
        _ => {
            out.push_str(at);
            out.push(' ');
            text::write_folded(out, value, world);
            out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::Vec2;

    use super::*;
    use crate::Transform;
    use crate::reflect::tests::{Everything, everything};

    #[derive(Clone, Copy, crate::Component)]
    struct Count(u32);

    /// Two worlds of the same entities, by their IDs, the second loaded from the first's scene
    /// into other slots: one that holds another, which is a third's child.
    fn twins() -> (World, World, [Entity; 3]) {
        let mut world = World::seeded(5);
        let child = world.spawn(Transform::IDENTITY);
        let holder = world.spawn(Everything {
            target: Some(child),
            ..everything()
        });
        let parent = world.spawn(Transform::IDENTITY);
        world.set_parent(child, Some(parent)).expect("no cycle");
        let text = world.save_scene(&[holder, parent]).expect("saved");
        let mut other = World::seeded(6);
        other.register::<Transform>().expect("registered");
        other.register::<Everything>().expect("registered");
        let spare: Vec<Entity> = (0..3).map(|_| other.spawn(())).collect();
        for entity in spare {
            other.despawn(entity).expect("alive");
        }
        other.load_scene(&text).expect("loaded");
        (world, other, [holder, child, parent])
    }

    /// The twin in `other` of `entity`, of `world`.
    fn twin(world: &World, other: &World, entity: Entity) -> Entity {
        other
            .entity(world.id(entity).expect("alive"))
            .expect("loaded")
    }

    #[test]
    fn the_same_entities_in_other_slots_are_the_same() {
        let (world, other, [holder, ..]) = twins();
        assert_ne!(holder, twin(&world, &other, holder), "in another slot");
        assert_eq!(world.hash().content, other.hash().content);
        assert_eq!(world.difference(&other), None);
        assert_eq!(world.dump(), other.dump());
    }

    #[test]
    fn the_first_difference_names_its_entity_component_and_field() {
        let (mut world, other, [holder, ..]) = twins();
        let mut set = |path, value| {
            let set = world.set_field(holder, "Everything", path, &value);
            set.expect("set");
        };
        set("rect[1].y", Value::F32(2.0));
        // A later field, which comes second.
        set("pair.1", Value::Int(9));
        let id = world.id(holder).expect("alive");
        let expected = Difference {
            entity: id,
            component: Some("Everything".to_owned()),
            path: "rect[1].y".to_owned(),
            ours: Some(Value::F32(2.0)),
            theirs: Some(Value::F32(1.0)),
        };
        assert_eq!(world.difference(&other), Some(expected));
        // Their dumps differ in those fields' lines alone.
        let (ours, theirs) = (world.dump(), other.dump());
        assert_eq!(ours.lines().count(), theirs.lines().count());
        let lines = ours.lines().zip(theirs.lines());
        let differ: Vec<(String, String)> = lines
            .filter(|(a, b)| a != b)
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect();
        let line = |path: &str, value: &str| format!("{id} Everything.{path} {value}");
        let expected = [
            (line("rect[1].y", "F32(2.0)"), line("rect[1].y", "F32(1.0)")),
            (line("pair.1", "Int(9)"), line("pair.1", "Int(-4)")),
        ];
        assert_eq!(differ, expected);
    }

    #[test]
    fn floats_differ_as_the_hash_tells_them_apart() {
        let (mut world, mut other, [holder, ..]) = twins();
        let theirs = twin(&world, &other, holder);
        let angle = |world: &mut World, entity, bits: u32| {
            let all = world.get_mut::<Everything>(entity).expect("everything");
            all.angle = f32::from_bits(bits);
        };
        // NaNs of other signs and payloads are one.
        angle(&mut world, holder, 0x7fc0_0000);
        angle(&mut other, theirs, 0xffc1_2345);
        assert_eq!(world.difference(&other), None);
        assert_eq!(world.dump(), other.dump());
        // -0.0 isn't 0.0.
        angle(&mut world, holder, 0.0f32.to_bits());
        angle(&mut other, theirs, (-0.0f32).to_bits());
        let difference = world.difference(&other).expect("a difference");
        assert_eq!(difference.path, "angle");
        let (Some(Value::F32(x)), Some(Value::F32(y))) = (difference.ours, difference.theirs)
        else {
            panic!("two floats");
        };
        assert_eq!(
            (x.to_bits(), y.to_bits()),
            (0.0f32.to_bits(), (-0.0f32).to_bits())
        );
        // A vector's component, by its name.
        angle(&mut world, holder, (-0.0f32).to_bits());
        let all = world.get_mut::<Everything>(holder).expect("everything");
        all.rect[0] = Vec2::new(0.0, -0.0);
        let difference = world.difference(&other).expect("a difference");
        assert_eq!(difference.path, "rect[0].y");
    }

    #[test]
    fn an_entity_held_is_the_same_by_its_id_in_each_world() {
        let (mut world, other, [holder, child, parent]) = twins();
        let target = |entity| Value::Option(Some(Box::new(Value::Entity(entity))));
        let set = world.set_field(holder, "Everything", "target", &target(parent));
        set.expect("set");
        let difference = world.difference(&other).expect("a difference");
        assert_eq!(difference.path, "target");
        let held = |value: Option<Value>, world: &World| match value {
            Some(Value::Option(Some(held))) => match *held {
                Value::Entity(entity) => world.id(entity),
                _ => None,
            },
            _ => None,
        };
        assert_eq!(held(difference.ours, &world), world.id(parent));
        assert_eq!(held(difference.theirs, &other), world.id(child));
    }

    #[test]
    fn an_entity_or_a_component_in_one_world_alone_differs() {
        let (mut world, other, [_, child, _]) = twins();
        world.insert(child, Count(1)).expect("alive");
        let difference = world.difference(&other).expect("a difference");
        assert_eq!(difference.component.as_deref(), Some("Count"));
        assert_eq!((difference.path.as_str(), &difference.theirs), ("", &None));
        assert!(difference.ours.is_some());
        world.remove::<Count>(child).expect("alive");
        assert_eq!(world.difference(&other), None);
        // Without components, which the content hash leaves out.
        let spawned = world.spawn(());
        let expected = Difference {
            entity: world.id(spawned).expect("alive"),
            component: None,
            path: String::new(),
            ours: None,
            theirs: None,
        };
        assert_eq!(other.difference(&world), Some(expected.clone()));
        let alone = expected.entity.to_string();
        assert!(world.dump().lines().any(|line| line == alone));
    }

    #[test]
    fn entities_are_walked_by_their_ids_not_their_tables() {
        let mut world = World::seeded(8);
        let entities: Vec<Entity> = (0..8).map(|i| world.spawn(Count(i))).collect();
        let other = world.clone();
        for &entity in &entities {
            world.get_mut::<Count>(entity).expect("alive").0 += 100;
        }
        let lowest = entities.iter().min_by_key(|&&entity| world.id(entity));
        let lowest = *lowest.expect("entities");
        assert_ne!(lowest, entities[0], "the lowest ID isn't the first row");
        let difference = world.difference(&other).expect("a difference");
        assert_eq!(Some(difference.entity), world.id(lowest));
        assert_eq!(difference.path, "0");
    }

    #[test]
    fn a_layout_alone_is_no_difference() {
        let mut world = World::seeded(9);
        let entities: Vec<Entity> = (0..4).map(|i| world.spawn(Count(i))).collect();
        let other = world.clone();
        // To the end of its table, through another.
        world
            .insert(entities[0], Transform::IDENTITY)
            .expect("alive");
        world.remove::<Transform>(entities[0]).expect("alive");
        assert_ne!(world.hash().layout, other.hash().layout);
        assert_eq!(world.difference(&other), None);
        assert_eq!(world.dump(), other.dump());
    }
}
