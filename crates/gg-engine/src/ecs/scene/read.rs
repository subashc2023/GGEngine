//! The reader: one typed parse of a scene's text through `ron`, whose seeds reflection's
//! descriptions drive, never `ron`'s own values, which can't tell a struct from a map or an `f32`
//! from an `f64`. It checks everything a load could refuse before anything changes, and builds
//! each component as it ends. A refusal says where it is: the parse numbers each key and value it
//! enters, and to locate the one refused, parses again, stopping as it enters it, where `ron`'s
//! cursor is at its first character.

use std::collections::HashMap;
use std::fmt;
use std::mem;

use gg_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use ron::Options;
use ron::de::SpannedError;
use ron::extensions::Extensions;
use serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
};

use super::template::Expanding;
use super::{MAX_DEPTH, NO_HINTS};
use crate::assets::Asset;
use crate::ecs::by_name::about;
use crate::ecs::id::Named;
use crate::ecs::table::{AnyColumn, ComponentId, Row};
use crate::ecs::{Entity, EntityId, Parent, World};
use crate::reflect::{self, Field, Ident, Meta, Shape, Type, Types, Value, Variant};
use crate::{Font, Model, ModelNode, Prefab, PrefabRoot, Reflect, Texture, Transform};

/// A scene's entity, as the parse reads it.
pub(super) struct Loaded {
    pub(super) id: EntityId,
    /// The handle it takes as it's spawned.
    pub(super) handle: Entity,
    /// Its components as the text gives them, until the parse has built them into `row`.
    given: Vec<Given>,
    pub(super) row: Row,
    /// Its parent's index among the scene's entities, and the item its `Parent` is, or, for one an
    /// instance's prefab places, its root's `PrefabRoot`.
    pub(super) parent: Option<(usize, u32)>,
    /// For one an instance's prefab gives, the index of the instance's root among the scene's
    /// entities, or of the root of the instance within it whose prefab gives it, and its ID as that
    /// prefab gives it, which its own mixes from that root's; `None` for one of the text's own.
    pub(super) spawned: Option<(usize, EntityId)>,
    /// The item its key is, once it's read.
    key: Option<u32>,
    /// The item it's first named at, as a key or a reference.
    first: u32,
}

/// A component as a scene's text gives it, which the parse builds once it knows whose instance its
/// entity is of, if it's one: its type, the item its value is, and its value.
struct Given {
    component: ComponentId,
    item: u32,
    value: Partial,
}

/// A component's value as a scene's text gives it: a struct's of named fields, whose fields a load
/// takes from the prefab where an instance's entity leaves them out, field by field; or another
/// type's, whole.
enum Partial {
    Fields(Vec<Option<Value>>),
    Whole(Value),
}

/// The scene `text`'s entities, in the order it first names them, then the instances' entities it
/// leaves out, with the handles they take in that order: a prefab's, whose IDs are its own, if
/// `prefab`, and which has one root. Each instance's entities are its prefab's, as `expanding`
/// makes its prefabs' templates, under IDs mixed from its root's, each field the text leaves out
/// taking the prefab's, as [`PrefabRoot`] says.
pub(super) fn parse(
    world: &World,
    text: &str,
    prefab: bool,
    expanding: &mut Expanding,
) -> Result<Vec<Loaded>> {
    let mut load = Load::new(world, prefab, None, false);
    let mut deserializer = deserializer(text)?;
    if let Err(error) = run(&mut load, &mut deserializer) {
        return Err(match load.refused.take() {
            Some((item, refusal)) => locate(world, text, prefab, false, item, refusal),
            None => syntax(&error),
        });
    }
    resolve(world, text, load, expanding)
}

/// The textures, models, fonts and prefabs scene `text` names that none loaded has, each once, in
/// the order it first names them, as [`parse`] reads it into `world`, but on past them; and those
/// that the prefabs it names that are loaded name, through the prefabs they name, in turn.
///
/// Fails as the parse does, but for those; a loaded prefab's text that doesn't parse is the parse's
/// to refuse.
pub(super) fn assets(world: &World, text: &str) -> Result<Vec<Asset<String>>> {
    let mut load = Load::new(world, false, None, true);
    let mut reading = deserializer(text)?;
    if let Err(error) = run(&mut load, &mut reading) {
        return Err(match load.refused.take() {
            Some((item, refusal)) => locate(world, text, false, true, item, refusal),
            None => syntax(&error),
        });
    }
    let mut missing = load.missing.unwrap_or_default();
    // Each prefab once, through those that the loaded prefabs name.
    let mut seen = load.prefabs;
    let mut next = seen.clone();
    while let Some(prefab) = next.pop() {
        let Some(text) = world.assets().prefab_text(prefab) else {
            continue;
        };
        let mut load = Load::new(world, true, None, true);
        let Ok(mut reading) = deserializer(text) else {
            continue;
        };
        if run(&mut load, &mut reading).is_err() {
            continue;
        }
        for asset in load.missing.unwrap_or_default() {
            if !missing.contains(&asset) {
                missing.push(asset);
            }
        }
        for named in load.prefabs {
            if !seen.contains(&named) {
                seen.push(named);
                next.push(named);
            }
        }
    }
    Ok(missing)
}

