//! What tells two worlds apart, as their hashes' content does: the first differing entity,
//! component and field (where a replay diverged), and a world written a field a line, so two
//! machines' dumps diff to the field where they part.

#[cfg(feature = "dev")]
use std::collections::{HashMap, HashSet};

#[cfg(feature = "dev")]
use gg_core::Result;

#[cfg(feature = "dev")]
use super::ops::Operation;
#[cfg(feature = "dev")]
use super::scene::Expanding;
use super::{Entity, EntityId, World};
#[cfg(feature = "dev")]
use crate::ModelNode;
use crate::recording::text;
use crate::reflect::{self, Value};

/// Where two worlds first differ, as [`World::difference`] finds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Difference {
    /// The entity, by its ID.
    pub entity: EntityId,
    /// Its component's type, by name; `None` where one world alone has the entity.
    pub component: Option<String>,
    /// The field's path, as [`World::get_field`] reads it: empty for the whole component, as
    /// where one world alone has it.
    pub path: String,
    /// The field's value in the world `difference` was called on; `None` if it lacks the entity
    /// or component.
    pub ours: Option<Value>,
    /// The field's value in the other world.
    pub theirs: Option<Value>,
}

/// A [`Difference`] as a diagnostic writes it, by [`Difference::written`].
pub(crate) struct Written {
    /// The component's field as [`reflect::join`] writes it; empty where one world alone has the
    /// entity.
    pub(crate) field: String,
    /// Where it is: the entity's ID and its field, if any.
    pub(crate) at: String,
    /// The value in the world `difference` was called on, as recordings write values; `none`
    /// without the component. For an entity one world alone has: `alive` or `none`.
    pub(crate) ours: String,
    /// The same in the other world.
    pub(crate) theirs: String,
}

impl Difference {
    /// As a diagnostic writes it; `ours` is the world [`World::difference`] was called on.
    pub(crate) fn written(&self, ours: &World, theirs: &World) -> Written {
        let entity = self.entity;
        let Some(component) = &self.component else {
            let alive = |world: &World| {
                let alive = world.entity(entity).is_some();
                if alive { "alive" } else { "none" }.to_owned()
            };
            return Written {
                field: String::new(),
                at: entity.to_string(),
                ours: alive(ours),
                theirs: alive(theirs),
            };
        };
        let value = |value: &Option<Value>, world: &World| {
            let mut written = String::new();
            match value {
                Some(value) => text::write_folded(&mut written, value, world),
                None => written.push_str("none"),
            }
            written
        };
        let field = reflect::join(component, &self.path);
        Written {
            at: format!("{entity}'s `{field}`"),
            field,
            ours: value(&self.ours, ours),
            theirs: value(&self.theirs, theirs),
        }
    }
}

impl World {
    /// The first place this world's content and `other`'s differ, as their hashes'
    /// [`content`](crate::WorldHash::content) tells them apart.
    ///
    /// The walk goes by entity ID, component type name, then fields in declared order, down
    /// through structs, lists, arrays and vectors. Options, variants and quaternions compare
    /// whole, as paths take them. Floats compare by bits, every NaN as one, -0.0 apart from 0.0;
    /// an entity by its ID in each world, or none if dead there; opaque values as equal.
    ///
    /// An entity in one world alone differs, even without components, which the content leaves to
    /// the layout. `None` where nothing differs, though layouts (where entities sit, where ID
    /// streams are) may.
    ///
    /// It reads by name and allocates, for a replay that has diverged.
    pub fn difference(&self, other: &World) -> Option<Difference> {
        let _zone = gg_core::zone!();
        let mut ids: Vec<EntityId> = ids(self).chain(ids(other)).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().find_map(|id| differs(id, self, other))
    }

