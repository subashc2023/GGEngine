//! Reflection: what the engine knows of its types' fields while it runs, so that the inspector,
//! and saving, operations and the tool API to come, reach components by name, without their Rust
//! types. A type describes itself once to [`Types`], by name; its values go in and out as
//! [`Value`]s, checked against that description. Types derive [`Reflect`], with
//! `#[derive(Reflect)]`, or `#[derive(Clone, Component)]` for components.

mod impls;

use std::any::{TypeId, type_name};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use gg_core::math::{Quat, Vec2, Vec3, Vec4};

/// The error and result types reflection returns, `gg-core`'s.
pub use gg_core::{Error, Result};
/// Describes a struct or an enum to reflection: [`Reflect`] says how.
pub use gg_macros::Reflect;

use crate::Entity;
pub use crate::ecs::Digest;

/// A type's, a field's or a variant's name: borrowed for Rust's types, and owned for those that
/// scripts and files will declare.
pub type Name = Cow<'static, str>;

/// A value of a reflected type, without its Rust type: what reflection reads from a field, and sets
/// one to. The field's [`Type`] says which of these its values are.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    /// A signed integer's.
    Int(i64),
    /// An unsigned integer's. Either sets an integer field of either sign that it fits.
    UInt(u64),
    /// Kept apart from `F64`, so that an `f32`'s bits come back as they went.
    F32(f32),
    F64(f64),
    String(String),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    /// A unit quaternion.
    Quat(Quat),
    /// The entity's handle in this process: files, the tool API and replication will map it to
    /// persistent IDs where values cross into them.
    Entity(Entity),
    Option(Option<Box<Value>>),
    /// A list's elements, or an array's.
    List(Vec<Value>),
    /// A struct's fields by name, each once: a tuple struct's are named `0`, `1` and so on.
    Struct(Vec<(Name, Value)>),
    /// An enum's variant by name, with its fields, as a struct's.
    Variant(Name, Vec<(Name, Value)>),
    /// What reflection doesn't read, such as a texture's handle: an opaque field reads as this, and
    /// set to it keeps what it has, so that what's opaque doesn't keep a value read whole from
    /// setting back whole. Anywhere else it's `reflect.wrong_kind`.
    Opaque,
}

/// What a field holds, as reflection describes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Bool,
    /// An integer of `bits`, 8 to 64; there are none whose size differs by platform, so that a
    /// value means the same everywhere it's saved or sent.
    Int {
        bits: u8,
        signed: bool,
    },
    F32,
    F64,
    String,
    Vec2,
    Vec3,
    Vec4,
    Quat,
    Entity,
    Option(Box<Type>),
    List(Box<Type>),
    /// This many elements, always.
    Array(Box<Type>, u32),
    /// A struct or an enum, which [`Types`] describes.
    Named(TypeIndex),
    /// A field marked `#[reflect(opaque)]`, which reflection keeps but doesn't read.
    Opaque,
}

impl Type {
    /// Whether reflection leaves values of the type as they are: an opaque field's, or an opaque
    /// type's. Reading one gives [`Value::Opaque`], and only that sets one, keeping it as it is.
    pub fn is_opaque(&self, types: &Types) -> bool {
        match self {
            Self::Opaque => true,
            Self::Named(index) => matches!(types.info(*index).shape, Shape::Opaque),
            _ => false,
        }
    }
}

/// A named type's place among [`Types`]' own, in the order they were described.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeIndex(u32);

/// A struct or an enum, as reflection describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeInfo {
    /// Its identifier, unless `#[reflect(name = "...")]` gives another: no other type has it.
    pub name: Name,
    /// Its doc comment, its lines joined into paragraphs.
    pub doc: Name,
    pub shape: Shape,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// Its fields, in the order it declares them; a tuple struct's are named `0`, `1` and so on.
    Struct { fields: Vec<Field>, tuple: bool },
    /// Its variants, in the order it declares them.
    Enum(Vec<Variant>),
    /// Marked `#[reflect(opaque)]`: kept and listed, but not read or set.
    Opaque,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: Name,
    pub ty: Type,
    pub meta: Meta,
    /// What it is unless something says otherwise, as in a component added, if its description
    /// gives it, as `#[reflect(default = ..)]` does: otherwise its type's, under its bounds, as
    /// [`Types::field_default`] makes it. The field's, not its [`Meta`]'s, which reaches its parts
    /// and elements: a vector's default isn't its components'.
    pub default: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: Name,
    pub fields: Vec<Field>,
    pub tuple: bool,
}

/// What a field's hints say of it, beyond its type.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    /// The least and the most its numbers may be, and the components of its vectors, and its
    /// elements': a value set outside them is refused.
    pub min: Option<Bound>,
    pub max: Option<Bound>,
    pub show: Show,
    /// Its doc comment, its lines joined into paragraphs.
    pub doc: Name,
}

/// A field's `min` or `max`, as it's written: integers apart from floats, so that they compare
/// with integers exactly. An `f32`, and a vector's component, compares with it rounded to an
/// `f32`, so that `max = 0.1` takes `0.1f32`, which is a little more than 0.1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bound {
    Int(i128),
    Float(f64),
}

impl Bound {
    /// It as an `f32` compares with it: the nearest `f32`.
    pub(crate) fn as_f32(self) -> f32 {
        match self {
            Self::Int(n) => n as f32,
            Self::Float(x) => x as f32,
        }
    }

    /// It as an `f64` compares with it.
    pub(crate) fn as_f64(self) -> f64 {
        match self {
            Self::Int(n) => n as f64,
            Self::Float(x) => x,
        }
    }
}

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::Float(x) => write!(f, "{x}"),
        }
    }
}

/// How the inspector shows a field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Show {
    #[default]
    Plain,
    /// An `f32` of radians, or of radians a second, in degrees.
    Degrees,
    /// A `Vec3` or `Vec4` of linear color, alpha not multiplied in.
    Color,
}

/// The named types, structs and enums, in the order they were described. Each describes itself
/// once, under a name no other has; a type of fields is described with the first type that has
/// one.
#[derive(Clone, Default)]
pub struct Types {
    entries: Vec<Entry>,
    /// Looked up, never iterated, as is `by_type`.
    by_name: HashMap<Name, TypeIndex>,
    by_type: HashMap<TypeId, TypeIndex>,
}

#[derive(Clone)]
struct Entry {
    info: TypeInfo,
    type_id: TypeId,
    /// Its Rust type's name, for errors.
    rust: &'static str,
}

