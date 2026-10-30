//! The reader: one typed parse of a scene's text through `ron`, whose seeds reflection's
//! descriptions drive, never `ron`'s own values, which can't tell a struct from a map or an `f32`
//! from an `f64`. It checks everything a load could refuse before anything changes, and builds
//! each component as it ends. A refusal says where it is: the parse numbers each key and value it
//! enters, and to locate the one refused, parses again, stopping as it enters it, where `ron`'s
//! cursor is at its first character.

use std::collections::HashMap;
use std::fmt;

use gg_core::math::{Quat, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use ron::Options;
use ron::de::SpannedError;
use ron::extensions::Extensions;
use serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
};

use super::{MAX_DEPTH, NO_HINTS};
use crate::Reflect;
use crate::ecs::by_name::about;
use crate::ecs::id::Named;
use crate::ecs::table::{AnyColumn, ComponentId, Row};
use crate::ecs::{Entity, EntityId, Parent, World};
use crate::reflect::{self, Field, Ident, Meta, Shape, Type, Types, Value, Variant};

/// A scene's entity, as the parse reads it.
pub(super) struct Loaded {
    pub(super) id: EntityId,
    /// The handle it takes as it's spawned.
    pub(super) handle: Entity,
    pub(super) row: Row,
    /// Its parent's index among the scene's entities, and the item its `Parent` is.
    pub(super) parent: Option<(usize, u32)>,
    /// The item its key is, once it's read.
    key: Option<u32>,
    /// The item it's first named at, as a key or a reference.
    first: u32,
}

/// The scene `text`'s entities, in the order it first names them, with the handles they take in
/// that order: a prefab's, whose IDs are its own, if `prefab`, and which has one root.
pub(super) fn parse(world: &World, text: &str, prefab: bool) -> Result<Vec<Loaded>> {
    let mut load = Load::new(world, prefab, None);
    let mut deserializer = deserializer(text)?;
    if let Err(error) = run(&mut load, &mut deserializer) {
        return Err(match load.refused.take() {
            Some((item, refusal)) => locate(world, text, prefab, item, refusal),
            None => syntax(&error),
        });
    }
    let refuse = |item, error| Err(locate(world, text, prefab, item, error));
    let entities = load.entities;
    if let Some(unknown) = entities.iter().find(|entity| entity.key.is_none()) {
        let error = Error::new(
            "scene.unknown_id",
            format!(
                "the scene has no entity {}: a scene's entities refer only to each other",
                unknown.id
            ),
        );
        return refuse(unknown.first, error);
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
                return refuse(load.map, error);
            }
            [first, second, ..] => {
                let error = Error::new(
                    "scene.prefab_roots",
                    format!(
                        "a prefab has one root, but entities {} and {} have no parent",
                        first.id, second.id
                    ),
                );
                return refuse(second.key.expect("every entity has a key"), error);
            }
        }
    }
    Ok(entities)
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

/// `refusal`, at item `item`, saying where it is: the text is parsed again, up to the item, where
/// the parser is at its first character, and the refusal is carried out from there, gathering the
/// entity, the component and the path.
fn locate(world: &World, text: &str, prefab: bool, item: u32, refusal: Error) -> Error {
    let mut load = Load::new(world, prefab, Some((item, refusal)));
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
}

impl<'w> Load<'w> {
    fn new(world: &'w World, prefab: bool, stop: Option<(u32, Error)>) -> Self {
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
            row: Vec::new(),
            parent: None,
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
        let mut row: Row = Vec::new();
        loop {
            let key = ComponentKey {
                load: &mut *self.load,
                id,
            };
            let Some((component, name, item)) = map.next_key_seed(key)? else {
                break;
            };
            let about = |error| about(error, "load", Named::Id(id), name, "");
            if row.iter().any(|&(other, _)| other == component) {
                let error = Error::new(
                    "reflect.duplicate_component",
                    "it's given twice, but an entity has one of each component",
                );
                return Err(self.load.refuse(item, about(error)));
            }
            let built = Built {
                load: &mut *self.load,
                component,
            };
            let built = map.next_value_seed(built);
            let (column, item) = built.inspect_err(|_| self.load.wrap(about))?;
            if component == parent {
                let up = self.load.referenced;
                self.load.entities[self.index].parent = Some((up, item));
            }
            row.push((component, column));
        }
        row.sort_unstable_by_key(|&(component, _)| component);
        self.load.entities[self.index].row = row;
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
            Ok(component)
        }))?;
        let component = found.map_err(|error| self.load.refuse(item, error))?;
        Ok((component, world.components.name(component), item))
    }
}

