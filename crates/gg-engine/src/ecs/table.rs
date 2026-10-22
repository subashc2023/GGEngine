//! Where components live: a table for each set of component types, whose columns are each
//! component's own `Vec`, reached through a trait object and downcast once per table, so that the
//! world needs no unsafe code; and the component types a world has seen.

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;

use super::{Component, Entity};

/// A component type's index among those a world has seen, in the order it first saw them: the
/// world's own, which never leaves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentId(u32);

/// The component types a world has seen.
#[derive(Default)]
pub struct Components {
    ids: HashMap<TypeId, ComponentId>,
    /// By id.
    infos: Vec<Info>,
}

struct Info {
    name: &'static str,
    /// Makes an empty column of the type, for a new table.
    new_column: fn() -> Box<dyn AnyColumn>,
}

impl Components {
    /// `T`'s id, which it gets the first time it's asked for.
    pub fn id<T: Component>(&mut self) -> ComponentId {
        *self.ids.entry(TypeId::of::<T>()).or_insert_with(|| {
            let id = ComponentId(u32::try_from(self.infos.len()).expect("fewer than 2^32 types"));
            self.infos.push(Info {
                name: type_name::<T>(),
                new_column: || Box::new(Vec::<T>::new()),
            });
            id
        })
    }

    /// The id of the type `type_id` names, if it has one yet.
    pub fn get(&self, type_id: TypeId) -> Option<ComponentId> {
        self.ids.get(&type_id).copied()
    }

    pub fn name(&self, id: ComponentId) -> &'static str {
        self.infos[id.0 as usize].name
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
}

/// A column of a table: one component type's `Vec`, without its type, for the changes that move
/// rows whatever their types.
pub trait AnyColumn: Any + Send + Sync {
    /// Drops `row`'s value, the last row's taking its place.
    fn swap_remove(&mut self, row: usize);

    /// Moves `row`'s value onto the end of `to`, a column of the same type, the last row's taking
    /// its place.
    fn move_row(&mut self, row: usize, to: &mut dyn AnyColumn);
}

impl<T: Component> AnyColumn for Vec<T> {
    fn swap_remove(&mut self, row: usize) {
        Vec::swap_remove(self, row);
    }

    fn move_row(&mut self, row: usize, to: &mut dyn AnyColumn) {
        let value = Vec::swap_remove(self, row);
        to.vec_mut::<T>().push(value);
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
