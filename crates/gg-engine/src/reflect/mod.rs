//! Reflection: the engine's runtime knowledge of its types' fields.
//!
//! The inspector, scenes,
//! operations, snapshots, replication, Luau's bindings and the tool API reach components by name
//! through it, without Rust types. A type describes itself once to [`Types`]. Its values go in and
//! out as [`Value`]s, checked against that description. Derive [`Reflect`], or
//! `#[derive(Clone, Component)]` for components.

mod impls;
mod path;
#[cfg(test)]
pub(crate) mod tests;

use std::any::{TypeId, type_name};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use gg_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};

/// `gg-core`'s error and result types.
pub use gg_core::{Error, Result};
/// Describes a struct or enum to reflection.
pub use gg_macros::Reflect;

pub use crate::ecs::Digest;
use crate::{Entity, Font, Model, Prefab, Texture};
pub(crate) use path::{Resolved, reach, reach_mut, resolve};

/// A type's, field's or variant's name. Borrowed for Rust's types; owned when read from text,
/// copied or declared by Luau scripts.
pub type Ident = Cow<'static, str>;

/// A value of a reflected type, without its Rust type. The field's [`Type`] says which variant.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    /// Either variant sets an integer field of either sign that it fits.
    UInt(u64),
    /// Kept apart from `F64` so an `f32`'s bits come back unchanged.
    F32(f32),
    F64(f64),
    String(String),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    /// Kept apart from `Vec3`, as `F64` is from `F32`: an entity's translation keeps its
    /// precision far from the origin.
    DVec3(DVec3),
    /// A unit quaternion.
    Quat(Quat),
    /// The entity's handle in this process. Scenes, recordings, the tool API and replication map
    /// it to its persistent ID where values cross into them.
    Entity(Entity),
    /// Only the renderer hands out texture handles, so no value makes an index shaders sample
    /// unchecked. Where values cross out of the process (scenes, recordings, snapshots, the tool
    /// API) the world's [assets](crate::App::root) map it to an asset ID, else a path.
    Texture(Texture),
    /// Only adding a model hands out its handle. Named like a texture where values cross out.
    Model(Model),
    /// Only adding a font hands out its handle. Named like a texture.
    Font(Font),
    /// Only adding a prefab hands out its handle. Named like a texture.
    Prefab(Prefab),
    Option(Option<Box<Value>>),
    /// A list's or array's elements.
    List(Vec<Value>),
    /// A struct's fields by name, each once. A tuple struct's are named `0`, `1`, and so on.
    Struct(Vec<(Ident, Value)>),
    /// An enum's variant by name, with its fields as a struct's.
    Variant(Ident, Vec<(Ident, Value)>),
    /// What reflection doesn't read, such as a [`MeshDraw`](crate::MeshDraw). An opaque field
    /// reads as this, and set to it keeps what it has, so a value read whole sets back whole.
    /// Anywhere else it's `reflect.wrong_kind`.
    Opaque,
}

/// What a field holds, as reflection describes it.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Bool,
    /// An integer of `bits`, 8 to 64. None varies by platform, so values mean the same wherever
    /// saved or sent.
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
    DVec3,
    Quat,
    Entity,
    /// Named by its path. No default, so a field that may show none, as a sprite's does, holds an
    /// `Option`.
    Texture,
    /// Named like a texture. No default.
    Model,
    /// Named like a texture. No default: a field that may name none, as a text's does for the
    /// engine's font, holds an `Option`.
    Font,
    /// Named like a texture. No default.
    Prefab,
    Option(Box<Type>),
    List(Box<Type>),
    /// This many elements, always.
    Array(Box<Type>, u32),
    /// A struct or enum, described by [`Types`].
    Named(TypeIndex),
    /// A field marked `#[reflect(opaque)]`: kept, not read.
    Opaque,
}

