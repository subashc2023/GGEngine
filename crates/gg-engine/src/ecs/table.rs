//! Where components live: a table for each set of component types, whose columns are each
//! component's own `Vec`, reached through a trait object and downcast once per table, so that the
//! world needs no unsafe code; and the component types a world has seen, described to reflection.

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;

use gg_core::{Error, Result};

use super::{Component, Entity};
use crate::reflect::{Name, Reflect, Type, TypeIndex, Types, Value};

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
    by_name: HashMap<Name, ComponentId>,
    types: Types,
}

#[derive(Clone)]
struct Info {
    /// Its name in its description.
    name: Name,
    index: TypeIndex,
    /// Makes an empty column of the type, for a new table.
    new_column: fn() -> Box<dyn AnyColumn>,
}

impl Components {
    /// `T`'s id, which it gets once it's described, the first time it's asked for.
    ///
    /// Fails as describing it does, such as with `reflect.name_taken`.
    pub fn register<T: Component>(&mut self) -> Result<ComponentId> {
        if let Some(&id) = self.ids.get(&TypeId::of::<T>()) {
            return Ok(id);
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
        self.infos.push(Info {
            name: name.clone(),
            index,
            new_column: || Box::new(Vec::<T>::new()),
        });
        self.ids.insert(TypeId::of::<T>(), id);
        self.by_name.insert(name, id);
        Ok(id)
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

    /// Every component type's name, in the order they registered.
    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.infos.iter().map(|info| &*info.name)
    }

    /// An empty column of its type.
    pub fn new_column(&self, id: ComponentId) -> Box<dyn AnyColumn> {
        (self.infos[id.0 as usize].new_column)()
    }

    /// Its type, as reflection describes it.
    pub fn ty(&self, id: ComponentId) -> Type {
        Type::Named(self.infos[id.0 as usize].index)
    }

    pub fn types(&self) -> &Types {
        &self.types
    }

    /// Whether `other` has seen the same types as these, in the same order, so that its ids name
    /// the same types: those described, and the component types among them.
    pub(super) fn same(&self, other: &Self) -> bool {
        let indices = self.infos.iter().map(|info| info.index);
        self.types.same(&other.types) && indices.eq(other.infos.iter().map(|info| info.index))
    }
}

/// The entities with one set of component types, a row each: each column holds one type's
/// values, row by row, and `entities` which entity each row is.
pub struct Table {
    /// Sorted.
    pub(super) components: Box<[ComponentId]>,
    /// `components[i]`'s values in `columns[i]`.
    pub(super) columns: Box<[Box<dyn AnyColumn>]>,
    pub(super) entities: Vec<Entity>,
}

impl Table {
    /// An empty table of `signature`, sorted, whose types `components` has seen.
    pub(super) fn new(signature: &[ComponentId], components: &Components) -> Self {
        let columns = signature
            .iter()
            .map(|&id| (components.infos[id.0 as usize].new_column)())
            .collect();
        Self {
            components: signature.into(),
            columns,
            entities: Vec::new(),
        }
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

    /// Component `id`'s values, of type `T`, if the table has them.
    pub fn column<T: Component>(&self, id: ComponentId) -> Option<&[T]> {
        Some(self.columns[self.position(id)?].vec())
    }

    pub fn column_mut<T: Component>(&mut self, id: ComponentId) -> Option<&mut Vec<T>> {
        let position = self.position(id)?;
        Some(self.columns[position].vec_mut())
    }

    /// Takes `row` out of every column and the entities, the last row taking its place, and
    /// returns the entity whose row that is now, if one moved.
    pub(super) fn swap_remove(&mut self, row: usize) -> Option<Entity> {
        for column in &mut self.columns {
            column.swap_remove(row);
        }
        self.entities.swap_remove(row);
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
        for (&id, column) in self.components.iter().zip(&mut self.columns) {
            let mut one = components.new_column(id);
            column.move_row(row, &mut *one);
            taken.push((id, one));
        }
        self.entities.swap_remove(row);
        (taken, self.entities.get(row).copied())
    }
}

impl Clone for Table {
    fn clone(&self) -> Self {
        Self {
            components: self.components.clone(),
            columns: self.columns.iter().map(|column| column.cloned()).collect(),
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

    /// `row`'s value, for reflection.
    fn row(&self, row: usize) -> &dyn Reflect;

    fn row_mut(&mut self, row: usize) -> &mut dyn Reflect;

    /// Pushes the component `value` makes, as a scene's is made: on its type's
    /// [`base`](Component::base), if it has one.
    ///
    /// Fails as [`Reflect::with_value`] and [`Reflect::from_value`] do.
    fn push_value(&mut self, value: &Value) -> Result<()>;

    /// A copy of the column.
    fn cloned(&self) -> Box<dyn AnyColumn>;

    /// Makes `to`, a column of the same type, a copy of this one, in the memory it holds.
    fn copy_to(&self, to: &mut dyn AnyColumn);
}

impl<T: Component> AnyColumn for Vec<T> {
    fn swap_remove(&mut self, row: usize) {
        Vec::swap_remove(self, row);
    }

    fn move_row(&mut self, row: usize, to: &mut dyn AnyColumn) {
        let value = Vec::swap_remove(self, row);
        to.vec_mut::<T>().push(value);
    }

    fn row(&self, row: usize) -> &dyn Reflect {
        &self[row]
    }

    fn row_mut(&mut self, row: usize) -> &mut dyn Reflect {
        &mut self[row]
    }

    fn push_value(&mut self, value: &Value) -> Result<()> {
        let made = match T::base() {
            Some(base) => base.with_value(value)?,
            None => T::from_value(value)?,
        };
        self.push(made);
        Ok(())
    }

    fn cloned(&self) -> Box<dyn AnyColumn> {
        Box::new(self.clone())
    }

    fn copy_to(&self, to: &mut dyn AnyColumn) {
        to.vec_mut::<T>().clone_from(self);
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
