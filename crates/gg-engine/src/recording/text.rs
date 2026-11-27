//! The text of a recording's values: reflection's [`Value`]s on one line, every part tagged by
//! kind so they read back without their types. Floats keep their bits, entities go by ID, and
//! textures, models and fonts by name, ID or else path. The engine writes it; `ron` only reads.

use std::fmt::Write as _;
use std::sync::Arc;

use gg_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use ron::Options;
use ron::extensions::Extensions;
use serde::Deserialize;
use serde::de::{self, Deserializer};

use crate::assets::Asset;
use crate::ecs::{MAX_DEPTH, float, quote};
use crate::luau::ModText;
use crate::reflect::{Ident, Value};
use crate::{Entity, EntityId, World};

/// Whether `value` nests more than `levels` levels, looking no deeper. A struct, variant, list or
/// option is a level above what it holds; anything else is one level.
pub(crate) fn deeper_than(value: &Value, levels: u32) -> bool {
    let Some(below) = levels.checked_sub(1) else {
        return true;
    };
    match value {
        Value::Option(Some(held)) => deeper_than(held, below),
        Value::List(elements) => elements.iter().any(|element| deeper_than(element, below)),
        Value::Struct(fields) | Value::Variant(_, fields) => {
            fields.iter().any(|(_, field)| deeper_than(field, below))
        }
        _ => false,
    }
}

/// Writes `value` on one line, each part tagged by kind: `Variant("Swarm", [("0",
/// UInt(10000))])`.
///
/// - A float is the shortest text reading back to its bits, except a NaN: it keeps its sign, not
///   its payload, which arithmetic leaves to the platform.
/// - A string is quoted by a scene's escapes, leaving no line break.
/// - An entity is its ID in `world`, or `Dead` if not alive there.
/// - A texture is its name in `world` (ID or else path), or `Opaque` if it has none, which
///   nothing sets.
pub(crate) fn write(out: &mut String, value: &Value, world: &World) {
    put(out, value, world, false);
}

/// Like [`write()`], but every NaN is one, as the world's hash takes them: the same world writes
/// the same text on every platform.
pub(crate) fn write_folded(out: &mut String, value: &Value, world: &World) {
    put(out, value, world, true);
}

fn put(out: &mut String, value: &Value, world: &World, fold: bool) {
    let float32 = |out: &mut String, x: f32| {
        let negative = x.is_sign_negative() && !(fold && x.is_nan());
        float(out, x, x.is_nan(), negative, x.is_finite());
    };
    let float64 = |out: &mut String, x: f64| {
        let negative = x.is_sign_negative() && !(fold && x.is_nan());
        float(out, x, x.is_nan(), negative, x.is_finite());
    };
    let floats = |out: &mut String, kind: &str, xs: &[f32]| {
        out.push_str(kind);
        out.push('(');
        for (i, &x) in xs.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            float32(out, x);
        }
        out.push(')');
    };
    let fields = |out: &mut String, fields: &[(Ident, Value)]| {
        out.push('[');
        for (i, (name, value)) in fields.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push('(');
            quote(out, name);
            out.push_str(", ");
            put(out, value, world, fold);
            out.push(')');
        }
        out.push(']');
    };
    match value {
        Value::Bool(b) => write!(out, "Bool({b})").expect("a string takes any text"),
        Value::Int(n) => write!(out, "Int({n})").expect("a string takes any text"),
        Value::UInt(n) => write!(out, "UInt({n})").expect("a string takes any text"),
        &Value::F32(x) => {
            out.push_str("F32(");
            float32(out, x);
            out.push(')');
        }
        &Value::F64(x) => {
            out.push_str("F64(");
            float64(out, x);
            out.push(')');
        }
        Value::String(text) => {
            out.push_str("String(");
            quote(out, text);
            out.push(')');
        }
        Value::Vec2(v) => floats(out, "Vec2", v.as_ref()),
        Value::Vec3(v) => floats(out, "Vec3", v.as_ref()),
        Value::Vec4(v) => floats(out, "Vec4", v.as_ref()),
        Value::DVec3(v) => {
            out.push_str("DVec3(");
            for (i, &x) in v.as_ref().iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                float64(out, x);
            }
            out.push(')');
        }
        Value::Quat(q) => floats(out, "Quat", &q.to_array()),
        &Value::Entity(entity) => match world.id(entity) {
            Some(id) => write!(out, "Entity(\"{id}\")").expect("a string takes any text"),
            None => out.push_str("Dead"),
        },
        &Value::Texture(texture) => match world.assets().name(texture) {
            Some(name) => {
                out.push_str("Texture(");
                quote(out, name);
                out.push(')');
            }
            None => out.push_str("Opaque"),
        },
        &Value::Model(model) => {
            out.push_str("Model(");
            quote(out, world.assets().model_name(model));
            out.push(')');
        }
        &Value::Font(font) => {
            out.push_str("Font(");
            quote(out, world.assets().font_name(font));
            out.push(')');
        }
        &Value::Prefab(prefab) => {
            out.push_str("Prefab(");
            quote(out, world.assets().prefab_name(prefab));
            out.push(')');
        }
        Value::Option(None) => out.push_str("None"),
        Value::Option(Some(value)) => {
            out.push_str("Some(");
            put(out, value, world, fold);
            out.push(')');
        }
        Value::List(items) => {
            out.push_str("List([");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                put(out, item, world, fold);
            }
            out.push_str("])");
        }
        Value::Struct(given) => {
            out.push_str("Struct(");
            fields(out, given);
            out.push(')');
        }
        Value::Variant(name, given) => {
            out.push_str("Variant(");
            quote(out, name);
            out.push_str(", ");
            fields(out, given);
            out.push(')');
        }
        Value::Opaque => out.push_str("Opaque"),
    }
}

