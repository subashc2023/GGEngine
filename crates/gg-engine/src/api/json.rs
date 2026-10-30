//! Reflection's values as the tool API writes and reads them, as gg-api's docs describe: JSON
//! written by hand, so that a struct's fields keep their order, with the entities they hold by
//! their IDs and the textures by their paths; and read by the type of the field it goes in, as
//! reflection describes it.

use std::fmt::Write as _;

use gg_core::math::{Quat, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use serde_json::{Map, Value as JsonValue};

use crate::ecs::MAX_DEPTH;
use crate::reflect::{self, Field, Ident, Shape, Type, Value};
use crate::{Entity, EntityId, World};

/// Writes `value` as JSON, with the entities it holds by their IDs in `world`, `null` for one that
/// isn't alive there, and the textures by their paths, `null` for one without.
pub(super) fn write(out: &mut String, value: &Value, world: &World) {
    match value {
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => write!(out, "{n}").expect("a string takes any text"),
        Value::UInt(n) => write!(out, "{n}").expect("a string takes any text"),
        &Value::F32(x) => float32(out, x),
        &Value::F64(x) => {
            if x.is_finite() {
                let text = serde_json::to_string(&x).expect("a finite float is a number");
                out.push_str(&text);
            } else {
                not_finite(out, x.is_nan(), x > 0.0);
            }
        }
        Value::String(text) => string(out, text),
        Value::Vec2(v) => floats(out, &v.to_array()),
        Value::Vec3(v) => floats(out, &v.to_array()),
        Value::Vec4(v) => floats(out, &v.to_array()),
        Value::Quat(q) => floats(out, &q.to_array()),
        &Value::Entity(entity) => match world.id(entity) {
            Some(id) => string(out, &id.to_string()),
            None => out.push_str("null"),
        },
        &Value::Texture(texture) => match world.assets().path(texture) {
            Some(path) => string(out, path),
            None => out.push_str("null"),
        },
        Value::Option(None) => out.push_str("null"),
        Value::Option(Some(value)) => write(out, value, world),
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(out, item, world);
            }
            out.push(']');
        }
        Value::Struct(fields) => object(out, fields, world),
        Value::Variant(name, fields) if fields.is_empty() => string(out, name),
        Value::Variant(name, fields) => {
            out.push('{');
            string(out, name);
            out.push(':');
            object(out, fields, world);
            out.push('}');
        }
        Value::Opaque => out.push_str("\"(opaque)\""),
    }
}

/// The value `json` gives a field of type `ty`, as gg-api's docs describe: with the entities it
/// names by their IDs, alive in `world`, or `null` for a dead one, as one that isn't alive writes,
/// and the textures by their paths, among the assets `world`'s layers added; a struct, or a
/// variant's fields, missing a field takes its default, or is opaque, which keeps or makes it as its
/// type's own is.
///
/// Fails, saying where in the value, with `reflect.wrong_kind` for JSON of another kind than the
/// type's, `reflect.no_variant` and `reflect.no_field` for a variant or a field the type hasn't,
/// `ecs.bad_id` and `ecs.no_entity` for an ID that isn't one or names no living entity,
/// `api.no_texture` for a path no texture was added from, and `api.too_deep` for a value nested
/// more than a scene's values may be.
pub(super) fn read(json: &JsonValue, ty: &Type, world: &World) -> Result<Value> {
    read_at(json, ty, world, 1)
}

