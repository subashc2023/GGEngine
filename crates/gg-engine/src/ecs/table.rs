//! Where components live: a table for each set of component types, whose columns are each
//! component's own `Vec`, reached through a trait object and downcast once per table, so that the
//! world needs no unsafe code; and the component types a world has seen, described to reflection.

use std::any::{Any, TypeId, type_name};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::{iter, mem};

use gg_core::{Error, Result};

use super::declared::{self, Declared, Kind, is_identifier};
use super::{Component, Entity, World, hash};
use crate::reflect::{self, Ident, Meta, Reflect, Shape, Type, TypeIndex, TypeInfo, Types, Value};

/// A component type's index among those a world has seen, in the order it first saw them: the
/// world's own, which never leaves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentId(u32);

/// An entity's components out of their table: each in a one-row column of its own, beside its id,
/// sorted by id, as a table's columns are.
pub type Row = Vec<(ComponentId, Box<dyn AnyColumn>)>;

/// The component types a world has seen, and the descriptions of them and their fields' types.
#[derive(Clone, Default)]
pub struct Components {
    ids: HashMap<TypeId, ComponentId>,
    /// By id.
    infos: Vec<Info>,
    /// By name; looked up, never iterated.
    by_name: HashMap<Ident, ComponentId>,
    types: Types,
    /// What names the types as they are, which each change to them changes, to a number no
    /// other world's types have had.
    revision: u64,
    /// Whether the world has loaded mods, past which no Rust type registers.
    pub(super) mods_loaded: bool,
}

#[derive(Clone)]
struct Info {
    /// Its name in its description.
    name: Ident,
    /// What the world's hash takes it as, from its name.
    key: u64,
    index: TypeIndex,
    /// Its exclusive field's name, as [`Component::exclusive`] gives it.
    exclusive: Option<&'static str>,
    /// Makes an empty column of the type, for a new table.
    new_column: NewColumn,
}

/// How an empty column of a component type is made: a Rust type's `Vec`, or a declared type's
/// slots.
#[derive(Clone)]
enum NewColumn {
    Rust(fn() -> Box<dyn AnyColumn>),
    Declared(Arc<Declared>),
}