/// Writes a texture's `name` and `path` (from
/// [`Assets::named_from`](crate::assets::Assets::named_from)) as a list of two strings:
/// `List([String("5f3c9a1e07b2d846"), String("assets/stone.png")])`. Snapshots', recordings' and a
/// lockstep host's `named` lines hold them so.
pub(crate) fn write_named(out: &mut String, name: &str, path: &str) {
    out.push_str("List([String(");
    quote(out, name);
    out.push_str("), String(");
    quote(out, path);
    out.push_str(")])");
}

/// Reads what [`write_named`] wrote, `text` starting at column `from`.
pub(crate) fn read_named(text: &str, from: usize) -> Option<(Arc<str>, Arc<str>)> {
    let Text::List(parts) = Text::parse(text, from).ok()? else {
        return None;
    };
    match <[Text; 2]>::try_from(parts).ok()? {
        [Text::String(name), Text::String(path)] => Some((name.into(), path.into())),
        _ => None,
    }
}

/// Writes a patch's `library` as a `patch` line has it: a string.
#[cfg(feature = "dev")]
pub(crate) fn write_patch(out: &mut String, library: &str) {
    out.push_str("String(");
    quote(out, library);
    out.push(')');
}

/// Reads what [`write_patch`] wrote, `text` starting at column `from`.
pub(crate) fn read_patch(text: &str, from: usize) -> Option<String> {
    match Text::parse(text, from).ok()? {
        Text::String(library) => Some(library),
        _ => None,
    }
}

/// Writes the mod `text` as a `mod` line has it: a list of its manifest's text, then each
/// script's path and text.
pub(crate) fn write_mod(out: &mut String, text: &ModText) {
    out.push_str("List([String(");
    quote(out, &text.manifest);
    out.push(')');
    for (path, script) in &text.scripts {
        for string in [path, script] {
            out.push_str(", String(");
            quote(out, string);
            out.push(')');
        }
    }
    out.push_str("])");
}

/// Reads what [`write_mod`] wrote, `text` starting at column `from`.
pub(crate) fn read_mod(text: &str, from: usize) -> Option<ModText> {
    let Text::List(parts) = Text::parse(text, from).ok()? else {
        return None;
    };
    let mut strings = parts.into_iter().map(|part| match part {
        Text::String(string) => Some(string),
        _ => None,
    });
    let manifest = strings.next()??;
    let mut scripts = Vec::new();
    while let Some(path) = strings.next() {
        scripts.push((path?, strings.next()??));
    }
    Some(ModText { manifest, scripts })
}