impl Type {
    /// Whether values of the type are an opaque field's or type's. Reading one gives
    /// [`Value::Opaque`]; only that sets one, keeping it.
    pub fn is_opaque(&self, types: &Types) -> bool {
        match self {
            Self::Opaque => true,
            Self::Named(index) => matches!(types.info(*index).shape, Shape::Opaque),
            _ => false,
        }
    }
}

/// A named type's place among [`Types`]', in description order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeIndex(u32);

/// A struct or enum, as reflection describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeInfo {
    /// Its identifier, or `#[reflect(name = "...")]`. No other type has it.
    pub name: Ident,
    /// Its doc comment, lines joined into paragraphs.
    pub doc: Cow<'static, str>,
    /// Its fields or variants, unless opaque.
    pub shape: Shape,
}

/// What a named type holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// Fields in declaration order. A tuple struct's are named `0`, `1`, and so on.
    Struct { fields: Vec<Field>, tuple: bool },
    /// Variants in declaration order.
    Enum(Vec<Variant>),
    /// `#[reflect(opaque)]`: kept and listed, not read or set.
    Opaque,
}

/// A field of a struct or enum variant.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Its identifier, or its tuple index such as `0`: what paths and values name it by.
    pub name: Ident,
    pub ty: Type,
    pub meta: Meta,
    /// What it is unless something says otherwise, as in a component added: its description's
    /// (`#[reflect(default = ..)]`), else its type's under its bounds, as
    /// [`Types::field_default`] makes it. The field's, not its [`Meta`]'s, which reaches its parts
    /// and elements: a vector's default isn't its components'.
    pub default: Option<Value>,
}

/// An enum's variant.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: Ident,
    pub fields: Vec<Field>,
    /// Whether its fields are a tuple's, named `0`, `1`, and so on. Scenes write them by position.
    pub tuple: bool,
}

/// A field's hints, beyond its type.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    /// The least and most its numbers, its vectors' components and its elements may be. A value
    /// outside them is refused.
    pub min: Option<Bound>,
    pub max: Option<Bound>,
    pub show: Show,
    /// Its doc comment, lines joined into paragraphs.
    pub doc: Cow<'static, str>,
}

/// A field's `min` or `max` as written. Integers stay apart from floats to compare exactly. An
/// `f32` or vector component compares with it rounded to `f32`, so `max = 0.1` takes `0.1f32`, a
/// little more than 0.1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bound {
    Int(i128),
    Float(f64),
}