/// The rest of [`parse`], once the text has parsed: each instance's entities found, by its
/// prefab's template, and those the text leaves out added; each entity's components built, from
/// the text and the prefab; and the hierarchy checked.
fn resolve(
    world: &World,
    text: &str,
    load: Load<'_>,
    expanding: &mut Expanding,
) -> Result<Vec<Loaded>> {
    let Load {
        prefab,
        mut slots,
        mut entities,
        mut by_id,
        map,
        ..
    } = load;
    let refuse = |item, error| Err(locate(world, text, prefab, false, item, error));
    let linked = world.component::<PrefabRoot>();
    // Each instance's root, by its index, its prefab, and the item its `PrefabRoot` is.
    let mut roots = Vec::new();
    for (i, entity) in entities.iter().enumerate() {
        let mut given = entity.given.iter();
        let Some(link) = given.find(|given| Some(given.component) == linked) else {
            continue;
        };
        let Partial::Whole(Value::Struct(fields)) = &link.value else {
            unreachable!("a `PrefabRoot` is a tuple struct");
        };
        let Some((_, Value::Prefab(of))) = fields.first() else {
            unreachable!("a `PrefabRoot` holds a prefab");
        };
        let of = *of;
        if prefab && entity.parent.is_none() {
            let error = Error::new(
                "scene.prefab_variant",
                "a prefab's root links no prefab: link it from an entity under the root, or place \
                 the other prefab's entities in this one",
            );
            return refuse(link.item, error);
        }
        roots.push((i, of, link.item));
    }
    // Each instance's template, and the IDs of its entities, by which the text names them; and
    // each entity an instance gives, by its ID, with the instance and its place in the template.
    let mut instances = Vec::with_capacity(roots.len());
    let mut claims: HashMap<EntityId, (usize, usize)> = HashMap::new();
    for (r, &(i, of, item)) in roots.iter().enumerate() {
        let template = match expanding.template(world, of) {
            Ok(template) => template,
            Err(error) => return refuse(item, error),
        };
        let ids = match template.ids(entities[i].id) {
            Ok(ids) => ids,
            Err(error) => return refuse(item, error),
        };
        for (part, &id) in ids.iter().enumerate().skip(1) {
            if claims.insert(id, (r, part)).is_some() {
                let error = Error::new(
                    "scene.duplicate_id",
                    format!(
                        "two instances' prefabs give entity {id}, but a scene gives each entity \
                         once"
                    ),
                );
                return refuse(item, error);
            }
            if !prefab && world.entity(id).is_some() {
                let error = Error::new(
                    "ecs.id_taken",
                    format!(
                        "its prefab gives entity {id}, which is in the world already, but a \
                         scene's entities are new"
                    ),
                );
                return refuse(item, error);
            }
        }
        instances.push((template, ids));
    }
    for &(i, _, item) in &roots {
        if claims.contains_key(&entities[i].id) {
            let error = Error::new(
                "scene.prefab_entity",
                format!(
                    "entity {} is an entity of another instance's prefab, whose `PrefabRoot` is \
                     the prefab's",
                    entities[i].id
                ),
            );
            return refuse(item, error);
        }
    }
    let unknown = entities
        .iter()
        .find(|entity| entity.key.is_none() && !claims.contains_key(&entity.id));
    if let Some(unknown) = unknown {
        let error = Error::new(
            "scene.unknown_id",
            format!(
                "the scene has no entity {}: a scene's entities refer only to each other and to \
                 those its instances' prefabs give",
                unknown.id
            ),
        );
        return refuse(unknown.first, error);
    }
    // The instances' entities the text names nowhere, which take the handles after its own.
    for (r, (_, ids)) in instances.iter().enumerate() {
        for &id in &ids[1..] {
            if by_id.contains_key(&id) {
                continue;
            }
            by_id.insert(id, entities.len());
            entities.push(Loaded {
                id,
                handle: slots.next().expect("there's always another slot"),
                given: Vec::new(),
                row: Vec::new(),
                parent: None,
                spawned: None,
                key: None,
                first: roots[r].2,
            });
        }
    }
    // Each instance's template's entities, by their handles in it, as their instance has them,
    // which its references are retargeted to.
    let handles: Vec<HashMap<Entity, Entity>> = (instances.iter())
        .map(|(template, ids)| {
            let parts = template.parts.iter().zip(ids);
            let handle = |id: &EntityId| entities[by_id[id]].handle;
            parts.map(|(part, id)| (part.handle, handle(id))).collect()
        })
        .collect();
    let parent_type = world.component::<Parent>();
    let parent_type = parent_type.expect("every world has registered `Parent`");
    let transform = world.component::<Transform>();
    for i in 0..entities.len() {
        let id = entities[i].id;
        let base = match roots.iter().position(|&(root, ..)| root == i) {
            Some(r) => Some((r, 0)),
            None => claims.get(&id).copied(),
        };
        let given = mem::take(&mut entities[i].given);
        let named: Vec<ComponentId> = given.iter().map(|given| given.component).collect();
        let mut row: Row = Vec::with_capacity(given.len());
        let (template, ids, handles) = match base {
            Some((r, _)) => (
                Some(&*instances[r].0),
                &instances[r].1[..],
                Some(&handles[r]),
            ),
            None => (None, &[][..], None),
        };
        let part = base.and_then(|(_, part)| Some(&template?.parts[part]));
        let retarget = |column: &mut dyn AnyColumn| {
            let handles = handles.expect("a template's column is an instance's");
            column.entities_mut(0, &mut |entity| {
                *entity = handles.get(entity).copied().unwrap_or(Entity::DEAD);
            });
        };
        // A root's transform is its own, which takes nothing of its prefab's.
        let rooted =
            |component| base.is_some_and(|(_, at)| at == 0) && Some(component) == transform;
        for given in given {
            let from = part.filter(|_| !rooted(given.component)).and_then(|part| {
                let column = part.row.iter().find(|&&(of, _)| of == given.component);
                column.map(|(_, column)| column)
            });
            let value = match given.value {
                Partial::Whole(value) => Ok(value),
                Partial::Fields(fields) => {
                    let base = from.map(|column| {
                        let mut column = column.cloned();
                        retarget(&mut *column);
                        column.value(0)
                    });
                    let base = match &base {
                        Some(Value::Struct(base)) => Some(&base[..]),
                        _ => None,
                    };
                    let of = named_fields(world, given.component);
                    fill(world.types(), of, fields, base).map(Value::Struct)
                }
            };
            let mut column = world.components.new_column(given.component);
            let built = value.and_then(|value| column.push_value(&value));
            if let Err(error) = built {
                return refuse(given.item, error);
            }
            row.push((given.component, column));
        }
        if let (Some(part), Some((r, at))) = (part, base) {
            for (component, column) in &part.row {
                let own = *component == parent_type || (at == 0 && Some(*component) == transform);
                if own || named.contains(component) {
                    continue;
                }
                let mut column = column.cloned();
                retarget(&mut *column);
                row.push((*component, column));
            }
            if at > 0 {
                entities[i].spawned = Some(match part.spawner {
                    None => (roots[r].0, part.id),
                    Some((spawner, local)) => (by_id[&ids[spawner]], local),
                });
            }
            if let (true, None, Some(up)) = (at > 0, entities[i].parent, part.parent) {
                let up = by_id[&ids[up]];
                let column: Box<dyn AnyColumn> = Box::new(vec![Parent(entities[up].handle)]);
                row.push((parent_type, column));
                entities[i].parent = Some((up, roots[r].2));
            }
        }
        row.sort_unstable_by_key(|&(component, _)| component);
        entities[i].row = row;
    }
    if let Some(item) = cycle(&entities) {
        let error = Error::new(
            "ecs.parent_cycle",
            "it makes the entity its own ancestor, but an entity is never its own ancestor",
        );
        return refuse(item, error);
    }
    if prefab {
        let mut roots: Vec<&Loaded> = entities.iter().filter(|e| e.parent.is_none()).collect();
        roots.sort_unstable_by_key(|root| root.key);
        match roots[..] {
            [_] => {}
            [] => {
                let error = Error::new(
                    "scene.prefab_roots",
                    "a prefab is an entity and its descendants, but this one has no entities",
                );
                return refuse(map, error);
            }
            [first, second, ..] => {
                let error = Error::new(
                    "scene.prefab_roots",
                    format!(
                        "a prefab has one root, but entities {} and {} have no parent",
                        first.id, second.id
                    ),
                );
                return refuse(second.key.unwrap_or(second.first), error);
            }
        }
    }
    Ok(entities)
}

