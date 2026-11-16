//! Reflection's values as the tool API writes and reads them (gg-api's docs).
//!
//! JSON is written by hand so a struct's fields keep their order: entities by ID, textures by
//! name, ID or else path. It's read by the type of the field it goes in, a texture by name or file
//! path. A game uses its world's entities and textures; a client, as the editor's Inspector is,
//! uses another process's ([`Remote`]). Reflection's type descriptions go to and come back from
//! gg-api's, as `types` answers them.

use std::borrow::Cow;
use std::fmt::Write as _;

use gg_core::math::{DVec3, Quat, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use serde_json::{Map, Value as JsonValue};

use crate::ecs::MAX_DEPTH;
use crate::reflect::{self, Bound, Field, Ident, Meta, Shape, Show, Type, Types, Value, Variant};
use crate::{Entity, EntityId, Font, Model, Prefab, Texture, World};

/// Finds the entities and assets JSON names, by ID or name, and names them back: a game's
/// [`World`] or a client's [`Remote`].
pub(crate) trait Names {
    /// The types values are read by.
    fn types(&self) -> &Types;
    /// The ID of `entity`, if it's alive.
    fn id(&self, entity: Entity) -> Option<EntityId>;
    /// The name of `texture`, its ID or else its path, if it has one.
    fn name(&self, texture: Texture) -> Option<&str>;
    /// The entity of ID `id`.
    ///
    /// Fails with `ecs.no_entity` if it isn't alive.
    fn entity(&self, id: EntityId) -> Result<Value>;
    /// The texture called `name`, by name or file path.
    ///
    /// Fails with `api.no_texture` if none is.
    fn texture(&self, name: &str) -> Result<Value>;
    /// The name of `model`, as a texture's.
    fn model_name(&self, model: Model) -> Option<&str>;
    /// The model called `name`, by name or file path.
    ///
    /// Fails with `api.no_model` if none is.
    fn model(&self, name: &str) -> Result<Value>;
    /// The name of `font`, as a texture's.
    fn font_name(&self, font: Font) -> Option<&str>;
    /// The font called `name`, by name or file path.
    ///
    /// Fails with `api.no_font` if none is.
    fn font(&self, name: &str) -> Result<Value>;
    /// The name of `prefab`, as a texture's.
    fn prefab_name(&self, prefab: Prefab) -> Option<&str>;
    /// The prefab called `name`, by name or file path.
    ///
    /// Fails with `api.no_prefab` if none is.
    fn prefab(&self, name: &str) -> Result<Value>;
}

impl Names for World {
    fn types(&self) -> &Types {
        World::types(self)
    }

    fn id(&self, entity: Entity) -> Option<EntityId> {
        World::id(self, entity)
    }

    fn name(&self, texture: Texture) -> Option<&str> {
        self.assets().name(texture)
    }

    fn entity(&self, id: EntityId) -> Result<Value> {
        let entity = World::entity(self, id)
            .ok_or_else(|| Error::new("ecs.no_entity", format!("no living entity has ID {id}")))?;
        Ok(Value::Entity(entity))
    }

    fn texture(&self, name: &str) -> Result<Value> {
        let texture = self.assets().texture(name).ok_or_else(|| {
            let message = self.assets().not_found("texture", name);
            Error::new("api.no_texture", message)
        })?;
        Ok(Value::Texture(texture))
    }

    fn model_name(&self, model: Model) -> Option<&str> {
        Some(self.assets().model_name(model))
    }

    fn model(&self, name: &str) -> Result<Value> {
        let model = self.assets().model(name).ok_or_else(|| {
            let message = self.assets().not_found("model", name);
            Error::new("api.no_model", message)
        })?;
        Ok(Value::Model(model))
    }

    fn font_name(&self, font: Font) -> Option<&str> {
        Some(self.assets().font_name(font))
    }

    fn font(&self, name: &str) -> Result<Value> {
        let font = self.assets().font(name).ok_or_else(|| {
            let message = self.assets().not_found("font", name);
            Error::new("api.no_font", message)
        })?;
        Ok(Value::Font(font))
    }

    fn prefab_name(&self, prefab: Prefab) -> Option<&str> {
        Some(self.assets().prefab_name(prefab))
    }

    fn prefab(&self, name: &str) -> Result<Value> {
        let prefab = self.assets().prefab(name).ok_or_else(|| {
            let message = self.assets().not_found("prefab", name);
            Error::new("api.no_prefab", message)
        })?;
        Ok(Value::Prefab(prefab))
    }
}

/// Another process's values as its API's client keeps them, by the types it described. An entity
/// is its ID as a [`Value::String`], or [`Entity::DEAD`] for JSON's `null`; an asset is its name as
/// a [`Value::String`]. Written back, they're the JSON they were read from, which the game reads
/// by its own world.
pub(crate) struct Remote<'a>(pub(crate) &'a Types);

