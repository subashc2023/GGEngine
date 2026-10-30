//! Reflection: what the engine knows of its types' fields while it runs, so that the inspector,
//! scenes and operations, and the tool API to come, reach components by name, without their Rust
//! types. A type describes itself once to [`Types`], by name; its values go in and out as
//! [`Value`]s, checked against that description. Types derive [`Reflect`], with
//! `#[derive(Reflect)]`, or `#[derive(Clone, Component)]` for components.

mod impls;
mod path;
#[cfg(test)]
pub(crate) mod tests;

use std::any::{TypeId, type_name};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use gg_core::math::{Quat, Vec2, Vec3, Vec4};

/// The error and result types reflection returns, `gg-core`'s.
pub use gg_core::{Error, Result};
/// Describes a struct or an enum to reflection: [`Reflect`] says how.
pub use gg_macros::Reflect;

pub use crate::ecs::Digest;
use crate::{Entity, Texture};
pub(crate) use path::{reach, reach_mut, reach_opaque, resolve};

/// An identifier: a type's, a field's or a variant's name, borrowed for Rust's types, and owned
/// where it's read from text, as a recording's values are, or copied, as operations copy theirs,
/// and for the types Luau scripts will declare.
pub type Ident = Cow<'static, str>;

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
    /// The entity's handle in this process: scenes and recordings map it to its persistent ID
    /// where values cross into them, as the tool API and replication will.
    Entity(Entity),
    /// A texture's handle, which the renderer alone hands out, so that no value makes one it
    /// didn't: scenes, recordings, snapshots and the tool API name it by its path, which the
    /// world's [assets](crate::App::root) map it to where values cross into them.
    Texture(Texture),
    Option(Option<Box<Value>>),
    /// A list's elements, or an array's.
    List(Vec<Value>),
    /// A struct's fields by name, each once: a tuple struct's are named `0`, `1` and so on.
    Struct(Vec<(Ident, Value)>),
    /// An enum's variant by name, with its fields, as a struct's.
    Variant(Ident, Vec<(Ident, Value)>),
    /// What reflection doesn't read, such as a [`MeshDraw`](crate::MeshDraw): an opaque field reads
    /// as this, and set to it keeps what it has, so that what's opaque doesn't keep a value read
    /// whole from setting back whole. Anywhere else it's `reflect.wrong_kind`.
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
    /// A texture, named by its path: it has no default, so a component that shows none, as a
    /// sprite does by default, holds one in an `Option`.
    Texture,
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
    pub name: Ident,
    /// Its doc comment, its lines joined into paragraphs.
    pub doc: Cow<'static, str>,
    /// What it holds: its fields or its variants, unless it's opaque.
    pub shape: Shape,
}

/// What a named type holds, as reflection describes it: a struct's fields, an enum's variants, or
/// nothing it reads.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// Its fields, in the order it declares them; a tuple struct's are named `0`, `1` and so on.
    Struct { fields: Vec<Field>, tuple: bool },
    /// Its variants, in the order it declares them.
    Enum(Vec<Variant>),
    /// Marked `#[reflect(opaque)]`: kept and listed, but not read or set.
    Opaque,
}

/// A field of a struct or of an enum's variant, as reflection describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Its identifier, or its index in a tuple, such as `0`: what paths and values name it by.
    pub name: Ident,
    pub ty: Type,
    pub meta: Meta,
    /// What it is unless something says otherwise, as in a component added, if its description
    /// gives it, as `#[reflect(default = ..)]` does: otherwise its type's, under its bounds, as
    /// [`Types::field_default`] makes it. The field's, not its [`Meta`]'s, which reaches its parts
    /// and elements: a vector's default isn't its components'.
    pub default: Option<Value>,
}