/// The IDs scene `text` gives its entities, its map of entities' keys, read from its syntax alone,
/// through `ron`'s own reading of what the keys hold, without the world's types: so that a text
/// that names a component type or a field the world no longer has still says which entities it
/// holds.
///
/// Fails with `scene.syntax` for text that isn't a scene's RON, or enables RON's extensions, and
/// with `ecs.bad_id` for a key that isn't an ID.
pub(super) fn ids(text: &str) -> Result<Vec<EntityId>> {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde::de::IgnoredAny;

    #[derive(Deserialize)]
    struct Keys {
        entities: BTreeMap<String, IgnoredAny>,
    }

    let mut deserializer = deserializer(text)?;
    let keys = Keys::deserialize(&mut deserializer).and_then(|keys| {
        deserializer.end()?;
        Ok(keys)
    });
    let keys = keys.map_err(|error| syntax(&deserializer.span_error(error)))?;
    keys.entities.keys().map(|id| id.parse()).collect()
}

/// The item of the `Parent` of an entity that would be its own ancestor, if there's one.
fn cycle(entities: &[Loaded]) -> Option<u32> {
    const NEW: u8 = 0;
    const WALKED: u8 = 1;
    const DONE: u8 = 2;
    let mut state = vec![NEW; entities.len()];
    let up = |at: usize| entities[at].parent.map(|(up, _)| up);
    for start in 0..entities.len() {
        // Up from each to a root, or to an entity walked before: one walked on this walk is in a
        // cycle.
        let mut at = Some(start);
        while let Some(i) = at.filter(|&i| state[i] == NEW) {
            state[i] = WALKED;
            at = up(i);
        }
        if let Some(i) = at.filter(|&i| state[i] == WALKED) {
            return entities[i].parent.map(|(_, item)| item);
        }
        let mut at = Some(start);
        while let Some(i) = at.filter(|&i| state[i] == WALKED) {
            state[i] = DONE;
            at = up(i);
        }
    }
    None
}

/// A deserializer of `text`, which refuses `ron`'s extensions, since they change its grammar.
fn deserializer(text: &str) -> Result<ron::Deserializer<'_>> {
    // Each level of a value takes `ron` at most 4 levels of its recursion.
    let options = Options::default().with_recursion_limit(4 * MAX_DEPTH as usize + 16);
    let deserializer =
        ron::Deserializer::from_str_with_options(text, &options).map_err(|e| syntax(&e))?;
    if deserializer.extensions() != Extensions::empty() {
        // The attribute that enables one begins at the last `#` before the value that `ron` reads
        // the text up to with none enabled: the text up to a `#` inside it doesn't read, and up to
        // one after it reads with one enabled.
        let attributes = &text[..text.len() - deserializer.remainder().len()];
        let none_before = |&at: &usize| {
            ron::Deserializer::from_str(&text[..at]).is_ok_and(|d| d.extensions().is_empty())
        };
        let at = attributes
            .rmatch_indices('#')
            .map(|(at, _)| at)
            .find(none_before);
        let before = &text[..at.expect("an attribute begins with `#`")];
        let line = 1 + before.matches('\n').count();
        let col = 1 + before.chars().rev().take_while(|&c| c != '\n').count();
        return Err(Error::new(
            "scene.syntax",
            format!(
                "{line}:{col}: a scene enables none of RON's extensions, but this one's \
                 `#![enable(..)]` does"
            ),
        ));
    }
    Ok(deserializer)
}

/// Parses the scene, and the end of the text after it.
fn run(load: &mut Load<'_>, deserializer: &mut ron::Deserializer<'_>) -> Result<(), SpannedError> {
    let parsed = Scene(load).deserialize(&mut *deserializer);
    let parsed = parsed.and_then(|()| deserializer.end());
    parsed.map_err(|error| deserializer.span_error(error))
}

/// `error`, from `ron`, at the line and column where its parser stopped.
fn syntax(error: &SpannedError) -> Error {
    let at = &error.span.end;
    Error::new(
        "scene.syntax",
        format!("{}:{}: {}", at.line, at.col, error.code),
    )
}

/// `refusal`, at item `item`, saying where it is: the text is parsed again, as it was, past the
/// textures, models, fonts and prefabs none loaded has if `collect`, up to the item, where the
/// parser is at its first character, and the refusal is carried out from there, gathering the
/// entity, the component and the path.
fn locate(
    world: &World,
    text: &str,
    prefab: bool,
    collect: bool,
    item: u32,
    refusal: Error,
) -> Error {
    let mut load = Load::new(world, prefab, Some((item, refusal)), collect);
    let mut deserializer = deserializer(text).expect("the text parsed this far before");
    let stopped = run(&mut load, &mut deserializer).expect_err("the parse stops at the item");
    let refusal = load
        .carried
        .take()
        .expect("the parse reaches the item again");
    let at = stopped.span.end;
    let message = format!("{}:{}: {refusal}", at.line, at.col);
    refusal.with_message(message)
}

/// What the parse knows as it goes.
struct Load<'w> {
    world: &'w World,
    types: &'w Types,
    /// Whether the IDs are a prefab's own, which the world may have too.
    prefab: bool,
    /// The handles the next spawns take.
    slots: Box<dyn Iterator<Item = Entity> + 'w>,
    entities: Vec<Loaded>,
    /// Each entity's index in `entities`; looked up, never iterated.
    by_id: HashMap<EntityId, usize>,
    /// The entity the last reference read named, which a `Parent`, of one reference, names.
    referenced: usize,
    /// How many keys and values the parse has entered, which numbers each.
    items: u32,
    /// The item the map of entities is.
    map: u32,
    /// The refusal, and the item it's at.
    refused: Option<(u32, Error)>,
    /// When the parse locates a refusal: the item it stops at, and the refusal, which is carried
    /// out from there as `carried`.
    stop: Option<(u32, Error)>,
    carried: Option<Error>,
    /// The textures, models, fonts and prefabs named that none loaded has, while the parse reads on
    /// past them, as [`assets`] does, rather than refusing them.
    missing: Option<Vec<Asset<String>>>,
    /// The prefabs named that are loaded, each once, whose own names [`assets`] gathers too.
    prefabs: Vec<Prefab>,
}