impl Components {
    /// `T`'s id, which it gets once it's described, the first time it's asked for.
    ///
    /// Fails with `ecs.registered_late` once the world has loaded mods; as describing it does, such
    /// as with `reflect.name_taken` or `reflect.bad_default`; with `reflect.unnamed` if it
    /// describes itself as something other than a struct or an enum; and with `ecs.bad_exclusive`
    /// if it names an exclusive field that isn't a `bool` of its own, as [`Component::exclusive`]
    /// says.
    pub fn register<T: Component>(&mut self) -> Result<ComponentId> {
        if let Some(&id) = self.ids.get(&TypeId::of::<T>()) {
            return Ok(id);
        }
        // A mod may have declared its name, or a field's type's, which would then be the mod's:
        // refused whatever's installed, so that the game fails as it first runs, not as a player
        // installs a mod that takes the name.
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
        let id = ComponentId(u32::try_from(self.infos.len()).expect("fewer than 2^32 types"));
        let name = self.types.info(index).name.clone();
        // Another type's description, as one that passes everything to a field might give: two
        // component types would share a name, whichever registered first.
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
        let exclusive = T::exclusive();
        if let Some(field) = exclusive {
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
        self.infos.push(Info {
            key: hash::type_key(&name),
            name: name.clone(),
            index,
            exclusive,
            new_column: NewColumn::Rust(|| Box::new(Vec::<T>::new())),
        });
        self.ids.insert(TypeId::of::<T>(), id);
        self.by_name.insert(name, id);
        self.revision = super::next_serial();
        Ok(id)
    }

    /// Declares a component type that a script declares, which no Rust type is: a struct named
    /// `name`, of `fields`, in their order, each a name, a kind and a default, which its
    /// description gives, and with `doc` for its doc. Checks everything before it records anything,
    /// so that a type refused leaves its name free.
    ///
    /// Fails with `reflect.name_taken` if a type has `name` already; `reflect.bad_name` for a name,
    /// the type's or a field's, that isn't an identifier, or a field's given twice; and
    /// `reflect.bad_default` for a default its kind refuses, or a number that isn't finite.
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
            new_column: NewColumn::Declared(declared),
        });
        self.by_name.insert(name, id);
        self.revision = super::next_serial();
        Ok(id)
    }

    /// Declares type `id`, which a script declared, again, as [`declare`](Self::declare) declares
    /// one, under its name and its id: its fields now `fields`, and its doc `doc`. Returns its
    /// fields, which each of its columns then [migrates](declared::migrate) its rows to.
    ///
    /// Fails as `declare` does, but for its name, which is its own; and with `reflect.name_taken`
    /// if `id` is a Rust type's.
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

    /// The fields of declared type `id`, its names and kinds; `None` if it's a Rust type.
    pub fn declared(&self, id: ComponentId) -> Option<&Arc<Declared>> {
        match &self.infos[id.0 as usize].new_column {
            NewColumn::Declared(declared) => Some(declared),
            NewColumn::Rust(_) => None,
        }
    }

    /// `T`'s id, as [`register`](Self::register) gives it.
    ///
    /// # Panics
    ///
    /// If `T` fails to register, as when another type has its name.
    pub fn id<T: Component>(&mut self) -> ComponentId {
        self.register::<T>()
            .unwrap_or_else(|error| panic!("{error:?}"))
    }

    /// The id of the type `type_id` names, if it has one yet.
    pub fn get(&self, type_id: TypeId) -> Option<ComponentId> {
        self.ids.get(&type_id).copied()
    }

    /// The id of the type named `name`, if it has one yet.
    pub fn named(&self, name: &str) -> Option<ComponentId> {
        self.by_name.get(name).copied()
    }

    /// Its name in its description.
    pub fn name(&self, id: ComponentId) -> &str {
        &self.infos[id.0 as usize].name
    }

    /// What the world's hash takes it as: from its name, so that it's the same in every world.
    pub(super) fn key(&self, id: ComponentId) -> u64 {
        self.infos[id.0 as usize].key
    }

    /// Every component type's name, in the order they registered.
    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.infos.iter().map(|info| &*info.name)
    }

    /// The name of its exclusive field, if it has one.
    pub(super) fn exclusive(&self, id: ComponentId) -> Option<&'static str> {
        self.infos[id.0 as usize].exclusive
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

    /// What names the types as they are, as a type registers or a script declares one, or declares
    /// one again: another number for each change, which no other world's types have had.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether `other` has seen the same types as these, in the same order, so that its ids name
    /// the same types: those described, and the component types among them.
    pub(super) fn same(&self, other: &Self) -> bool {
        let indices = self.infos.iter().map(|info| info.index);
        self.types.same(&other.types) && indices.eq(other.infos.iter().map(|info| info.index))
    }
}