impl Names for Remote<'_> {
    fn types(&self) -> &Types {
        self.0
    }

    fn id(&self, _: Entity) -> Option<EntityId> {
        None
    }

    fn name(&self, _: Texture) -> Option<&str> {
        None
    }

    fn entity(&self, id: EntityId) -> Result<Value> {
        Ok(Value::String(id.to_string()))
    }

    fn texture(&self, name: &str) -> Result<Value> {
        Ok(Value::String(name.to_owned()))
    }

    fn model_name(&self, _: Model) -> Option<&str> {
        None
    }

    fn model(&self, name: &str) -> Result<Value> {
        Ok(Value::String(name.to_owned()))
    }

    fn font_name(&self, _: Font) -> Option<&str> {
        None
    }

    fn font(&self, name: &str) -> Result<Value> {
        Ok(Value::String(name.to_owned()))
    }

    fn prefab_name(&self, _: Prefab) -> Option<&str> {
        None
    }

    fn prefab(&self, name: &str) -> Result<Value> {
        Ok(Value::String(name.to_owned()))
    }
}

/// Writes `value` as JSON. Entities are IDs and textures names, as `names` has them; `null` for a
/// dead entity or an unnamed texture.
pub(crate) fn write(out: &mut String, value: &Value, names: &dyn Names) {
    match value {
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => write!(out, "{n}").expect("a string takes any text"),
        Value::UInt(n) => write!(out, "{n}").expect("a string takes any text"),
        &Value::F32(x) => float32(out, x),
        &Value::F64(x) => float64(out, x),
        Value::String(text) => string(out, text),
        Value::Vec2(v) => floats(out, &v.to_array()),
        Value::Vec3(v) => floats(out, &v.to_array()),
        Value::Vec4(v) => floats(out, &v.to_array()),
        Value::DVec3(v) => float64s(out, &v.to_array()),
        Value::Quat(q) => floats(out, &q.to_array()),
        &Value::Entity(entity) => match names.id(entity) {
            Some(id) => string(out, &id.to_string()),
            None => out.push_str("null"),
        },
        &Value::Texture(texture) => match names.name(texture) {
            Some(name) => string(out, name),
            None => out.push_str("null"),
        },
        &Value::Model(model) => match names.model_name(model) {
            Some(name) => string(out, name),
            None => out.push_str("null"),
        },
        &Value::Font(font) => match names.font_name(font) {
            Some(name) => string(out, name),
            None => out.push_str("null"),
        },
        &Value::Prefab(prefab) => match names.prefab_name(prefab) {
            Some(name) => string(out, name),
            None => out.push_str("null"),
        },
        Value::Option(None) => out.push_str("null"),
        Value::Option(Some(value)) => write(out, value, names),
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(out, item, names);
            }
            out.push(']');
        }
        Value::Struct(fields) => object(out, fields, names),
        Value::Variant(name, fields) if fields.is_empty() => string(out, name),
        Value::Variant(name, fields) => {
            out.push('{');
            string(out, name);
            out.push(':');
            object(out, fields, names);
            out.push('}');
        }
        Value::Opaque => out.push_str("\"(opaque)\""),
    }
}

/// The value `json` gives a field of type `ty` (gg-api's docs).
///
/// Entities are IDs that `names` finds alive, or `null` for a dead one. Textures, models, fonts and
/// prefabs are names or file paths. A struct or variant missing a field takes its default, or is
/// opaque if it is. A whole number is an `Int` for a signed field, a `UInt` for an unsigned one if
/// it fits.
///
/// Fails, saying where in the value, with:
/// - `reflect.wrong_kind` for JSON of another kind than the type's.
/// - `reflect.no_variant` or `reflect.no_field` for a variant or field the type hasn't.
/// - `ecs.bad_id` for an ID that isn't one, `ecs.no_entity` for one with no living entity.
/// - `api.no_texture`, `api.no_model`, `api.no_font` or `api.no_prefab` for a name or path none
///   has.
/// - `api.too_deep` for a value nested deeper than a scene's may be.
pub(crate) fn read(json: &JsonValue, ty: &Type, names: &dyn Names) -> Result<Value> {
    read_at(json, ty, names, 1)
}