impl Bound {
    /// The nearest `f32`.
    pub(crate) fn as_f32(self) -> f32 {
        match self {
            Self::Int(n) => n as f32,
            Self::Float(x) => x as f32,
        }
    }

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

/// The named types (structs and enums) in description order. Each describes itself once, under a
/// name no other has. A field's type is described with the first type that has one.
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
    /// Describes `T`, a struct or enum, under `name`, returning a [`Type::Named`]. Called by
    /// [`Reflect::describe`]. `describe` describes its fields or variants and their types. A type
    /// already described isn't again, so a recursive type, such as
    /// `struct Node { kids: Vec<Node> }`, describes itself once.
    ///
    /// Fails with `reflect.name_taken` if another type has `name`; with `reflect.bad_default` if a
    /// field's [`default`](Field::default) is outside its bounds or opaque; and as `describe`
    /// does. Either way the types are left as they were, without `T` or any type described since.
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
        // Reserved first, so the fields' types find it and undoing it undoes everything after.
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
        // Every type described since too, now each has its shape: a default holding this type,
        // in a type it holds, was checked against its placeholder.
        let described = describe(self).and_then(|shape| {
            self.entries[index.0 as usize].info.shape = shape;
            let len = self.entries.len() as u32;
            (index.0..len).try_for_each(|i| self.check_defaults(TypeIndex(i)))
        });
        if let Err(error) = described {
            self.truncate(index.0 as usize);
            return Err(error);
        }
        Ok(Type::Named(index))
    }

    /// Forgets the types described since there were `len`.
    pub(crate) fn truncate(&mut self, len: usize) {
        for entry in self.entries.drain(len..) {
            self.by_name.remove(&entry.info.name);
            self.by_type.remove(&entry.type_id);
        }
    }

    /// Checks the defaults of `index`'s fields, and its variants', against their bounds like
    /// [`check`]. An opaque one is refused: what's opaque is kept, not made.
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

    /// The default of a value of `ty` in a field of hints `meta`, of which only the bounds count,
    /// for its numbers, vectors' components and elements. [`Meta::default`] gives none. A:
    ///
    /// - number is 0, or the bound nearest 0 if 0 is outside them, rounded to an integer of its
    ///   type inside them, or for an `f32` to the `f32` it's compared with;
    /// - bool is false; quaternion the identity; string empty; option `None`; list empty;
    /// - array is that many of its element's default;
    /// - struct is each field's [`field_default`](Self::field_default);
    /// - enum is its first variant's, as [`variant_default`](Self::variant_default);
    /// - opaque value is [`Value::Opaque`], which keeps what a component's
    ///   [`base`](crate::Component::base) gives.
    ///
    /// Fails with `reflect.no_default` for:
    ///
    /// - an entity: there's none to name;
    /// - a number whose bounds leave none of its type between them;
    /// - an enum without variants;
    /// - a type whose default holds one of its own without end. Only a hand-written description
    ///   gives one: a derived type holds itself only in an option or a list.
    ///
    /// The path to where it failed leads the message.
    pub fn default_value(&self, ty: &Type, meta: &Meta) -> Result<Value> {
        self.default_inside(ty, meta, &mut Vec::new())
    }

    /// The default of `field`: its description's, else its type's under its bounds. Fails like
    /// [`default_value`](Self::default_value).
    pub fn field_default(&self, field: &Field) -> Result<Value> {
        self.field_inside(field, &mut Vec::new())
    }

    /// `variant` with each field's [`field_default`](Self::field_default), as switching an enum to
    /// it makes it. Fails like `field_default`, at the failing field.
    pub fn variant_default(&self, variant: &Variant) -> Result<Value> {
        self.variant_inside(variant, &mut Vec::new())
    }

    /// [`default_value`](Self::default_value) inside the defaults of `named`, outside in: a cycle
    /// is found as a type already held, not by depth.
    fn default_inside(&self, ty: &Type, meta: &Meta, named: &mut Vec<TypeIndex>) -> Result<Value> {
        let none = || no_number(self, ty, meta);
        // Finite, as `check` takes only finite numbers: a bound beyond `f32`'s range rounds to
        // infinity.
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
            Type::DVec3 => {
                let x = nearest_zero(meta, Bound::as_f64, Bound::as_f64);
                Value::DVec3(DVec3::splat(x.filter(|x| x.is_finite()).ok_or_else(none)?))
            }
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
            Type::Model => {
                return Err(Error::new(
                    "reflect.no_default",
                    "a model has no default, since only a model loaded from a file has a name to \
                     save",
                ));
            }
            Type::Font => {
                return Err(Error::new(
                    "reflect.no_default",
                    "a font has no default, since only a font loaded from a file has a name to \
                     save; a field that may name none is an `Option<Font>`, whose default is \
                     `None`",
                ));
            }
            Type::Prefab => {
                return Err(Error::new(
                    "reflect.no_default",
                    "a prefab has no default, since only a prefab loaded from a file has a name to \
                     save",
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
    /// Types another process described, as the tool API's `types` gives them: name and doc, in
    /// order. Opaque until [`info_mut`](Self::info_mut) gives each its shape, whose
    /// [`Type::Named`]s [`find`](Self::find) finds among them. Values are read, drawn and written
    /// by them, though no Rust type is any of them. A repeated name is skipped.
    pub(crate) fn described(names: impl IntoIterator<Item = (Ident, Cow<'static, str>)>) -> Self {
        /// Stands for the Rust type of each, of which there's none.
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
    /// To describe a type of [`described`](Self::described)'s.
    pub(crate) fn info_mut(&mut self, index: TypeIndex) -> &mut TypeInfo {
        &mut self.entries[index.0 as usize].info
    }

    /// Describes `info`, a script's struct, which no Rust type is. The caller has checked no type
    /// has its name.
    pub(crate) fn declare(&mut self, info: TypeInfo) -> TypeIndex {
        debug_assert!(
            !self.by_name.contains_key(&info.name),
            "a declared type's name is its own"
        );
        let index = TypeIndex(u32::try_from(self.entries.len()).expect("fewer than 2^32 types"));
        self.by_name.insert(info.name.clone(), index);
        self.entries.push(Entry {
            info,
            type_id: TypeId::of::<Declared>(),
            rust: "a type a script declared",
        });
        index
    }

    /// Describes the struct a script declared at `index` again, its fields changed. The caller has
    /// checked `info` has its name.
    pub(crate) fn redeclare(&mut self, index: TypeIndex, info: TypeInfo) {
        let entry = &mut self.entries[index.0 as usize];
        debug_assert!(
            entry.type_id == TypeId::of::<Declared>() && entry.info.name == info.name,
            "a declared type is declared again under its name"
        );
        entry.info = info;
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

    /// The Rust type `index` describes, and its name for errors.
    pub(crate) fn rust_type(&self, index: TypeIndex) -> (TypeId, &'static str) {
        let entry = &self.entries[index.0 as usize];
        (entry.type_id, entry.rust)
    }

    /// Every type, in description order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (TypeIndex, &TypeInfo)> {
        let entries = self.entries.iter().enumerate();
        entries.map(|(i, entry)| (TypeIndex(i as u32), &entry.info))
    }

    /// Whether `other` has described the same Rust types, and declared the same script types, in
    /// the same order, so its indices name the same types.
    pub(crate) fn same(&self, other: &Self) -> bool {
        let same = |ours: &Entry, theirs: &Entry| {
            ours.type_id == theirs.type_id
                && (ours.type_id != TypeId::of::<Declared>() || ours.info == theirs.info)
        };
        let theirs = &other.entries;
        self.entries.len() == theirs.len()
            && self.entries.iter().zip(theirs).all(|(a, b)| same(a, b))
    }
}

/// Stands for the Rust type of each type a script declares, of which there's none.
struct Declared;

/// A type reflection reads and sets without its Rust type: it describes itself to [`Types`].
///
/// Structs and enums derive it with `#[derive(Reflect)]`, components with
/// `#[derive(Clone, Component)]`. The derive takes each field's type's description and its doc
/// comment as its doc. Hints in `#[reflect(...)]`:
///
/// - On a type:
///   - `name = "..."` replaces its identifier, as two types of one name need.
///   - `opaque`, on a `Clone` type, keeps its values unread, as for a handle no value may make.
/// - On a field:
///   - `min = ..`, `max = ..` bound its numbers and its vectors' components, in its own unit,
///     such as radians. A value outside them is refused.
///   - `default = ..` is its [`default`](Field::default): an expression of its type in its own
///     unit, run as the type registers. Its bounds must take it, and a field of a type opaque as
///     a whole takes none (`reflect.bad_default`). Without it, its type's under its bounds,
///     per [`Types::default_value`].
///   - `degrees` shows an `f32` of radians, or radians a second, in degrees.
///   - `color` shows a `Vec3` or `Vec4` as a linear color, alpha not multiplied in.
///   - `opaque` keeps a field of a `Clone` type that isn't reflected. It reads as
///     [`Value::Opaque`] and takes no other hint.
///   - `hashed_when_set`, beside `default = ..`, on a component's own integer or bool field: the
///     world's hash reads it, after its index, only while it isn't its default. A field added to
///     a component then leaves the hash of a world that doesn't set it unchanged, as a reference
///     hash checks. The derive refuses it elsewhere: in a nested type, a field left out can't
///     be told from the words that follow, as a list's; and a float's default compares equal to
///     its other zero (0.0 to -0.0), which the hash tells apart.
///
/// Each hint is given once. A variant takes none, nor does a field of an opaque type.
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
/// Nor does an unknown hint, one that doesn't suit its field, one given twice or beside `opaque`,
/// or one on a variant or inside an opaque type:
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
/// Implemented by hand, `describe` calls [`Types::add`] for its own type (a struct or enum) only,
/// and the rest keep to its description:
///
/// - `with_value` checks the value's kinds, fields and variants, and makes it whole before
///   anything changes, keeping what's opaque.
/// - `digest` writes what `to_value` reads.
/// - `entities_mut` reaches the entities `to_value` reads.
#[diagnostic::on_unimplemented(
    message = "`{Self}` isn't reflected",
    note = "a type of fields describes itself with `#[derive(gg_engine::Reflect)]`, and a \
            component with `#[derive(Clone, gg_engine::Component)]`; a field of a type that isn't \
            reflected can be kept with `#[reflect(opaque)]`"
)]
pub trait Reflect: std::any::Any + Send + Sync + Swap {
    /// Adds the type's description to `types` if it's a struct or enum.
    fn describe(types: &mut Types) -> Result<Type>
    where
        Self: Sized;

    /// Its value in its description's shape. What's opaque reads as [`Value::Opaque`].
    fn to_value(&self) -> Value;

    /// One made from `value`. Fails with `reflect.opaque` where it would make something opaque,
    /// and like [`with_value`](Self::with_value).
    fn from_value(value: &Value) -> Result<Self>
    where
        Self: Sized;

    /// One like this with `value`'s parts, keeping its own opaque ones. A list's are kept by
    /// element:
    ///
    /// - Set to a list of another length, the elements alike (by value) at its start and end keep
    ///   their own, so an element taken out or put in moves no other's to the wrong one.
    /// - Set to a list of the same length, each keeps its place's.
    ///
    /// Only values tell elements apart: no set says which it took out, moved or changed. So
    /// swapped elements of a same-length list trade what's opaque; elements alike keep theirs
    /// first to last; and an element taken out in the same set as one after it changes (`[a, b,
    /// c]` set to `[a, c2]`) leaves its own to the changed one, whose own is gone.
    ///
    /// A variant switched to, an option's value that was `None`, and a list's new elements are
    /// made by [`from_value`](Self::from_value): the opaque state of the variant left or the
    /// element taken out is gone. An operation's undo restores it from the component it keeps.
    ///
    /// Fails, at the failing part, with:
    ///
    /// - `reflect.wrong_kind`: a value of another kind;
    /// - `reflect.out_of_range`: an integer that doesn't fit;
    /// - `reflect.not_normalized`: a quaternion that isn't a unit;
    /// - `reflect.missing_field`, `reflect.no_field`, `reflect.duplicate_field`: a struct's
    ///   fields aren't each given once;
    /// - `reflect.no_variant`: a variant the enum hasn't;
    /// - `reflect.wrong_length`: an array's elements;
    /// - `reflect.opaque`: something opaque set to anything but [`Value::Opaque`].
    fn with_value(&self, value: &Value) -> Result<Self>
    where
        Self: Sized;

    /// Sets this to [`with_value`](Self::with_value)'s result, all or nothing. Fields' bounds are
    /// [`World::set_field`](crate::World::set_field)'s to check.
    fn set_value(&mut self, value: &Value) -> Result<()>;

    /// Its part `index`: a struct's field, a vector's component, or a list's or array's element.
    /// `None` past its parts, for what's opaque, and for enums, options and quaternions, which are
    /// set whole.
    fn part(&self, index: u32) -> Option<&dyn Reflect>;

    /// [`part`](Self::part), to change.
    fn part_mut(&mut self, index: u32) -> Option<&mut dyn Reflect>;

    /// Writes it into `to` for the world's hash: what [`to_value`](Self::to_value) reads, in its
    /// order, without what's opaque.
    ///
    /// - A float goes by its bits, every NaN as one.
    /// - An entity goes by its ID, or 0 if it isn't alive.
    /// - A variant goes by its index, then its fields.
    /// - An option, list and string go with what tells where they end, so values moved from one
    ///   field to the next give another hash.
    ///
    /// By hand, write its parts' digests as `to_value` reads them, and a variant's index as a
    /// `u32`: `<u32 as Reflect>::digest(&index, to)`.
    fn digest(&self, to: &mut Digest<'_>);

    /// Calls `each` with each entity it holds, to change: those [`to_value`](Self::to_value) reads
    /// as [`Value::Entity`], in its order. Code repoints references this way, as a duplicate
    /// points its copies' at each other. Unlike setting a value again, every other bit stays and
    /// nothing is re-checked, so what code stored that a set would refuse survives.
    fn entities_mut(&mut self, each: &mut dyn FnMut(&mut Entity));
}

/// Swaps a value with another of its type through `dyn Reflect`, which no generic swap reaches:
/// an operation's undo puts a field back whole, opaque state too. Every type has it.
#[doc(hidden)]
pub trait Swap {
    /// Swaps this with `other` if it's of this type. Returns whether it was.
    fn swap_reflected(&mut self, other: &mut dyn std::any::Any) -> bool;
}

impl<T: std::any::Any> Swap for T {
    fn swap_reflected(&mut self, other: &mut dyn std::any::Any) -> bool {
        other
            .downcast_mut::<T>()
            .map(|other| std::mem::swap(self, other))
            .is_some()
    }
}

/// Types whose fields `#[reflect(min = ..)]` and `#[reflect(max = ..)]` bound: numbers, vectors,
/// and options, lists and arrays of them (the bounds apply to elements).
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

/// The fields of the struct `value`, in the order of `names`, each given once; for the derives.
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
    pick_into(given, &names, &mut found)?;
    Ok(found.map(|value| value.expect("every field is found")))
}

/// [`pick`] into `found`, whose every place is filled.
pub(crate) fn pick_into<'v, N: AsRef<str>>(
    given: &'v [(Ident, Value)],
    names: &[N],
    found: &mut [Option<&'v Value>],
) -> Result<()> {
    for (name, value) in given {
        let Some(i) = names.iter().position(|known| known.as_ref() == name) else {
            let names = names.iter().map(AsRef::as_ref);
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
            format!(
                "field `{}` isn't given, but each field is",
                names[i].as_ref()
            ),
        ));
    }
    Ok(())
}

/// The variant `value` is, by name, and its fields; for the derives.
#[doc(hidden)]
pub fn variant(value: &Value) -> Result<(&str, &[(Ident, Value)])> {
    match value {
        Value::Variant(name, given) => Ok((name, given)),
        other => Err(wrong_kind("Variant", other)),
    }
}

/// The error that an enum of variants `names` lacks `name`; for the derives.
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

/// Fails: reflection doesn't make what's opaque. The derives make any opaque field with it.
#[doc(hidden)]
pub fn make_opaque<T>(value: &Value) -> Result<T> {
    let _ = value;
    Err(Error::new(
        "reflect.opaque",
        "it's opaque: reflection keeps one, but doesn't make one",
    ))
}

/// The opaque `current` as `value` sets it: only [`Value::Opaque`] does, keeping it. For the
/// derives.
#[doc(hidden)]
pub fn keep<T: Clone>(current: &T, value: &Value) -> Result<T> {
    match value {
        Value::Opaque => Ok(current.clone()),
        other => Err(not_opaque(other)),
    }
}

pub(crate) fn not_opaque(value: &Value) -> Error {
    Error::new(
        "reflect.opaque",
        format!(
            "it's opaque, so only `Opaque` sets it, which keeps it as it is; got {}",
            kind(value)
        ),
    )
}

/// `error` from part `step` of a value (`rotation`, `[1]`) as from the whole; for the derives.
/// Its message leads with the path, as `` at `rect[1].y`: ``.
#[doc(hidden)]
pub fn within(error: Error, step: &str) -> Error {
    let (path, what) = located(&error);
    let message = format!("at `{}`: {what}", join(step, &path));
    error.with_message(message)
}

/// Where in a value `error` happened, from [`within`]'s prefix, and the rest of its message.
pub(crate) fn located(error: &Error) -> (String, String) {
    let message = error.to_string();
    if let Some(rest) = message.strip_prefix("at `")
        && let Some((path, what)) = rest.split_once("`: ")
    {
        return (path.to_owned(), what.to_owned());
    }
    (String::new(), message)
}

/// Path `inner` from where `outer` leads: `rect` and `[1].y`, or `translation` and `x`.
pub(crate) fn join(outer: &str, inner: &str) -> String {
    if outer.is_empty() {
        inner.to_owned()
    } else if inner.is_empty() || inner.starts_with('[') {
        format!("{outer}{inner}")
    } else {
        format!("{outer}.{inner}")
    }
}

/// The error that `value` isn't the kind `expected`, named as [`Value`]'s variants are.
pub(crate) fn wrong_kind(expected: &str, value: &Value) -> Error {
    Error::new(
        "reflect.wrong_kind",
        format!("expected {expected}, got {}", kind(value)),
    )
}

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
        Value::DVec3(_) => "DVec3",
        Value::Quat(_) => "Quat",
        Value::Entity(_) => "Entity",
        Value::Texture(_) => "Texture",
        Value::Model(_) => "Model",
        Value::Font(_) => "Font",
        Value::Prefab(_) => "Prefab",
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

/// Checks `value` of a field of type `ty` and hints `meta` against the bounds only the
/// description knows: its numbers', and its fields' and elements', which inherit the field's. A
/// non-finite number is outside every range. The impls check kinds as they set.
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
        (Type::Vec2, Value::Vec2(v)) => {
            components(meta, v.as_ref().iter().map(|&x| Number::F32(x)))
        }
        (Type::Vec3, Value::Vec3(v)) => {
            components(meta, v.as_ref().iter().map(|&x| Number::F32(x)))
        }
        (Type::Vec4, Value::Vec4(v)) => {
            components(meta, v.as_ref().iter().map(|&x| Number::F32(x)))
        }
        (Type::DVec3, Value::DVec3(v)) => {
            components(meta, v.as_ref().iter().map(|&x| Number::Float(x)))
        }
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

fn components(meta: &Meta, components: impl Iterator<Item = Number>) -> Result<()> {
    for (x, name) in components.zip(COMPONENTS) {
        in_range(meta, x).map_err(|error| within(error, name))?;
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

/// Whether `number` is below `bound`: integers compared exactly, an `f32` with the bound rounded
/// to one.
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

/// Checks `number` against `meta`, which has bounds.
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

/// `meta`'s bounds as a phrase, such as `from 0 to 1`.
fn range(meta: &Meta) -> String {
    match (meta.min, meta.max) {
        (Some(min), Some(max)) => format!("from {min} to {max}"),
        (Some(min), None) => format!("of {min} and up"),
        (None, Some(max)) => format!("up to {max}"),
        (None, None) => unreachable!("only bounds leave a number out"),
    }
}

/// The number nearest 0 within `meta`'s bounds, the min as `least` rounds it and the max as `most`
/// does: 0, or the bound nearest it if 0 is outside; `None` if they leave none.
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

/// A vector's component names, in order, as paths, dumps, differences and scenes' errors name
/// them, and scenes' errors a quaternion's.
pub(crate) const COMPONENTS: [&str; 4] = ["x", "y", "z", "w"];

/// What a value of `ty` is, as a phrase, such as `an f32` or `a struct, Transform`.
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
        Type::DVec3 => "a DVec3".to_owned(),
        Type::Quat => "a quaternion".to_owned(),
        Type::Entity => "an entity".to_owned(),
        Type::Texture => "a texture".to_owned(),
        Type::Model => "a model".to_owned(),
        Type::Font => "a font".to_owned(),
        Type::Prefab => "a prefab".to_owned(),
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
