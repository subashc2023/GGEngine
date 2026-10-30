//! Reflection's values as the tool API writes them, as gg-api's docs describe: JSON written by hand,
//! so that a struct's fields keep their order, with the entities they hold by their IDs.

use std::fmt::Write as _;

use crate::World;
use crate::reflect::{Ident, Value};

/// Writes `value` as JSON, with the entities it holds by their IDs in `world`, `null` for one that
/// isn't alive there.
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
}