/// The fields of the type a script declares as `name`, of `fields`, each a name, a kind and a
/// default, and its description, with `doc` for its doc, as [`Components::declare`] declares it.
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
        // So that a default compares equal to itself, as a reload compares them.
        let finite = match default {
            Value::F64(x) => x.is_finite(),
            Value::Vec3(v) => v.is_finite(),
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

/// The entities with one set of component types, a row each: each column holds one type's
/// values, row by row, `entities` which entity each row is, `changed` when each column's rows last
/// changed, by the world's change ticks, and `joined` when each row joined the table. Its fields
/// are its own, so that whatever reaches a column's values to change them does it through the
/// table, which marks what it reaches.
pub struct Table {
    /// Sorted.
    components: Box<[ComponentId]>,
    /// `components[i]`'s values in `columns[i]`.
    columns: Box<[Box<dyn AnyColumn>]>,
    /// When `columns[i]`'s rows last changed, in `changed[i]`.
    changed: Box<[Changed]>,
    /// When each row's entity joined the table, spawned or moved into it, as a table without
    /// columns marks too.
    joined: Changed,
    entities: Vec<Entity>,
}

/// When the rows of one of a table's columns last changed, by the world's change ticks: each row's
/// own tick, and the tick at which they all changed, as a query that reaches the column to change
/// it changes them, without visiting them. A row is new at the tick it joins the table, as an
/// entity spawned or moved into it is; and the last row moves with its values into a row taken out.
#[derive(Clone, Debug, Default)]
pub(crate) struct Changed {
    rows: Vec<u64>,
    all: u64,
    /// The latest tick of any row, or of all of them, which no row's is past.
    latest: u64,
}

impl Changed {
    /// Whether a row changed after tick `tick`, or may have: one that changed and then left keeps
    /// its tick here.
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
    /// An empty table of `signature`, sorted, whose types `components` has seen.
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

    /// Its component types, sorted, whose values its columns hold, in that order.
    pub fn components(&self) -> &[ComponentId] {
        &self.components
    }

    /// Where component `id`'s column is, if the table has one.
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

    /// Component `id`'s values, of type `T`, if the table has them.
    pub fn column<T: Component>(&self, id: ComponentId) -> Option<&[T]> {
        Some(self.columns[self.position(id)?].vec())
    }

    /// When component `id`'s rows last changed, if the table has them.
    pub(crate) fn changed(&self, id: ComponentId) -> Option<&Changed> {
        Some(&self.changed[self.position(id)?])
    }

    /// When each row's entity joined the table: spawned into it, or moved into it as it was given
    /// a component or had one taken away.
    pub(crate) fn joined(&self) -> &Changed {
        &self.joined
    }

    /// Its columns, without their types, in its components' order.
    pub(super) fn columns(&self) -> &[Box<dyn AnyColumn>] {
        &self.columns
    }

    /// The column at `position`, without its type.
    pub(super) fn column_at(&self, position: usize) -> &dyn AnyColumn {
        &*self.columns[position]
    }

    /// The column at `position`, without its type, to change its row `row`, which it marks changed
    /// at tick `tick`.
    pub(super) fn row_mut(&mut self, position: usize, row: usize, tick: u64) -> &mut dyn AnyColumn {
        self.changed[position].mark(row, tick);
        &mut *self.columns[position]
    }

    /// The column at `position`, without its type, to change any of its rows, which it marks all
    /// changed at tick `tick`.
    pub(super) fn column_at_mut(&mut self, position: usize, tick: u64) -> &mut dyn AnyColumn {
        self.changed[position].mark_all(tick);
        &mut *self.columns[position]
    }

    /// Row `row`'s value of component `id`, of type `T`, to change, which it marks changed at tick
    /// `tick`; `None` if the table hasn't the component.
    pub(super) fn get_mut<T: Component>(
        &mut self,
        id: ComponentId,
        row: usize,
        tick: u64,
    ) -> Option<&mut T> {
        let position = self.position(id)?;
        Some(&mut self.row_mut(position, row, tick).vec_mut::<T>()[row])
    }

    /// Its component types, its columns, which it marks all changed at tick `tick` where `changes`
    /// says of a column's type that they change, and its entities: for a query, whose terms take
    /// the columns apart.
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

    /// Pushes `value` onto the end of component `id`'s column, of type `T`, for the row that
    /// [`push_entity`](Self::push_entity) makes, before or after.
    ///
    /// # Panics
    ///
    /// If the table hasn't the component.
    pub(super) fn push<T: Component>(&mut self, id: ComponentId, value: T) {
        let position = self.position(id).expect("the table has the component");
        self.columns[position].vec_mut::<T>().push(value);
    }

    /// Moves the value in `from`, a one-row column of the type at `position`, onto the end of that
    /// column, for the row that [`push_entity`](Self::push_entity) makes, before or after.
    pub(super) fn push_from(&mut self, position: usize, from: &mut dyn AnyColumn) {
        from.move_row(0, &mut *self.columns[position]);
    }

    /// Ends a row of `entity`, whose values the columns have had pushed onto them or will, new in
    /// every column at tick `tick`.
    pub(super) fn push_entity(&mut self, entity: Entity, tick: u64) {
        self.entities.push(entity);
        self.joined.push(tick);
        for changed in &mut self.changed {
            changed.push(tick);
        }
    }

    /// Moves row `row`'s value of the column at `position` onto the end of `to`, a column of its
    /// type, the last row's taking its place: as the row leaves the table, which takes its other
    /// values or moves them after.
    pub(super) fn take_out(&mut self, position: usize, row: usize, to: &mut dyn AnyColumn) {
        self.columns[position].move_row(row, to);
        self.changed[position].rows.swap_remove(row);
    }

    /// Takes row `row`'s value of component `id`, of type `T`, out of its column, the last row's
    /// taking its place, as the row leaves the table, which takes its other values or moves them
    /// after; `None` if the table hasn't the component.
    pub(super) fn take<T: Component>(&mut self, id: ComponentId, row: usize) -> Option<T> {
        let position = self.position(id)?;
        self.changed[position].rows.swap_remove(row);
        Some(self.columns[position].vec_mut::<T>().swap_remove(row))
    }

    /// Moves row `row`, whose entity is `entity`, onto the end of `into`, new there at tick
    /// `tick`: its values of the types `into` has, the rest of its values having left already, as
    /// [`take`](Self::take) and [`take_out`](Self::take_out) take them; the last row taking its
    /// place. Returns the entity whose row `row` is now, if one moved. Values of `into`'s types it
    /// hadn't are pushed onto `into` before or after.
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

    /// Takes `row` out of every column and the entities, the last row taking its place, and
    /// returns the entity whose row that is now, if one moved.
    pub(super) fn swap_remove(&mut self, row: usize) -> Option<Entity> {
        for (column, changed) in self.columns.iter_mut().zip(&mut self.changed) {
            column.swap_remove(row);
            changed.rows.swap_remove(row);
        }
        self.entities.swap_remove(row);
        self.joined.rows.swap_remove(row);
        self.entities.get(row).copied()
    }

    /// As [`swap_remove`](Self::swap_remove), but keeps `row`: each of its values in a one-row
    /// column of its own, beside its component's id, in the table's order, which is the id's.
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

    /// Marks every row of every column changed at tick `tick`, and as joining the table then.
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

    /// Makes the table a copy of `source`, whose ids name the types this one's do, as two copies
    /// of a world's do: in the memory it holds, if they have the same component types.
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

/// A column of a table: one component type's `Vec`, without its type, for the changes that move
/// rows whatever their types.
pub trait AnyColumn: Any + Send + Sync {
    /// Drops `row`'s value, the last row's taking its place.
    fn swap_remove(&mut self, row: usize);

    /// Moves `row`'s value onto the end of `to`, a column of the same type, the last row's taking
    /// its place.
    fn move_row(&mut self, row: usize, to: &mut dyn AnyColumn);

    /// `row`'s value, as reflection reads it.
    fn value(&self, row: usize) -> Value;

    /// Part `index` of `row`'s value, as [`Reflect::part`] finds it: a field, of a component.
    /// What reflection reaches of a row starts here, as a declared type's row is no Rust value.
    fn part(&self, row: usize, index: u32) -> Option<&dyn Reflect>;

    fn part_mut(&mut self, row: usize, index: u32) -> Option<&mut dyn Reflect>;

    /// Sets `row`'s value whole, as [`Reflect::set_value`] does.
    fn set_value(&mut self, row: usize, value: &Value) -> Result<()>;

    /// Calls `each` with each entity `row`'s value holds, as [`Reflect::entities_mut`] does.
    fn entities_mut(&mut self, row: usize, each: &mut dyn FnMut(&mut Entity));

    /// Pushes the component `value` makes, as a scene's is made: on its type's
    /// [`base`](Component::base), if it has one.
    ///
    /// Fails as [`Reflect::with_value`] and [`Reflect::from_value`] do.
    fn push_value(&mut self, value: &Value) -> Result<()>;

    /// What a snapshot keeps of `row`'s value's opaque state, as [`Component::save_opaque`] gives
    /// it.
    fn opaque(&self, row: usize) -> Option<String>;

    /// Pushes the component `text` makes, as [`Component::load_opaque`] makes it.
    ///
    /// Fails as `load_opaque` does.
    fn push_opaque(&mut self, text: &str) -> Result<()>;

    /// A copy of the column.
    fn cloned(&self) -> Box<dyn AnyColumn>;

    /// A copy of `row`'s value, in a one-row column of its own.
    fn clone_row(&self, row: usize) -> Box<dyn AnyColumn>;

    /// Swaps `row`'s value with the one of `with`, a one-row column of the same type, as
    /// [`clone_row`](Self::clone_row) makes one.
    fn swap_row(&mut self, row: usize, with: &mut dyn AnyColumn);

    /// Makes `to`, a column of the same type, a copy of this one, in the memory it holds.
    fn copy_to(&self, to: &mut dyn AnyColumn);

    /// The sum of its values' terms of the world's hash, its type's being `key`, and the entity
    /// each is `rows`'s at its row.
    fn digest(&self, rows: &[Entity], key: u64, world: &World) -> u64;
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

    fn digest(&self, rows: &[Entity], key: u64, world: &World) -> u64 {
        hash::terms(self, rows, key, world)
    }
}

impl dyn AnyColumn {
    /// The column as the `Vec` it is.
    ///
    /// # Panics
    ///
    /// If it holds another type than `T`, which a world's columns never do.
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