impl<'w> Load<'w> {
    fn new(world: &'w World, prefab: bool, stop: Option<(u32, Error)>, collect: bool) -> Self {
        Self {
            world,
            types: world.types(),
            prefab,
            slots: Box::new(world.next_slots()),
            entities: Vec::new(),
            by_id: HashMap::new(),
            referenced: 0,
            items: 0,
            map: 0,
            refused: None,
            stop,
            carried: None,
            missing: collect.then(Vec::new),
            prefabs: Vec::new(),
        }
    }

    /// Enters the next item, and returns its number; when locating a refusal, stops at its item.
    fn enter<E: de::Error>(&mut self) -> Result<u32, E> {
        self.items += 1;
        if self
            .stop
            .as_ref()
            .is_some_and(|&(item, _)| item == self.items)
        {
            self.carried = self.stop.take().map(|(_, refusal)| refusal);
            return Err(E::custom(""));
        }
        Ok(self.items)
    }

    /// Refuses the scene at `item`, for `error`, and returns the error that stops the parse.
    fn refuse<E: de::Error>(&mut self, item: u32, error: Error) -> E {
        self.refused = Some((item, error));
        E::custom("")
    }

    /// Says more of where the refusal being located is, as it's carried out through a part.
    fn wrap(&mut self, with: impl FnOnce(Error) -> Error) {
        if let Some(refusal) = self.carried.take() {
            self.carried = Some(with(refusal));
        }
    }

    /// The index of the entity with ID `id`, named at `item`: a new one, with the next handle, the
    /// first time it's named.
    fn named(&mut self, id: EntityId, item: u32) -> usize {
        if let Some(&index) = self.by_id.get(&id) {
            return index;
        }
        let index = self.entities.len();
        self.entities.push(Loaded {
            id,
            handle: self.slots.next().expect("there's always another slot"),
            given: Vec::new(),
            row: Vec::new(),
            parent: None,
            spawned: None,
            key: None,
            first: item,
        });
        self.by_id.insert(id, index);
        index
    }

    /// The index of the entity whose key, at `item`, is `id`.
    fn key(&mut self, id: EntityId, item: u32) -> Result<usize> {
        let index = self.named(id, item);
        if self.entities[index].key.is_some() {
            return Err(Error::new(
                "scene.duplicate_id",
                format!("entity {id} is given twice, but a scene gives each entity once"),
            ));
        }
        if !self.prefab && self.world.entity(id).is_some() {
            return Err(Error::new(
                "ecs.id_taken",
                format!(
                    "entity {id} is in the world already, but a scene's entities are new: a load \
                     keeps the IDs it gives"
                ),
            ));
        }
        self.entities[index].key = Some(item);
        Ok(index)
    }

    /// The handle of the entity with ID `id`, which a reference at `item` names.
    fn reference(&mut self, id: EntityId, item: u32) -> Entity {
        let index = self.named(id, item);
        self.referenced = index;
        self.entities[index].handle
    }
}

/// A closure for the error a part's refusal stops the parse with, which has `$load` say the
/// refusal is in part `$step`, such as a field's name or a list's `[2]`.
macro_rules! within {
    ($load:expr, $step:expr) => {
        |_| $load.wrap(|refusal| reflect::within(refusal, $step))
    };
}

/// An identifier's or a string's text, borrowed where it can be, as `.0` takes it.
struct Text<F>(F);

impl<T, F: FnOnce(&str) -> T> Visitor<'_> for Text<F> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a name or a string")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<T, E> {
        Ok((self.0)(text))
    }
}

/// What a tuple is given past its parts, refused as it's entered, at its first character, for the
/// error `.1` makes.
struct Extra<'a, 'w, F>(&'a mut Load<'w>, F);

impl<'de, F: FnOnce() -> Error> DeserializeSeed<'de> for Extra<'_, '_, F> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, _: D) -> Result<(), D::Error> {
        let item = self.0.enter()?;
        Err(self.0.refuse(item, (self.1)()))
    }
}

/// The whole scene: `(entities: {...})`.
struct Scene<'a, 'w>(&'a mut Load<'w>);

impl<'de> DeserializeSeed<'de> for Scene<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        let item = self.0.enter()?;
        deserializer.deserialize_struct("", &[], SceneFields { load: self.0, item })
    }
}

struct SceneFields<'a, 'w> {
    load: &'a mut Load<'w>,
    item: u32,
}

impl<'de> Visitor<'de> for SceneFields<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a scene, `(entities: {...})`")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut given = false;
        while let Some(key) = map.next_key_seed(SceneKey(&mut *self.load))? {
            if given {
                let error = Error::new(
                    "reflect.duplicate_field",
                    "field `entities` is given twice, but each field is given once",
                );
                return Err(self.load.refuse(key, error));
            }
            map.next_value_seed(EntityMap(&mut *self.load))?;
            given = true;
        }
        if !given {
            let error = Error::new(
                "reflect.missing_field",
                "field `entities` isn't given, but a scene gives it",
            );
            return Err(self.load.refuse(self.item, error));
        }
        Ok(())
    }
}

/// The name of the scene's one field, `entities`; returns its item.
struct SceneKey<'a, 'w>(&'a mut Load<'w>);

impl<'de> DeserializeSeed<'de> for SceneKey<'_, '_> {
    type Value = u32;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<u32, D::Error> {
        let item = self.0.enter()?;
        let named = deserializer.deserialize_identifier(Text(|name: &str| {
            if name == "entities" {
                return Ok(());
            }
            Err(Error::new(
                "reflect.no_field",
                format!("there's no field `{name}`; a scene's one field is `entities`"),
            ))
        }))?;
        named.map_err(|error| self.0.refuse(item, error))?;
        Ok(item)
    }
}

/// The map of entities, by ID.
struct EntityMap<'a, 'w>(&'a mut Load<'w>);

