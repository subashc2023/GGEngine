//! Where components live: a table for each set of component types, with a `Vec` per component
//! as a column, reached through a trait object and downcast once per table, so the world needs
//! no unsafe code. Also the component types a world has seen, described to reflection.

use std::any::{Any, TypeId, type_name};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::{iter, mem};

use gg_core::{Error, Result};

use super::declared::{self, Declared, Kind, is_identifier};
use super::scene::WORDS;
use super::{Component, Entity, World, hash};
use crate::reflect::{self, Ident, Meta, Reflect, Shape, Type, TypeIndex, TypeInfo, Types, Value};

/// A component type's index among those a world has seen, in order first seen. World-local.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentId(u32);

/// An entity's components out of their table: one-row columns with their ids, sorted by id.
pub type Row = Vec<(ComponentId, Box<dyn AnyColumn>)>;

/// The component types a world has seen, with their descriptions and their fields' types'.
#[derive(Clone, Default)]
pub struct Components {
    ids: HashMap<TypeId, ComponentId>,
    /// By id.
    infos: Vec<Info>,
    /// By name; looked up, never iterated.
    by_name: HashMap<Ident, ComponentId>,
    types: Types,
    /// Each change gives it a number no other world's types have had.
    revision: u64,
    /// Whether the world has loaded mods, past which no Rust type registers.
    pub(super) mods_loaded: bool,
}

#[derive(Clone)]
struct Info {
    name: Ident,
    /// What the world's hash takes it as, from its name.
    key: u64,
    index: TypeIndex,
    /// [`Component::exclusive`].
    exclusive: Option<&'static str>,
    /// [`Component::cleared`].
    cleared: &'static [&'static str],
    new_column: NewColumn,
}

/// How an empty column is made: a Rust type's `Vec`, or a declared type's slots.
#[derive(Clone)]
enum NewColumn {
    Rust(fn() -> Box<dyn AnyColumn>),
    Declared(Arc<Declared>),
}

impl Components {
    /// `T`'s id, assigned on first request, when it's described.
    ///
    /// Fails, leaving the types as they were:
    /// - `ecs.registered_late`: the world has loaded mods;
    /// - describing it fails, such as `reflect.name_taken` or `reflect.bad_default`;
    /// - `reflect.unnamed`: it isn't described as a struct or enum;
    /// - `ecs.bad_exclusive`: its [exclusive field](Component::exclusive) isn't a `bool` of its
    ///   own.
    pub fn register<T: Component>(&mut self) -> Result<ComponentId> {
        if let Some(&id) = self.ids.get(&TypeId::of::<T>()) {
            return Ok(id);
        }
        // A mod may have declared its name, or a field type's. Refused whatever's installed, so
        // the game fails at its first run, not when a player installs a mod taking the name.
        if self.mods_loaded {
            return Err(Error::new(
                "ecs.registered_late",
                format!(
                    "`{}` registers after the world loaded mods, whose scripts may declare any \
                     name: a game registers its component types (`World::register`) before it \
                     loads mods (`Mods::load`)",
                    type_name::<T>()
                ),
            ));
        }
        // Forgotten if refused, with the types described for it, so a refused type leaves its
        // name free, as a script's does.
        let before = self.types.iter().len();
        let index = (self.describe::<T>()).inspect_err(|_| self.types.truncate(before))?;
        let id = ComponentId(u32::try_from(self.infos.len()).expect("fewer than 2^32 types"));
        let name = self.types.info(index).name.clone();
        self.infos.push(Info {
            key: hash::type_key(&name),
            name: name.clone(),
            index,
            exclusive: T::exclusive(),
            cleared: T::cleared(),
            new_column: NewColumn::Rust(|| Box::new(Vec::<T>::new())),
        });
        self.ids.insert(TypeId::of::<T>(), id);
        self.by_name.insert(name, id);
        self.revision = super::next_serial();
        Ok(id)
    }