/// A value as the text gives it: a [`Value`] whose entities are held by ID, as it's read before
/// the world the ID names an entity in, at the tick that applies it.
#[derive(Debug, Deserialize)]
pub(crate) enum Text {
    Bool(bool),
    Int(i64),
    UInt(u64),
    F32(f32),
    F64(f64),
    String(String),
    Vec2(f32, f32),
    Vec3(f32, f32, f32),
    Vec4(f32, f32, f32, f32),
    DVec3(f64, f64, f64),
    Quat(f32, f32, f32, f32),
    Entity(Id),
    /// An entity that wasn't alive when written.
    Dead,
    /// A texture, by its name or a path to its file.
    Texture(String),
    /// A model, named like a texture.
    Model(String),
    /// A font, named like a texture.
    Font(String),
    /// A prefab, named like a texture.
    Prefab(String),
    None,
    Some(Box<Text>),
    List(Vec<Text>),
    Struct(Vec<(String, Text)>),
    Variant(String, Vec<(String, Text)>),
    Opaque,
}

#[derive(Debug)]
pub(crate) struct Id(pub(crate) EntityId);

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let id = text
            .parse()
            .map_err(|error: Error| de::Error::custom(error))?;
        Ok(Self(id))
    }
}

impl Text {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "Bool",
            Self::Int(_) => "Int",
            Self::UInt(_) => "UInt",
            Self::F32(_) => "F32",
            Self::F64(_) => "F64",
            Self::String(_) => "String",
            Self::Vec2(..) => "Vec2",
            Self::Vec3(..) => "Vec3",
            Self::Vec4(..) => "Vec4",
            Self::DVec3(..) => "DVec3",
            Self::Quat(..) => "Quat",
            Self::Entity(_) => "Entity",
            Self::Dead => "Dead",
            Self::Texture(_) => "Texture",
            Self::Model(_) => "Model",
            Self::Font(_) => "Font",
            Self::Prefab(_) => "Prefab",
            Self::None => "None",
            Self::Some(_) => "Some",
            Self::List(_) => "List",
            Self::Struct(_) => "Struct",
            Self::Variant(..) => "Variant",
            Self::Opaque => "Opaque",
        }
    }

    /// Adds the textures, models, fonts and prefabs it holds to `names`, in order.
    pub(crate) fn assets<'t>(&'t self, names: &mut Vec<Asset<&'t str>>) {
        let fields = |given: &'t [(String, Text)], names: &mut Vec<Asset<&'t str>>| {
            for (_, text) in given {
                text.assets(names);
            }
        };
        match self {
            Self::Texture(name) => names.push(Asset::Texture(name)),
            Self::Model(name) => names.push(Asset::Model(name)),
            Self::Font(name) => names.push(Asset::Font(name)),
            Self::Prefab(name) => names.push(Asset::Prefab(name)),
            Self::Some(text) => text.assets(names),
            Self::List(items) => items.iter().for_each(|item| item.assets(names)),
            Self::Struct(given) | Self::Variant(_, given) => fields(given, names),
            _ => {}
        }
    }

    /// Reads all of `text`, starting at column `from`, as one value. It may nest as deep as a
    /// scene's values, 64 levels.
    ///
    /// Fails with `replay.syntax`, naming the column, for text that isn't a value, nests deeper,
    /// or enables RON's extensions, which change its grammar.
    pub(crate) fn parse(text: &str, from: usize) -> Result<Self> {
        let syntax = |column: usize, what: &dyn std::fmt::Display| {
            let column = from + column - 1;
            Error::new("replay.syntax", format!("column {column}: {what}"))
        };
        // A level of a value takes `ron` up to 7 levels of recursion (a variant's). 64 levels, as
        // deep as a scene's values nest, fit the 1 MB stack of Windows' main thread in a debug
        // build, as a test checks; about 73 overflow it.
        let options = Options::default().with_recursion_limit(7 * MAX_DEPTH as usize + 16);
        let mut deserializer = ron::Deserializer::from_str_with_options(text, &options)
            .map_err(|error| syntax(error.span.end.col, &error.code))?;
        if deserializer.extensions() != Extensions::empty() {
            return Err(syntax(1, &"a recording enables none of RON's extensions"));
        }
        let parsed = Self::deserialize(&mut deserializer).and_then(|parsed| {
            deserializer.end()?;
            Ok(parsed)
        });
        parsed.map_err(|error| {
            let error = deserializer.span_error(error);
            syntax(error.span.end.col, &error.code)
        })
    }

    /// The value in `world`: an entity is the one with its ID, else a handle naming none; a
    /// texture the one it names, else `Opaque`, which nothing sets.
    pub(crate) fn value(&self, world: &World) -> Value {
        let fields = |given: &[(String, Text)]| {
            let given = given.iter();
            given
                .map(|(name, text)| (Ident::Owned(name.clone()), text.value(world)))
                .collect()
        };
        match self {
            &Self::Bool(b) => Value::Bool(b),
            &Self::Int(n) => Value::Int(n),
            &Self::UInt(n) => Value::UInt(n),
            &Self::F32(x) => Value::F32(x),
            &Self::F64(x) => Value::F64(x),
            Self::String(text) => Value::String(text.clone()),
            &Self::Vec2(x, y) => Value::Vec2(Vec2::new(x, y)),
            &Self::Vec3(x, y, z) => Value::Vec3(Vec3::new(x, y, z)),
            &Self::Vec4(x, y, z, w) => Value::Vec4(Vec4::new(x, y, z, w)),
            &Self::DVec3(x, y, z) => Value::DVec3(DVec3::new(x, y, z)),
            &Self::Quat(x, y, z, w) => Value::Quat(Quat::from_xyzw(x, y, z, w)),
            Self::Entity(Id(id)) => Value::Entity(world.entity(*id).unwrap_or(Entity::DEAD)),
            Self::Dead => Value::Entity(Entity::DEAD),
            Self::Texture(name) => {
                (world.assets().texture(name)).map_or(Value::Opaque, Value::Texture)
            }
            Self::Model(name) => (world.assets().model(name)).map_or(Value::Opaque, Value::Model),
            Self::Font(name) => (world.assets().font(name)).map_or(Value::Opaque, Value::Font),
            Self::Prefab(name) => {
                (world.assets().prefab(name)).map_or(Value::Opaque, Value::Prefab)
            }
            Self::None => Value::Option(None),
            Self::Some(text) => Value::Option(Some(Box::new(text.value(world)))),
            Self::List(items) => Value::List(items.iter().map(|item| item.value(world)).collect()),
            Self::Struct(given) => Value::Struct(fields(given)),
            Self::Variant(name, given) => Value::Variant(Ident::Owned(name.clone()), fields(given)),
            Self::Opaque => Value::Opaque,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transform;
    use crate::reflect::tests::same;

    fn round_trip(value: &Value, world: &World, into: &World) -> (String, Value) {
        let mut text = String::new();
        write(&mut text, value, world);
        let read = Text::parse(&text, 1).unwrap_or_else(|error| panic!("{text}: {error}"));
        (text, read.value(into))
    }

    #[test]
    fn every_kind_of_value_reads_back_bit_for_bit() {
        let world = World::default();
        let f32s = [
            0.0,
            -0.0,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            f32::MAX,
            0.1,
            -1.0e-30,
        ];
        let f64s = [-0.0, f64::from_bits(1), f64::MAX, 0.1, f64::NEG_INFINITY];
        let named = |name: &str, value| (Ident::Owned(name.to_owned()), value);
        let mut values: Vec<Value> = f32s.iter().map(|&x| Value::F32(x)).collect();
        values.extend(f64s.iter().map(|&x| Value::F64(x)));
        values.extend([
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(i64::MIN),
            Value::Int(i64::MAX),
            Value::UInt(u64::MAX),
            Value::UInt(0),
            Value::String(
                "a \"quote\", a \\, a\nnew line, a\ttab, \u{0}, \u{202e} and é".to_owned(),
            ),
            Value::String(String::new()),
            Value::Vec2(Vec2::new(-0.0, 1.5)),
            Value::Vec3(Vec3::new(1.0, f32::from_bits(1), -2.0)),
            Value::Vec4(Vec4::new(0.3, 0.6, 0.9, 0.0)),
            Value::DVec3(DVec3::new(1.0e6 + 0.1, f64::from_bits(1), -0.0)),
            Value::Quat(Quat::from_rotation_z(1.0)),
            Value::Option(None),
            Value::Option(Some(Box::new(Value::Option(None)))),
            Value::List(Vec::new()),
            Value::List(vec![Value::UInt(1), Value::List(vec![Value::Int(-1)])]),
            Value::Struct(Vec::new()),
            Value::Struct(vec![
                named("0", Value::F32(0.5)),
                named("odd \"name\"", Value::Opaque),
            ]),
            Value::Variant("Swarm".into(), vec![named("0", Value::UInt(10_000))]),
            Value::Variant("Point".into(), Vec::new()),
            Value::Opaque,
        ]);
        for value in &values {
            let (text, read) = round_trip(value, &world, &world);
            assert!(!text.contains('\n'), "{text}");
            assert!(same(&read, value), "{text}: {read:?}");
        }
        let (text, _) = round_trip(&Value::Int(i64::MIN), &world, &world);
        assert_eq!(text, "Int(-9223372036854775808)");
        let swarm = Value::Variant("Swarm".into(), vec![named("0", Value::UInt(10_000))]);
        assert_eq!(
            round_trip(&swarm, &world, &world).0,
            r#"Variant("Swarm", [("0", UInt(10000))])"#
        );
    }

    #[test]
    fn a_nan_keeps_its_sign_unless_folded() {
        let world = World::default();
        for bits in [0x7fc0_0000u32, 0xffc0_0000, 0x7f80_0001, 0xffc1_2345] {
            let x = f32::from_bits(bits);
            let (text, read) = round_trip(&Value::F32(x), &world, &world);
            let Value::F32(read) = read else {
                panic!("{text}: an f32");
            };
            assert!(read.is_nan(), "{text}");
            assert_eq!(read.is_sign_negative(), x.is_sign_negative(), "{text}");
            let mut folded = String::new();
            write_folded(&mut folded, &Value::Vec2(Vec2::new(x, 1.0)), &world);
            assert_eq!(folded, "Vec2(NaN, 1.0)");
        }
        let (text, read) = round_trip(&Value::F64(-f64::NAN), &world, &world);
        assert!(
            matches!(read, Value::F64(x) if x.is_nan() && x.is_sign_negative()),
            "{text}"
        );
    }

    #[test]
    fn an_entity_goes_by_its_id_and_a_dead_one_stays_dead() {
        let mut world = World::seeded(1);
        let entity = world.spawn(Transform::IDENTITY);
        let dead = world.spawn(Transform::IDENTITY);
        world.despawn(dead).expect("alive");
        let text = world.save_scene(&[entity]).expect("saved");
        let mut other = World::seeded(2);
        // The entity takes another slot there.
        other.spawn(());
        other.register::<Transform>().expect("registered");
        let loaded = other.load_scene(&text).expect("loaded")[0];
        assert_ne!(loaded, entity);
        let held = |entity| Value::Option(Some(Box::new(Value::Entity(entity))));
        let (written, read) = round_trip(&held(entity), &world, &other);
        assert_eq!(
            written,
            format!("Some(Entity(\"{}\"))", world.id(entity).expect("alive"))
        );
        assert_eq!(read, held(loaded));
        let (written, read) = round_trip(&held(dead), &world, &other);
        assert_eq!(written, "Some(Dead)");
        assert_eq!(read, held(Entity::DEAD));
        // An ID no entity has where it's read.
        let (_, read) = round_trip(&held(entity), &world, &World::seeded(3));
        assert_eq!(read, held(Entity::DEAD));
        assert!(!World::seeded(3).contains(Entity::DEAD));
    }

    /// A value nests as deep as a scene's, no deeper, in a Windows-main-thread-sized stack.
    #[test]
    fn a_value_nests_as_deep_as_a_scenes() {
        let nested = |levels: usize| {
            let mut value = Value::Opaque;
            for _ in 1..levels {
                let field = (Ident::Borrowed("0"), value);
                value = Value::Variant(Ident::Borrowed("Deeper"), vec![field]);
            }
            value
        };
        let read = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                let world = World::default();
                let deepest = nested(MAX_DEPTH as usize);
                let (_, read) = round_trip(&deepest, &world, &world);
                assert_eq!(read, deepest);
                let mut text = String::new();
                write(&mut text, &nested(2 * MAX_DEPTH as usize), &world);
                Text::parse(&text, 1)
                    .map(drop)
                    .map_err(|error| error.code())
            });
        let deeper = read
            .expect("a thread")
            .join()
            .expect("read without overflowing");
        assert_eq!(deeper, Err("replay.syntax"));
    }

    #[test]
    fn text_that_isnt_a_value_is_refused_where_it_stops() {
        // Each column is just past where reading stops, for text starting at column 5.
        for (text, column) in [
            ("UInt(-1)", 12),
            ("F32(1.0", 12),
            ("Bool(true) Bool(false)", 16),
            ("Entity(\"5F3A9C0E12B47D68\")", 30),
            ("Number(1)", 11),
            ("#![enable(implicit_some)] Some(Opaque)", 5),
        ] {
            let error = Text::parse(text, 5).expect_err(text);
            assert_eq!(error.code(), "replay.syntax", "{text}");
            let says = format!("column {column}: ");
            assert!(error.to_string().starts_with(&says), "{text}: {error}");
        }
    }
}