impl Types {
    /// Describes `T`, a struct or an enum, under `name`, and returns it as a [`Type::Named`]; its
    /// [`Reflect::describe`] calls this. `describe` describes its fields or variants, with the
    /// types they hold. A type already described isn't again, so that a recursive type, such as
    /// `struct Node { kids: Vec<Node> }`, describes itself once.
    ///
    /// Fails with `reflect.name_taken` if another type has `name`; with `reflect.bad_default` if a
    /// field's [`default`](Field::default) is outside its bounds, or opaque; and as `describe`
    /// does: either way it leaves the types as they were, without `T` or any type described since.
    pub fn add<T: ?Sized + 'static>(
        &mut self,
        name: Name,
        doc: Name,
        describe: impl FnOnce(&mut Self) -> Result<Shape>,
    ) -> Result<Type> {
        let type_id = TypeId::of::<T>();
        if let Some(&index) = self.by_type.get(&type_id) {
            return Ok(Type::Named(index));
        }
        if let Some(&taken) = self.by_name.get(&name) {
            let other = self.entries[taken.0 as usize].rust;
            return Err(Error::new(
                "reflect.name_taken",
                format!(
                    "`{}` and `{other}` are both named `{name}`, but each type's name is its own; \
                     give one another with `#[reflect(name = \"...\")]`",
                    type_name::<T>()
                ),
            ));
        }
        // Reserved first, so that the fields' types find it, and so that undoing it undoes
        // everything after it.
        let index = TypeIndex(u32::try_from(self.entries.len()).expect("fewer than 2^32 types"));
        self.entries.push(Entry {
            info: TypeInfo {
                name: name.clone(),
                doc,
                shape: Shape::Opaque,
            },
            type_id,
            rust: type_name::<T>(),
        });
        self.by_name.insert(name, index);
        self.by_type.insert(type_id, index);
        // Every type described since too, now that each has its shape: a default that holds this
        // type, in a type it holds, was checked against its placeholder.
        let described = describe(self).and_then(|shape| {
            self.entries[index.0 as usize].info.shape = shape;
            let len = self.entries.len() as u32;
            (index.0..len).try_for_each(|i| self.check_defaults(TypeIndex(i)))
        });
        if let Err(error) = described {
            for entry in self.entries.drain(index.0 as usize..) {
                self.by_name.remove(&entry.info.name);
                self.by_type.remove(&entry.type_id);
            }
            return Err(error);
        }
        Ok(Type::Named(index))
    }

    /// Checks the defaults that `index`'s fields, and its variants', give against their bounds,
    /// as [`check`] checks a value set: what's not given is made within them. One that's opaque
    /// as a whole is refused, since it would give nothing: what's opaque is kept, not made.
    fn check_defaults(&self, index: TypeIndex) -> Result<()> {
        let info = self.info(index);
        // The fields of the struct, or of its variant `variant`.
        let checked = |fields: &[Field], variant: Option<&Name>| -> Result<()> {
            let owner = || match variant {
                Some(variant) => format!("{}::{variant}", info.name),
                None => info.name.to_string(),
            };
            for field in fields {
                let Some(default) = &field.default else {
                    continue;
                };
                if matches!(default, Value::Opaque) {
                    let message = format!(
                        "`{}`'s field `{}` is opaque, which reflection keeps but doesn't make, so \
                         it takes no default",
                        owner(),
                        field.name
                    );
                    return Err(Error::new("reflect.bad_default", message));
                }
                check(self, &field.ty, &field.meta, default).map_err(|error| {
                    let (path, what) = located(&error);
                    let message = format!(
                        "`{}`'s field `{}` defaults to a value it refuses: {what}",
                        owner(),
                        join(&field.name, &path)
                    );
                    Error::new("reflect.bad_default", message)
                })?;
            }
            Ok(())
        };
        match &info.shape {
            Shape::Struct { fields, .. } => checked(fields, None),
            Shape::Enum(variants) => variants
                .iter()
                .try_for_each(|variant| checked(&variant.fields, Some(&variant.name))),
            Shape::Opaque => Ok(()),
        }
    }

    /// The default of a value of `ty`, in a field of hints `meta`, whose bounds alone it reads, for
    /// its numbers, its vectors' components and its elements: [`Meta::default`] gives none. A
    /// number is 0, or the bound nearest 0 if 0 is outside them, rounded to an integer of its
    /// type inside them, or for an `f32`, to the `f32` it's compared with; a bool is false; a
    /// quaternion the identity; a string empty; an option `None`; a list empty; an array that many
    /// of its element's default; a struct each field's [`field_default`](Self::field_default); an
    /// enum its first variant, as [`variant_default`](Self::variant_default) makes it; and
    /// what's opaque [`Value::Opaque`], which keeps what a component's
    /// [`base`](crate::Component::base) gives it.
    ///
    /// Fails with `reflect.no_default` for an entity, of which there's none to name; for a number
    /// whose bounds leave none of its type between them; for an enum without variants; and for a
    /// type whose default holds one of its own without end, which only a description written by
    /// hand can give, since a derived type holds itself only in an option or a list. The path to
    /// where it failed leads the message.
    pub fn default_value(&self, ty: &Type, meta: &Meta) -> Result<Value> {
        self.default_inside(ty, meta, &mut Vec::new())
    }

    /// The default of `field`: the one its description gives, or else its type's under its
    /// bounds, as [`default_value`](Self::default_value) makes it, and fails.
    pub fn field_default(&self, field: &Field) -> Result<Value> {
        self.field_inside(field, &mut Vec::new())
    }

    /// `variant`, with each field's [`field_default`](Self::field_default), as switching an enum
    /// to it makes it; an enum's default is its first variant's.
    ///
    /// Fails as `field_default` does, at the field that failed.
    pub fn variant_default(&self, variant: &Variant) -> Result<Value> {
        self.variant_inside(variant, &mut Vec::new())
    }

    /// [`default_value`](Self::default_value), inside the defaults of `named`, from the outside
    /// in: a cycle is found as a type it holds already, rather than by how deep it goes.
    fn default_inside(&self, ty: &Type, meta: &Meta, named: &mut Vec<TypeIndex>) -> Result<Value> {
        let none = || no_number(self, ty, meta);
        // Finite, as `check` takes only finite numbers: a bound beyond an `f32`'s range rounds to
        // an infinite one.
        let component = || {
            let x = nearest_zero(meta, Bound::as_f32, Bound::as_f32);
            x.filter(|x| x.is_finite()).ok_or_else(none)
        };
        let value = match ty {
            Type::Bool => Value::Bool(false),
            &Type::Int { bits, signed } => {
                let (least, most) = if signed {
                    (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
                } else {
                    (0, (1i128 << bits) - 1)
                };
                let n = nearest_zero(meta, ceiled, floored).filter(|n| (least..=most).contains(n));
                let n = n.ok_or_else(none)?;
                if signed {
                    Value::Int(n as i64)
                } else {
                    Value::UInt(n as u64)
                }
            }
            Type::F32 => Value::F32(component()?),
            Type::F64 => {
                let x = nearest_zero(meta, Bound::as_f64, Bound::as_f64);
                Value::F64(x.filter(|x| x.is_finite()).ok_or_else(none)?)
            }
            Type::String => Value::String(String::new()),
            Type::Vec2 => Value::Vec2(Vec2::splat(component()?)),
            Type::Vec3 => Value::Vec3(Vec3::splat(component()?)),
            Type::Vec4 => Value::Vec4(Vec4::splat(component()?)),
            Type::Quat => Value::Quat(Quat::IDENTITY),
            Type::Entity => {
                return Err(Error::new(
                    "reflect.no_default",
                    "an entity has no default, since a handle to none can't be saved or set; a \
                     field that may hold none is an `Option<Entity>`, whose default is `None`",
                ));
            }
            Type::Option(_) => Value::Option(None),
            Type::List(_) => Value::List(Vec::new()),
            Type::Array(_, 0) => Value::List(Vec::new()),
            Type::Array(inner, len) => {
                let item = self.default_inside(inner, meta, named);
                Value::List(vec![
                    item.map_err(|error| within(error, "[0]"))?;
                    *len as usize
                ])
            }
            Type::Named(index) => {
                let info = self.info(*index);
                if named.contains(index) {
                    return Err(Error::new(
                        "reflect.no_default",
                        format!("`{0}`'s default holds a `{0}`, without end", info.name),
                    ));
                }
                named.push(*index);
                let value = match &info.shape {
                    Shape::Struct { fields, .. } => {
                        self.fields_inside(fields, named).map(Value::Struct)
                    }
                    Shape::Enum(variants) => match variants.first() {
                        Some(first) => self.variant_inside(first, named),
                        None => Err(Error::new(
                            "reflect.no_default",
                            format!("`{}` has no variants, so it has no default", info.name),
                        )),
                    },
                    Shape::Opaque => Ok(Value::Opaque),
                };
                named.pop();
                value?
            }
            Type::Opaque => Value::Opaque,
        };
        Ok(value)
    }

    fn field_inside(&self, field: &Field, named: &mut Vec<TypeIndex>) -> Result<Value> {
        match &field.default {
            Some(default) => Ok(default.clone()),
            None => self.default_inside(&field.ty, &field.meta, named),
        }
    }

    /// Each of `fields`' defaults, by name.
    fn fields_inside(
        &self,
        fields: &[Field],
        named: &mut Vec<TypeIndex>,
    ) -> Result<Vec<(Name, Value)>> {
        let defaults = fields.iter().map(|field| {
            let default = self.field_inside(field, named);
            Ok((
                field.name.clone(),
                default.map_err(|error| within(error, &field.name))?,
            ))
        });
        defaults.collect()
    }

    fn variant_inside(&self, variant: &Variant, named: &mut Vec<TypeIndex>) -> Result<Value> {
        let fields = self.fields_inside(&variant.fields, named)?;
        Ok(Value::Variant(variant.name.clone(), fields))
    }

    /// The type named `name`, if there's one.
    pub fn find(&self, name: &str) -> Option<TypeIndex> {
        self.by_name.get(name).copied()
    }

    /// # Panics
    ///
    /// If `index` is another `Types`' own.
    pub fn info(&self, index: TypeIndex) -> &TypeInfo {
        &self.entries[index.0 as usize].info
    }

    /// The Rust type that `index` describes, and its name, for errors.
    pub(crate) fn rust_type(&self, index: TypeIndex) -> (TypeId, &'static str) {
        let entry = &self.entries[index.0 as usize];
        (entry.type_id, entry.rust)
    }

    /// Every type, in the order they were described.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (TypeIndex, &TypeInfo)> {
        let entries = self.entries.iter().enumerate();
        entries.map(|(i, entry)| (TypeIndex(i as u32), &entry.info))
    }

    /// Whether `other` has described the same Rust types as these, in the same order, so that
    /// its indices name the same types.
    pub(crate) fn same(&self, other: &Self) -> bool {
        let ours = self.entries.iter().map(|entry| entry.type_id);
        ours.eq(other.entries.iter().map(|entry| entry.type_id))
    }
}

/// A type whose values reflection reads and sets, without its Rust type: one that describes
/// itself to [`Types`]. Structs and enums derive it, with `#[derive(Reflect)]`, or with
/// `#[derive(Clone, Component)]` for components, which reads each field's type's own, and writes
/// what each field's doc comment says as its doc. Hints in `#[reflect(...)]` say more:
///
/// - on a type, `name = "..."` names it in place of its identifier, as two types of one name
///   need; and `opaque`, on a `Clone` type, has reflection keep its values but not read them, as
///   for a handle that a value must never make;
/// - on a field, `min = ..` and `max = ..` bound its numbers, and its vectors' components, in its
///   own unit, such as radians: a value set outside them is refused; `default = ..` gives its
///   [`default`](Field::default), an expression of its type in its own unit, run as the type
///   registers, which its bounds must take, though a field of a type opaque as a whole takes none
///   (`reflect.bad_default`), and without it its type's is its default, under its bounds, as
///   [`Types::default_value`] makes it; `degrees` shows an `f32` of radians, or of radians a
///   second, in degrees; `color` shows a `Vec3` or `Vec4` as a linear color, alpha not multiplied
///   in; and `opaque` keeps a field of a `Clone` type that isn't reflected, which reads as
///   [`Value::Opaque`], and takes no other hint.
///
/// Each hint is given once, and a variant takes none, nor does a field of an opaque type.
///
/// ```
/// use gg_engine::Component;
/// use gg_engine::math::Vec3;
///
/// /// How a light shines.
/// #[derive(Clone, Component)]
/// struct Light {
///     #[reflect(color, default = Vec3::ONE)]
///     color: Vec3,
///     #[reflect(min = 0.0, default = 1.0)]
///     intensity: f32,
///     /// Of its cone, from its axis.
///     #[reflect(degrees, min = 0.0, max = 3.1416)]
///     angle: f32,
/// }
/// ```
///
/// Generic types and unions don't derive it:
///
/// ```compile_fail
/// #[derive(Clone, gg_engine::Component)]
/// struct Pair<T>(T, T);
/// ```
///
/// ```compile_fail
/// #[derive(Clone, Copy, gg_engine::Component)]
/// union Bits {
///     float: f32,
///     int: u32,
/// }
/// ```
///
/// Nor does a hint it doesn't know, one that doesn't suit its field, one given twice or beside
/// `opaque`, or one on a variant or inside an opaque type:
///
/// ```compile_fail
/// #[derive(Clone, gg_engine::Component)]
/// struct Speed(#[reflect(fast)] f32);
/// ```
///
/// ```compile_fail,E0277
/// #[derive(Clone, gg_engine::Component)]
/// struct Turned(#[reflect(degrees)] gg_engine::math::Vec3);
/// ```
///
/// ```compile_fail,E0308
/// #[derive(Clone, gg_engine::Component)]
/// struct Speed(#[reflect(default = 1)] f32);
/// ```
///
/// ```compile_fail
/// #[derive(Clone, gg_engine::Component)]
/// struct Bounded(#[reflect(min = 0, min = 1)] f32);
/// ```
///
/// ```compile_fail
/// #[derive(Clone, gg_engine::Component)]
/// struct Held(#[reflect(opaque, default = 0)] u32);
/// ```
///
/// ```compile_fail
/// #[derive(Clone, gg_engine::Component)]
/// enum Shape {
///     #[reflect(opaque)]
///     Held(u32),
/// }
/// ```
///
/// ```compile_fail
/// #[derive(Clone, gg_engine::Component)]
/// #[reflect(opaque)]
/// struct Sealed(#[reflect(min = 0)] u32);
/// ```
///
/// Implemented by hand, `describe` calls [`Types::add`] for its own type, a struct or an enum,
/// never another's, and the rest
/// keep to its description: `with_value` checks the value's kinds, fields and variants, and makes
/// what it's given whole before anything changes, keeping what's opaque; and `digest` writes what
/// `to_value` reads.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't reflected",
    note = "a type of fields describes itself with `#[derive(gg_engine::Reflect)]`, and a \
            component with `#[derive(Clone, gg_engine::Component)]`; a field of a type that isn't \
            reflected can be kept with `#[reflect(opaque)]`"
)]
pub trait Reflect: std::any::Any + Send + Sync {
    /// The type's description, which it adds to `types` if it's a struct or an enum.
    fn describe(types: &mut Types) -> Result<Type>
    where
        Self: Sized;