/// An enum's variant, as reflection describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: Ident,
    pub fields: Vec<Field>,
    /// Whether its fields are a tuple's, named `0`, `1` and so on, which scenes write by position.
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
    pub doc: Cow<'static, str>,
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
    by_name: HashMap<Ident, TypeIndex>,
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
        name: Ident,
        doc: Cow<'static, str>,
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
        let checked = |fields: &[Field], variant: Option<&Ident>| -> Result<()> {
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
            Type::Texture => {
                return Err(Error::new(
                    "reflect.no_default",
                    "a texture has no default, since only a texture loaded from a file has a path \
                     to save; a field that may show none is an `Option<Texture>`, whose default is \
                     `None`",
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
    ) -> Result<Vec<(Ident, Value)>> {
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

    #[cfg(feature = "dev")]
    /// Types another process described, as the tool API's `types` gives them, each by its name and
    /// doc, in their order, opaque until [`info_mut`](Self::info_mut) gives each its shape, whose
    /// [`Type::Named`]s [`find`](Self::find) finds among them: values are read, drawn and written
    /// by them, but no Rust type is any of them. A name given again is skipped.
    pub(crate) fn described(names: impl IntoIterator<Item = (Ident, Cow<'static, str>)>) -> Self {
        /// What stands for the Rust type of each, of which there's none.
        struct Described;

        let mut types = Self::default();
        for (name, doc) in names {
            if types.by_name.contains_key(&name) {
                continue;
            }
            let index = TypeIndex(u32::try_from(types.entries.len()).expect("fewer than 2^32"));
            types.by_name.insert(name.clone(), index);
            types.entries.push(Entry {
                info: TypeInfo {
                    name,
                    doc,
                    shape: Shape::Opaque,
                },
                type_id: TypeId::of::<Described>(),
                rust: "a type another process described",
            });
        }
        types
    }

    #[cfg(feature = "dev")]
    /// The description of the type `index` names, to describe a type of
    /// [`described`](Self::described)'s.
    pub(crate) fn info_mut(&mut self, index: TypeIndex) -> &mut TypeInfo {
        &mut self.entries[index.0 as usize].info
    }

    /// The type named `name`, if there's one.
    pub fn find(&self, name: &str) -> Option<TypeIndex> {
        self.by_name.get(name).copied()
    }

    /// The description of the type `index` names.
    ///
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
/// Nor does a hint it doesn't know, one that doesn't suit its field, one given twice or beside
/// `opaque`, or one on a variant or inside an opaque type:
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
/// Implemented by hand, `describe` calls [`Types::add`] for its own type, a struct or an enum,
/// never another's, and the rest keep to its description: `with_value` checks the value's kinds,
/// fields and variants, and makes what it's given whole before anything changes, keeping what's
/// opaque; and `digest` writes what `to_value` reads.
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

    /// Its value, in the shape its description gives: what's opaque reads as [`Value::Opaque`].
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

    /// Its part `index`, as [`part`](Self::part) finds it, to change.
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
    given: &'v [(Ident, Value)],
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
pub fn variant(value: &Value) -> Result<(&str, &[(Ident, Value)])> {
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
        Value::Texture(_) => "Texture",
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

fn check_fields(types: &Types, fields: &[Field], given: &[(Ident, Value)]) -> Result<()> {
    for (name, value) in given {
        if let Some(field) = fields.iter().find(|field| field.name == *name) {
            check(types, &field.ty, &field.meta, value).map_err(|error| within(error, name))?;
        }
    }
    Ok(())
}

/// Checks each of a vector's `components` against `meta`'s bounds.
fn components(meta: &Meta, components: &[f32]) -> Result<()> {
    for (&x, name) in components.iter().zip(COMPONENTS) {
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

/// The names of a vector's components, in order: what paths, dumps, differences and scenes' errors
/// name them by, as scenes' errors name a quaternion's too.
pub(crate) const COMPONENTS: [&str; 4] = ["x", "y", "z", "w"];

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
        Type::Texture => "a texture".to_owned(),
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