    /// Describes `T` for [`register`](Self::register). Refused, it may leave what it described.
    fn describe<T: Component>(&mut self) -> Result<TypeIndex> {
        let index = match T::describe(&mut self.types)? {
            Type::Named(index) => index,
            other => {
                return Err(Error::new(
                    "reflect.unnamed",
                    format!(
                        "`{}` describes itself as {other:?}, but a component is a struct or an \
                         enum, which reflection names",
                        type_name::<T>()
                    ),
                ));
            }
        };
        let name = &self.types.info(index).name;
        // Another type's description (a type that passes everything to a field might give it):
        // two component types would share a name.
        let (owner, rust) = self.types.rust_type(index);
        if owner != TypeId::of::<T>() {
            return Err(Error::new(
                "reflect.name_taken",
                format!(
                    "`{}` describes itself as `{name}`, which is `{rust}`'s description, but each \
                     component type describes itself, under a name of its own",
                    type_name::<T>()
                ),
            ));
        }
        if let Some(field) = T::exclusive() {
            let fields = match &self.types.info(index).shape {
                Shape::Struct {
                    fields,
                    tuple: false,
                } => &fields[..],
                _ => &[],
            };
            let named = fields.iter().find(|of| of.name == field);
            if named.is_none_or(|of| of.ty != Type::Bool) {
                return Err(Error::new(
                    "ecs.bad_exclusive",
                    format!(
                        "`{name}` names `{field}` its exclusive field, but it has no `bool` field \
                         `{field}`: a component's exclusive field is a `bool` of its own, of a \
                         struct with named fields"
                    ),
                ));
            }
        }
        for &field in T::cleared() {
            let fields = match &self.types.info(index).shape {
                Shape::Struct {
                    fields,
                    tuple: false,
                } => &fields[..],
                _ => &[],
            };
            let entity = Type::Option(Box::new(Type::Entity));
            let named = fields.iter().find(|of| of.name == field);
            if named.is_none_or(|of| of.ty != entity) {
                return Err(Error::new(
                    "ecs.bad_cleared",
                    format!(
                        "`{name}` names `{field}` a field a delete clears, but it has no \
                         `Option<Entity>` field `{field}`: a field a delete clears is an \
                         `Option<Entity>` of its own, of a struct with named fields"
                    ),
                ));
            }
        }
        Ok(index)
    }

    /// Declares a script's component type, which no Rust type backs: a struct `name` of `fields`
    /// in order (name, kind, default), documented by `doc`. Checks everything before recording
    /// anything, so a refused type leaves its name free.
    ///
    /// Fails:
    /// - `reflect.name_taken`: a type has `name` already;
    /// - `reflect.bad_name`: a type or field name isn't an identifier, is one of RON's words
    ///   (no scene could save it), or a field's is repeated;
    /// - `reflect.bad_default`: a kind refuses its default, or a number isn't finite.
    pub fn declare(
        &mut self,
        name: &str,
        doc: &str,
        fields: Vec<(Ident, Kind, Value)>,
    ) -> Result<ComponentId> {
        if let Some(index) = self.types.find(name) {
            let what = match self.by_name.get(name) {
                Some(_) => "a component type",
                None => "a type of fields",
            };
            let (_, rust) = self.types.rust_type(index);
            return Err(Error::new(
                "reflect.name_taken",
                format!("`{name}` names {what} already, {rust}, but each type's name is its own"),
            ));
        }
        let (declared, info) = described(name, doc, fields)?;
        let name = declared.name.clone();
        let index = self.types.declare(info);
        let id = ComponentId(u32::try_from(self.infos.len()).expect("fewer than 2^32 types"));
        self.infos.push(Info {
            key: hash::type_key(&name),
            name: name.clone(),
            index,
            exclusive: None,
            cleared: &[],
            new_column: NewColumn::Declared(declared),
        });
        self.by_name.insert(name, id);
        self.revision = super::next_serial();
        Ok(id)
    }