    fn to_value(&self) -> Value;

    /// One made from `value`. Fails with `reflect.opaque` where it would have to make something
    /// opaque, and as [`with_value`](Self::with_value) does.
    fn from_value(value: &Value) -> Result<Self>
    where
        Self: Sized;

    /// One like this one, with `value`'s parts, keeping its own opaque ones where they are, a
    /// list's by index: set to a list with an element taken out, the elements after it keep what's
    /// opaque of those that were in their places. A variant of an enum switched to, an option's
    /// value that was `None`, and a list's elements past its end are made with
    /// [`from_value`](Self::from_value). Fails, at the part that failed, with
    /// `reflect.wrong_kind` for a value of another kind; `reflect.out_of_range` for an integer that
    /// doesn't fit; `reflect.not_normalized` for a quaternion that isn't a unit;
    /// `reflect.missing_field`, `reflect.no_field` and `reflect.duplicate_field` for a struct's
    /// fields that aren't each given once; `reflect.no_variant` for a variant the enum hasn't;
    /// `reflect.wrong_length` for an array's elements; and `reflect.opaque` for something
    /// opaque set to anything but [`Value::Opaque`].
    fn with_value(&self, value: &Value) -> Result<Self>
    where
        Self: Sized;

    /// Sets this to `value` as [`with_value`](Self::with_value) makes it, all or nothing: a value
    /// refused changes nothing. The bounds of fields are
    /// [`World::set_field`](crate::World::set_field)'s to check.
    fn set_value(&mut self, value: &Value) -> Result<()>;

    /// Its part `index`: a struct's field, a vector's component, or a list's or an array's
    /// element; `None` past its parts, for what's opaque, and for enums, options and quaternions,
    /// which are set whole.
    fn part(&self, index: u32) -> Option<&dyn Reflect>;

    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect>;

    /// Writes it into `to`, for the world's hash: what [`to_value`](Self::to_value) reads, in the
    /// order it reads it, but for what's opaque, which it leaves out. A float goes by its bits,
    /// every NaN as one; an entity by its ID, or 0 if it isn't alive; a variant by its index,
    /// then its fields; and an option, a list and a string with what tells where they end, so that
    /// values moved from one field to the next give another hash. Implemented by hand, it writes
    /// its parts' own, as `to_value` reads them, and a variant's index as a `u32` does:
    /// `<u32 as Reflect>::digest(&index, to)`.
    fn digest(&self, to: &mut Digest<'_>);
}

/// Types whose fields `#[reflect(min = ..)]` and `#[reflect(max = ..)]` bound: numbers, vectors,
/// and options, lists and arrays of them, whose elements the bounds apply to.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`#[reflect(min)]` and `#[reflect(max)]` bound numbers and vectors, not `{Self}`"
)]
pub trait Ranged {}

/// Types whose fields `#[reflect(degrees)]` shows in degrees: an `f32` of radians, and options,
/// lists and arrays of them.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`#[reflect(degrees)]` shows an `f32` of radians in degrees, not `{Self}`"
)]
pub trait Angle {}

/// Types whose fields `#[reflect(color)]` shows as a color: a `Vec3` or a `Vec4`, and options,
/// lists and arrays of them.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`#[reflect(color)]` shows a `Vec3` or `Vec4` as a color, not `{Self}`"
)]
pub trait Color {}

/// The fields of `value`, a struct's, in the order of `names`, each given once; for the derives.
#[doc(hidden)]
pub fn fields<'v, const N: usize>(value: &'v Value, names: [&str; N]) -> Result<[&'v Value; N]> {
    match value {
        Value::Struct(given) => pick(given, names),
        other => Err(wrong_kind("Struct", other)),
    }
}

/// The fields in `given` in the order of `names`, each given once; for the derives.
#[doc(hidden)]
pub fn pick<'v, const N: usize>(
    given: &'v [(Name, Value)],
    names: [&str; N],
) -> Result<[&'v Value; N]> {
    let mut found = [None; N];
    for (name, value) in given {
        let Some(i) = names.iter().position(|known| known == name) else {
            return Err(Error::new(
                "reflect.no_field",
                format!("there's no field `{name}`; the fields are {}", list(names)),
            ));
        };
        if found[i].replace(value).is_some() {
            return Err(Error::new(
                "reflect.duplicate_field",
                format!("field `{name}` is given twice, but each field is given once"),
            ));
        }
    }
    if let Some(i) = found.iter().position(Option::is_none) {
        return Err(Error::new(
            "reflect.missing_field",
            format!("field `{}` isn't given, but each field is", names[i]),
        ));
    }
    Ok(found.map(|value| value.expect("every field is found")))
}

/// The variant `value` is, by name, and its fields; for the derives.
#[doc(hidden)]
pub fn variant(value: &Value) -> Result<(&str, &[(Name, Value)])> {
    match value {
        Value::Variant(name, given) => Ok((name, given)),
        other => Err(wrong_kind("Variant", other)),
    }
}

/// The error that an enum of variants `names` has no variant `name`; for the derives.
#[doc(hidden)]
pub fn no_variant(name: &str, names: &[&str]) -> Error {
    Error::new(
        "reflect.no_variant",
        format!(
            "there's no variant `{name}`; the variants are {}",
            list(names)
        ),
    )
}

/// Fails: reflection doesn't make what's opaque. For the derives, which make an opaque field with
/// it, whatever its type.
#[doc(hidden)]
pub fn make_opaque<T>(value: &Value) -> Result<T> {
    let _ = value;
    Err(Error::new(
        "reflect.opaque",
        "it's opaque: reflection keeps one, but doesn't make one",
    ))
}

/// `current`, which is opaque, as `value` sets it: only [`Value::Opaque`] does, keeping it as it
/// is. For the derives.
#[doc(hidden)]
pub fn keep<T: Clone>(current: &T, value: &Value) -> Result<T> {
    match value {
        Value::Opaque => Ok(current.clone()),
        other => Err(not_opaque(other)),
    }
}

/// The error that something opaque is given `value`, which isn't [`Value::Opaque`].
pub(crate) fn not_opaque(value: &Value) -> Error {
    Error::new(
        "reflect.opaque",
        format!(
            "it's opaque, so only `Opaque` sets it, which keeps it as it is; got {}",
            kind(value)
        ),
    )
}

/// `error`, from part `step` of a value, such as `rotation` or `[1]`, as from the whole value;
/// for the derives. The path it happened at leads its message, as `` at `rect[1].y`: ``.
#[doc(hidden)]
pub fn within(error: Error, step: &str) -> Error {
    let (path, what) = located(&error);
    let message = format!("at `{}`: {what}", join(step, &path));
    error.with_message(message)
}

/// Where in a value `error` happened, from [`within`]'s prefix of its message, and the rest of it.
pub(crate) fn located(error: &Error) -> (String, String) {
    let message = error.to_string();
    if let Some(rest) = message.strip_prefix("at `")
        && let Some((path, what)) = rest.split_once("`: ")
    {
        return (path.to_owned(), what.to_owned());
    }
    (String::new(), message)
}

/// Path `inner`, from where path `outer` leads: such as `rect` and `[1].y`, or `translation` and
/// `x`.
pub(crate) fn join(outer: &str, inner: &str) -> String {
    if outer.is_empty() {
        inner.to_owned()
    } else if inner.is_empty() || inner.starts_with('[') {
        format!("{outer}{inner}")
    } else {
        format!("{outer}.{inner}")
    }
}

/// The error that `value` isn't of the kind `expected`, named as [`Value`]'s variants are.
pub(crate) fn wrong_kind(expected: &str, value: &Value) -> Error {
    Error::new(
        "reflect.wrong_kind",
        format!("expected {expected}, got {}", kind(value)),
    )
}

/// The name of `value`'s variant.
pub(crate) fn kind(value: &Value) -> &'static str {
    match value {
        Value::Bool(_) => "Bool",
        Value::Int(_) => "Int",
        Value::UInt(_) => "UInt",
        Value::F32(_) => "F32",
        Value::F64(_) => "F64",
        Value::String(_) => "String",
        Value::Vec2(_) => "Vec2",
        Value::Vec3(_) => "Vec3",
        Value::Vec4(_) => "Vec4",
        Value::Quat(_) => "Quat",
        Value::Entity(_) => "Entity",
        Value::Option(_) => "Option",
        Value::List(_) => "List",
        Value::Struct(_) => "Struct",
        Value::Variant(..) => "Variant",
        Value::Opaque => "Opaque",
    }
}