/// A component's value, made as it ends, in a column of its own; returns the column, and its item.
struct Built<'a, 'w> {
    load: &'a mut Load<'w>,
    component: ComponentId,
}

impl<'de> DeserializeSeed<'de> for Built<'_, '_> {
    type Value = (Box<dyn AnyColumn>, u32);

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let item = self.load.enter()?;
        let world = self.load.world;
        let components = &world.components;
        let ty = components.ty(self.component);
        let read = Read {
            load: &mut *self.load,
            ty: &ty,
            meta: &NO_HINTS,
            depth: 1,
        };
        let value = read.deserialize(deserializer)?;
        let mut column = components.new_column(self.component);
        let built = column.push_value(&value);
        built.map_err(|error| self.load.refuse(item, error))?;
        Ok((column, item))
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
            | Type::Texture => {
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
            Type::Vec2 | Type::Vec3 | Type::Vec4 | Type::Quat => {
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
                    let fields = Fields {
                        load,
                        item,
                        fields,
                        depth,
                    };
                    let read = deserializer.deserialize_struct("", &[], fields);
                    read.map(Value::Struct)
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
    let mut parts = Vec::with_capacity(fields.len());
    for (field, value) in fields.iter().zip(given) {
        let value = match value {
            Some(value) => value,
            None if field.ty.is_opaque(load.types) => Value::Opaque,
            None => match load.types.field_default(field) {
                Ok(value) => value,
                Err(error) => {
                    let error = Error::new(
                        "reflect.missing_field",
                        format!(
                            "field `{}` isn't given, and has no default to start from: {error}",
                            field.name
                        ),
                    );
                    return Err(load.refuse(item, error));
                }
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
            Type::Texture => match self.load.world.assets().texture(text) {
                Some(texture) => Ok(Value::Texture(texture)),
                None => {
                    let error = Error::new(
                        "asset.no_texture",
                        format!(
                            "no texture was loaded from `{text}`: a game loads the textures its \
                             scenes show, with `Context::add_texture` or `add_sprite_sheet`, \
                             before it loads them"
                        ),
                    );
                    Err(self.load.refuse(self.item, error))
                }
            },
            _ => Ok(Value::String(text.to_owned())),
        }
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<Value, E> {
        match self.ty {
            Type::Entity | Type::Texture => self.visit_str(&text),
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
            Type::Vec3 => 3,
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
            _ => "a quaternion",
        };
        let mut xs = [0.0; 4];
        for (i, x) in xs[..len].iter_mut().enumerate() {
            let read = Read {
                load: &mut *self.load,
                ty: &F32,
                meta: self.meta,
                depth: self.depth,
            };
            let step = reflect::COMPONENTS[i];
            match seq
                .next_element_seed(read)
                .inspect_err(within!(self.load, step))?
            {
                Some(Value::F32(component)) => *x = component,
                Some(_) => unreachable!("an f32 is read as one"),
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
        let value = match self.ty {
            Type::Vec2 => Value::Vec2(Vec2::new(xs[0], xs[1])),
            Type::Vec3 => Value::Vec3(Vec3::new(xs[0], xs[1], xs[2])),
            Type::Vec4 => Value::Vec4(Vec4::from_array(xs)),
            _ => Value::Quat(Quat::from_array(xs)),
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

/// A struct's or a struct variant's fields, by name.
struct Fields<'a, 'w> {
    load: &'a mut Load<'w>,
    item: u32,
    fields: &'a [Field],
    depth: u32,
}

impl<'de> Visitor<'de> for Fields<'_, '_> {
    type Value = Vec<(Ident, Value)>;

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
        assemble(self.load, self.item, self.fields, given)
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
                load: self.load,
                item: self.item,
                fields,
                depth: self.depth,
            };
            access.struct_variant(&[], named)?
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