impl<'de> DeserializeSeed<'de> for EntityMap<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        self.0.map = self.0.enter()?;
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for EntityMap<'_, '_> {
    type Value = ();

    /// Not starting `struct `, which `ron` takes for a flattened struct's.
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map of entities by ID")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(index) = map.next_key_seed(EntityKey(&mut *self.0))? {
            let components = Components {
                load: &mut *self.0,
                index,
            };
            map.next_value_seed(components)?;
        }
        Ok(())
    }
}

/// An entity's ID, as its key; returns its index.
struct EntityKey<'a, 'w>(&'a mut Load<'w>);

impl<'de> DeserializeSeed<'de> for EntityKey<'_, '_> {
    type Value = usize;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<usize, D::Error> {
        let item = self.0.enter()?;
        let id = deserializer.deserialize_str(Text(str::parse::<EntityId>))?;
        let index = id.and_then(|id| self.0.key(id, item));
        index.map_err(|error| self.0.refuse(item, error))
    }
}

/// An entity's components, by name.
struct Components<'a, 'w> {
    load: &'a mut Load<'w>,
    index: usize,
}

impl<'de> DeserializeSeed<'de> for Components<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        self.load.enter()?;
        deserializer.deserialize_struct("", &[], self)
    }
}

impl<'de> Visitor<'de> for Components<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an entity's components, by name")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let id = self.load.entities[self.index].id;
        let parent = self.load.world.component::<Parent>();
        let parent = parent.expect("every world has registered `Parent`");
        let mut given: Vec<Given> = Vec::new();
        loop {
            let key = ComponentKey {
                load: &mut *self.load,
                id,
            };
            let Some((component, name, item)) = map.next_key_seed(key)? else {
                break;
            };
            let about = |error| about(error, "load", Named::Id(id), name, "");
            if given.iter().any(|given| given.component == component) {
                let error = Error::new(
                    "reflect.duplicate_component",
                    "it's given twice, but an entity has one of each component",
                );
                return Err(self.load.refuse(item, about(error)));
            }
            let read = Top {
                load: &mut *self.load,
                component,
            };
            let read = map.next_value_seed(read);
            let (value, item) = read.inspect_err(|_| self.load.wrap(about))?;
            if component == parent {
                let up = self.load.referenced;
                self.load.entities[self.index].parent = Some((up, item));
            }
            given.push(Given {
                component,
                item,
                value,
            });
        }
        self.load.entities[self.index].given = given;
        Ok(())
    }
}

/// A component type's name, of entity `id`'s component: returns the type's id, its name and the
/// item.
struct ComponentKey<'a, 'w> {
    load: &'a mut Load<'w>,
    id: EntityId,
}

impl<'de, 'w> DeserializeSeed<'de> for ComponentKey<'_, 'w> {
    type Value = (ComponentId, &'w str, u32);

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let item = self.load.enter()?;
        let world = self.load.world;
        let types = self.load.types;
        let id = self.id;
        let found = deserializer.deserialize_identifier(Text(|name: &str| {
            let about = |error| about(error, "load", Named::Id(id), name, "");
            let component = world.named(name).map_err(|error| {
                let message = format!(
                    "{error}; a game registers its types with `World::register` before it loads \
                     a scene of them"
                );
                about(error.with_message(message))
            })?;
            if world.components.ty(component).is_opaque(types) {
                let error = Error::new(
                    "reflect.opaque",
                    "it's opaque, so scenes leave it out, and code gives it",
                );
                return Err(about(error));
            }
            if Some(component) == world.component::<ModelNode>() {
                let error = Error::new(
                    "reflect.read_only",
                    "the world keeps it, as it spawns a model's nodes under the model's root, \
                     which a scene names alone",
                );
                return Err(about(error));
            }
            Ok(component)
        }))?;
        let component = found.map_err(|error| self.load.refuse(item, error))?;
        Ok((component, world.components.name(component), item))
    }
}

/// A component's value, as the text gives it: a struct's of named fields field by field, which the
/// parse fills in once it knows whose instance its entity is of, if it's one, and builds; another
/// type's whole. Returns it, and its item.
struct Top<'a, 'w> {
    load: &'a mut Load<'w>,
    component: ComponentId,
}

impl<'de> DeserializeSeed<'de> for Top<'_, '_> {
    type Value = (Partial, u32);

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let item = self.load.enter()?;
        let world = self.load.world;
        let ty = world.components.ty(self.component);
        if let Type::Named(index) = ty
            && let Shape::Struct {
                fields,
                tuple: false,
            } = &self.load.types.info(index).shape
        {
            // As `Read` enters it, a level deep.
            self.load.enter()?;
            let fields = Fields {
                load: &mut *self.load,
                fields,
                depth: 1,
            };
            let given = deserializer.deserialize_struct("", &[], fields)?;
            return Ok((Partial::Fields(given), item));
        }
        let read = Read {
            load: &mut *self.load,
            ty: &ty,
            meta: &NO_HINTS,
            depth: 1,
        };
        let value = read.deserialize(deserializer)?;
        // Built as it's read, as a whole value never takes a prefab's fields, so that a value its
        // type refuses is refused where it's read.
        let mut column = world.components.new_column(self.component);
        let built = column.push_value(&value);
        built.map_err(|error| self.load.refuse(item, error))?;
        Ok((Partial::Whole(value), item))
    }
}

/// The fields of the struct of named fields that component type `component` is.
///
/// # Panics
///
/// If it isn't one.
fn named_fields(world: &World, component: ComponentId) -> &[Field] {
    let Type::Named(index) = world.components.ty(component) else {
        unreachable!("a struct's fields are given for a struct");
    };
    match &world.types().info(index).shape {
        Shape::Struct { fields, .. } => fields,
        _ => unreachable!("a struct's fields are given for a struct"),
    }
}

/// A value of type `ty`, a field's of hints `meta`, `depth` levels deep.
struct Read<'a, 'w> {
    load: &'a mut Load<'w>,
    ty: &'a Type,
    meta: &'a Meta,
    depth: u32,
}

/// A vector's components' type.
static F32: Type = Type::F32;
static F64: Type = Type::F64;