    /// The operations taking this world from what scene `old` held to what `new` holds, as when a
    /// scene file a game loaded is saved while it runs:
    ///
    /// - an entity the file now holds is created; one it held no more is deleted with its
    ///   descendants;
    /// - a component it gives is added, one it takes away is removed;
    /// - each field it changes is set, as [`difference`](Self::difference) walks them;
    /// - a parent it changes is changed.
    ///
    /// What the world holds already (a game's own change) is left out, and so is an entity the
    /// file held that the world has since deleted. The world keeps what the file doesn't change.
    ///
    /// New entities come first, alone, as [`SceneEdit::Creates`], so the rest may hold them once
    /// made; the caller asks again for the rest. So do changed instance links:
    ///
    /// - a removed link unlinks the instance; the rest then sets or deletes its entities as the
    ///   file holds them;
    /// - a given or changed link first deletes the world's own entities holding the IDs the
    ///   prefab gives, as an unlink leaves them, with what's under them. The rest then treats
    ///   the instance's root and link-given entities as without `old`, as the world holds them
    ///   as the link gave them.
    ///
    /// Without `old` (the last applied file no longer loads, say after a rebuild removed a field)
    /// the world's entities stand for what the file held: each field of `new`'s entities the
    /// world holds otherwise is set, and nothing is removed or deleted.
    ///
    /// Fails like [`load_scene`](Self::load_scene) for `new` and `old`.
    #[cfg(feature = "dev")]
    pub(crate) fn scene_edit(&self, old: Option<&str>, new: &str) -> Result<SceneEdit> {
        let _zone = gg_core::zone!();
        let mut before = self.emptied();
        if let Some(old) = old {
            before.load_scene(old)?;
        }
        let mut after = self.emptied();
        after.load_scene(new)?;
        // The entities instances give, which come and go with their roots' links.
        let given_after = given(&after)?;
        let parent_id = |world: &World, entity: Entity| {
            let parent = world.parent(entity).expect("alive");
            parent.map(|parent| world.id(parent).expect("a parent is alive"))
        };
        let mut order: Vec<EntityId> = scene_ids(&after).collect();
        // Each parent before its children, then by ID.
        let depth = |id: &EntityId| {
            let mut entity = after.entity(*id).expect("alive");
            let mut depth = 0;
            while let Some(parent) = after.parent(entity).expect("alive") {
                entity = parent;
                depth += 1;
            }
            depth
        };
        order.sort_unstable_by_key(|id| (depth(id), *id));
        let mut creates = Vec::new();
        let mut made = Vec::new();
        let link = |world: &World, entity: Option<Entity>| world.get_field(entity?, LINK, "").ok();
        // A prefab's value names it alone, whatever world it's read in.
        let same_link = |a: &Option<Value>, b: &Option<Value>| match (a, b) {
            (Some(a), Some(b)) => same(a, self, b, &after),
            (a, b) => a.is_none() && b.is_none(),
        };
        for &id in &order {
            if given_after.contains(&id) {
                continue;
            }
            let linked = link(&after, after.entity(id));
            if let Some(entity) = self.entity(id) {
                // A link the file changes, taken first so the rest may set what the prefab gives.
                let was = match old {
                    Some(_) => link(&before, before.entity(id)),
                    None => link(self, Some(entity)),
                };
                let now = link(self, Some(entity));
                let component = || reflect::Ident::from(LINK);
                if same_link(&was, &linked) || same_link(&now, &linked) {
                    continue;
                }
                let Some(value) = linked else {
                    // Its entities stay its own; the rest sets or deletes them as the file holds.
                    creates.push(Operation::Unlink { entity: id });
                    continue;
                };
                // Its own entities that hold the IDs the link gives, as an unlink left them: the
                // file holds them as the prefab's; an entity of its own under one goes with it.
                let mut expanding = Expanding::default();
                let giving = self.instance(entity, &mut expanding)?;
                let giving = giving.map_or_else(Vec::new, |instance| instance.ids);
                let to_give = after.instance(after.entity(id).expect("alive"), &mut expanding)?;
                let held: Vec<Entity> = (to_give.expect("linked").ids[1..].iter())
                    .filter(|id| !giving.contains(id))
                    .filter_map(|&id| self.entity(id))
                    .collect();
                let above_held = |entity: Entity| {
                    let mut at = entity;
                    while let Some(up) = self.parent(at).expect("alive") {
                        if held.contains(&up) {
                            return true;
                        }
                        at = up;
                    }
                    false
                };
                let held = held.iter().filter(|&&entity| !above_held(entity));
                creates.extend(held.map(|&entity| Operation::Delete {
                    entity: self.id(entity).expect("alive"),
                }));
                creates.push(match now {
                    Some(_) => Operation::Set {
                        entity: id,
                        component: component(),
                        path: String::new(),
                        value,
                    },
                    None => Operation::Add {
                        entity: id,
                        component: component(),
                        value,
                    },
                });
                continue;
            }
            if before.entity(id).is_some() {
                continue;
            }
            let parent = parent_id(&after, after.entity(id).expect("alive"));
            // Under an entity the game deleted: the world moved on.
            if parent.is_some_and(|parent| self.entity(parent).is_none() && !made.contains(&parent))
            {
                continue;
            }
            made.push(id);
            // With its link, so the entities its prefab gives come with it.
            let components = linked.map(|value| (reflect::Ident::from(LINK), value));
            creates.push(Operation::Create {
                entity: Some(id),
                parent,
                components: components.into_iter().collect(),
            });
        }
        if !creates.is_empty() {
            return Ok(SceneEdit::Creates(creates));
        }
        // The entities the world's instances give, whose links match the file's now; they come and
        // go with them. One the file gave through a link the world has since dropped is the
        // world's own.
        let given_now = given(self)?;
        // Roots of instances whose links the file changed and the world took, with the entities
        // those links give now: the world holds them as the links gave them, so each stands for
        // what the file held, as without `old`.
        let mut relinked = HashSet::new();
        if old.is_some() {
            let mut expanding = Expanding::default();
            for &id in &order {
                let (Some(b), Some(entity)) = (after.entity(id), self.entity(id)) else {
                    continue;
                };
                let linked = link(&after, Some(b));
                if same_link(&link(&before, before.entity(id)), &linked)
                    || !same_link(&link(self, Some(entity)), &linked)
                {
                    continue;
                }
                relinked.insert(id);
                if let Some(instance) = after.instance(b, &mut expanding)? {
                    relinked.extend(instance.ids[1..].iter().copied());
                }
            }
        }
        let mut operations = Vec::new();
        for &id in &order {
            // Deleted since: the world moved on.
            let Some(entity) = self.entity(id) else {
                continue;
            };
            let b = after.entity(id).expect("alive");
            // What the file held of the entity; without `old`, the world's.
            let (held, a) = match old {
                Some(_) if !relinked.contains(&id) => (&before, before.entity(id)),
                _ => (self, Some(entity)),
            };
            let mut names: Vec<&str> = components(&after, b).collect();
            if old.is_some() {
                let a = before.entity(id);
                names.extend(a.into_iter().flat_map(|a| components(&before, a)));
            }
            names.retain(|&name| name != PARENT);
            names.sort_unstable();
            names.dedup();
            for name in names {
                let was = a.and_then(|a| held.get_field(a, name, "").ok());
                let is = after.get_field(b, name, "").ok();
                let now = self.get_field(entity, name, "").ok();
                let component = || reflect::Ident::from(name.to_owned());
                match (was, is, now) {
                    (Some(_), None, Some(_)) => operations.push(Operation::Remove {
                        entity: id,
                        component: component(),
                    }),
                    // Unless the game took away what the file holds as it held it.
                    (was, Some(is), None)
                        if was.as_ref().is_none_or(|was| !same(was, held, &is, &after)) =>
                    {
                        operations.push(Operation::Add {
                            entity: id,
                            component: component(),
                            value: rehome(&is, &after, self),
                        });
                    }
                    (None, Some(is), Some(now)) if !same(&now, self, &is, &after) => {
                        operations.push(Operation::Set {
                            entity: id,
                            component: component(),
                            path: String::new(),
                            value: rehome(&is, &after, self),
                        });
                    }
                    (Some(was), Some(is), Some(_)) => {
                        let mut changed = Vec::new();
                        changes(&was, held, &is, &after, String::new(), &mut changed);
                        for (path, value) in changed {
                            let now = self.get_field(entity, name, &path).ok();
                            if now.is_some_and(|now| same(&now, self, &value, &after)) {
                                continue;
                            }
                            operations.push(Operation::Set {
                                entity: id,
                                component: component(),
                                path,
                                value: rehome(&value, &after, self),
                            });
                        }
                    }
                    _ => {}
                }
            }
            let parent = parent_id(&after, b);
            let moved = a.is_none_or(|a| parent_id(held, a) != parent);
            let there = parent.is_none_or(|parent| self.entity(parent).is_some());
            if moved && there && parent_id(self, entity) != parent {
                operations.push(Operation::Reparent { entity: id, parent });
            }
        }
        // The file's entities no more, except those that will be under another of them once the
        // reparents above have moved what they move: those go with it.
        let mut gone: Vec<EntityId> = scene_ids(&before)
            .filter(|&id| after.entity(id).is_none() && self.entity(id).is_some())
            .filter(|id| !given_now.contains(id))
            .collect();
        gone.sort_unstable();
        let moved: HashMap<EntityId, Option<EntityId>> = (operations.iter())
            .filter_map(|operation| match operation {
                Operation::Reparent { entity, parent } => Some((*entity, *parent)),
                _ => None,
            })
            .collect();
        let parent_then = |id: EntityId| match moved.get(&id) {
            Some(&parent) => parent,
            None => parent_id(self, self.entity(id).expect("alive")),
        };
        let under_gone = |id: EntityId| {
            let mut at = id;
            while let Some(up) = parent_then(at) {
                if gone.binary_search(&up).is_ok() {
                    return true;
                }
                at = up;
            }
            false
        };
        let deletes: Vec<Operation> = (gone.iter().copied())
            .filter(|&id| !under_gone(id))
            .map(|entity| Operation::Delete { entity })
            .collect();
        operations.extend(deletes);
        Ok(SceneEdit::Changes(operations))
    }