/// [`read`] for a value `depth` levels deep, a component's being the first.
fn read_at(json: &JsonValue, ty: &Type, names: &dyn Names, depth: u32) -> Result<Value> {
    if depth > MAX_DEPTH {
        let message =
            format!("a value more than {MAX_DEPTH} levels deep, the most a scene's may be");
        return Err(Error::new("api.too_deep", message));
    }
    let types = names.types();
    let wrong = |expected: &str| {
        let found = match json {
            JsonValue::Null => "null",
            JsonValue::Bool(_) => "a boolean",
            JsonValue::Number(_) => "a number",
            JsonValue::String(_) => "a string",
            JsonValue::Array(_) => "an array",
            JsonValue::Object(_) => "an object",
        };
        Error::new(
            "reflect.wrong_kind",
            format!("expected {expected}, got {found}"),
        )
    };
    let float = |json: &JsonValue| match json {
        JsonValue::Number(n) => n.as_f64().ok_or_else(|| wrong("a number")),
        JsonValue::String(text) => match text.as_str() {
            "inf" => Ok(f64::INFINITY),
            "-inf" => Ok(f64::NEG_INFINITY),
            "NaN" => Ok(f64::NAN),
            _ => Err(wrong("a number, or `inf`, `-inf` or `NaN`")),
        },
        _ => Err(wrong("a number")),
    };
    let float64s = |n: usize| -> Result<Vec<f64>> {
        let expected = format!("an array of {n} numbers");
        match json {
            JsonValue::Array(items) if items.len() == n => items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    float(item).map_err(|error| reflect::within(error, &format!("[{i}]")))
                })
                .collect(),
            _ => Err(wrong(&expected)),
        }
    };
    let floats =
        |n: usize| -> Result<Vec<f32>> { Ok(float64s(n)?.into_iter().map(|x| x as f32).collect()) };
    Ok(match ty {
        Type::Bool => match json {
            &JsonValue::Bool(b) => Value::Bool(b),
            _ => return Err(wrong("a boolean")),
        },
        &Type::Int { signed, .. } => match json {
            // The other kind for a number outside this one's; reflection refuses it.
            JsonValue::Number(n) => match (signed, n.as_i64(), n.as_u64()) {
                (true, Some(n), _) | (false, Some(n), None) => Value::Int(n),
                (_, _, Some(n)) => Value::UInt(n),
                (_, None, None) => return Err(wrong("a whole number")),
            },
            _ => return Err(wrong("a whole number")),
        },
        Type::F32 => Value::F32(float(json)? as f32),
        Type::F64 => Value::F64(float(json)?),
        Type::String => match json {
            JsonValue::String(text) => Value::String(text.clone()),
            _ => return Err(wrong("a string")),
        },
        Type::Vec2 => Value::Vec2(Vec2::from_slice(&floats(2)?)),
        Type::Vec3 => Value::Vec3(Vec3::from_slice(&floats(3)?)),
        Type::Vec4 => Value::Vec4(Vec4::from_slice(&floats(4)?)),
        Type::DVec3 => Value::DVec3(DVec3::from_slice(&float64s(3)?)),
        Type::Quat => Value::Quat(Quat::from_slice(&floats(4)?)),
        Type::Entity => match json {
            JsonValue::String(text) => names.entity(text.parse()?)?,
            // As a dead reference writes.
            JsonValue::Null => Value::Entity(Entity::DEAD),
            _ => return Err(wrong("an entity's ID, a string, or null for none alive")),
        },
        Type::Texture => match json {
            JsonValue::String(name) => names.texture(name)?,
            _ => return Err(wrong("a texture's ID or path, a string")),
        },
        Type::Model => match json {
            JsonValue::String(name) => names.model(name)?,
            _ => return Err(wrong("a model's ID or path, a string")),
        },
        Type::Font => match json {
            JsonValue::String(name) => names.font(name)?,
            _ => return Err(wrong("a font's ID or path, a string")),
        },
        Type::Prefab => match json {
            JsonValue::String(name) => names.prefab(name)?,
            _ => return Err(wrong("a prefab's ID or path, a string")),
        },
        Type::Option(inner) => match json {
            JsonValue::Null => Value::Option(None),
            json => Value::Option(Some(Box::new(read_at(json, inner, names, depth + 1)?))),
        },
        Type::List(inner) => match json {
            JsonValue::Array(items) => Value::List(list(items, inner, names, depth)?),
            _ => return Err(wrong("an array")),
        },
        Type::Array(inner, n) => match json {
            JsonValue::Array(items) if items.len() == *n as usize => {
                Value::List(list(items, inner, names, depth)?)
            }
            _ => return Err(wrong(&format!("an array of {n}"))),
        },
        Type::Opaque => opaque(json, &wrong)?,
        &Type::Named(index) => match &types.info(index).shape {
            Shape::Struct { fields, .. } => match json {
                JsonValue::Object(given) => {
                    Value::Struct(object_fields(given, fields, names, depth)?)
                }
                _ => return Err(wrong("an object of the struct's fields")),
            },
            Shape::Enum(variants) => {
                let variant_names = || variants.iter().map(|variant| variant.name.as_ref());
                let (name, given) = match json {
                    JsonValue::String(name) => (name.as_str(), None),
                    JsonValue::Object(one) if one.len() == 1 => {
                        let (name, fields) = one.iter().next().expect("one key");
                        (name.as_str(), Some(fields))
                    }
                    _ => {
                        return Err(wrong(
                            "a variant's name, or an object of one key, a variant's name, whose \
                             value is an object of its fields",
                        ));
                    }
                };
                let Some(variant) = variants.iter().find(|variant| variant.name == name) else {
                    let given: Vec<_> = variant_names().collect();
                    return Err(reflect::no_variant(name, &given));
                };
                let fields = match given {
                    None => object_fields(&Map::new(), &variant.fields, names, depth)?,
                    Some(JsonValue::Object(given)) => {
                        object_fields(given, &variant.fields, names, depth)
                            .map_err(|error| reflect::within(error, name))?
                    }
                    Some(_) => {
                        let error = wrong("an object of the variant's fields");
                        return Err(reflect::within(error, name));
                    }
                };
                Value::Variant(variant.name.clone(), fields)
            }
            Shape::Opaque => opaque(json, &wrong)?,
        },
    })
}

