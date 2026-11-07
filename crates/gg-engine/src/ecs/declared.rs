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

/// A declared type: its name, and its fields' names and kinds, in their order, with their
/// defaults.
#[derive(Debug, PartialEq)]
pub struct Declared {
    pub name: Ident,
    pub fields: Box<[(Ident, Kind)]>,
    pub(super) defaults: Box<[Slot]>,
}

impl Declared {
    /// Whether its fields are `fields`, each a name, a kind and a default, in their order.
    pub(crate) fn declares(&self, fields: &[(Ident, Kind, Value)]) -> bool {
        let ours = self.fields.iter().zip(&self.defaults);
        self.fields.len() == fields.len()
            && ours
                .zip(fields)
                .all(|(((name, kind), default), (given, of, value))| {
                    name == given && kind == of && of.slot(value).ok().as_ref() == Some(default)
                })
    }

    /// Pushes `row`, of `from`'s fields, onto `to` as a row of these: each field of a name and a
    /// kind `from` has too as `row` holds it, and each other at its default.
    fn fit(&self, from: &Self, row: &[Slot], to: &mut Vec<Slot>) {
        for ((name, kind), default) in self.fields.iter().zip(&self.defaults) {
            let kept = (from.fields.iter()).position(|(field, of)| field == name && of == kind);
            to.push(kept.map_or(*default, |at| row[at]));
        }
    }
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
        if same(&self.declared, &to.declared) {
            to.slots.extend_from_slice(self.row(row));
        } else {
            // Kept from before the type's fields changed, as a delete's undo keeps it.
            to.declared
                .fit(&self.declared, self.row(row), &mut to.slots);
        }
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
        let with = Self::of(with);
        if same(&self.declared, &with.declared) {
            self.row_mut(row).swap_with_slice(with.row_mut(0));
            return;
        }
        // Kept from before the type's fields changed: it comes back as they are now, and keeps
        // the row it takes as they are too.
        let mut fitted = Vec::with_capacity(self.declared.fields.len());
        self.declared.fit(&with.declared, with.row(0), &mut fitted);
        with.slots = self.row(row).to_vec();
        with.declared = Arc::clone(&self.declared);
        self.row_mut(row).copy_from_slice(&fitted);
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

/// Whether `a` and `b` are the same fields, as they are but where a type's fields changed between
/// the rows of two columns.
fn same(a: &Arc<Declared>, b: &Arc<Declared>) -> bool {
    Arc::ptr_eq(a, b) || a == b
}

/// Has `column`, a declared type's, hold its rows as rows of `to`, the type's fields now, each
/// field it holds of a name and a kind `to` has too as it holds it, and each other at its default.
pub(super) fn migrate(column: &mut dyn AnyColumn, to: &Arc<Declared>) {
    let column = Column::of(column);
    let mut slots = Vec::with_capacity(column.len * to.fields.len());
    for row in 0..column.len {
        to.fit(&column.declared, column.row(row), &mut slots);
    }
    column.slots = slots;
    column.declared = Arc::clone(to);
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
        // Nor one of RON's words, a type's or a field's, which no scene could save it by.
        assert_eq!(refused("None", vec![]), "reflect.bad_name");
        assert_eq!(
            refused("Spin", vec![number("inf", 0.0)]),
            "reflect.bad_name"
        );
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

    /// A type declared again holds each of its rows as its fields are now, in every table, those of
    /// a name and a kind it had keeping their values and the rest taking their defaults; and so
    /// does a component an undo puts back, which was taken before.
    #[test]
    fn a_type_declared_again_has_its_rows_and_those_put_back_hold_its_fields_as_they_are() {
        use crate::{Log, Operation, Transform};

        let mut world = World::default();
        let fields = vec![
            number("speed", 1.5),
            ("on".into(), Kind::Bool, Value::Bool(true)),
            number("gone", 2.0),
        ];
        let id = world.declare("Spin", "", fields).expect("declared");
        let alone = world.spawn(());
        let moved = world.spawn(Transform::IDENTITY);
        let removed = world.spawn(());
        for (i, entity) in [alone, moved, removed].into_iter().enumerate() {
            world.add_default(entity, "Spin").expect("added");
            let speed = Value::F64(i as f64);
            world
                .set_field(entity, "Spin", "speed", &speed)
                .expect("set");
        }
        let mut log = Log::default();
        let remove = Operation::Remove {
            entity: world.id(removed).expect("alive"),
            component: "Spin".into(),
        };
        log.apply_all(&mut world, vec![remove]).expect("removed");
        let fields = vec![
            ("toward".into(), Kind::Vector, Value::Vec3(Vec3::Y)),
            number("on", 7.0),
            number("speed", 9.0),
        ];
        let revision = world.components.revision();
        world.redeclare(id, "", fields).expect("declared again");
        let revised = world.components.revision();
        assert_ne!(revised, revision, "the types' revision moves on");
        let spin = |world: &World, entity, speed| {
            let value = world.get_field(entity, "Spin", "").expect("a spin");
            let expected = Value::Struct(vec![
                ("toward".into(), Value::Vec3(Vec3::Y)),
                ("on".into(), Value::F64(7.0)),
                ("speed".into(), Value::F64(speed)),
            ]);
            assert_eq!(value, expected);
        };
        spin(&world, alone, 0.0);
        spin(&world, moved, 1.0);
        log.undo(&mut world).expect("undone");
        spin(&world, removed, 2.0);
        let again = world.spawn(());
        world.add_default(again, "Spin").expect("added");
        spin(&world, again, 9.0);
        let bad = world.redeclare(id, "", vec![number("speed", f64::NAN)]);
        assert_eq!(bad.expect_err("refused").code(), "reflect.bad_default");
        spin(&world, alone, 0.0);
        assert_eq!(
            world.components.revision(),
            revised,
            "a refusal changes nothing"
        );
    }
}