    /// Checks that scene `text` would load, by [`load_scene`](Self::load_scene) into this world
    /// without its entities.
    ///
    /// Fails like `load_scene`, except for IDs the world has.
    pub(crate) fn scene_loads(&self, text: &str) -> gg_core::Result<()> {
        self.emptied().load_scene(text).map(drop)
    }

    /// Scene `text` in canonical form: loaded as [`scene_loads`](Self::scene_loads) checks, saved
    /// again by [`save_scene`](Self::save_scene), so hand-written text compares with engine-saved.
    ///
    /// Fails like `scene_loads` and `save_scene`.
    #[cfg(feature = "dev")]
    pub(crate) fn scene_canonical(&self, text: &str) -> Result<String> {
        let mut loaded = self.emptied();
        let roots = loaded.load_scene(text)?;
        loaded.save_scene(&roots)
    }

    /// The world's content as text, a line per field as [`difference`](Self::difference) walks
    /// them: entity ID, component and field path as an error names them (`Sprite.color.x`), and
    /// the value as a recording writes it, every NaN as one. An entity without components gets
    /// its ID alone. Two machines' dumps of worlds that should match, diffed, give where they
    /// part.
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

/// What [`World::scene_edit`] finds: entities to create first, then the rest.
#[cfg(feature = "dev")]
#[derive(Debug, PartialEq)]
pub(crate) enum SceneEdit {
    /// Bare creates, under their parents, of entities the scene holds and the world lacks; each
    /// parent before its children.
    Creates(Vec<Operation>),
    /// With every entity there: adds, removes, sets, reparents, then deletes.
    Changes(Vec<Operation>),
}

/// The name of [`Parent`](super::Parent)'s type, which reparents change, not sets.
#[cfg(feature = "dev")]
const PARENT: &str = "Parent";

/// The name of [`PrefabRoot`](crate::PrefabRoot)'s type, changed before the rest as the
/// instance's prefab entities come with it.
#[cfg(feature = "dev")]
const LINK: &str = "PrefabRoot";

/// The IDs of the entities `world`'s instances give, per [`World::claims`].
///
/// Fails if an instance's template does.
#[cfg(feature = "dev")]
fn given(world: &World) -> Result<HashSet<EntityId>> {
    let tops = world.query::<Entity>();
    let tops = tops.filter(|&entity| world.parent(entity).is_ok_and(|up| up.is_none()));
    let tops: Vec<Entity> = tops.collect();
    let claims = world.claims(&tops, &mut Expanding::default())?;
    let given = claims
        .instances
        .iter()
        .flat_map(|(_, instance)| &instance.ids[1..]);
    Ok(given.copied().collect())
}

/// The IDs of `world`'s entities, in no order.
fn ids(world: &World) -> impl Iterator<Item = EntityId> + '_ {
    let ids = world.query::<Entity>().map(|entity| world.id(entity));
    ids.map(|id| id.expect("a living entity has an ID"))
}