/// `items`, each read as a `ty` a level below `depth`.
fn list(items: &[JsonValue], ty: &Type, names: &dyn Names, depth: u32) -> Result<Vec<Value>> {
    let items = items.iter().enumerate().map(|(i, item)| {
        read_at(item, ty, names, depth + 1)
            .map_err(|error| reflect::within(error, &format!("[{i}]")))
    });
    items.collect()
}

/// The fields `fields` describes, of a struct or variant a level above them, each from `given`'s
/// key of its name, else its default, else opaque if it is.
///
/// Fails with `reflect.no_field` for a key that isn't a field's.
fn object_fields(
    given: &Map<String, JsonValue>,
    fields: &[Field],
    names: &dyn Names,
    depth: u32,
) -> Result<Vec<(Ident, Value)>> {
    if let Some(name) = given
        .keys()
        .find(|key| !fields.iter().any(|field| field.name == **key))
    {
        let names = fields.iter().map(|field| field.name.as_ref());
        let message = format!(
            "no field `{name}`; its fields are {}",
            reflect::list(names.map(|name| format!("`{name}`")))
        );
        return Err(Error::new("reflect.no_field", message));
    }
    let types = names.types();
    let fields = fields.iter().map(|field| {
        let value = match given.get(field.name.as_ref()) {
            Some(json) => read_at(json, &field.ty, names, depth + 1),
            None if field.ty.is_opaque(types) => Ok(Value::Opaque),
            None => types.field_default(field),
        };
        let value = value.map_err(|error| reflect::within(error, &field.name));
        value.map(|value| (field.name.clone(), value))
    });
    fields.collect()
}

/// What's opaque: only `(opaque)` gives it, kept or made as its type's own is.
fn opaque(json: &JsonValue, wrong: &dyn Fn(&str) -> Error) -> Result<Value> {
    match json {
        JsonValue::String(text) if text == "(opaque)" => Ok(Value::Opaque),
        _ => Err(wrong("`(opaque)`, since what's opaque isn't read or set")),
    }
}

/// Writes `text` as a JSON string.
pub(crate) fn string(out: &mut String, text: &str) {
    out.push_str(&serde_json::to_string(text).expect("a string is a JSON string"));
}

/// Writes `fields` as an object, in their order.
fn object(out: &mut String, fields: &[(Ident, Value)], names: &dyn Names) {
    out.push('{');
    for (i, (name, value)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        string(out, name);
        out.push(':');
        write(out, value, names);
    }
    out.push('}');
}

/// Writes `xs` as an array of numbers, non-finite ones as strings.
pub(crate) fn floats(out: &mut String, xs: &[f32]) {
    out.push('[');
    for (i, &x) in xs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        float32(out, x);
    }
    out.push(']');
}

/// Like [`floats`], each written by [`float64`].
pub(crate) fn float64s(out: &mut String, xs: &[f64]) {
    out.push('[');
    for (i, &x) in xs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        float64(out, x);
    }
    out.push(']');
}

/// Writes `x` as the shortest number that reads back to its bits, a string if it isn't finite.
pub(crate) fn float64(out: &mut String, x: f64) {
    if x.is_finite() {
        let text = serde_json::to_string(&x).expect("a finite float is a number");
        out.push_str(&text);
    } else {
        not_finite(out, x.is_nan(), x > 0.0);
    }
}

/// Writes `x` as the shortest number that reads back to its `f32` bits: `0.1` for 0.1, not its
/// widened `f64`.
pub(crate) fn float32(out: &mut String, x: f32) {
    if x.is_finite() {
        let text = serde_json::to_string(&x).expect("a finite float is a number");
        out.push_str(&text);
    } else {
        not_finite(out, x.is_nan(), x > 0.0);
    }
}

/// Writes a non-finite float, which JSON has no number for, as the string `NaN`, `inf` or `-inf`.
fn not_finite(out: &mut String, nan: bool, positive: bool) {
    out.push_str(match (nan, positive) {
        (true, _) => "\"NaN\"",
        (false, true) => "\"inf\"",
        (false, false) => "\"-inf\"",
    });
}

/// `value` as [`write()`] writes it.
fn json(value: &Value, names: &dyn Names) -> JsonValue {
    let mut out = String::new();
    write(&mut out, value, names);
    serde_json::from_str(&out).expect("written as JSON")
}