    /// Redeclares script type `id` under its name and id with new `fields` and `doc`. Returns its
    /// fields, which each column [migrates](declared::migrate) its rows to.
    ///
    /// Fails like [`declare`](Self::declare), except for its own name, and with
    /// `reflect.name_taken` if `id` is a Rust type's.
    pub fn redeclare(
        &mut self,
        id: ComponentId,
        doc: &str,
        fields: Vec<(Ident, Kind, Value)>,
    ) -> Result<Arc<Declared>> {
        let info = &self.infos[id.0 as usize];
        if let NewColumn::Rust(_) = info.new_column {
            let (_, rust) = self.types.rust_type(info.index);
            return Err(Error::new(
                "reflect.name_taken",
                format!("`{}` names {rust}, which no script declares", info.name),
            ));
        }
        let (declared, described) = described(&info.name, doc, fields)?;
        let info = &mut self.infos[id.0 as usize];
        info.new_column = NewColumn::Declared(Arc::clone(&declared));
        self.types.redeclare(info.index, described);
        self.revision = super::next_serial();
        Ok(declared)
    }

    /// Declared type `id`'s field names and kinds; `None` for a Rust type.
    pub fn declared(&self, id: ComponentId) -> Option<&Arc<Declared>> {
        match &self.infos[id.0 as usize].new_column {
            NewColumn::Declared(declared) => Some(declared),
            NewColumn::Rust(_) => None,
        }
    }

    /// `T`'s id, from [`register`](Self::register).
    ///
    /// # Panics
    ///
    /// If `T` fails to register, such as when another type has its name.
    pub fn id<T: Component>(&mut self) -> ComponentId {
        self.register::<T>()
            .unwrap_or_else(|error| panic!("{error:?}"))
    }

    /// The id of the type `type_id` names, once it has one.
    pub fn get(&self, type_id: TypeId) -> Option<ComponentId> {
        self.ids.get(&type_id).copied()
    }

    /// The id of the type named `name`, once it has one.
    pub fn named(&self, name: &str) -> Option<ComponentId> {
        self.by_name.get(name).copied()
    }

    /// Its name in its description.
    pub fn name(&self, id: ComponentId) -> &str {
        &self.infos[id.0 as usize].name
    }

    /// What the world's hash takes it as: from its name, the same in every world.
    pub(super) fn key(&self, id: ComponentId) -> u64 {
        self.infos[id.0 as usize].key
    }

    /// Every component type's name, in registration order.
    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.infos.iter().map(|info| &*info.name)
    }

    pub(super) fn exclusive(&self, id: ComponentId) -> Option<&'static str> {
        self.infos[id.0 as usize].exclusive
    }

    /// Each type whose fields a delete clears, with the field names, in registration order.
    pub(super) fn cleared(&self) -> impl Iterator<Item = (ComponentId, &'static [&'static str])> {
        let ids = (0..).map(ComponentId);
        let cleared = ids.zip(&self.infos).map(|(id, info)| (id, info.cleared));
        cleared.filter(|(_, fields)| !fields.is_empty())
    }

    /// An empty column of its type.
    pub fn new_column(&self, id: ComponentId) -> Box<dyn AnyColumn> {
        match &self.infos[id.0 as usize].new_column {
            NewColumn::Rust(new) => new(),
            NewColumn::Declared(declared) => Box::new(declared::Column::new(declared)),
        }
    }

    /// Its type, as reflection describes it.
    pub fn ty(&self, id: ComponentId) -> Type {
        Type::Named(self.infos[id.0 as usize].index)
    }

    pub fn types(&self) -> &Types {
        &self.types
    }

    /// Each registration, declaration or redeclaration gives another number, which no other
    /// world's types have had.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether `other` has seen the same types in the same order, so its ids name the same types.
    pub(super) fn same(&self, other: &Self) -> bool {
        let indices = self.infos.iter().map(|info| info.index);
        self.types.same(&other.types) && indices.eq(other.infos.iter().map(|info| info.index))
    }
}