/// The IDs of `world`'s entities a scene names, in no order: all but models' nodes, which the
/// world spawns with their roots.
#[cfg(feature = "dev")]
fn scene_ids(world: &World) -> impl Iterator<Item = EntityId> + '_ {
    let entities = world.query::<Entity>();
    let named = entities.filter(|&entity| world.get::<ModelNode>(entity).is_none());
    named.map(|entity| world.id(entity).expect("a living entity has an ID"))
}

/// Like [`first`], but every path under `path` where `a` and `b` differ, with `b`'s value.
#[cfg(feature = "dev")]
fn changes(
    a: &Value,
    ours: &World,
    b: &Value,
    theirs: &World,
    path: String,
    out: &mut Vec<(String, Value)>,
) {
    let floats = |a: &[f32], b: &[f32], out: &mut Vec<(String, Value)>| {
        let parts = reflect::COMPONENTS.into_iter().zip(a.iter().zip(b));
        for (name, (&x, &y)) in parts {
            if !same_f32(x, y) {
                out.push((reflect::join(&path, name), Value::F32(y)));
            }
        }
    };
    let float64s = |a: &[f64], b: &[f64], out: &mut Vec<(String, Value)>| {
        let parts = reflect::COMPONENTS.into_iter().zip(a.iter().zip(b));
        for (name, (&x, &y)) in parts {
            if !same_f64(x, y) {
                out.push((reflect::join(&path, name), Value::F64(y)));
            }
        }
    };
    match (a, b) {
        (Value::Struct(x), Value::Struct(y)) if same_names(x, y) => {
            for ((name, x), (_, y)) in x.iter().zip(y) {
                changes(x, ours, y, theirs, reflect::join(&path, name), out);
            }
        }
        (Value::List(x), Value::List(y)) if x.len() == y.len() => {
            for (i, (x, y)) in x.iter().zip(y).enumerate() {
                changes(x, ours, y, theirs, format!("{path}[{i}]"), out);
            }
        }
        (Value::Vec2(x), Value::Vec2(y)) => floats(x.as_ref(), y.as_ref(), out),
        (Value::Vec3(x), Value::Vec3(y)) => floats(x.as_ref(), y.as_ref(), out),
        (Value::Vec4(x), Value::Vec4(y)) => floats(x.as_ref(), y.as_ref(), out),
        (Value::DVec3(x), Value::DVec3(y)) => float64s(x.as_ref(), y.as_ref(), out),
        _ => {
            if !same(a, ours, b, theirs) {
                out.push((path, b.clone()));
            }
        }
    }
}