/// `names` as a list in a sentence: `` `a`, `b` and `c` ``, or `none`.
pub(crate) fn list<T: fmt::Display>(names: impl IntoIterator<Item = T>) -> String {
    let names: Vec<String> = names.into_iter().map(|name| format!("`{name}`")).collect();
    match names.split_last() {
        None => "none".to_owned(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// Checks `value`, for a field of type `ty` with hints `meta`, against what only the description
/// knows: the bounds of its numbers, and of its fields' and elements', which inherit their field's
/// bounds. A number that isn't finite is outside every range. The impls check the rest, its
/// kinds, as they set it.
pub(crate) fn check(types: &Types, ty: &Type, meta: &Meta, value: &Value) -> Result<()> {
    let bounded = meta.min.is_some() || meta.max.is_some();
    match (ty, value) {
        (Type::Named(index), _) => match (&types.info(*index).shape, value) {
            (Shape::Struct { fields, .. }, Value::Struct(given)) => {
                check_fields(types, fields, given)
            }
            (Shape::Enum(variants), Value::Variant(name, given)) => {
                match variants.iter().find(|variant| variant.name == *name) {
                    Some(variant) => check_fields(types, &variant.fields, given),
                    None => Ok(()),
                }
            }
            _ => Ok(()),
        },
        (Type::Option(inner), Value::Option(Some(value))) => check(types, inner, meta, value),
        (Type::List(inner) | Type::Array(inner, _), Value::List(items)) => {
            for (i, item) in items.iter().enumerate() {
                check(types, inner, meta, item)
                    .map_err(|error| within(error, &format!("[{i}]")))?;
            }
            Ok(())
        }
        _ if !bounded => Ok(()),
        (Type::Int { .. }, &Value::Int(n)) => in_range(meta, Number::Int(n.into())),
        (Type::Int { .. }, &Value::UInt(n)) => in_range(meta, Number::Int(n.into())),
        (Type::F32, &Value::F32(x)) => in_range(meta, Number::F32(x)),
        (Type::F64, &Value::F64(x)) => in_range(meta, Number::Float(x)),
        (Type::Vec2, Value::Vec2(v)) => components(meta, v.as_ref()),
        (Type::Vec3, Value::Vec3(v)) => components(meta, v.as_ref()),
        (Type::Vec4, Value::Vec4(v)) => components(meta, v.as_ref()),
        _ => Ok(()),
    }
}

fn check_fields(types: &Types, fields: &[Field], given: &[(Name, Value)]) -> Result<()> {
    for (name, value) in given {
        if let Some(field) = fields.iter().find(|field| field.name == *name) {
            check(types, &field.ty, &field.meta, value).map_err(|error| within(error, name))?;
        }
    }
    Ok(())
}

/// Checks each of a vector's `components` against `meta`'s bounds.
fn components(meta: &Meta, components: &[f32]) -> Result<()> {
    for (&x, name) in components.iter().zip(["x", "y", "z", "w"]) {
        in_range(meta, Number::F32(x)).map_err(|error| within(error, name))?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Number {
    Int(i128),
    F32(f32),
    Float(f64),
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::F32(x) => write!(f, "{x}"),
            Self::Float(x) => write!(f, "{x}"),
        }
    }
}

/// Whether `number` is below `bound`: integers compared exactly, and an `f32` with the bound
/// rounded to one.
fn below(number: Number, bound: Bound) -> bool {
    match (number, bound) {
        (Number::Int(n), Bound::Int(b)) => n < b,
        (Number::Int(n), Bound::Float(b)) => (n as f64) < b,
        (Number::F32(x), bound) => x < bound.as_f32(),
        (Number::Float(x), bound) => x < bound.as_f64(),
    }
}

fn above(number: Number, bound: Bound) -> bool {
    match (number, bound) {
        (Number::Int(n), Bound::Int(b)) => n > b,
        (Number::Int(n), Bound::Float(b)) => (n as f64) > b,
        (Number::F32(x), bound) => x > bound.as_f32(),
        (Number::Float(x), bound) => x > bound.as_f64(),
    }
}

/// Checks `number` against `meta`'s bounds, which it has.
fn in_range(meta: &Meta, number: Number) -> Result<()> {
    let out = |what: String| Err(Error::new("reflect.out_of_range", what));
    let finite = match number {
        Number::Int(_) => true,
        Number::F32(x) => x.is_finite(),
        Number::Float(x) => x.is_finite(),
    };
    if !finite {
        let range = range(meta);
        return out(format!("{number} is outside the field's range, {range}"));
    }
    if let Some(min) = meta.min
        && below(number, min)
    {
        return out(format!("{number} is below the field's min, {min}"));
    }
    if let Some(max) = meta.max
        && above(number, max)
    {
        return out(format!("{number} is above the field's max, {max}"));
    }
    Ok(())
}

/// `meta`'s bounds, which it has, as a phrase: such as `from 0 to 1`.
fn range(meta: &Meta) -> String {
    match (meta.min, meta.max) {
        (Some(min), Some(max)) => format!("from {min} to {max}"),
        (Some(min), None) => format!("of {min} and up"),
        (None, Some(max)) => format!("up to {max}"),
        (None, None) => unreachable!("only bounds leave a number out"),
    }
}

/// The number nearest 0 within `meta`'s bounds, its min as `least` rounds it and its max as
/// `most` does: 0, or the bound nearest it if it's outside them; `None` if they leave none.
fn nearest_zero<T: Copy + Default + PartialOrd>(
    meta: &Meta,
    least: fn(Bound) -> T,
    most: fn(Bound) -> T,
) -> Option<T> {
    let zero = T::default();
    let (min, max) = (meta.min.map(least), meta.max.map(most));
    let nearest = match (min, max) {
        (Some(min), _) if min > zero => min,
        (_, Some(max)) if max < zero => max,
        _ => zero,
    };
    let inside = min.is_none_or(|min| min <= nearest) && max.is_none_or(|max| nearest <= max);
    inside.then_some(nearest)
}

/// The least integer that isn't below `bound`.
fn ceiled(bound: Bound) -> i128 {
    match bound {
        Bound::Int(n) => n,
        Bound::Float(x) => x.ceil() as i128,
    }
}

/// The most integer that isn't above `bound`.
fn floored(bound: Bound) -> i128 {
    match bound {
        Bound::Int(n) => n,
        Bound::Float(x) => x.floor() as i128,
    }
}

/// The error that `meta`'s bounds leave no value of `ty`, a number or a vector, between them.
fn no_number(types: &Types, ty: &Type, meta: &Meta) -> Error {
    Error::new(
        "reflect.no_default",
        format!(
            "{} has no value in the field's range, {}, to be its default",
            what_is(types, ty),
            range(meta)
        ),
    )
}

/// Where a path leads in a type: the parts to take, from the outside in, the type it reaches, and
/// the hints of the field it's in, whose bounds its components and elements inherit.
#[derive(Debug)]
pub(crate) struct Resolved {
    pub(crate) steps: Vec<u32>,
    pub(crate) ty: Type,
    pub(crate) meta: Meta,
}

/// Where `path` leads in a value of `root`: a field's name, then `.name` for a struct's field,
/// `.x` to `.w` for a vector's component, and `[i]` for an array's or a list's element, such as
/// `rect[1].y`. An empty path is the whole value.
///
/// Fails with `reflect.bad_path` for a path it can't read, and `reflect.no_field` for a step to
/// a part there isn't, saying what's there: enums, options and quaternions are set whole, so a
/// step into one is one. An index past a list's end only fails as the list is reached.
pub(crate) fn resolve(types: &Types, root: &Type, path: &str) -> Result<Resolved> {
    let mut to = Resolved {
        steps: Vec::new(),
        ty: root.clone(),
        meta: Meta::default(),
    };
    let bad =
        |what: String| Error::new("reflect.bad_path", format!("`{path}` isn't a path: {what}"));
    let mut rest = path;
    while !rest.is_empty() {
        let walked = &path[..path.len() - rest.len()];
        let step = if let Some(index) = rest.strip_prefix('[') {
            let Some((digits, after)) = index.split_once(']') else {
                return Err(bad("a `[` has no `]`".to_owned()));
            };
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad(format!("`[{digits}]` isn't an index, such as `[0]`")));
            }
            let Ok(index) = digits.parse() else {
                return Err(bad(format!(
                    "`[{digits}]` is past the last index, {}",
                    u32::MAX
                )));
            };
            rest = after;
            Step::Index(index)
        } else {
            let name = if walked.is_empty() {
                rest
            } else {
                rest.strip_prefix('.').ok_or_else(|| {
                    bad(format!(
                        "`{rest}` follows `{walked}` without a `.` or a `[`"
                    ))
                })?
            };
            let end = name.find(['.', '[']).unwrap_or(name.len());
            let (name, after) = name.split_at(end);
            if name.is_empty() {
                return Err(bad("a step names no field".to_owned()));
            }
            // Any identifier's: those that aren't ASCII are left to the lookup, as Rust's
            // identifiers take so many characters.
            if !name
                .chars()
                .all(|c| !c.is_ascii() || c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(bad(format!("`{name}` isn't a field's name")));
            }
            rest = after;
            Step::Field(name)
        };
        to.take(types, step, walked)?;
    }
    Ok(to)
}

/// One step of a path.
#[derive(Clone, Copy)]
enum Step<'a> {
    Field(&'a str),
    Index(u32),
}

impl fmt::Display for Step<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Field(name) => write!(f, "{name}"),
            Self::Index(index) => write!(f, "[{index}]"),
        }
    }
}

impl Resolved {
    /// Takes `step` from where it's reached, `walked` along the path.
    fn take(&mut self, types: &Types, step: Step<'_>, walked: &str) -> Result<()> {
        let none = |what: String| Err(Error::new("reflect.no_field", what));
        let here = |types: &Types, ty: &Type| match walked {
            "" => what_is(types, ty),
            walked => format!("`{walked}` is {}", what_is(types, ty)),
        };
        let components = match self.ty {
            Type::Vec2 => 2,
            Type::Vec3 => 3,
            Type::Vec4 => 4,
            _ => 0,
        };
        match (&self.ty, step) {
            (Type::Named(index), Step::Field(name)) => {
                let info = types.info(*index);
                let Shape::Struct { fields, .. } = &info.shape else {
                    return none(format!("{}, set whole", here(types, &self.ty)));
                };
                let Some(i) = fields.iter().position(|field| field.name == name) else {
                    let names = list(fields.iter().map(|field| &field.name));
                    return none(format!(
                        "{}, which has no field `{name}`; its fields are {names}",
                        here(types, &self.ty)
                    ));
                };
                self.steps.push(i as u32);
                self.ty = fields[i].ty.clone();
                self.meta = fields[i].meta.clone();
            }
            (_, Step::Field(name)) if components > 0 => {
                let names = &["x", "y", "z", "w"][..components];
                let Some(i) = names.iter().position(|&component| component == name) else {
                    return none(format!(
                        "{}, whose components are {}",
                        here(types, &self.ty),
                        list(names)
                    ));
                };
                self.steps.push(i as u32);
                self.ty = Type::F32;
                self.meta.show = Show::Plain;
            }
            (Type::Array(_, len), Step::Index(index)) if index >= *len => {
                let last = match len {
                    0 => "none".to_owned(),
                    len => format!("`[0]` to `[{}]`", len - 1),
                };
                return none(format!(
                    "{}, whose elements are {last}",
                    here(types, &self.ty)
                ));
            }
            (Type::Array(inner, _) | Type::List(inner), Step::Index(index)) => {
                self.steps.push(index);
                self.ty = (**inner).clone();
            }
            (Type::Option(_) | Type::Quat, _) => {
                return none(format!("{}, set whole", here(types, &self.ty)));
            }
            _ => {
                return none(format!("{}, which has no `{step}`", here(types, &self.ty)));
            }
        }
        Ok(())
    }
}