/// `world`'s component types and the named types, as `types` answers them.
pub(super) fn describe(world: &World) -> gg_api::Types {
    let types = world.types();
    let components = world.component_types().map(|name| {
        let (default, refused) = match world.default_of(name) {
            Ok(default) => (Some(json(&default, world)), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        gg_api::ComponentInfo {
            name: name.to_owned(),
            default,
            refused,
        }
    });
    let ty = |ty: &Type| api_type(types, ty);
    let fields = |fields: &[Field]| -> Vec<gg_api::FieldInfo> {
        let field = |field: &Field| gg_api::FieldInfo {
            name: field.name.to_string(),
            ty: ty(&field.ty),
            min: field.meta.min.map(api_bound),
            max: field.meta.max.map(api_bound),
            show: match field.meta.show {
                Show::Plain => gg_api::Show::Plain,
                Show::Degrees => gg_api::Show::Degrees,
                Show::Color => gg_api::Show::Color,
            },
            doc: field.meta.doc.to_string(),
            // No default holds a living entity or a texture.
            default: field.default.as_ref().map(|default| json(default, world)),
        };
        fields.iter().map(field).collect()
    };
    let described = types.iter().map(|(_, info)| gg_api::TypeInfo {
        name: info.name.to_string(),
        doc: info.doc.to_string(),
        shape: match &info.shape {
            Shape::Struct {
                fields: given,
                tuple,
            } => gg_api::Shape::Struct {
                fields: fields(given),
                tuple: *tuple,
            },
            Shape::Enum(variants) => gg_api::Shape::Enum(
                variants
                    .iter()
                    .map(|variant| gg_api::VariantInfo {
                        name: variant.name.to_string(),
                        fields: fields(&variant.fields),
                        tuple: variant.tuple,
                    })
                    .collect(),
            ),
            Shape::Opaque => gg_api::Shape::Opaque,
        },
    });
    gg_api::Types {
        components: components.collect(),
        types: described.collect(),
        revision: world.types_revision(),
    }
}

fn api_type(types: &Types, ty: &Type) -> gg_api::Type {
    use gg_api::Type as Api;

    let inner = |ty: &Type| Box::new(api_type(types, ty));
    match ty {
        Type::Bool => Api::Bool,
        &Type::Int { bits, signed } => Api::Int { bits, signed },
        Type::F32 => Api::F32,
        Type::F64 => Api::F64,
        Type::String => Api::String,
        Type::Vec2 => Api::Vec2,
        Type::Vec3 => Api::Vec3,
        Type::Vec4 => Api::Vec4,
        Type::DVec3 => Api::Dvec3,
        Type::Quat => Api::Quat,
        Type::Entity => Api::Entity,
        Type::Texture => Api::Texture,
        Type::Model => Api::Model,
        Type::Font => Api::Font,
        Type::Prefab => Api::Prefab,
        Type::Option(of) => Api::Option(inner(of)),
        Type::List(of) => Api::List(inner(of)),
        Type::Array(of, n) => Api::Array(inner(of), *n),
        &Type::Named(index) => Api::Named(types.info(index).name.to_string()),
        Type::Opaque => Api::Opaque,
    }
}

fn api_bound(bound: Bound) -> gg_api::Bound {
    match bound {
        Bound::Int(n) => gg_api::Bound::Int(n),
        Bound::Float(x) => gg_api::Bound::Float(x),
    }
}

/// The types another process's `types` described, by which a client reads, draws and writes its
/// values ([`Remote`]), and its component types in registration order, each with the value an add
/// without one gives it, or why there's none.
pub(crate) struct Described {
    pub(crate) types: Types,
    pub(crate) components: Vec<(String, std::result::Result<Value, String>)>,
}

/// The types `given` describes.
///
/// Fails with `api.protocol` for a description that isn't one: two types of one name, a type named
/// but not described, or a default that isn't its field's.
pub(crate) fn described(given: &gg_api::Types) -> Result<Described> {
    let protocol = |what: String| Error::new("api.protocol", format!("`types` answered {what}"));
    let names = given.types.iter().map(|info| {
        let doc = Cow::Owned(info.doc.clone());
        (Ident::from(info.name.clone()), doc)
    });
    let mut types = Types::described(names);
    if types.iter().len() != given.types.len() {
        return Err(protocol("two types of one name".to_owned()));
    }
    // Shapes first: they read the fields' defaults.
    let fields = |types: &Types, given: &[gg_api::FieldInfo]| -> Result<Vec<Field>> {
        let field = |field: &gg_api::FieldInfo| {
            Ok(Field {
                name: Ident::from(field.name.clone()),
                ty: own_type(types, &field.ty).map_err(protocol)?,
                meta: Meta {
                    min: field.min.map(own_bound),
                    max: field.max.map(own_bound),
                    show: match field.show {
                        gg_api::Show::Plain => Show::Plain,
                        gg_api::Show::Degrees => Show::Degrees,
                        gg_api::Show::Color => Show::Color,
                    },
                    doc: Cow::Owned(field.doc.clone()),
                },
                default: None,
            })
        };
        given.iter().map(field).collect()
    };
    for info in &given.types {
        let shape = match &info.shape {
            gg_api::Shape::Struct {
                fields: given,
                tuple,
            } => Shape::Struct {
                fields: fields(&types, given)?,
                tuple: *tuple,
            },
            gg_api::Shape::Enum(variants) => Shape::Enum(
                variants
                    .iter()
                    .map(|variant| {
                        Ok(Variant {
                            name: Ident::from(variant.name.clone()),
                            fields: fields(&types, &variant.fields)?,
                            tuple: variant.tuple,
                        })
                    })
                    .collect::<Result<_>>()?,
            ),
            gg_api::Shape::Opaque => Shape::Opaque,
        };
        let index = types.find(&info.name).expect("each named");
        types.info_mut(index).shape = shape;
    }
    // Then the defaults, set once all are read.
    let mut defaults = Vec::new();
    for info in &given.types {
        let given = match &info.shape {
            gg_api::Shape::Struct { fields, .. } => vec![fields],
            gg_api::Shape::Enum(variants) => variants.iter().map(|v| &v.fields).collect(),
            gg_api::Shape::Opaque => Vec::new(),
        };
        for (i, fields) in given.into_iter().enumerate() {
            for (j, field) in fields.iter().enumerate() {
                let Some(default) = &field.default else {
                    continue;
                };
                let ty = own_type(&types, &field.ty).map_err(protocol)?;
                let value = read(default, &ty, &Remote(&types)).map_err(|error| {
                    protocol(format!(
                        "`{}.{}`'s default {default}: {error}",
                        info.name, field.name
                    ))
                })?;
                defaults.push((types.find(&info.name).expect("each named"), i, j, value));
            }
        }
    }
    for (index, i, j, value) in defaults {
        let fields = match &mut types.info_mut(index).shape {
            Shape::Struct { fields, .. } => fields,
            Shape::Enum(variants) => &mut variants[i].fields,
            Shape::Opaque => unreachable!("an opaque type has no fields"),
        };
        fields[j].default = Some(value);
    }
    let components = given.components.iter().map(|component| {
        let default = match (&component.default, &component.refused) {
            (Some(default), _) => {
                let index = types.find(&component.name).ok_or_else(|| {
                    protocol(format!("component type `{}` undescribed", component.name))
                })?;
                let value = read(default, &Type::Named(index), &Remote(&types));
                Ok(value.map_err(|error| {
                    protocol(format!("`{}`'s default {default}: {error}", component.name))
                })?)
            }
            (None, refused) => Err(refused.clone().unwrap_or_default()),
        };
        Ok((component.name.clone(), default))
    });
    let components = components.collect::<Result<_>>()?;
    Ok(Described { types, components })
}

/// The type `ty` names, among `types`.
///
/// Fails for a named type that isn't among them.
fn own_type(types: &Types, ty: &gg_api::Type) -> std::result::Result<Type, String> {
    use gg_api::Type as Api;

    let inner = |ty: &Api| own_type(types, ty).map(Box::new);
    Ok(match ty {
        Api::Bool => Type::Bool,
        &Api::Int { bits, signed } => Type::Int { bits, signed },
        Api::F32 => Type::F32,
        Api::F64 => Type::F64,
        Api::String => Type::String,
        Api::Vec2 => Type::Vec2,
        Api::Vec3 => Type::Vec3,
        Api::Vec4 => Type::Vec4,
        Api::Dvec3 => Type::DVec3,
        Api::Quat => Type::Quat,
        Api::Entity => Type::Entity,
        Api::Texture => Type::Texture,
        Api::Model => Type::Model,
        Api::Font => Type::Font,
        Api::Prefab => Type::Prefab,
        Api::Option(of) => Type::Option(inner(of)?),
        Api::List(of) => Type::List(inner(of)?),
        Api::Array(of, n) => Type::Array(inner(of)?, *n),
        Api::Named(name) => match types.find(name) {
            Some(index) => Type::Named(index),
            None => return Err(format!("a type named `{name}` that it doesn't describe")),
        },
        Api::Opaque => Type::Opaque,
    })
}

fn own_bound(bound: gg_api::Bound) -> Bound {
    match bound {
        gg_api::Bound::Int(n) => Bound::Int(n),
        gg_api::Bound::Float(x) => Bound::Float(x),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use gg_core::math::{DVec3, Quat, Vec3};
    use serde_json::json;

    use super::*;
    use crate::Transform;
    use crate::reflect::tests::{Everything, everything};

    #[test]
    fn values_write_as_json_with_their_fields_in_order() {
        let mut world = World::seeded(1);
        let alive = world.spawn(Transform::IDENTITY);
        let dead = world.spawn(Transform::IDENTITY);
        world.despawn(dead).expect("alive");
        let id = world.id(alive).expect("alive");
        let field = |name: &'static str, value| (Cow::Borrowed(name), value);
        let value = Value::Struct(vec![
            field("zeta", Value::F32(0.1)),
            field("alpha", Value::List(vec![Value::Int(-3), Value::UInt(7)])),
            field("at", Value::Vec3(Vec3::new(1.0, -0.5, 1e-7))),
            field("turn", Value::Quat(Quat::IDENTITY)),
            field("big", Value::F64(f64::NEG_INFINITY)),
            field("odd", Value::F32(f32::NAN)),
            field("who", Value::Entity(alive)),
            field("gone", Value::Option(Some(Box::new(Value::Entity(dead))))),
            field("none", Value::Option(None)),
            field("say", Value::String("a \"quote\"\n".to_owned())),
            field("unit", Value::Variant(Cow::Borrowed("Idle"), Vec::new())),
            field(
                "with",
                Value::Variant(
                    Cow::Borrowed("Swarm"),
                    vec![field("0", Value::UInt(10_000))],
                ),
            ),
            field("texture", Value::Opaque),
            field("on", Value::Bool(true)),
        ]);
        let mut out = String::new();
        write(&mut out, &value, &world);
        let want = format!(
            "{{\"zeta\":0.1,\"alpha\":[-3,7],\"at\":[1.0,-0.5,1e-7],\"turn\":[0.0,0.0,0.0,1.0],\
             \"big\":\"-inf\",\"odd\":\"NaN\",\"who\":\"{id}\",\"gone\":null,\"none\":null,\
             \"say\":\"a \\\"quote\\\"\\n\",\"unit\":\"Idle\",\"with\":{{\"Swarm\":{{\"0\":10000}}}},\
             \"texture\":\"(opaque)\",\"on\":true}}"
        );
        assert_eq!(out, want);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("JSON");
        assert_eq!(parsed["at"][2].as_f64().map(|x| x as f32), Some(1e-7));
    }

    /// A far position writes as the shortest JSON that reads back to its `f64` bits; `f32` would
    /// round 1,000,000.1 to 1,000,000.125.
    #[test]
    fn a_far_position_keeps_its_f64s_through_json() {
        let world = World::seeded(3);
        let far = DVec3::new(1.0e6 + 0.1, -0.0, -1.0e12 - 0.5);
        let mut out = String::new();
        write(&mut out, &Value::DVec3(far), &world);
        assert_eq!(out, "[1000000.1,-0.0,-1000000000000.5]");
        let json: JsonValue = serde_json::from_str(&out).expect("JSON");
        let read = read(&json, &Type::DVec3, &world).expect("read");
        let Value::DVec3(back) = read else {
            panic!("{read:?}");
        };
        let bits = |v: DVec3| v.to_array().map(f64::to_bits);
        assert_eq!(bits(back), bits(far));
    }

    /// A component's JSON reads back by its type to a value that sets it as it was. Missing fields
    /// take defaults; JSON of the wrong kind is refused where it is.
    #[test]
    fn values_read_from_json_by_their_types() {
        let mut world = World::seeded(2);
        let target = world.spawn(Transform::IDENTITY);
        let holder = world.spawn(Everything {
            target: Some(target),
            angle: -0.0,
            ..everything()
        });
        let ty = Type::Named(world.types().find("Everything").expect("registered"));
        let value = world.get_field(holder, "Everything", "").expect("whole");
        let mut out = String::new();
        write(&mut out, &value, &world);
        let json: JsonValue = serde_json::from_str(&out).expect("JSON");
        let read = read(&json, &ty, &world).expect("read");
        let before = world.dump();
        world
            .set_field(holder, "Everything", "", &read)
            .expect("set");
        assert_eq!(world.dump(), before, "set as it was");
        // Missing: a field's default; an opaque field's own.
        let some = read_json(&world, &ty, r#"{"count": 3, "form": {"Circle": {}}}"#).expect("read");
        let Value::Struct(fields) = &some else {
            panic!("{some:?}");
        };
        let field = |name: &str| {
            &fields
                .iter()
                .find(|(field, _)| field == name)
                .expect(name)
                .1
        };
        assert_eq!(field("count"), &Value::UInt(3));
        assert_eq!(field("handle"), &Value::Opaque);
        let defaults = world
            .types()
            .default_value(&ty, &Default::default())
            .expect("a default");
        let Value::Struct(defaults) = defaults else {
            panic!("{defaults:?}");
        };
        let default = |name: &str| {
            &defaults
                .iter()
                .find(|(field, _)| field == name)
                .expect(name)
                .1
        };
        assert_eq!(field("offset"), default("offset"));
        let Value::Variant(variant, _) = field("form") else {
            panic!("{:?}", field("form"));
        };
        assert_eq!(variant, "Circle");
        let dead = world.spawn(());
        let dead_id = world.id(dead).expect("alive");
        world.despawn(dead).expect("alive");
        let refused = [
            (
                r#"{"offset": 1}"#,
                "reflect.wrong_kind",
                "at `offset`: expected an array of 3",
            ),
            (r#"{"nope": 1}"#, "reflect.no_field", "no field `nope`"),
            (r#"{"form": "Hexagon"}"#, "reflect.no_variant", ""),
            (
                r#"{"count": 1.5}"#,
                "reflect.wrong_kind",
                "at `count`: expected a whole number",
            ),
            (
                r#"{"list": [1, "two"]}"#,
                "reflect.wrong_kind",
                "at `list[1]`: ",
            ),
            (
                r#"{"handle": 3}"#,
                "reflect.wrong_kind",
                "at `handle`: expected `(opaque)`",
            ),
            (
                &format!(r#"{{"target": "{dead_id}"}}"#),
                "ecs.no_entity",
                "at `target`: ",
            ),
            (r#"{"target": "nope"}"#, "ecs.bad_id", "at `target`: "),
        ];
        for (json, code, starts) in refused {
            let error = read_json(&world, &ty, json).expect_err(json);
            assert_eq!(error.code(), code, "{json}: {error}");
            assert!(error.to_string().starts_with(starts), "{json}: {error}");
        }
        // `null` reads back as a dead entity.
        let none = super::read(&JsonValue::Null, &Type::Entity, &world).expect("read");
        assert_eq!(none, Value::Entity(Entity::DEAD));
        // A texture by a path no layer here added it from.
        world.register::<crate::Sprite>().expect("registered");
        let sprite = Type::Named(world.types().find("Sprite").expect("registered"));
        let error =
            read_json(&world, &sprite, r#"{"texture": "assets/none.png"}"#).expect_err("none");
        assert_eq!(error.code(), "api.no_texture", "{error}");
        // A font, likewise.
        world.register::<crate::Text>().expect("registered");
        let text = Type::Named(world.types().find("Text").expect("registered"));
        let error = read_json(&world, &text, r#"{"font": "fonts/none.ttf"}"#).expect_err("none");
        assert_eq!(error.code(), "api.no_font", "{error}");
        // A prefab, likewise.
        world.register::<crate::PrefabRoot>().expect("registered");
        let root = Type::Named(world.types().find("PrefabRoot").expect("registered"));
        let error = read_json(&world, &root, r#"{"0": "prefabs/none.prefab"}"#).expect_err("none");
        assert_eq!(error.code(), "api.no_prefab", "{error}");
    }

    /// A whole number reads as an `Int` for a signed field, a `UInt` for an unsigned one, except
    /// outside its kind, which a set then refuses.
    #[test]
    fn whole_numbers_read_by_their_fields_sign() {
        let types = Types::default();
        let names = Remote(&types);
        let int = |signed| Type::Int { bits: 64, signed };
        let read = |json: JsonValue, ty: &Type| super::read(&json, ty, &names).expect("read");
        assert_eq!(read(json!(3), &int(true)), Value::Int(3));
        assert_eq!(read(json!(3), &int(false)), Value::UInt(3));
        assert_eq!(read(json!(-1), &int(false)), Value::Int(-1));
        assert_eq!(read(json!(u64::MAX), &int(true)), Value::UInt(u64::MAX));
    }

    /// A description that can't mean anything is refused: two types of one name, an undescribed
    /// named type, a default for an undescribed component type, a default that isn't its field's.
    #[test]
    fn a_description_that_isnt_one_is_refused() {
        let field = |ty: gg_api::Type, default| gg_api::FieldInfo {
            name: "a".to_owned(),
            ty,
            min: None,
            max: None,
            show: gg_api::Show::Plain,
            doc: String::new(),
            default,
        };
        let named = |name: &str, fields| gg_api::TypeInfo {
            name: name.to_owned(),
            doc: String::new(),
            shape: gg_api::Shape::Struct {
                fields,
                tuple: false,
            },
        };
        let component = |name: &str, default| gg_api::ComponentInfo {
            name: name.to_owned(),
            default,
            refused: None,
        };
        let types = |components, types| gg_api::Types {
            components,
            types,
            revision: 1,
        };
        let byte = gg_api::Type::Int {
            bits: 8,
            signed: false,
        };
        let fine = named("Fine", vec![field(byte.clone(), Some(json!(3)))]);
        let given = types(
            vec![component("Fine", Some(json!({"a": 4})))],
            vec![fine.clone()],
        );
        let described = described(&given).map_err(|error| error.to_string());
        let described = described.expect("described");
        let index = described.types.find("Fine").expect("described");
        let Shape::Struct { fields, .. } = &described.types.info(index).shape else {
            panic!("a struct");
        };
        assert_eq!(fields[0].default, Some(Value::UInt(3)));
        let missing = gg_api::Type::Named("Missing".to_owned());
        let refused = [
            types(Vec::new(), vec![fine.clone(), fine.clone()]),
            types(
                Vec::new(),
                vec![named("Holder", vec![field(missing, None)])],
            ),
            types(
                vec![component("Missing", Some(json!({})))],
                vec![fine.clone()],
            ),
            types(
                Vec::new(),
                vec![named("Bad", vec![field(byte, Some(json!("x")))])],
            ),
        ];
        for given in refused {
            let error = super::described(&given).err().expect("refused");
            assert_eq!(error.code(), "api.protocol", "{error}");
        }
    }

    fn read_json(world: &World, ty: &Type, json: &str) -> Result<Value> {
        read(&serde_json::from_str(json).expect("JSON"), ty, world)
    }
}