impl<'de> DeserializeSeed<'de> for Read<'_, '_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        let Self {
            load,
            ty,
            meta,
            depth,
        } = self;
        let item = load.enter()?;
        if depth > MAX_DEPTH {
            let error = Error::new(
                "scene.too_deep",
                format!("it's more than {MAX_DEPTH} levels deep, the most a scene nests"),
            );
            return Err(load.refuse(item, error));
        }
        let types = load.types;
        match ty {
            Type::Bool
            | Type::Int { .. }
            | Type::F32
            | Type::F64
            | Type::String
            | Type::Entity
            | Type::Texture
            | Type::Model
            | Type::Font
            | Type::Prefab => {
                let leaf = Leaf {
                    load,
                    item,
                    ty,
                    meta,
                };
                match ty {
                    Type::Bool => deserializer.deserialize_bool(leaf),
                    Type::Int { signed: true, .. } => deserializer.deserialize_i64(leaf),
                    Type::Int { .. } => deserializer.deserialize_u64(leaf),
                    Type::F32 => deserializer.deserialize_f32(leaf),
                    Type::F64 => deserializer.deserialize_f64(leaf),
                    Type::String => deserializer.deserialize_string(leaf),
                    _ => deserializer.deserialize_str(leaf),
                }
            }
            Type::Vec2 | Type::Vec3 | Type::Vec4 | Type::DVec3 | Type::Quat => {
                let vector = Vector {
                    load,
                    item,
                    ty,
                    meta,
                    depth,
                };
                deserializer.deserialize_tuple(vector.len(), vector)
            }
            Type::Option(inner) => deserializer.deserialize_option(Optional {
                load,
                inner,
                meta,
                depth,
            }),
            Type::List(inner) | Type::Array(inner, _) => {
                let len = match ty {
                    &Type::Array(_, len) => Some(len),
                    _ => None,
                };
                deserializer.deserialize_seq(List {
                    load,
                    item,
                    inner,
                    meta,
                    depth,
                    len,
                })
            }
            &Type::Named(index) => match &types.info(index).shape {
                Shape::Struct {
                    fields,
                    tuple: false,
                } => {
                    let named = Fields {
                        load: &mut *load,
                        fields,
                        depth,
                    };
                    let given = deserializer.deserialize_struct("", &[], named)?;
                    assemble(load, item, fields, given).map(Value::Struct)
                }
                Shape::Struct {
                    fields,
                    tuple: true,
                } => {
                    let len = saved(types, fields).count();
                    let parts = Parts {
                        load,
                        item,
                        fields,
                        depth,
                    };
                    let read = deserializer.deserialize_tuple_struct("", len, parts);
                    read.map(Value::Struct)
                }
                Shape::Enum(variants) => {
                    let choice = Choice {
                        load,
                        item,
                        variants,
                        depth,
                    };
                    deserializer.deserialize_enum("", &[], choice)
                }
                Shape::Opaque => Err(load.refuse(item, opaque())),
            },
            Type::Opaque => Err(load.refuse(item, opaque())),
        }
    }
}

/// The error that a scene gives something opaque, which scenes leave out.
fn opaque() -> Error {
    Error::new(
        "reflect.opaque",
        "it's opaque, so scenes leave it out, and it's made as its type's base has it",
    )
}

/// The fields of `fields` that a scene saves, those that aren't opaque, and their indices.
fn saved<'f>(types: &Types, fields: &'f [Field]) -> impl Iterator<Item = (usize, &'f Field)> {
    let saved = fields.iter().enumerate();
    saved.filter(move |(_, field)| !field.ty.is_opaque(types))
}

/// The fields `given` of `fields`, each saved field's from the scene, each opaque one's as
/// [`Value::Opaque`], and each other the scene leaves out as its default, as a scene saved before
/// its type had the field leaves it out; in the description's order, for a struct or a variant at
/// `item`.
fn assemble<E: de::Error>(
    load: &mut Load<'_>,
    item: u32,
    fields: &[Field],
    given: Vec<Option<Value>>,
) -> Result<Vec<(Ident, Value)>, E> {
    fill(load.types, fields, given, None).map_err(|error| load.refuse(item, error))
}

/// The fields `given` of `fields`, as [`assemble`] fills them in, but each one left out that isn't
/// opaque taken from `base`, the fields of the component of an instance's prefab, where it's given,
/// rather than its default.
///
/// Fails with `reflect.missing_field` for a field left out that has no default.
fn fill(
    types: &Types,
    fields: &[Field],
    given: Vec<Option<Value>>,
    base: Option<&[(Ident, Value)]>,
) -> Result<Vec<(Ident, Value)>> {
    let mut parts = Vec::with_capacity(fields.len());
    for (field, value) in fields.iter().zip(given) {
        let from_base = || {
            let base = base?.iter().find(|(name, _)| *name == field.name);
            base.map(|(_, value)| value.clone())
        };
        let value = match value {
            Some(value) => value,
            None if field.ty.is_opaque(types) => Value::Opaque,
            None => match from_base() {
                Some(value) => value,
                None => types.field_default(field).map_err(|error| {
                    Error::new(
                        "reflect.missing_field",
                        format!(
                            "field `{}` isn't given, and has no default to start from: {error}",
                            field.name
                        ),
                    )
                })?,
            },
        };
        parts.push((field.name.clone(), value));
    }
    Ok(parts)
}

/// A number, a bool, a string or a reference.
struct Leaf<'a, 'w> {
    load: &'a mut Load<'w>,
    item: u32,
    ty: &'a Type,
    meta: &'a Meta,
}

impl Leaf<'_, '_> {
    /// `value`, checked against its field's bounds.
    fn bounded<E: de::Error>(self, value: Value) -> Result<Value, E> {
        match reflect::check(self.load.types, self.ty, self.meta, &value) {
            Ok(()) => Ok(value),
            Err(error) => Err(self.load.refuse(self.item, error)),
        }
    }

    /// The integer `n`, as `value`, if it fits its type.
    fn integer<E: de::Error>(self, n: i128, value: Value) -> Result<Value, E> {
        let &Type::Int { bits, signed } = self.ty else {
            unreachable!("an integer is read for an integer's type");
        };
        let (min, max, sign) = match signed {
            true => (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1, 'i'),
            false => (0, (1i128 << bits) - 1, 'u'),
        };
        if !(min..=max).contains(&n) {
            let error = Error::new(
                "reflect.out_of_range",
                format!("{n} doesn't fit {sign}{bits}, from {min} to {max}"),
            );
            return Err(self.load.refuse(self.item, error));
        }
        self.bounded(value)
    }
}