/// What a value of `ty` is, as a phrase: such as `an f32`, or `a struct, Transform`.
fn what_is(types: &Types, ty: &Type) -> String {
    match ty {
        Type::Bool => "a bool".to_owned(),
        Type::Int { bits, signed } => {
            let (article, sign) = if *signed { ("an", 'i') } else { ("a", 'u') };
            format!("{article} {sign}{bits}")
        }
        Type::F32 => "an f32".to_owned(),
        Type::F64 => "an f64".to_owned(),
        Type::String => "a string".to_owned(),
        Type::Vec2 => "a Vec2".to_owned(),
        Type::Vec3 => "a Vec3".to_owned(),
        Type::Vec4 => "a Vec4".to_owned(),
        Type::Quat => "a quaternion".to_owned(),
        Type::Entity => "an entity".to_owned(),
        Type::Option(inner) => format!("an option of {}", what_is(types, inner)),
        Type::List(inner) => format!("a list of {}", what_is(types, inner)),
        Type::Array(inner, len) => format!("an array of {len} of {}", what_is(types, inner)),
        Type::Named(index) => {
            let info = types.info(*index);
            match info.shape {
                Shape::Struct { .. } => format!("a struct, {}", info.name),
                Shape::Enum(_) => format!("an enum, {}", info.name),
                Shape::Opaque => format!("an opaque type, {}", info.name),
            }
        }
        Type::Opaque => "opaque".to_owned(),
    }
}

/// The part of `root` that `steps`, a resolved path's, lead to.
///
/// Fails with `reflect.no_field` at an index past a list's end.
pub(crate) fn reach<'a>(root: &'a dyn Reflect, steps: &[u32]) -> Result<&'a dyn Reflect> {
    let mut at = root;
    for &step in steps {
        at = at.part(step).ok_or_else(|| past_end(at, step))?;
    }
    Ok(at)
}

/// Checks that the opaque part that `to` leads to in `root` is there, as [`reach`] does for any
/// other: an opaque field isn't a part of the struct it's in, so that struct is reached in its
/// place.
pub(crate) fn reach_opaque(root: &dyn Reflect, to: &Resolved) -> Result<()> {
    let steps = match to.ty {
        Type::Opaque => to.steps.split_last().map_or(&[][..], |(_, before)| before),
        _ => &to.steps,
    };
    reach(root, steps).map(drop)
}

pub(crate) fn reach_mut<'a>(
    root: &'a mut dyn Reflect,
    steps: &[u32],
) -> Result<&'a mut dyn Reflect> {
    let mut at = root;
    for &step in steps {
        // Asked first, since the borrow checker keeps `at` borrowed in the branch where
        // `part_mut` returns nothing.
        if at.part(step).is_none() {
            return Err(past_end(at, step));
        }
        at = at.part_mut(step).expect("the part is there");
    }
    Ok(at)
}