/// The fields and description of the type [`Components::declare`] declares as `name`.
///
/// Fails as `declare` does, but for a name another type has.
fn described(
    name: &str,
    doc: &str,
    fields: Vec<(Ident, Kind, Value)>,
) -> Result<(Arc<Declared>, TypeInfo)> {
    let names = iter::once(name).chain(fields.iter().map(|(field, ..)| &**field));
    if let Some(bad) = names.clone().find(|name| !is_identifier(name)) {
        return Err(Error::new(
            "reflect.bad_name",
            format!("`{bad}` isn't an identifier: a letter or `_`, then letters, digits or `_`"),
        ));
    }
    if let Some(word) = names.clone().find(|name| WORDS.contains(name)) {
        return Err(Error::new(
            "reflect.bad_name",
            format!(
                "`{word}` is one of RON's words, `true`, `false`, `Some`, `None`, `inf` and \
                 `NaN`, which a scene writes no name as, so that no scene could save the type"
            ),
        ));
    }
    for (i, (field, ..)) in fields.iter().enumerate() {
        if fields[..i].iter().any(|(other, ..)| other == field) {
            return Err(Error::new(
                "reflect.bad_name",
                format!("`{name}` has two fields named `{field}`, but each field's is its own"),
            ));
        }
    }
    let mut defaults = Vec::with_capacity(fields.len());
    for (field, kind, default) in &fields {
        let refused = |what: &dyn std::fmt::Display| {
            Error::new(
                "reflect.bad_default",
                format!("`{name}`'s field `{field}` defaults to a value it refuses: {what}"),
            )
        };
        let slot = kind
            .slot(default)
            .map_err(|error| refused(&reflect::located(&error).1))?;
        // A default must compare equal to itself, as a reload compares them.
        let finite = match default {
            Value::F64(x) => x.is_finite(),
            Value::Vec3(v) => v.is_finite(),
            Value::DVec3(v) => v.is_finite(),
            _ => true,
        };
        if !finite {
            return Err(refused(&"a number that isn't finite"));
        }
        defaults.push(slot);
    }
    let name: Ident = Cow::Owned(name.to_owned());
    let declared = Arc::new(Declared {
        name: name.clone(),
        fields: fields
            .iter()
            .map(|(field, kind, _)| (field.clone(), *kind))
            .collect(),
        defaults: defaults.into(),
    });
    let info = TypeInfo {
        name,
        doc: Cow::Owned(doc.to_owned()),
        shape: Shape::Struct {
            fields: (fields.into_iter())
                .map(|(name, kind, default)| reflect::Field {
                    name,
                    ty: kind.ty(),
                    meta: Meta::default(),
                    default: Some(default),
                })
                .collect(),
            tuple: false,
        },
    };
    Ok((declared, info))
}

/// The entities with one set of component types, a row each. Each column holds one type's values;
/// `entities` says which entity each row is, `changed` when each column's rows last changed (in
/// the world's change ticks), and `joined` when each row joined the table.
///
/// Its fields are private so every change to a column's values goes through the table, which
/// marks what it reaches.
pub struct Table {
    /// Sorted.
    components: Box<[ComponentId]>,
    /// `components[i]`'s values in `columns[i]`.
    columns: Box<[Box<dyn AnyColumn>]>,
    /// When `columns[i]`'s rows last changed, in `changed[i]`.
    changed: Box<[Changed]>,
    /// When each row's entity joined, by spawn or move; tables without columns mark it too.
    joined: Changed,
    entities: Vec<Entity>,
}

