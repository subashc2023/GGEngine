//! Component types that scripts declare as they load, which no Rust type is: each a struct of
//! fields of a few kinds, numbers, bools, vectors and entities, whose values a table keeps flat, a
//! row's after another's in one `Vec` for the column, so that copying the world allocates no more
//! for them than for a Rust type's `Vec`. Reflection reaches them as it does Rust's, by their
//! descriptions, which [`Components::declare`](super::table::Components::declare) makes.

use std::any::Any;
use std::sync::Arc;

use gg_core::math::Vec3;
use gg_core::{Error, Result};

use super::table::AnyColumn;
use super::{Entity, World, hash};
use crate::reflect::{self, Ident, Reflect, Type, Value};

/// What a field of a declared type holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    /// An `f64`, as Luau's numbers are.
    Number,
    /// A `Vec3`.
    Vector,
    /// An `Option<Entity>`: an entity, or none.
    Entity,
}

impl Kind {
    /// Its fields' type, as reflection describes it.
    pub fn ty(self) -> Type {
        match self {
            Self::Bool => Type::Bool,
            Self::Number => Type::F64,
            Self::Vector => Type::Vec3,
            Self::Entity => Type::Option(Box::new(Type::Entity)),
        }
    }

    /// A field's value of this kind, made from `value`, as its type's [`Reflect::from_value`]
    /// makes one, and fails.
    pub(super) fn slot(self, value: &Value) -> Result<Slot> {
        Ok(match self {
            Self::Bool => Slot::Bool(bool::from_value(value)?),
            Self::Number => Slot::Number(f64::from_value(value)?),
            Self::Vector => Slot::Vector(Vec3::from_value(value)?),
            Self::Entity => Slot::Entity(Option::<Entity>::from_value(value)?),
        })
    }
}

/// A declared type: its name, and its fields' names and kinds, in their order.
#[derive(Debug, PartialEq)]
pub struct Declared {
    pub name: Ident,
    pub fields: Box<[(Ident, Kind)]>,
}

/// A field's value, of a declared type's row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    Bool(bool),
    Number(f64),
    Vector(Vec3),
    Entity(Option<Entity>),
}

impl Slot {
    /// Its value, for reflection, as the Rust type its kind's is.
    pub fn reflect(&self) -> &dyn Reflect {
        match self {
            Self::Bool(b) => b,
            Self::Number(x) => x,
            Self::Vector(v) => v,
            Self::Entity(entity) => entity,
        }
    }

    pub fn reflect_mut(&mut self) -> &mut dyn Reflect {
        match self {
            Self::Bool(b) => b,
            Self::Number(x) => x,
            Self::Vector(v) => v,
            Self::Entity(entity) => entity,
        }
    }
}

/// A declared type's column: each row's fields, in their order, a row's after another's.
#[derive(Clone)]
pub(super) struct Column {
    declared: Arc<Declared>,
    slots: Vec<Slot>,
    /// Its rows, which a type without fields has none of in `slots`.
    len: usize,
}

impl Column {
    pub(super) fn new(declared: &Arc<Declared>) -> Self {
        Self {
            declared: Arc::clone(declared),
            slots: Vec::new(),
            len: 0,
        }
    }

    /// `row`'s fields.
    pub(super) fn row(&self, row: usize) -> &[Slot] {
        let width = self.declared.fields.len();
        &self.slots[row * width..(row + 1) * width]
    }

    pub(super) fn row_mut(&mut self, row: usize) -> &mut [Slot] {
        let width = self.declared.fields.len();
        &mut self.slots[row * width..(row + 1) * width]
    }

    /// The fields `value` gives, a struct's, each once, in the order of the type's, made as each
    /// kind makes its own.
    fn slots(&self, value: &Value) -> Result<Vec<Slot>> {
        let Value::Struct(given) = value else {
            return Err(reflect::wrong_kind("Struct", value));
        };
        let fields = &self.declared.fields;
        let names: Vec<&str> = fields.iter().map(|(name, _)| &**name).collect();
        let mut found = vec![None; fields.len()];
        reflect::pick_into(given, &names, &mut found)?;
        let made = fields.iter().zip(found).map(|((name, kind), value)| {
            let value = value.expect("every field is found");
            kind.slot(value)
                .map_err(|error| reflect::within(error, name))
        });
        made.collect()
    }

    fn of(column: &mut dyn AnyColumn) -> &mut Self {
        let any: &mut dyn Any = column;
        any.downcast_mut()
            .expect("a declared type's columns are its own")
    }
}

impl AnyColumn for Column {
    fn swap_remove(&mut self, row: usize) {
        let width = self.declared.fields.len();
        let last = self.len - 1;
        if row != last {
            self.slots
                .copy_within(last * width..(last + 1) * width, row * width);
        }
        self.slots.truncate(last * width);
        self.len = last;
    }

    fn move_row(&mut self, row: usize, to: &mut dyn AnyColumn) {
        let to = Self::of(to);
        to.slots.extend_from_slice(self.row(row));
        to.len += 1;
        self.swap_remove(row);
    }

    fn value(&self, row: usize) -> Value {
        let fields = self.declared.fields.iter().zip(self.row(row));
        let fields = fields.map(|((name, _), slot)| (name.clone(), slot.reflect().to_value()));
        Value::Struct(fields.collect())
    }

    fn part(&self, row: usize, index: u32) -> Option<&dyn Reflect> {
        self.row(row).get(index as usize).map(Slot::reflect)
    }

    fn part_mut(&mut self, row: usize, index: u32) -> Option<&mut dyn Reflect> {
        self.row_mut(row)
            .get_mut(index as usize)
            .map(Slot::reflect_mut)
    }