/// `value` of world `from` with each entity it holds replaced by the one of the same ID in `to`,
/// or a dead one, like a game's reference to a deleted entity.
#[cfg(feature = "dev")]
fn rehome(value: &Value, from: &World, to: &World) -> Value {
    let fields = |fields: &[(reflect::Ident, Value)]| {
        let fields = fields
            .iter()
            .map(|(name, value)| (name.clone(), rehome(value, from, to)));
        fields.collect()
    };
    match value {
        &Value::Entity(entity) => {
            let id = from.id(entity).expect("a scene's entities are alive");
            Value::Entity(to.entity(id).unwrap_or(Entity::DEAD))
        }
        Value::Option(Some(value)) => Value::Option(Some(Box::new(rehome(value, from, to)))),
        Value::List(items) => {
            Value::List(items.iter().map(|item| rehome(item, from, to)).collect())
        }
        Value::Struct(given) => Value::Struct(fields(given)),
        Value::Variant(name, given) => Value::Variant(name.clone(), fields(given)),
        other => other.clone(),
    }
}

fn components(world: &World, entity: Entity) -> impl Iterator<Item = &str> {
    world.components_of(entity).expect("the entity is alive")
}

/// Where entity `id`, which a world at least has, first differs between `ours` and `theirs`.
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