/// When a column's rows last changed, in the world's change ticks: each row's own tick, plus the
/// tick at which all changed, so a query changing the column needn't visit the rows. A row is new
/// at the tick it joins the table; the last row moves with its values into a row taken out.
#[derive(Clone, Debug, Default)]
pub(crate) struct Changed {
    rows: Vec<u64>,
    all: u64,
    /// The latest tick of any row or of all, which no row's is past.
    latest: u64,
}

impl Changed {
    /// Whether a row changed after `tick`, or may have: a row that changed then left keeps its
    /// tick here.
    pub(crate) fn since(&self, tick: u64) -> bool {
        self.latest > tick
    }

    /// The tick at which row `row` last changed.
    pub(crate) fn row(&self, row: usize) -> u64 {
        self.rows[row].max(self.all)
    }

    fn mark(&mut self, row: usize, tick: u64) {
        self.rows[row] = tick;
        self.latest = tick;
    }

    fn mark_all(&mut self, tick: u64) {
        self.all = tick;
        self.latest = tick;
    }

    fn push(&mut self, tick: u64) {
        self.rows.push(tick);
        self.latest = tick;
    }
}

impl Table {
    /// An empty table of sorted `signature`, whose types `components` has seen.
    pub(super) fn new(signature: &[ComponentId], components: &Components) -> Self {
        let columns = signature
            .iter()
            .map(|&id| components.new_column(id))
            .collect();
        Self {
            components: signature.into(),
            columns,
            changed: iter::repeat_with(Changed::default)
                .take(signature.len())
                .collect(),
            joined: Changed::default(),
            entities: Vec::new(),
        }
    }

    /// Its component types, sorted, in column order.
    pub fn components(&self) -> &[ComponentId] {
        &self.components
    }

    /// Where component `id`'s column is.
    pub fn position(&self, id: ComponentId) -> Option<usize> {
        self.components.binary_search(&id).ok()
    }

    pub fn has(&self, id: ComponentId) -> bool {
        self.position(id).is_some()
    }

    /// Which entity each row is.
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    /// How many rows it has room for, without growing.
    #[cfg(test)]
    pub(super) fn capacity(&self) -> usize {
        self.entities.capacity()
    }

    /// Component `id`'s values, of type `T`.
    pub fn column<T: Component>(&self, id: ComponentId) -> Option<&[T]> {
        Some(self.columns[self.position(id)?].vec())
    }

    /// When component `id`'s rows last changed.
    pub(crate) fn changed(&self, id: ComponentId) -> Option<&Changed> {
        Some(&self.changed[self.position(id)?])
    }

    /// When each row's entity joined: spawned in, or moved in by gaining or losing a component.
    pub(crate) fn joined(&self) -> &Changed {
        &self.joined
    }

    /// Whether a row joined, or a column's rows changed, after `tick` or maybe (see
    /// [`Changed::since`]). A row taken out marks nothing: the last row moves into its place.
    pub(crate) fn changed_since(&self, tick: u64) -> bool {
        self.joined.since(tick) || self.changed.iter().any(|changed| changed.since(tick))
    }

    /// Its columns, without their types, in its components' order.
    pub(super) fn columns(&self) -> &[Box<dyn AnyColumn>] {
        &self.columns
    }

    /// The column at `position`, without its type.
    pub(super) fn column_at(&self, position: usize) -> &dyn AnyColumn {
        &*self.columns[position]
    }

    /// The untyped column at `position`, to change row `row`, marked changed at `tick`.
    pub(super) fn row_mut(&mut self, position: usize, row: usize, tick: u64) -> &mut dyn AnyColumn {
        self.changed[position].mark(row, tick);
        &mut *self.columns[position]
    }

    /// The untyped column at `position`, to change any rows, all marked changed at `tick`.
    pub(super) fn column_at_mut(&mut self, position: usize, tick: u64) -> &mut dyn AnyColumn {
        self.changed[position].mark_all(tick);
        &mut *self.columns[position]
    }