    fn set_value(&mut self, row: usize, value: &Value) -> Result<()> {
        let slots = self.slots(value)?;
        self.row_mut(row).copy_from_slice(&slots);
        Ok(())
    }

    fn entities_mut(&mut self, row: usize, each: &mut dyn FnMut(&mut Entity)) {
        for slot in self.row_mut(row) {
            slot.reflect_mut().entities_mut(each);
        }
    }

    fn push_value(&mut self, value: &Value) -> Result<()> {
        let slots = self.slots(value)?;
        self.slots.extend_from_slice(&slots);
        self.len += 1;
        Ok(())
    }

    fn opaque(&self, _row: usize) -> Option<String> {
        None
    }

    fn push_opaque(&mut self, _text: &str) -> Result<()> {
        Err(Error::new(
            "reflect.opaque",
            format!(
                "`{}` is a type a script declared, which holds nothing opaque",
                self.declared.name
            ),
        ))
    }

    fn cloned(&self) -> Box<dyn AnyColumn> {
        Box::new(self.clone())
    }

    fn clone_row(&self, row: usize) -> Box<dyn AnyColumn> {
        Box::new(Self {
            declared: Arc::clone(&self.declared),
            slots: self.row(row).to_vec(),
            len: 1,
        })
    }

    fn swap_row(&mut self, row: usize, with: &mut dyn AnyColumn) {
        self.row_mut(row).swap_with_slice(Self::of(with).row_mut(0));
    }

    fn copy_to(&self, to: &mut dyn AnyColumn) {
        let to = Self::of(to);
        to.declared.clone_from(&self.declared);
        to.slots.clone_from(&self.slots);
        to.len = self.len;
    }

    fn digest(&self, rows: &[Entity], key: u64, world: &World) -> u64 {
        // As a Rust struct's derived digest: each field's, in their order.
        (0..self.len).zip(rows).fold(0, |sum, (row, &entity)| {
            let term = hash::term(entity, key, world, |to| {
                for slot in self.row(row) {
                    slot.reflect().digest(to);
                }
            });
            sum.wrapping_add(term)
        })
    }
}

/// Whether `name` is an identifier: a letter or `_`, then letters, digits or `_`, as Luau's and
/// Rust's are, so that a path and a script both name it.
pub(crate) fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(name: &str, x: f64) -> (Ident, Kind, Value) {
        (Ident::Owned(name.to_owned()), Kind::Number, Value::F64(x))
    }

    #[test]
    fn a_type_refused_leaves_its_name_free() {
        let mut world = World::default();
        let mut refused = |name: &str, fields: Vec<(Ident, Kind, Value)>| {
            let error = world.declare(name, "", fields).expect_err("refused");
            error.code().to_owned()
        };
        assert_eq!(refused("Parent", vec![]), "reflect.name_taken");
        assert_eq!(refused("Not a name", vec![]), "reflect.bad_name");
        assert_eq!(
            refused("Spin", vec![number("1st", 0.0)]),
            "reflect.bad_name"
        );
        let twice = vec![number("speed", 0.0), number("speed", 1.0)];
        assert_eq!(refused("Spin", twice), "reflect.bad_name");
        assert_eq!(
            refused("Spin", vec![number("speed", f64::NAN)]),
            "reflect.bad_default"
        );
        let wrong = vec![("on".into(), Kind::Bool, Value::F64(1.0))];
        assert_eq!(refused("Spin", wrong), "reflect.bad_default");
        let spin = world.declare("Spin", "", vec![number("speed", 1.5)]);
        spin.expect("declared, its name free");
        let again = world.declare("Spin", "", Vec::new()).expect_err("refused");
        assert_eq!(again.code(), "reflect.name_taken");
    }

    #[test]
    fn a_declared_type_s_rows_keep_their_values_as_entities_come_and_go() {
        let mut world = World::default();
        let fields = vec![
            number("speed", 1.5),
            ("on".into(), Kind::Bool, Value::Bool(true)),
            ("toward".into(), Kind::Vector, Value::Vec3(Vec3::X)),
            ("other".into(), Kind::Entity, Value::Option(None)),
        ];
        world.declare("Spin", "", fields).expect("declared");
        let spun: Vec<Entity> = (0..4).map(|_| world.spawn(())).collect();
        for (i, &entity) in spun.iter().enumerate() {
            world.add_default(entity, "Spin").expect("added");
            let speed = Value::F64(i as f64);
            world
                .set_field(entity, "Spin", "speed", &speed)
                .expect("set");
        }
        let other = Value::Option(Some(Box::new(Value::Entity(spun[1]))));
        world
            .set_field(spun[3], "Spin", "other", &other)
            .expect("set");
        world.despawn(spun[0]).expect("alive");
        world.despawn(spun[1]).expect("alive");
        let speed = |world: &World, entity| world.get_field(entity, "Spin", "speed").expect("one");
        assert_eq!(speed(&world, spun[2]), Value::F64(2.0));
        assert_eq!(speed(&world, spun[3]), Value::F64(3.0));
        let whole = world.get_field(spun[3], "Spin", "").expect("a spin");
        let expected = Value::Struct(vec![
            ("speed".into(), Value::F64(3.0)),
            ("on".into(), Value::Bool(true)),
            ("toward".into(), Value::Vec3(Vec3::X)),
            ("other".into(), other),
        ]);
        assert_eq!(whole, expected);
        let again = world.spawn(());
        world.add_default(again, "Spin").expect("added");
        assert_eq!(speed(&world, again), Value::F64(1.5));
        let refused = world.add_default(again, "Spin").expect_err("refused");
        assert_eq!(refused.code(), "reflect.has_component");
    }
}