impl Visitor<'_> for Leaf<'_, '_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a value of {:?}", self.ty)
    }

    fn visit_bool<E: de::Error>(self, b: bool) -> Result<Value, E> {
        Ok(Value::Bool(b))
    }

    fn visit_i64<E: de::Error>(self, n: i64) -> Result<Value, E> {
        self.integer(n.into(), Value::Int(n))
    }

    fn visit_u64<E: de::Error>(self, n: u64) -> Result<Value, E> {
        self.integer(n.into(), Value::UInt(n))
    }

    fn visit_f32<E: de::Error>(self, x: f32) -> Result<Value, E> {
        self.bounded(Value::F32(x))
    }

    fn visit_f64<E: de::Error>(self, x: f64) -> Result<Value, E> {
        self.bounded(Value::F64(x))
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Value, E> {
        match self.ty {
            Type::Entity => {
                let id = text.parse::<EntityId>();
                let id = id.map_err(|error| self.load.refuse(self.item, error))?;
                Ok(Value::Entity(self.load.reference(id, self.item)))
            }
            Type::Texture | Type::Model | Type::Font | Type::Prefab => {
                let assets = self.load.world.assets();
                let (found, asset, code) = match self.ty {
                    Type::Texture => {
                        let found = assets.texture(text).map(Value::Texture);
                        (found, Asset::Texture(text), "asset.no_texture")
                    }
                    Type::Model => {
                        let found = assets.model(text).map(Value::Model);
                        (found, Asset::Model(text), "asset.no_model")
                    }
                    Type::Prefab => {
                        let found = assets.prefab(text).map(Value::Prefab);
                        (found, Asset::Prefab(text), "asset.no_prefab")
                    }
                    _ => {
                        let found = assets.font(text).map(Value::Font);
                        (found, Asset::Font(text), "asset.no_font")
                    }
                };
                match (found, &mut self.load.missing) {
                    (Some(value), _) => {
                        if let &Value::Prefab(prefab) = &value
                            && !self.load.prefabs.contains(&prefab)
                        {
                            self.load.prefabs.push(prefab);
                        }
                        Ok(value)
                    }
                    (None, Some(missing)) => {
                        if !missing.iter().any(|named| named.as_deref() == asset) {
                            missing.push(asset.owned());
                        }
                        // Placeholders, of which a read that gathers names builds nothing.
                        Ok(match asset {
                            Asset::Texture(_) => Value::Texture(Texture::WHITE),
                            Asset::Model(_) => Value::Model(Model::new(0)),
                            Asset::Font(_) => Value::Font(Font::new(0)),
                            Asset::Prefab(_) => Value::Prefab(Prefab::new(0)),
                        })
                    }
                    (None, None) => {
                        let kind = asset.kind();
                        let mut message = assets.not_found(kind, text);
                        if !assets.has_failed(text) {
                            message += &format!(
                                ": a scene that `Context::load_scene_file` or the tool API loads \
                                 has the {kind}s it names loaded first, but one a layer loads \
                                 into a world itself finds only those loaded already"
                            );
                        }
                        let error = Error::new(code, message);
                        Err(self.load.refuse(self.item, error))
                    }
                }
            }
            _ => Ok(Value::String(text.to_owned())),
        }
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<Value, E> {
        match self.ty {
            Type::Entity | Type::Texture | Type::Model | Type::Font | Type::Prefab => {
                self.visit_str(&text)
            }
            _ => Ok(Value::String(text)),
        }
    }
}

/// A vector's or a quaternion's components, inline.
struct Vector<'a, 'w> {
    load: &'a mut Load<'w>,
    item: u32,
    ty: &'a Type,
    meta: &'a Meta,
    depth: u32,
}

impl Vector<'_, '_> {
    fn len(&self) -> usize {
        match self.ty {
            Type::Vec2 => 2,
            Type::Vec3 | Type::DVec3 => 3,
            _ => 4,
        }
    }
}

impl<'de> Visitor<'de> for Vector<'_, '_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} components", self.len())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let len = self.len();
        let what = match self.ty {
            Type::Vec2 => "a Vec2",
            Type::Vec3 => "a Vec3",
            Type::Vec4 => "a Vec4",
            Type::DVec3 => "a DVec3",
            _ => "a quaternion",
        };
        // Each read as the f32 or f64 it is, so that an f32's text rounds once.
        let wide = *self.ty == Type::DVec3;
        let mut xs = [0.0; 4];
        for (i, x) in xs[..len].iter_mut().enumerate() {
            let read = Read {
                load: &mut *self.load,
                ty: if wide { &F64 } else { &F32 },
                meta: self.meta,
                depth: self.depth,
            };
            let step = reflect::COMPONENTS[i];
            match seq
                .next_element_seed(read)
                .inspect_err(within!(self.load, step))?
            {
                Some(Value::F32(component)) => *x = f64::from(component),
                Some(Value::F64(component)) => *x = component,
                Some(_) => unreachable!("a float is read as one"),
                None => {
                    let error = Error::new(
                        "reflect.wrong_length",
                        format!("{what} has {len} components, but it's given {i}"),
                    );
                    return Err(self.load.refuse(self.item, error));
                }
            }
        }
        seq.next_element_seed(Extra(&mut *self.load, || {
            Error::new(
                "reflect.wrong_length",
                format!("{what} has {len} components, but it's given more"),
            )
        }))?;
        // An f32's components went through f64 exactly.
        let narrow = xs.map(|x| x as f32);
        let value = match self.ty {
            Type::Vec2 => Value::Vec2(Vec2::new(narrow[0], narrow[1])),
            Type::Vec3 => Value::Vec3(Vec3::new(narrow[0], narrow[1], narrow[2])),
            Type::Vec4 => Value::Vec4(Vec4::from_array(narrow)),
            Type::DVec3 => Value::DVec3(DVec3::new(xs[0], xs[1], xs[2])),
            _ => Value::Quat(Quat::from_array(narrow)),
        };
        if let Value::Quat(_) = value
            && let Err(error) = Quat::from_value(&value)
        {
            return Err(self.load.refuse(self.item, error));
        }
        Ok(value)
    }
}

/// `None` or `Some(..)`.
struct Optional<'a, 'w> {
    load: &'a mut Load<'w>,
    inner: &'a Type,
    meta: &'a Meta,
    depth: u32,
}

impl<'de> Visitor<'de> for Optional<'_, '_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("`None` or `Some(..)`")
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Option(None))
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        let read = Read {
            load: self.load,
            ty: self.inner,
            meta: self.meta,
            depth: self.depth + 1,
        };
        let value = read.deserialize(deserializer)?;
        Ok(Value::Option(Some(Box::new(value))))
    }
}