    /// Row `row`'s value of component `id`, of type `T`, to change, marked changed at `tick`.
    pub(super) fn get_mut<T: Component>(
        &mut self,
        id: ComponentId,
        row: usize,
        tick: u64,
    ) -> Option<&mut T> {
        let position = self.position(id)?;
        Some(&mut self.row_mut(position, row, tick).vec_mut::<T>()[row])
    }

    /// Its component types, columns and entities, for a query whose terms take the columns apart.
    /// A column whose type `changes` accepts is marked all changed at `tick`.
    pub(super) fn reach(
        &mut self,
        tick: u64,
        changes: impl Fn(ComponentId) -> bool,
    ) -> (&[ComponentId], &mut [Box<dyn AnyColumn>], &[Entity]) {
        for (&id, changed) in self.components.iter().zip(&mut self.changed) {
            if changes(id) {
                changed.mark_all(tick);
            }
        }
        (&self.components, &mut self.columns, &self.entities)
    }

    /// Pushes `value` onto component `id`'s column for the row [`push_entity`](Self::push_entity)
    /// makes, before or after.
    ///
    /// # Panics
    ///
    /// If the table hasn't the component.
    pub(super) fn push<T: Component>(&mut self, id: ComponentId, value: T) {
        let position = self.position(id).expect("the table has the component");
        self.columns[position].vec_mut::<T>().push(value);
    }

    /// Moves the value in one-row `from` onto the column at `position`, for the row
    /// [`push_entity`](Self::push_entity) makes, before or after.
    pub(super) fn push_from(&mut self, position: usize, from: &mut dyn AnyColumn) {
        from.move_row(0, &mut *self.columns[position]);
    }

    /// Ends a row of `entity`, whose values are or will be pushed; new in every column at `tick`.
    pub(super) fn push_entity(&mut self, entity: Entity, tick: u64) {
        self.entities.push(entity);
        self.joined.push(tick);
        for changed in &mut self.changed {
            changed.push(tick);
        }
    }

    /// Moves row `row`'s value in the column at `position` onto `to`, a column of its type, the
    /// last row taking its place. The row is leaving: its other values are taken or moved too.
    pub(super) fn take_out(&mut self, position: usize, row: usize, to: &mut dyn AnyColumn) {
        self.columns[position].move_row(row, to);
        self.changed[position].rows.swap_remove(row);
    }

    /// Takes row `row`'s value of component `id` out of its column, the last row taking its
    /// place. The row is leaving: its other values are taken or moved too.
    pub(super) fn take<T: Component>(&mut self, id: ComponentId, row: usize) -> Option<T> {
        let position = self.position(id)?;
        self.changed[position].rows.swap_remove(row);
        Some(self.columns[position].vec_mut::<T>().swap_remove(row))
    }

    /// Moves row `row`, of `entity`, onto the end of `into`, new there at `tick`. It moves its
    /// values of the types `into` has; the rest have left already ([`take`](Self::take),
    /// [`take_out`](Self::take_out)). The last row takes its place; returns the entity now in row
    /// `row`, if one moved. Values of types it lacked are pushed onto `into` before or after.
    pub(super) fn move_row(
        &mut self,
        row: usize,
        into: &mut Table,
        entity: Entity,
        tick: u64,
    ) -> Option<Entity> {
        for ((id, column), changed) in self
            .components
            .iter()
            .zip(&mut self.columns)
            .zip(&mut self.changed)
        {
            if let Some(position) = into.position(*id) {
                column.move_row(row, &mut *into.columns[position]);
                changed.rows.swap_remove(row);
            }
        }
        self.entities.swap_remove(row);
        self.joined.rows.swap_remove(row);
        into.push_entity(entity, tick);
        self.entities.get(row).copied()
    }