/// The error that `list` has no element `index`.
fn past_end(list: &dyn Reflect, index: u32) -> Error {
    let len = match list.to_value() {
        Value::List(items) => items.len(),
        _ => 0,
    };
    Error::new(
        "reflect.no_field",
        format!("the list has no element [{index}]: it has {len}"),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::draw::SAMPLERS;
    use crate::{Component, EntityId, MeshDraw, Sprite, Transform, World};

    /// A struct a field holds.
    #[derive(Clone, Debug, PartialEq, Reflect)]
    pub(crate) struct Inner {
        #[reflect(min = -1, max = 1)]
        pub(crate) level: i8,
        pub(crate) name: String,
    }

    /// Kept, but not reflected.
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) struct Handle(pub(crate) u32);

    #[derive(Clone, Debug, PartialEq, Reflect)]
    pub(crate) enum Form {
        Point,
        Circle(#[reflect(min = 0.0)] f32),
        Box {
            size: Vec2,
            #[reflect(degrees)]
            turn: f32,
        },
        Held(#[reflect(opaque)] Handle),
    }

    /** Two numbers,
     * in a block comment, whose stars aren't its text. */
    #[derive(Clone, Debug, PartialEq, Reflect)]
    pub(crate) struct Pair(pub(crate) u16, pub(crate) i64);

    #[derive(Clone, Debug, PartialEq, Reflect)]
    pub(crate) struct Unit;

    /// A field of everything reflection describes.
    ///
    /// Its doc is two paragraphs.
    #[derive(Clone, Debug, PartialEq, Reflect)]
    pub(crate) struct Everything {
        /// Whether it's on.
        pub(crate) on: bool,
        #[reflect(min = 0, max = 100)]
        pub(crate) count: u8,
        pub(crate) byte: u8,
        #[reflect(min = 0.0)]
        pub(crate) weight: f64,
        #[reflect(degrees, min = -3.2, max = 3.2)]
        pub(crate) angle: f32,
        #[reflect(color)]
        pub(crate) tint: Vec4,
        pub(crate) offset: Vec3,
        pub(crate) turn: Quat,
        pub(crate) target: Option<Entity>,
        pub(crate) inner: Inner,
        pub(crate) form: Form,
        pub(crate) maybe: Option<Inner>,
        #[reflect(max = 10)]
        pub(crate) list: Vec<u32>,
        pub(crate) rect: [Vec2; 2],
        pub(crate) pair: Pair,
        pub(crate) unit: Unit,
        #[reflect(opaque)]
        pub(crate) handle: Handle,
    }

    /// Built on [`everything`], which gives it its handle, as a component with an opaque field is.
    impl Component for Everything {
        fn base() -> Option<Self> {
            Some(everything())
        }
    }

    impl crate::Writable for Everything {}

    pub(crate) fn everything() -> Everything {
        Everything {
            on: true,
            count: 42,
            byte: 7,
            weight: 2.5,
            angle: 0.5,
            // Transparent, so that its color is lost if alpha is multiplied in.
            tint: Vec4::new(0.3, 0.6, 0.9, 0.0),
            offset: Vec3::new(1.0, -0.0, 3.0),
            turn: Quat::from_rotation_x(0.25),
            target: None,
            inner: Inner {
                level: -1,
                name: "first".to_owned(),
            },
            form: Form::Point,
            maybe: None,
            list: vec![1, 2],
            rect: [Vec2::ZERO, Vec2::ONE],
            pair: Pair(3, -4),
            unit: Unit,
            handle: Handle(7),
        }
    }

    /// Whether `a` and `b` are the same, their numbers compared by their bits.
    pub(crate) fn same(a: &Value, b: &Value) -> bool {
        let floats = |a: &[f32], b: &[f32]| {
            a.iter()
                .map(|x| x.to_bits())
                .eq(b.iter().map(|x| x.to_bits()))
        };
        let fields = |a: &[(Name, Value)], b: &[(Name, Value)]| {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.0 == b.0 && same(&a.1, &b.1))
        };
        match (a, b) {
            (Value::F32(a), Value::F32(b)) => a.to_bits() == b.to_bits(),
            (Value::F64(a), Value::F64(b)) => a.to_bits() == b.to_bits(),
            (Value::Vec2(a), Value::Vec2(b)) => floats(a.as_ref(), b.as_ref()),
            (Value::Vec3(a), Value::Vec3(b)) => floats(a.as_ref(), b.as_ref()),
            (Value::Vec4(a), Value::Vec4(b)) => floats(a.as_ref(), b.as_ref()),
            (Value::Quat(a), Value::Quat(b)) => floats(&a.to_array(), &b.to_array()),
            (Value::Option(Some(a)), Value::Option(Some(b))) => same(a, b),
            (Value::List(a), Value::List(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
            }
            (Value::Struct(a), Value::Struct(b)) => fields(a, b),
            (Value::Variant(m, a), Value::Variant(n, b)) => m == n && fields(a, b),
            _ => a == b,
        }
    }

    /// Every component of every entity of `world`, read by name, beside the entity's handle and ID.
    pub(crate) fn snapshot(world: &World) -> Vec<(Entity, EntityId, String, Value)> {
        let mut all = Vec::new();
        for entity in world.entities(None).expect("every entity") {
            let id = world.id(entity).expect("alive");
            for component in world.components_of(entity).expect("alive") {
                let value = world.get_field(entity, component, "").expect("readable");
                all.push((entity, id, component.to_owned(), value));
            }
        }
        all
    }

    pub(crate) fn same_snapshots(
        a: &[(Entity, EntityId, String, Value)],
        b: &[(Entity, EntityId, String, Value)],
    ) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(a, b)| a.0 == b.0 && a.1 == b.1 && a.2 == b.2 && same(&a.3, &b.3))
    }

    fn field<'a>(types: &'a Types, ty: &Type, name: &str) -> &'a Field {
        let Type::Named(index) = ty else {
            panic!("{ty:?} isn't named");
        };
        let Shape::Struct { fields, .. } = &types.info(*index).shape else {
            panic!("{ty:?} isn't a struct");
        };
        fields
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("no field {name}"))
    }

    #[test]
    fn types_describe_their_fields_once_each() {
        let mut world = World::default();
        for registered in [
            world.register::<Transform>(),
            world.register::<Sprite>(),
            world.register::<MeshDraw>(),
            world.register::<Everything>(),
            world.register::<Everything>(),
        ] {
            registered.expect("described");
        }
        let types = world.types();
        // Each as it's first reached, the types of a type's fields after it.
        let names: Vec<&str> = types.iter().map(|(_, info)| &*info.name).collect();
        let expected = [
            "Parent",
            "Transform",
            "Sprite",
            "Sampler",
            "MeshDraw",
            "Everything",
            "Inner",
            "Form",
            "Pair",
            "Unit",
        ];
        assert_eq!(names, expected);
        let named = |name| Type::Named(types.find(name).expect(name));

        let transform = named("Transform");
        let ty = |name| field(types, &transform, name).ty.clone();
        assert_eq!(
            [ty("translation"), ty("rotation"), ty("scale")],
            [Type::Vec3, Type::Quat, Type::Vec3]
        );
        assert_eq!(
            field(types, &transform, "rotation").meta.doc,
            "As a unit quaternion."
        );
        let doc = |name| &types.info(types.find(name).expect(name)).doc;
        assert!(doc("Transform").starts_with("Where an entity is"));
        assert_eq!(
            doc("Everything"),
            "A field of everything reflection describes.\n\nIts doc is two paragraphs."
        );
        assert_eq!(
            doc("Pair"),
            "Two numbers, in a block comment, whose stars aren't its text."
        );

        let sprite = named("Sprite");
        let color = field(types, &sprite, "color");
        assert_eq!((&color.ty, color.meta.show), (&Type::Vec4, Show::Color));
        assert!(field(types, &sprite, "texture").ty.is_opaque(types));
        let rect = &field(types, &sprite, "rect").ty;
        assert_eq!(*rect, Type::Array(Box::new(Type::Vec2), 2));
        assert_eq!(field(types, &sprite, "sampler").ty, named("Sampler"));
        let Shape::Enum(samplers) = &types.info(types.find("Sampler").expect("described")).shape
        else {
            panic!("a sampler is an enum");
        };
        let samplers: Vec<&str> = samplers.iter().map(|variant| &*variant.name).collect();
        assert_eq!(samplers.len(), 6);
        assert!(samplers.contains(&"NearestRepeat"));
        assert!(named("MeshDraw").is_opaque(types));

        let everything = named("Everything");
        let meta = |name| field(types, &everything, name).meta.clone();
        let ty = |name| field(types, &everything, name).ty.clone();
        let u8 = Type::Int {
            bits: 8,
            signed: false,
        };
        assert_eq!(meta("on").doc, "Whether it's on.");
        assert_eq!(
            (ty("count"), meta("count").min, meta("count").max),
            (u8, Some(Bound::Int(0)), Some(Bound::Int(100)))
        );
        assert_eq!(
            (meta("weight").min, meta("weight").max),
            (Some(Bound::Float(0.0)), None)
        );
        assert_eq!(meta("angle").show, Show::Degrees);
        assert_eq!(meta("angle").min, Some(Bound::Float(-3.2)));
        assert_eq!(meta("tint").show, Show::Color);
        assert_eq!(ty("target"), Type::Option(Box::new(Type::Entity)));
        assert_eq!(ty("inner"), named("Inner"));
        assert_eq!(ty("maybe"), Type::Option(Box::new(named("Inner"))));
        let u32 = Type::Int {
            bits: 32,
            signed: false,
        };
        assert_eq!(
            (ty("list"), meta("list").max),
            (Type::List(Box::new(u32)), Some(Bound::Int(10)))
        );
        assert_eq!(ty("handle"), Type::Opaque);
        let level = field(types, &named("Inner"), "level");
        assert_eq!(level.meta.min, Some(Bound::Int(-1)));
        let Shape::Struct { fields, tuple } =
            &types.info(types.find("Pair").expect("described")).shape
        else {
            panic!("a pair is a struct");
        };
        let names: Vec<&str> = fields.iter().map(|field| &*field.name).collect();
        assert_eq!((names, *tuple), (vec!["0", "1"], true));
        let unit = &types.info(types.find("Unit").expect("described")).shape;
        assert_eq!(
            *unit,
            Shape::Struct {
                fields: Vec::new(),
                tuple: false
            }
        );
        let Shape::Enum(forms) = &types.info(types.find("Form").expect("described")).shape else {
            panic!("a form is an enum");
        };
        let shapes: Vec<(&str, usize, bool)> = forms
            .iter()
            .map(|variant| (&*variant.name, variant.fields.len(), variant.tuple))
            .collect();
        assert_eq!(
            shapes,
            [
                ("Point", 0, false),
                ("Circle", 1, true),
                ("Box", 2, false),
                ("Held", 1, true)
            ]
        );
        assert_eq!(forms[2].fields[1].meta.show, Show::Degrees);
        assert_eq!(forms[3].fields[0].ty, Type::Opaque);
    }

    #[derive(Clone, Debug, PartialEq, Reflect)]
    struct Node {
        kids: Vec<Node>,
    }

    #[test]
    fn a_recursive_type_describes_itself_once() {
        let mut types = Types::default();
        let ty = Node::describe(&mut types).expect("described");
        assert_eq!(types.iter().len(), 1);
        let kids = field(&types, &ty, "kids");
        assert_eq!(kids.ty, Type::List(Box::new(ty.clone())));
        let tree = Node {
            kids: vec![
                Node { kids: Vec::new() },
                Node {
                    kids: vec![Node { kids: Vec::new() }],
                },
            ],
        };
        assert_eq!(Node::from_value(&tree.to_value()).expect("a tree"), tree);
    }

    mod other {
        /// Named as the outer module's `Inner` is.
        #[derive(Clone, super::Reflect)]
        pub(super) struct Inner {
            pub(super) x: f32,
        }
    }

    /// Its second field's type takes a name that's taken, after its first's is described.
    #[derive(Clone, Reflect)]
    struct Holder {
        pair: Pair,
        inner: other::Inner,
    }

    #[test]
    fn a_type_that_fails_to_describe_itself_leaves_the_types_as_they_were() {
        let mut types = Types::default();
        Inner::describe(&mut types).expect("described");
        for _ in 0..2 {
            let error = Holder::describe(&mut types).expect_err("a name taken");
            assert_eq!(error.code(), "reflect.name_taken");
            let message = error.to_string();
            assert!(
                message.contains("reflect::tests::Inner")
                    && message.contains("reflect::tests::other::Inner")
                    && message.contains("#[reflect(name"),
                "{message}"
            );
            let names: Vec<&str> = types.iter().map(|(_, info)| &*info.name).collect();
            assert_eq!(names, ["Inner"]);
            assert_eq!(types.find("Holder").or(types.find("Pair")), None);
        }
        // Undone by its type too, so that it describes itself afresh.
        let pair = Pair::describe(&mut types).expect("described");
        assert_eq!(pair, Type::Named(TypeIndex(1)));
        let Shape::Struct { fields, .. } = &types.info(TypeIndex(1)).shape else {
            panic!("a pair is a struct");
        };
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn paths_lead_to_fields_and_their_hints() {
        let mut world = World::default();
        world.register::<Transform>().expect("described");
        world.register::<Sprite>().expect("described");
        world.register::<Everything>().expect("described");
        let types = world.types();
        let resolve = |root: &str, path| {
            let root = Type::Named(types.find(root).expect(root));
            resolve(types, &root, path)
        };
        let at = |root, path| {
            let to = resolve(root, path).unwrap_or_else(|error| panic!("{path}: {error}"));
            (to.steps, to.ty)
        };
        let everything = |name| {
            let Shape::Struct { fields, .. } = &types
                .info(types.find("Everything").expect("described"))
                .shape
            else {
                unreachable!()
            };
            fields
                .iter()
                .position(|field| field.name == name)
                .expect(name) as u32
        };
        assert_eq!(
            at("Transform", ""),
            (
                vec![],
                Type::Named(types.find("Transform").expect("described"))
            )
        );
        assert_eq!(at("Transform", "translation.x"), (vec![0, 0], Type::F32));
        assert_eq!(at("Transform", "scale.z"), (vec![2, 2], Type::F32));
        assert_eq!(at("Sprite", "rect[1].y"), (vec![2, 1, 1], Type::F32));
        let u16 = Type::Int {
            bits: 16,
            signed: false,
        };
        assert_eq!(
            at("Everything", "pair.0"),
            (vec![everything("pair"), 0], u16)
        );
        assert_eq!(
            at("Everything", "turn"),
            (vec![everything("turn")], Type::Quat)
        );
        // Elements and components keep their field's bounds.
        let list = resolve("Everything", "list[7]").expect("an element");
        assert_eq!(list.steps, [everything("list"), 7]);
        assert_eq!(list.meta.max, Some(Bound::Int(10)));
        assert_eq!(
            resolve("Everything", "angle").expect("a field").meta.show,
            Show::Degrees
        );
        assert_eq!(
            resolve("Everything", "inner.level")
                .expect("a field")
                .meta
                .min,
            Some(Bound::Int(-1))
        );
        assert!(
            resolve("Sprite", "texture")
                .expect("a field")
                .ty
                .is_opaque(types)
        );

        for (root, path, says) in [
            (
                "Transform",
                "translation.q",
                "whose components are `x`, `y` and `z`",
            ),
            ("Transform", "rotation.x", "a quaternion, set whole"),
            ("Sprite", "sampler.x", "an enum, Sampler, set whole"),
            ("Sprite", "rect[2]", "whose elements are `[0]` to `[1]`"),
            ("Sprite", "rect.x", "which has no `x`"),
            (
                "Transform",
                "spin",
                "its fields are `translation`, `rotation` and `scale`",
            ),
            (
                "Everything",
                "maybe.level",
                "an option of a struct, Inner, set whole",
            ),
            ("Everything", "form.size", "an enum, Form, set whole"),
            ("Everything", "on.x", "a bool, which has no `x`"),
            ("Everything", "[0]", "which has no `[0]`"),
        ] {
            let error = resolve(root, path).expect_err(path);
            assert_eq!(error.code(), "reflect.no_field", "{path}: {error}");
            assert!(error.to_string().contains(says), "{path}: {error}");
        }
        for path in [
            "rect[",
            "rect[]",
            "rect[x]",
            "rect[-1]",
            "rect[+1]",
            "rect[4294967296]",
            "rect..x",
            "rect.",
            "rect[1]y",
            " rect",
            "re-ct",
        ] {
            let error = resolve("Sprite", path).expect_err(path);
            assert_eq!(error.code(), "reflect.bad_path", "{path}: {error}");
            assert!(error.to_string().contains(&format!("`{path}`")), "{error}");
        }

        // A field's name is any identifier's.
        #[derive(Clone, Reflect)]
        struct Measure {
            größe: f32,
        }
        let mut types = Types::default();
        let measure = Measure::describe(&mut types).expect("described");
        let to = super::resolve(&types, &measure, "größe").expect("a field");
        assert_eq!((to.steps, to.ty), (vec![0], Type::F32));
    }

    /// `value` read and made again, which must give back the same value, bit for bit.
    fn round_trip<T: Reflect>(value: T) {
        let read = value.to_value();
        let made = T::from_value(&read).unwrap_or_else(|error| panic!("{read:?}: {error}"));
        assert!(same(&made.to_value(), &read), "{read:?}");
    }

    #[test]
    fn values_come_back_bit_for_bit() {
        round_trip(true);
        round_trip(i8::MIN);
        round_trip(i16::MAX);
        round_trip(-5i32);
        round_trip(i64::MIN);
        round_trip(u8::MAX);
        round_trip(u16::MAX);
        round_trip(7u32);
        round_trip(u64::MAX);
        round_trip(-0.0f32);
        round_trip(f32::from_bits(0x7fc0_1234));
        round_trip(f64::from_bits(0xfff8_0000_0000_4321));
        round_trip("text".to_owned());
        round_trip(Vec2::new(1.0, -0.0));
        round_trip(Vec3::new(f32::INFINITY, 2.0, 3.0));
        round_trip(Vec4::new(1.0, 2.0, 3.0, f32::from_bits(0x7fc0_0001)));
        round_trip(Quat::from_rotation_y(1.0));
        round_trip(World::default().spawn(()));
        round_trip(Some(3u8));
        round_trip(None::<u8>);
        round_trip(vec![1u32, 2, 3]);
        round_trip(Vec::<String>::new());
        round_trip([Vec2::X, Vec2::Y]);
        round_trip(Inner {
            level: 1,
            name: "x".to_owned(),
        });
        round_trip(Form::Point);
        round_trip(Form::Circle(f32::from_bits(0xffc0_0002)));
        round_trip(Form::Box {
            size: Vec2::ONE,
            turn: -0.0,
        });
        round_trip(Pair(1, -1));
        round_trip(Unit);
        round_trip(Transform {
            translation: Vec3::new(1.0, -0.0, 2.0),
            rotation: Quat::from_rotation_z(0.5),
            scale: Vec3::splat(2.0),
        });
        for name in SAMPLERS {
            round_trip(name);
        }
        // An integer of either sign sets one of either, if it fits.
        assert_eq!(u8::from_value(&Value::Int(5)).expect("fits"), 5);
        assert_eq!(i8::from_value(&Value::UInt(5)).expect("fits"), 5);
        // What's opaque comes back from what it was.
        let all = everything();
        let kept = all.with_value(&all.to_value()).expect("its own value");
        assert!(same(&kept.to_value(), &all.to_value()));
        assert_eq!(kept.handle, all.handle);
        let error = Everything::from_value(&all.to_value()).expect_err("an opaque field");
        assert_eq!(error.code(), "reflect.opaque");
        assert!(error.to_string().starts_with("at `handle`: "), "{error}");
    }

    #[derive(Clone, Debug, PartialEq, Reflect)]
    pub(crate) struct WithOpaque {
        #[reflect(opaque)]
        pub(crate) handle: Handle,
    }

    #[derive(Clone, Debug, PartialEq, Reflect)]
    struct Partly {
        a: f32,
        b: Option<WithOpaque>,
    }

    #[test]
    fn a_value_refused_partway_changes_nothing() {
        let mut partly = Partly { a: 1.0, b: None };
        let inner = Value::Struct(vec![("handle".into(), Value::Opaque)]);
        let value = Value::Struct(vec![
            ("a".into(), Value::F32(2.0)),
            ("b".into(), Value::Option(Some(Box::new(inner)))),
        ]);
        let error = partly.set_value(&value).expect_err("an opaque field made");
        assert_eq!(error.code(), "reflect.opaque");
        assert!(error.to_string().starts_with("at `b.handle`: "), "{error}");
        assert_eq!(partly, Partly { a: 1.0, b: None });
        // Kept, it's set.
        let mut partly = Partly {
            a: 1.0,
            b: Some(WithOpaque { handle: Handle(3) }),
        };
        partly.set_value(&value).expect("the opaque field kept");
        assert_eq!(partly.a, 2.0);
        assert_eq!(partly.b, Some(WithOpaque { handle: Handle(3) }));
    }

    #[test]
    fn bounds_apply_to_numbers_components_and_elements() {
        let meta = |min, max| Meta {
            min,
            max,
            ..Meta::default()
        };
        let types = Types::default();
        let int = Type::Int {
            bits: 64,
            signed: true,
        };
        let check = |ty: &Type, meta: &Meta, value: Value| check(&types, ty, meta, &value);
        let within = meta(Some(Bound::Int(-2)), Some(Bound::Int(2)));
        // Integers compare exactly, past what an f64 holds.
        let huge = meta(None, Some(Bound::Int(i128::from(i64::MAX) - 1)));
        assert!(check(&int, &huge, Value::Int(i64::MAX - 1)).is_ok());
        assert!(check(&int, &huge, Value::Int(i64::MAX)).is_err());
        assert!(check(&int, &within, Value::Int(-2)).is_ok());
        for (ty, value, at) in [
            (int.clone(), Value::Int(3), ""),
            (int.clone(), Value::Int(-3), ""),
            (Type::F32, Value::F32(f32::NAN), ""),
            (Type::F32, Value::F32(f32::INFINITY), ""),
            (Type::F64, Value::F64(2.5), ""),
            (
                Type::Vec3,
                Value::Vec3(Vec3::new(0.0, 0.0, 9.0)),
                "at `z`: ",
            ),
            (
                Type::Option(Box::new(Type::F32)),
                Value::Option(Some(Box::new(Value::F32(5.0)))),
                "",
            ),
            (
                Type::List(Box::new(int.clone())),
                Value::List(vec![Value::Int(0), Value::Int(5)]),
                "at `[1]`: ",
            ),
        ] {
            let error = check(&ty, &within, value.clone()).expect_err("out of range");
            assert_eq!(error.code(), "reflect.out_of_range", "{value:?}");
            assert!(error.to_string().starts_with(at), "{error}");
        }
        // An f32 compares with its bounds rounded to an f32: 3.2f32, a little more than 3.2, is
        // the f32 that `max = 3.2` means. An f64 compares with them as they are.
        let decimal = meta(Some(Bound::Float(-3.2)), Some(Bound::Float(3.2)));
        assert!(check(&Type::F32, &decimal, Value::F32(3.2)).is_ok());
        assert!(check(&Type::F32, &decimal, Value::F32(-3.2)).is_ok());
        let corner = Value::Vec2(Vec2::new(3.2, -3.2));
        assert!(check(&Type::Vec2, &decimal, corner).is_ok());
        assert!(check(&Type::F32, &decimal, Value::F32(3.2f32.next_up())).is_err());
        assert!(check(&Type::F64, &decimal, Value::F64(3.2f32.into())).is_err());
        // Without bounds, any number is in range.
        assert!(check(&Type::F32, &Meta::default(), Value::F32(f32::NAN)).is_ok());
        let half = meta(Some(Bound::Float(0.5)), None);
        assert!(check(&int, &half, Value::Int(0)).is_err());
        assert!(check(&int, &half, Value::Int(1)).is_ok());
    }

    #[test]
    fn a_type_defaults_to_zero_or_the_bound_nearest_it() {
        let types = Types::default();
        let meta = |min, max| Meta {
            min,
            max,
            ..Meta::default()
        };
        let none = Meta::default;
        let int = |bits, signed| Type::Int { bits, signed };
        let entities = |len| Type::Array(Box::new(Type::Entity), len);
        for (ty, meta, expected) in [
            (Type::Bool, none(), Value::Bool(false)),
            (int(8, true), none(), Value::Int(0)),
            (int(8, false), none(), Value::UInt(0)),
            (
                int(32, true),
                meta(Some(Bound::Int(3)), None),
                Value::Int(3),
            ),
            (
                int(32, true),
                meta(None, Some(Bound::Int(-3))),
                Value::Int(-3),
            ),
            // Rounded to an integer inside them.
            (
                int(8, false),
                meta(Some(Bound::Float(0.5)), None),
                Value::UInt(1),
            ),
            (
                int(8, true),
                meta(None, Some(Bound::Float(-0.5))),
                Value::Int(-1),
            ),
            (
                int(16, true),
                meta(Some(Bound::Int(-5)), Some(Bound::Float(5.5))),
                Value::Int(0),
            ),
            (
                int(64, false),
                meta(Some(Bound::Int(u64::MAX.into())), None),
                Value::UInt(u64::MAX),
            ),
            (
                int(64, true),
                meta(None, Some(Bound::Int(i64::MIN.into()))),
                Value::Int(i64::MIN),
            ),
            (Type::F32, none(), Value::F32(0.0)),
            // As `check` compares with it: rounded to an f32.
            (
                Type::F32,
                meta(Some(Bound::Float(0.1)), None),
                Value::F32(0.1),
            ),
            (
                Type::F64,
                meta(Some(Bound::Float(0.1)), None),
                Value::F64(0.1),
            ),
            (
                Type::F64,
                meta(None, Some(Bound::Int(-2))),
                Value::F64(-2.0),
            ),
            (
                Type::F32,
                meta(Some(Bound::Int(-1)), Some(Bound::Int(1))),
                Value::F32(0.0),
            ),
            (Type::Vec2, none(), Value::Vec2(Vec2::ZERO)),
            (
                Type::Vec3,
                meta(Some(Bound::Int(1)), None),
                Value::Vec3(Vec3::ONE),
            ),
            (
                Type::Vec4,
                meta(None, Some(Bound::Float(-0.5))),
                Value::Vec4(Vec4::splat(-0.5)),
            ),
            (Type::Quat, none(), Value::Quat(Quat::IDENTITY)),
            (Type::String, none(), Value::String(String::new())),
            (
                Type::Option(Box::new(Type::Entity)),
                none(),
                Value::Option(None),
            ),
            (
                Type::List(Box::new(Type::Entity)),
                none(),
                Value::List(Vec::new()),
            ),
            (entities(0), none(), Value::List(Vec::new())),
            (
                Type::Array(Box::new(Type::F32), 3),
                meta(Some(Bound::Int(2)), None),
                Value::List(vec![Value::F32(2.0); 3]),
            ),
            (Type::Opaque, none(), Value::Opaque),
        ] {
            let made = types.default_value(&ty, &meta);
            let made = made.unwrap_or_else(|error| panic!("{ty:?}, {meta:?}: {error}"));
            assert!(same(&made, &expected), "{ty:?}, {meta:?}: {made:?}");
            let checked = check(&types, &ty, &meta, &made);
            assert!(checked.is_ok(), "{ty:?}, {meta:?}: {checked:?}");
        }
        for (ty, meta, says) in [
            (
                int(8, false),
                meta(Some(Bound::Int(300)), None),
                "a u8 has no value in the field's range, of 300 and up, to be its default",
            ),
            (int(8, false), meta(None, Some(Bound::Int(-1))), "up to -1"),
            (
                int(8, true),
                meta(Some(Bound::Int(-200)), Some(Bound::Int(-150))),
                "an i8 has no value in the field's range, from -200 to -150",
            ),
            (
                int(32, true),
                meta(Some(Bound::Float(0.5)), Some(Bound::Float(0.7))),
                "from 0.5 to 0.7",
            ),
            (
                int(64, true),
                meta(Some(Bound::Int(i128::from(i64::MAX) + 1)), None),
                "an i64",
            ),
            (
                Type::F32,
                meta(Some(Bound::Int(2)), Some(Bound::Int(1))),
                "an f32 has no value in the field's range, from 2 to 1",
            ),
            (
                Type::Vec2,
                meta(Some(Bound::Int(2)), Some(Bound::Int(1))),
                "a Vec2",
            ),
            // Beyond an f32's range, which rounds it to an infinite bound, outside every range.
            (
                Type::F32,
                meta(Some(Bound::Float(1e39)), None),
                "an f32 has no value in the field's range",
            ),
            (Type::Vec3, meta(None, Some(Bound::Float(-1e39))), "a Vec3"),
            (Type::Entity, none(), "is an `Option<Entity>`"),
            (entities(2), none(), "at `[0]`: an entity has no default"),
        ] {
            let error = types.default_value(&ty, &meta).expect_err(says);
            assert_eq!(error.code(), "reflect.no_default", "{error}");
            assert!(error.to_string().contains(says), "{error}");
        }
    }

    /// Lit by its hints' defaults, but for `spare`'s, which is its type's, as `shade`'s is.
    #[derive(Clone, Debug, PartialEq, Reflect)]
    struct Lamp {
        #[reflect(min = 0.0, default = 2.5)]
        intensity: f32,
        #[reflect(degrees, default = std::f32::consts::FRAC_PI_4)]
        cone: f32,
        shade: Shade,
        spare: Shade,
        watts: Watts,
    }

    #[derive(Clone, Debug, PartialEq, Reflect)]
    struct Watts(#[reflect(min = 1, default = 60)] u16);

    #[derive(Clone, Debug, PartialEq, Reflect)]
    enum Shade {
        Ring {
            #[reflect(default = Vec2::ONE)]
            size: Vec2,
            #[reflect(min = 1)]
            rings: u8,
        },
        Dome(#[reflect(min = 0.5)] f32),
    }

    /// A struct's default is its fields', each the one its hint gives, or else its type's under
    /// its bounds; an enum's is its first variant, with its fields' defaults; and a type held
    /// twice is no cycle.
    #[test]
    fn defaults_are_the_hints_or_else_the_types() {
        let mut types = Types::default();
        let lamp = Lamp::describe(&mut types).expect("described");
        let ring = Value::Variant(
            "Ring".into(),
            vec![
                ("size".into(), Value::Vec2(Vec2::ONE)),
                ("rings".into(), Value::UInt(1)),
            ],
        );
        let expected = Value::Struct(vec![
            ("intensity".into(), Value::F32(2.5)),
            ("cone".into(), Value::F32(std::f32::consts::FRAC_PI_4)),
            ("shade".into(), ring.clone()),
            ("spare".into(), ring.clone()),
            (
                "watts".into(),
                Value::Struct(vec![("0".into(), Value::UInt(60))]),
            ),
        ]);
        let made = types
            .default_value(&lamp, &Meta::default())
            .expect("a default");
        assert!(same(&made, &expected), "{made:?}");
        let made = Lamp::from_value(&made).expect("made");
        assert_eq!(made.watts, Watts(60));
        // The hints are the fields' defaults, as their types' own values.
        let intensity = field(&types, &lamp, "intensity");
        assert_eq!(intensity.default, Some(Value::F32(2.5)));
        assert_eq!(field(&types, &lamp, "shade").default, None);
        let watts = Type::Named(types.find("Watts").expect("described"));
        assert_eq!(field(&types, &watts, "0").default, Some(Value::UInt(60)));
        let shade = types.find("Shade").expect("described");
        let Shape::Enum(variants) = &types.info(shade).shape else {
            panic!("a shade is an enum");
        };
        assert_eq!(variants[0].fields[0].default, Some(Value::Vec2(Vec2::ONE)));
        assert_eq!(
            types.variant_default(&variants[0]).expect("a default"),
            ring
        );
        let dome = Value::Variant("Dome".into(), vec![("0".into(), Value::F32(0.5))]);
        assert_eq!(
            types.variant_default(&variants[1]).expect("a default"),
            dome
        );
        // A field without one has its type's, under its own bounds.
        let rings = types.field_default(&variants[0].fields[1]);
        assert_eq!(rings.expect("a default"), Value::UInt(1));
    }

    /// What a description written by hand makes a struct that holds itself, without end.
    enum Loop {}

    /// What a description written by hand makes an enum without variants.
    enum Never {}

    #[test]
    fn a_default_without_end_or_without_a_variant_is_refused() {
        let mut types = Types::default();
        let looped = types.add::<Loop>("Loop".into(), "".into(), |types| {
            let index = types.find("Loop").expect("reserved");
            let next = Field {
                name: "next".into(),
                ty: Type::Named(index),
                meta: Meta::default(),
                default: None,
            };
            Ok(Shape::Struct {
                fields: vec![next],
                tuple: false,
            })
        });
        let looped = looped.expect("described");
        let error = types.default_value(&looped, &Meta::default());
        let error = error.expect_err("without end");
        assert_eq!(error.code(), "reflect.no_default");
        assert_eq!(
            error.to_string(),
            "at `next`: `Loop`'s default holds a `Loop`, without end"
        );
        let never = types.add::<Never>("Never".into(), "".into(), |_| Ok(Shape::Enum(Vec::new())));
        let never = never.expect("described");
        let error = types.default_value(&never, &Meta::default());
        let error = error.expect_err("no variants");
        assert_eq!(error.code(), "reflect.no_default");
        assert!(
            error.to_string().contains("`Never` has no variants"),
            "{error}"
        );
    }

    #[derive(Clone, Reflect)]
    struct Light {
        #[reflect(min = 0, default = -1.0)]
        intensity: f32,
    }

    /// Registered, it describes its field's type, whose default its own bounds refuse.
    #[derive(Clone, crate::Component)]
    struct Dim {
        light: Light,
    }

    #[derive(Clone, Reflect)]
    enum Glow {
        Off,
        Tinted(#[reflect(color, max = 1, default = Vec3::new(1.0, 2.0, 1.0))] Vec3),
    }

    /// Registered, it describes `Moons`, whose default holds an orbit its bounds refuse, which
    /// only `Orbit`'s description, finished after it, says.
    #[derive(Clone, crate::Component)]
    struct Orbit {
        moons: Moons,
        #[reflect(min = 0)]
        radius: i32,
    }

    #[derive(Clone, Reflect)]
    struct Moons {
        #[reflect(default = vec![Orbit { moons: Moons { orbits: Vec::new() }, radius: -1 }])]
        orbits: Vec<Orbit>,
    }

    /// Opaque as a whole, so that a default gives it nothing.
    #[derive(Clone, Reflect)]
    #[reflect(opaque)]
    struct Socket;

    #[derive(Clone, crate::Component)]
    struct Plugged {
        #[reflect(default = Socket)]
        socket: Socket,
    }

    /// A default its field's bounds refuse is refused as its type registers, even one holding the
    /// type registering, checked once that's described; and so is one opaque as a whole, which
    /// would give nothing. Either leaves the type unregistered, and every type described with it.
    #[test]
    fn a_bad_default_is_refused_as_its_type_registers() {
        let mut world = World::default();
        let error = world.register::<Dim>().expect_err("a bad default");
        assert_eq!(error.code(), "reflect.bad_default");
        assert_eq!(
            error.to_string(),
            "`Light`'s field `intensity` defaults to a value it refuses: -1 is below the \
             field's min, 0"
        );
        let error = world.register::<Orbit>().expect_err("a bad default");
        assert_eq!(error.code(), "reflect.bad_default");
        assert_eq!(
            error.to_string(),
            "`Moons`'s field `orbits[0].radius` defaults to a value it refuses: -1 is below the \
             field's min, 0"
        );
        let error = world.register::<Plugged>().expect_err("an opaque default");
        assert_eq!(error.code(), "reflect.bad_default");
        assert_eq!(
            error.to_string(),
            "`Plugged`'s field `socket` is opaque, which reflection keeps but doesn't make, so it \
             takes no default"
        );
        let names: Vec<&str> = world.types().iter().map(|(_, info)| &*info.name).collect();
        assert_eq!(names, ["Parent"]);
        assert_eq!(world.component_types().collect::<Vec<_>>(), ["Parent"]);
        let mut types = Types::default();
        let error = Glow::describe(&mut types).expect_err("a bad default");
        assert_eq!(error.code(), "reflect.bad_default");
        assert_eq!(
            error.to_string(),
            "`Glow::Tinted`'s field `0.y` defaults to a value it refuses: 2 is above the \
             field's max, 1"
        );
        assert_eq!(types.iter().len(), 0);
    }

    /// Each of the engine's components' default makes it, as an add makes it: a transform the
    /// identity, a sprite the default one and a name empty, and one with an opaque field, from
    /// its base; while a mesh, opaque as a whole without a base, and a parent, which holds an
    /// entity, have none.
    #[test]
    fn the_engines_components_defaults_make_them() {
        let mut world = World::default();
        for registered in [
            world.register::<Transform>(),
            world.register::<Sprite>(),
            world.register::<MeshDraw>(),
            world.register::<crate::Name>(),
            world.register::<Everything>(),
        ] {
            registered.expect("registered");
        }
        let default = |name| {
            let default = world.default_of(name);
            default.unwrap_or_else(|error| panic!("{name}: {error}"))
        };
        assert!(same(&default("Transform"), &Transform::IDENTITY.to_value()));
        assert!(same(&default("Sprite"), &Sprite::default().to_value()));
        let unnamed = crate::Name(String::new()).to_value();
        assert!(same(&default("Name"), &unnamed));
        let Value::Struct(fields) = default("Everything") else {
            panic!("everything is a struct");
        };
        assert_eq!(fields.last(), Some(&("handle".into(), Value::Opaque)));
        for (name, code, at) in [
            ("MeshDraw", "reflect.opaque", "`MeshDraw`"),
            ("Parent", "reflect.no_default", "`Parent.0`"),
        ] {
            let error = world.default_of(name).expect_err(name);
            assert_eq!(error.code(), code, "{error}");
            let says = format!("can't make a default {at}: ");
            assert!(error.to_string().starts_with(&says), "{error}");
        }
        let error = world.default_of("Inner").expect_err("a type of fields");
        assert_eq!(error.code(), "reflect.unknown_type");
    }

    #[test]
    fn an_error_keeps_its_cause_as_it_says_where() {
        let cause = std::io::Error::other("the disk is full");
        let error = Error::new("game.unsaved", "it isn't saved").with_source(cause);
        let error = within(within(error, "slot"), "saves[2]");
        assert_eq!(error.code(), "game.unsaved");
        assert_eq!(
            format!("{error:#}"),
            "at `saves[2].slot`: it isn't saved: the disk is full"
        );
    }
}