/// A list's elements, or an array's `len`.
struct List<'a, 'w> {
    load: &'a mut Load<'w>,
    item: u32,
    inner: &'a Type,
    meta: &'a Meta,
    depth: u32,
    len: Option<u32>,
}

impl<'de> Visitor<'de> for List<'_, '_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list, `[...]`")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        loop {
            let read = Read {
                load: &mut *self.load,
                ty: self.inner,
                meta: self.meta,
                depth: self.depth + 1,
            };
            let i = items.len();
            match seq
                .next_element_seed(read)
                .inspect_err(within!(self.load, &format!("[{i}]")))?
            {
                Some(item) => items.push(item),
                None => break,
            }
        }
        if let Some(len) = self.len
            && items.len() != len as usize
        {
            let error = Error::new(
                "reflect.wrong_length",
                format!("an array of {len} is given {} elements", items.len()),
            );
            return Err(self.load.refuse(self.item, error));
        }
        Ok(Value::List(items))
    }
}

/// A struct's or a struct variant's fields, by name: each one's value, if it's given, by its place
/// among `fields`, which [`assemble`] fills in.
struct Fields<'a, 'w> {
    load: &'a mut Load<'w>,
    fields: &'a [Field],
    depth: u32,
}

impl<'de> Visitor<'de> for Fields<'_, '_> {
    type Value = Vec<Option<Value>>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fields by name, `(name: value)`")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut given = vec![None; self.fields.len()];
        loop {
            let key = FieldKey {
                load: &mut *self.load,
                fields: self.fields,
            };
            let Some((i, item)) = map.next_key_seed(key)? else {
                break;
            };
            let field = &self.fields[i];
            if given[i].is_some() {
                let error = Error::new(
                    "reflect.duplicate_field",
                    format!(
                        "field `{}` is given twice, but each field is given once",
                        field.name
                    ),
                );
                return Err(self.load.refuse(item, error));
            }
            let read = Read {
                load: &mut *self.load,
                ty: &field.ty,
                meta: &field.meta,
                depth: self.depth + 1,
            };
            let value = map.next_value_seed(read);
            given[i] = Some(value.inspect_err(within!(self.load, &field.name))?);
        }
        Ok(given)
    }
}

/// A field's name: returns its index, and the item.
struct FieldKey<'a, 'w> {
    load: &'a mut Load<'w>,
    fields: &'a [Field],
}

impl<'de> DeserializeSeed<'de> for FieldKey<'_, '_> {
    type Value = (usize, u32);

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(usize, u32), D::Error> {
        let item = self.load.enter()?;
        let (types, fields) = (self.load.types, self.fields);
        let found = deserializer.deserialize_identifier(Text(|name: &str| {
            let Some(i) = fields.iter().position(|field| field.name == name) else {
                let names = reflect::list(saved(types, fields).map(|(_, field)| &field.name));
                return Err(Error::new(
                    "reflect.no_field",
                    format!("there's no field `{name}`; the fields are {names}"),
                ));
            };
            if fields[i].ty.is_opaque(types) {
                return Err(Error::new(
                    "reflect.opaque",
                    format!(
                        "field `{name}` is opaque, so scenes leave it out, and it's made as its \
                         type's base has it"
                    ),
                ));
            }
            Ok(i)
        }))?;
        let i = found.map_err(|error| self.load.refuse(item, error))?;
        Ok((i, item))
    }
}

/// A tuple struct's or a tuple variant's fields, by position.
struct Parts<'a, 'w> {
    load: &'a mut Load<'w>,
    item: u32,
    fields: &'a [Field],
    depth: u32,
}

impl<'de> Visitor<'de> for Parts<'_, '_> {
    type Value = Vec<(Ident, Value)>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fields by position, `(value, value)`")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let types = self.load.types;
        let mut given = vec![None; self.fields.len()];
        for (i, field) in saved(types, self.fields) {
            let read = Read {
                load: &mut *self.load,
                ty: &field.ty,
                meta: &field.meta,
                depth: self.depth + 1,
            };
            match seq
                .next_element_seed(read)
                .inspect_err(within!(self.load, &field.name))?
            {
                Some(value) => given[i] = Some(value),
                // Those left out take their defaults, as a struct's fields do.
                None => return assemble(self.load, self.item, self.fields, given),
            }
        }
        let fields = self.fields;
        seq.next_element_seed(Extra(&mut *self.load, || {
            let names = reflect::list(saved(types, fields).map(|(_, field)| &field.name));
            Error::new(
                "reflect.no_field",
                format!("it's given more than its fields, {names}"),
            )
        }))?;
        assemble(self.load, self.item, self.fields, given)
    }
}

/// An enum's variant, by name, and its fields.
struct Choice<'a, 'w> {
    load: &'a mut Load<'w>,
    item: u32,
    variants: &'a [Variant],
    depth: u32,
}

impl<'de> Visitor<'de> for Choice<'_, '_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a variant, by name")
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Value, A::Error> {
        let key = VariantKey {
            load: &mut *self.load,
            variants: self.variants,
        };
        let (i, access) = data.variant_seed(key)?;
        let variant = &self.variants[i];
        let len = saved(self.load.types, &variant.fields).count();
        let fields = &variant.fields;
        let given = if len == 0 {
            access.unit_variant()?;
            assemble(self.load, self.item, fields, vec![None; fields.len()])?
        } else if variant.tuple {
            let parts = Parts {
                load: self.load,
                item: self.item,
                fields,
                depth: self.depth,
            };
            access.tuple_variant(len, parts)?
        } else {
            let named = Fields {
                load: &mut *self.load,
                fields,
                depth: self.depth,
            };
            let given = access.struct_variant(&[], named)?;
            assemble(self.load, self.item, fields, given)?
        };
        Ok(Value::Variant(variant.name.clone(), given))
    }
}

/// A variant's name: returns its index.
struct VariantKey<'a, 'w> {
    load: &'a mut Load<'w>,
    variants: &'a [Variant],
}

impl<'de> DeserializeSeed<'de> for VariantKey<'_, '_> {
    type Value = usize;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<usize, D::Error> {
        let item = self.load.enter()?;
        let variants = self.variants;
        let found = deserializer.deserialize_identifier(Text(|name: &str| {
            let found = variants.iter().position(|variant| variant.name == name);
            found.ok_or_else(|| {
                let names: Vec<&str> = variants.iter().map(|variant| &*variant.name).collect();
                reflect::no_variant(name, &names)
            })
        }))?;
        found.map_err(|error| self.load.refuse(item, error))
    }
}