    /// Takes `row` out of every column and the entities, the last row taking its place. Returns
    /// the entity now in `row`, if one moved.
    pub(super) fn swap_remove(&mut self, row: usize) -> Option<Entity> {
        for (column, changed) in self.columns.iter_mut().zip(&mut self.changed) {
            column.swap_remove(row);
            changed.rows.swap_remove(row);
        }
        self.entities.swap_remove(row);
        self.joined.rows.swap_remove(row);
        self.entities.get(row).copied()
    }

    /// Like [`swap_remove`](Self::swap_remove), but keeps `row`'s values in one-row columns with
    /// their ids, in id order.
    pub(super) fn take_row(
        &mut self,
        row: usize,
        components: &Components,
    ) -> (Row, Option<Entity>) {
        let mut taken = Vec::with_capacity(self.columns.len());
        let columns = self.columns.iter_mut().zip(&mut self.changed);
        for (&id, (column, changed)) in self.components.iter().zip(columns) {
            let mut one = components.new_column(id);
            column.move_row(row, &mut *one);
            changed.rows.swap_remove(row);
            taken.push((id, one));
        }
        self.entities.swap_remove(row);
        self.joined.rows.swap_remove(row);
        (taken, self.entities.get(row).copied())
    }

    /// Marks every row of every column changed, and joined, at `tick`.
    pub(super) fn mark_all(&mut self, tick: u64) {
        self.joined.mark_all(tick);
        for changed in &mut self.changed {
            changed.mark_all(tick);
        }
    }
}

impl Clone for Table {
    fn clone(&self) -> Self {
        Self {
            components: self.components.clone(),
            columns: self.columns.iter().map(|column| column.cloned()).collect(),
            changed: self.changed.clone(),
            joined: self.joined.clone(),
            entities: self.entities.clone(),
        }
    }

    /// Copies `source`, whose ids name the same types (as copies of one world's do). With the same
    /// component types, it copies into the memory it holds.
    fn clone_from(&mut self, source: &Self) {
        if self.components != source.components {
            *self = source.clone();
            return;
        }
        for (from, to) in source.columns.iter().zip(&mut self.columns) {
            from.copy_to(&mut **to);
        }
        let changed = source.changed.iter().zip(&mut self.changed);
        for (from, to) in changed.chain([(&source.joined, &mut self.joined)]) {
            to.rows.clone_from(&from.rows);
            (to.all, to.latest) = (from.all, from.latest);
        }
        self.entities.clone_from(&source.entities);
    }
}

/// A table column: one component type's `Vec` without its type, for changes that move rows of
/// any type.
pub trait AnyColumn: Any + Send + Sync {
    /// Drops `row`'s value, the last row taking its place.
    fn swap_remove(&mut self, row: usize);

    /// Moves `row`'s value onto the end of `to`, a column of the same type, the last row taking
    /// its place.
    fn move_row(&mut self, row: usize, to: &mut dyn AnyColumn);

    /// `row`'s value, as reflection reads it.
    fn value(&self, row: usize) -> Value;

    /// Part `index` of `row`'s value, like [`Reflect::part`]. Reflection of a row starts here: a
    /// declared type's row is no Rust value.
    fn part(&self, row: usize, index: u32) -> Option<&dyn Reflect>;

    fn part_mut(&mut self, row: usize, index: u32) -> Option<&mut dyn Reflect>;

    /// Sets `row`'s value whole, like [`Reflect::set_value`].
    fn set_value(&mut self, row: usize, value: &Value) -> Result<()>;

    /// Calls `each` on each entity `row`'s value holds, like [`Reflect::entities_mut`].
    fn entities_mut(&mut self, row: usize, each: &mut dyn FnMut(&mut Entity));

    /// Pushes the component `value` makes, built as a scene builds it: on the type's
    /// [`base`](Component::base), if any.
    ///
    /// Fails like [`Reflect::with_value`] and [`Reflect::from_value`].
    fn push_value(&mut self, value: &Value) -> Result<()>;

    /// What a snapshot keeps of `row`'s opaque state ([`Component::save_opaque`]).
    fn opaque(&self, row: usize) -> Option<String>;