/// The first path under `path` where `a` (in `ours`) and `b` (in `theirs`) differ, with their
/// values.
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
    let float64s = |a: &[f64], b: &[f64]| {
        let mut parts = reflect::COMPONENTS.into_iter().zip(a.iter().zip(b));
        let (name, (&x, &y)) = parts.find(|(_, (x, y))| !same_f64(**x, **y))?;
        Some((reflect::join(&path, name), Value::F64(x), Value::F64(y)))
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
        (Value::DVec3(x), Value::DVec3(y)) => float64s(x.as_ref(), y.as_ref()),
        _ => (!same(a, ours, b, theirs)).then(|| (path, a.clone(), b.clone())),
    }
}

/// Whether `a` (in `ours`) and `b` (in `theirs`) are the same, as the hash tells values apart.
pub(super) fn same(a: &Value, ours: &World, b: &Value, theirs: &World) -> bool {
    let floats = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(&x, &y)| same_f32(x, y));
    let fields = |x: &[(reflect::Ident, Value)], y: &[(reflect::Ident, Value)]| {
        let mut both = x.iter().zip(y);
        same_names(x, y) && both.all(|((_, x), (_, y))| same(x, ours, y, theirs))
    };
    match (a, b) {
        (&Value::F32(x), &Value::F32(y)) => same_f32(x, y),
        (&Value::F64(x), &Value::F64(y)) => same_f64(x, y),
        (Value::Vec2(x), Value::Vec2(y)) => floats(x.as_ref(), y.as_ref()),
        (Value::Vec3(x), Value::Vec3(y)) => floats(x.as_ref(), y.as_ref()),
        (Value::Vec4(x), Value::Vec4(y)) => floats(x.as_ref(), y.as_ref()),
        (Value::DVec3(x), Value::DVec3(y)) => x
            .to_array()
            .into_iter()
            .zip(y.to_array())
            .all(|(x, y)| same_f64(x, y)),
        (Value::Quat(x), Value::Quat(y)) => floats(&x.to_array(), &y.to_array()),
        (&Value::Entity(x), &Value::Entity(y)) => ours.id(x) == theirs.id(y),
        (&Value::Texture(x), &Value::Texture(y)) => {
            ours.assets().name(x) == theirs.assets().name(y)
        }
        (&Value::Model(x), &Value::Model(y)) => {
            ours.assets().model_name(x) == theirs.assets().model_name(y)
        }
        (&Value::Font(x), &Value::Font(y)) => {
            ours.assets().font_name(x) == theirs.assets().font_name(y)
        }
        (&Value::Prefab(x), &Value::Prefab(y)) => {
            ours.assets().prefab_name(x) == theirs.assets().prefab_name(y)
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

/// By bits, every NaN as one, -0.0 apart from 0.0, as the hash takes them.
fn same_f32(x: f32, y: f32) -> bool {
    x.to_bits() == y.to_bits() || x.is_nan() && y.is_nan()
}

/// As [`same_f32`].
fn same_f64(x: f64, y: f64) -> bool {
    x.to_bits() == y.to_bits() || x.is_nan() && y.is_nan()
}

/// Whether two structs' or variants' fields have the same names, in order.
fn same_names(x: &[(reflect::Ident, Value)], y: &[(reflect::Ident, Value)]) -> bool {
    x.len() == y.len() && x.iter().zip(y).all(|((m, _), (n, _))| m == n)
}

/// Writes a line per field of `value` at `at`, for [`World::dump`].
fn leaves(out: &mut String, world: &World, at: &str, value: &Value) {
    let floats = |out: &mut String, xs: &[f32]| {
        for (name, &x) in reflect::COMPONENTS.into_iter().zip(xs) {
            leaves(out, world, &reflect::join(at, name), &Value::F32(x));
        }
    };
    let float64s = |out: &mut String, xs: &[f64]| {
        for (name, &x) in reflect::COMPONENTS.into_iter().zip(xs) {
            leaves(out, world, &reflect::join(at, name), &Value::F64(x));
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
        Value::DVec3(v) => float64s(out, v.as_ref()),
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
    #[cfg(feature = "dev")]
    use gg_core::math::DVec3;
    #[cfg(feature = "dev")]
    use gg_core::math::Quat;
    use gg_core::math::Vec2;

    use super::*;
    use crate::Transform;
    use crate::reflect::tests::{Everything, everything};

    #[derive(Clone, Copy, crate::Component)]
    struct Count(u32);

    /// Two worlds of the same entities by ID, the second loaded from the first's scene into other
    /// slots: one holds another, a third's child.
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

    /// A scene edit keeps what the world changed that the file doesn't and leaves alone what the
    /// world deleted: new entities first, then sets, an add, a remove, a reparent and a delete,
    /// which one undo reverses. Asked again, it finds nothing left.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_edit_takes_the_world_from_the_files_old_text_to_its_new() {
        // The file, as a world of its own.
        let mut file = World::seeded(10);
        let a = file.spawn((Transform::IDENTITY, Count(1)));
        let b = file.spawn(Transform::IDENTITY);
        let c = file.spawn(Transform::IDENTITY);
        let d = file.spawn(Count(4));
        file.set_parent(b, Some(a)).expect("no cycle");
        let old = file.save_scene(&[a, c, d]).expect("saved");
        // The game's world, loaded from it: the game turns a, takes c away and gives d a
        // transform.
        let mut world = World::seeded(11);
        world.register::<Transform>().expect("registered");
        world.register::<Everything>().expect("registered");
        world.register::<Count>().expect("registered");
        world.load_scene(&old).expect("loaded");
        let [a_id, b_id, c_id, d_id] = [a, b, c, d].map(|entity| file.id(entity).expect("alive"));
        let here = |world: &World, id| world.entity(id).expect("alive");
        let turned = Transform {
            rotation: Quat::from_rotation_z(1.0),
            ..Transform::IDENTITY
        };
        world.insert(here(&world, a_id), turned).expect("alive");
        world.despawn(here(&world, c_id)).expect("alive");
        world
            .insert(here(&world, d_id), Transform::IDENTITY)
            .expect("alive");
        // The file saved again: a moved and counted on, b gone, c moved, d under a without its
        // count, and two entities more, e holding a and its child f holding e.
        let moved = |x| Transform {
            translation: DVec3::new(x, 0.0, 0.0),
            ..Transform::IDENTITY
        };
        file.insert(a, moved(2.0)).expect("alive");
        file.insert(a, Count(7)).expect("alive");
        file.despawn(b).expect("alive");
        file.insert(c, moved(3.0)).expect("alive");
        file.set_parent(d, Some(a)).expect("no cycle");
        file.remove::<Count>(d).expect("alive");
        let e = file.spawn(Everything {
            target: Some(a),
            ..everything()
        });
        let f = file.spawn(everything());
        file.set_parent(f, Some(e)).expect("no cycle");
        file.get_mut::<Everything>(f).expect("everything").target = Some(e);
        let new = file.save_scene(&[a, c, e]).expect("saved");
        let [e_id, f_id] = [e, f].map(|entity| file.id(entity).expect("alive"));
        let before = world.dump();
        let mut log = crate::Log::default();
        let SceneEdit::Creates(creates) = world.scene_edit(Some(&old), &new).expect("an edit")
        else {
            panic!("the new entities first");
        };
        let made: Vec<Option<EntityId>> = creates.iter().map(Operation::entity).collect();
        assert_eq!(made, [Some(e_id), Some(f_id)], "each parent first");
        log.apply_all(&mut world, creates).expect("created");
        let SceneEdit::Changes(rest) = world.scene_edit(Some(&old), &new).expect("an edit") else {
            panic!("every entity is made");
        };
        log.append(&mut world, rest).expect("applied");
        let transform = |world: &World, id| *world.get::<Transform>(here(world, id)).expect("one");
        assert_eq!(
            transform(&world, a_id),
            Transform {
                rotation: turned.rotation,
                ..moved(2.0)
            },
            "moved, and still turned"
        );
        assert_eq!(world.get::<Count>(here(&world, a_id)).map(|n| n.0), Some(7));
        assert!(world.entity(b_id).is_none(), "deleted");
        assert!(world.entity(c_id).is_none(), "left alone");
        let d_here = here(&world, d_id);
        assert_eq!(
            world.parent(d_here).expect("alive"),
            Some(here(&world, a_id))
        );
        assert!(world.get::<Count>(d_here).is_none(), "removed");
        assert!(world.get::<Transform>(d_here).is_some(), "the game's, kept");
        let held = |world: &World, id| {
            world
                .get::<Everything>(here(world, id))
                .expect("one")
                .target
        };
        assert_eq!(held(&world, e_id), Some(here(&world, a_id)));
        assert_eq!(
            held(&world, f_id),
            Some(here(&world, e_id)),
            "a new one holds a new one"
        );
        assert_eq!(
            world.parent(here(&world, f_id)).expect("alive"),
            Some(here(&world, e_id))
        );
        assert_eq!(
            world.scene_edit(Some(&old), &new).expect("an edit"),
            SceneEdit::Changes(Vec::new()),
            "nothing left"
        );
        assert_eq!(log.edits().len(), 1, "one edit");
        assert!(log.undo(&mut world).expect("undone"));
        assert_eq!(world.dump(), before);
        // Without what the file held, the world stands for it: the file's values win where the
        // world's differ, and nothing is removed or deleted.
        let SceneEdit::Creates(creates) = world.scene_edit(None, &new).expect("an edit") else {
            panic!("the new entities first");
        };
        log.apply_all(&mut world, creates).expect("created");
        let SceneEdit::Changes(rest) = world.scene_edit(None, &new).expect("an edit") else {
            panic!("every entity is made");
        };
        log.append(&mut world, rest).expect("applied");
        assert_eq!(
            transform(&world, a_id),
            moved(2.0),
            "the file's, turn and all"
        );
        assert!(world.entity(b_id).is_some(), "not deleted");
        assert!(
            world.get::<Count>(here(&world, d_id)).is_some(),
            "not removed"
        );
        log.clear(&mut world);
    }

    /// A scene edit leaves alone a component the game took away that the file doesn't change, and
    /// what goes under an entity the game deleted. It deletes what the file no longer holds under
    /// an entity it moves out from under another it deletes.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scene_edit_leaves_alone_what_the_game_took_away() {
        let mut file = World::seeded(12);
        let [g1, k, g2] = [(); 3].map(|()| file.spawn(Transform::IDENTITY));
        file.set_parent(k, Some(g1)).expect("no cycle");
        file.set_parent(g2, Some(k)).expect("no cycle");
        let x = file.spawn((Transform::IDENTITY, Count(3)));
        let p = file.spawn(Transform::IDENTITY);
        let old = file.save_scene(&[g1, x, p]).expect("saved");
        let mut world = World::seeded(13);
        world.register::<Transform>().expect("registered");
        world.register::<Count>().expect("registered");
        world.load_scene(&old).expect("loaded");
        let [g1_id, k_id, g2_id, x_id, p_id] =
            [g1, k, g2, x, p].map(|entity| file.id(entity).expect("alive"));
        let here = |world: &World, id| world.entity(id).expect("alive");
        // The game takes x's count away, and deletes p.
        world.remove::<Count>(here(&world, x_id)).expect("alive");
        world.despawn(here(&world, p_id)).expect("alive");
        // The file keeps k alone of its chain, as a root, moves x, and puts a child under p.
        file.set_parent(k, None).expect("no cycle");
        file.set_parent(g2, Some(g1)).expect("no cycle");
        file.despawn(g1).expect("alive");
        file.insert(
            x,
            Transform {
                translation: DVec3::X,
                ..Transform::IDENTITY
            },
        )
        .expect("alive");
        let n = file.spawn(Transform::IDENTITY);
        file.set_parent(n, Some(p)).expect("no cycle");
        let new = file.save_scene(&[k, x, p]).expect("saved");
        let SceneEdit::Changes(changes) = world.scene_edit(Some(&old), &new).expect("an edit")
        else {
            panic!("nothing to make: n's parent is gone");
        };
        crate::Log::default()
            .apply_all(&mut world, changes)
            .expect("applied");
        assert!(
            world.entity(g1_id).is_none() && world.entity(g2_id).is_none(),
            "both deleted"
        );
        assert_eq!(world.parent(here(&world, k_id)).expect("alive"), None);
        let x_here = here(&world, x_id);
        assert!(world.get::<Count>(x_here).is_none(), "still taken away");
        assert_eq!(
            world.get::<Transform>(x_here).map(|at| at.translation.x),
            Some(1.0)
        );
        assert!(
            world.entity(file.id(n).expect("alive")).is_none(),
            "its parent is gone"
        );
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