/// As [`read`], for a value `depth` levels deep, the first being a component's.
fn read_at(json: &JsonValue, ty: &Type, world: &World, depth: u32) -> Result<Value> {
    if depth > MAX_DEPTH {
        let message =
            format!("a value more than {MAX_DEPTH} levels deep, the most a scene's may be");
        return Err(Error::new("api.too_deep", message));
    }
    let types = world.types();
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
    let floats = |n: usize| -> Result<Vec<f32>> {
        let expected = format!("an array of {n} numbers");
        match json {
            JsonValue::Array(items) if items.len() == n => items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    let x = float(item).map_err(|error| reflect::within(error, &format!("[{i}]")));
                    x.map(|x| x as f32)
                })
                .collect(),
            _ => Err(wrong(&expected)),
        }
    };
    Ok(match ty {
        Type::Bool => match json {
            &JsonValue::Bool(b) => Value::Bool(b),
            _ => return Err(wrong("a boolean")),
        },
        Type::Int { .. } => match json {
            JsonValue::Number(n) => match (n.as_u64(), n.as_i64()) {
                (Some(n), _) => Value::UInt(n),
                (None, Some(n)) => Value::Int(n),
                (None, None) => return Err(wrong("a whole number")),
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
        Type::Quat => Value::Quat(Quat::from_slice(&floats(4)?)),
        Type::Entity => match json {
            JsonValue::String(text) => {
                let id: EntityId = text.parse()?;
                let entity = world.entity(id).ok_or_else(|| {
                    Error::new("ecs.no_entity", format!("no living entity has ID {id}"))
                })?;
                Value::Entity(entity)
            }
            // As a reference to one that isn't alive writes.
            JsonValue::Null => Value::Entity(Entity::DEAD),
            _ => return Err(wrong("an entity's ID, a string, or null for none alive")),
        },
        Type::Texture => match json {
            JsonValue::String(path) => {
                let texture = world.assets().texture(path).ok_or_else(|| {
                    let message = format!(
                        "no texture was added from `{path}`, a path as the app's root names it"
                    );
                    Error::new("api.no_texture", message)
                })?;
                Value::Texture(texture)
            }
            _ => return Err(wrong("a texture's path, a string")),
        },
        Type::Option(inner) => match json {
            JsonValue::Null => Value::Option(None),
            json => Value::Option(Some(Box::new(read_at(json, inner, world, depth + 1)?))),
        },
        Type::List(inner) => match json {
            JsonValue::Array(items) => Value::List(list(items, inner, world, depth)?),
            _ => return Err(wrong("an array")),
        },
        Type::Array(inner, n) => match json {
            JsonValue::Array(items) if items.len() == *n as usize => {
                Value::List(list(items, inner, world, depth)?)
            }
            _ => return Err(wrong(&format!("an array of {n}"))),
        },
        Type::Opaque => opaque(json, &wrong)?,
        &Type::Named(index) => match &types.info(index).shape {
            Shape::Struct { fields, .. } => match json {
                JsonValue::Object(given) => {
                    Value::Struct(object_fields(given, fields, world, depth)?)
                }
                _ => return Err(wrong("an object of the struct's fields")),
            },
            Shape::Enum(variants) => {
                let names = || variants.iter().map(|variant| variant.name.as_ref());
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
                    return Err(reflect::no_variant(name, &names().collect::<Vec<_>>()));
                };
                let fields = match given {
                    None => object_fields(&Map::new(), &variant.fields, world, depth)?,
                    Some(JsonValue::Object(given)) => {
                        object_fields(given, &variant.fields, world, depth)
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
fn list(items: &[JsonValue], ty: &Type, world: &World, depth: u32) -> Result<Vec<Value>> {
    let items = items.iter().enumerate().map(|(i, item)| {
        read_at(item, ty, world, depth + 1)
            .map_err(|error| reflect::within(error, &format!("[{i}]")))
    });
    items.collect()
}

/// The fields `fields` describes, of a struct or a variant a level above them, each from `given`'s
/// key of its name, or its default without one, or opaque if it's opaque.
///
/// Fails with `reflect.no_field` for a key that isn't a field's.
fn object_fields(
    given: &Map<String, JsonValue>,
    fields: &[Field],
    world: &World,
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
    let types = world.types();
    let fields = fields.iter().map(|field| {
        let value = match given.get(field.name.as_ref()) {
            Some(json) => read_at(json, &field.ty, world, depth + 1),
            None if field.ty.is_opaque(types) => Ok(Value::Opaque),
            None => types.field_default(field),
        };
        let value = value.map_err(|error| reflect::within(error, &field.name));
        value.map(|value| (field.name.clone(), value))
    });
    fields.collect()
}

/// What's opaque, which only `(opaque)` gives: kept as it is, or made as its type's own is.
fn opaque(json: &JsonValue, wrong: &dyn Fn(&str) -> Error) -> Result<Value> {
    match json {
        JsonValue::String(text) if text == "(opaque)" => Ok(Value::Opaque),
        _ => Err(wrong("`(opaque)`, since what's opaque isn't read or set")),
    }
}

/// Writes `text` as a JSON string.
pub(super) fn string(out: &mut String, text: &str) {
    out.push_str(&serde_json::to_string(text).expect("a string is a JSON string"));
}

/// Writes `fields` as an object, in their order.
fn object(out: &mut String, fields: &[(Ident, Value)], world: &World) {
    out.push('{');
    for (i, (name, value)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        string(out, name);
        out.push(':');
        write(out, value, world);
    }
    out.push('}');
}

/// Writes `xs` as an array of numbers.
fn floats(out: &mut String, xs: &[f32]) {
    out.push('[');
    for (i, &x) in xs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        float32(out, x);
    }
    out.push(']');
}

/// Writes `x` as the shortest number that reads back to its bits as an `f32`, as `0.1` for 0.1,
/// rather than as the `f64` it widens to.
fn float32(out: &mut String, x: f32) {
    if x.is_finite() {
        let text = serde_json::to_string(&x).expect("a finite float is a number");
        out.push_str(&text);
    } else {
        not_finite(out, x.is_nan(), x > 0.0);
    }
}

/// Writes a float that isn't finite, which JSON has no number for: `NaN`, `inf` or `-inf`, as a
/// string.
fn not_finite(out: &mut String, nan: bool, positive: bool) {
    out.push_str(match (nan, positive) {
        (true, _) => "\"NaN\"",
        (false, true) => "\"inf\"",
        (false, false) => "\"-inf\"",
    });
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use gg_core::math::{Quat, Vec3};

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

    /// A component's value written as JSON reads back by its type to a value that sets it as it
    /// was; fields left out take their defaults, and JSON of the wrong kind is refused where it is.
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
        // Left out, a field takes its default; what's opaque, its own.
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
        // `null`, as a dead entity writes, reads back as one.
        let none = super::read(&JsonValue::Null, &Type::Entity, &world).expect("read");
        assert_eq!(none, Value::Entity(Entity::DEAD));
        // A texture by the path a layer added it from, which none here has.
        world.register::<crate::Sprite>().expect("registered");
        let sprite = Type::Named(world.types().find("Sprite").expect("registered"));
        let error =
            read_json(&world, &sprite, r#"{"texture": "assets/none.png"}"#).expect_err("none");
        assert_eq!(error.code(), "api.no_texture", "{error}");
    }

    fn read_json(world: &World, ty: &Type, json: &str) -> Result<Value> {
        read(&serde_json::from_str(json).expect("JSON"), ty, world)
    }
}