    /// Pushes the component [`Component::load_opaque`] makes of `text`.
    ///
    /// Fails like `load_opaque`.
    fn push_opaque(&mut self, text: &str) -> Result<()>;

    fn cloned(&self) -> Box<dyn AnyColumn>;

    /// A copy of `row`'s value, in a one-row column.
    fn clone_row(&self, row: usize) -> Box<dyn AnyColumn>;

    /// Swaps `row`'s value with one-row `with`, of the same type (see
    /// [`clone_row`](Self::clone_row)).
    fn swap_row(&mut self, row: usize, with: &mut dyn AnyColumn);

    /// Makes `to`, of the same type, a copy of this column, in the memory it holds.
    fn copy_to(&self, to: &mut dyn AnyColumn);

    /// The sum of its values' terms of the world's hash, its type's key being `key`, each value's
    /// entity `rows`'s at its row. Entities a client's view hides, by slot, read as dead where a
    /// value names them (`World::hash_seen`).
    fn digest(&self, rows: &[Entity], key: u64, world: &World, hidden: &[bool]) -> u64;
}

impl<T: Component> AnyColumn for Vec<T> {
    fn swap_remove(&mut self, row: usize) {
        Vec::swap_remove(self, row);
    }

    fn move_row(&mut self, row: usize, to: &mut dyn AnyColumn) {
        let value = Vec::swap_remove(self, row);
        to.vec_mut::<T>().push(value);
    }

    fn value(&self, row: usize) -> Value {
        self[row].to_value()
    }

    fn part(&self, row: usize, index: u32) -> Option<&dyn Reflect> {
        self[row].part(index)
    }

    fn part_mut(&mut self, row: usize, index: u32) -> Option<&mut dyn Reflect> {
        self[row].part_mut(index)
    }

    fn set_value(&mut self, row: usize, value: &Value) -> Result<()> {
        self[row].set_value(value)
    }

    fn entities_mut(&mut self, row: usize, each: &mut dyn FnMut(&mut Entity)) {
        self[row].entities_mut(each);
    }

    fn push_value(&mut self, value: &Value) -> Result<()> {
        let made = match T::base() {
            Some(base) => base.with_value(value)?,
            None => T::from_value(value)?,
        };
        self.push(made);
        Ok(())
    }

    fn opaque(&self, row: usize) -> Option<String> {
        self[row].save_opaque()
    }

    fn push_opaque(&mut self, text: &str) -> Result<()> {
        self.push(T::load_opaque(text)?);
        Ok(())
    }

    fn cloned(&self) -> Box<dyn AnyColumn> {
        Box::new(self.clone())
    }

    fn clone_row(&self, row: usize) -> Box<dyn AnyColumn> {
        Box::new(vec![self[row].clone()])
    }

    fn swap_row(&mut self, row: usize, with: &mut dyn AnyColumn) {
        mem::swap(&mut self[row], &mut with.vec_mut::<T>()[0]);
    }

    fn copy_to(&self, to: &mut dyn AnyColumn) {
        to.vec_mut::<T>().clone_from(self);
    }

    fn digest(&self, rows: &[Entity], key: u64, world: &World, hidden: &[bool]) -> u64 {
        hash::terms(self, rows, key, world, hidden)
    }
}

impl dyn AnyColumn {
    /// The column as its `Vec`.
    ///
    /// # Panics
    ///
    /// If it holds a type other than `T`, which a world's columns never do.
    pub fn vec<T: Component>(&self) -> &Vec<T> {
        let any: &dyn Any = self;
        any.downcast_ref()
            .expect("a component's columns hold its type")
    }

    pub fn vec_mut<T: Component>(&mut self) -> &mut Vec<T> {
        let any: &mut dyn Any = self;
        any.downcast_mut()
            .expect("a component's columns hold its type")
    }
}
