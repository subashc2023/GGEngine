//! Components' fields as scripts reach them: handles, their reads and writes, and values.

use std::any::Any;
use std::ptr;

use gg_core::math::{Quat, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use mlua::{LightUserData, Lua, LuaString, Table, Value as Lua_, Vector};

#[cfg(doc)]
use super::Scripts;
use super::host::doc;
use super::{Declaring, Field, FieldRef, Loading, Shared, denied, entities, shared, ungranted};
use crate::ecs::{ComponentId, Kind};
use crate::reflect::{Ident, Reflect, Type, Value};
use crate::{Entity, World};

/// Light userdata of `bits`: an entity's, a field's handle's, or `gg.entity`'s, 0.
pub(super) fn handle(bits: u64) -> LightUserData {
    LightUserData(ptr::without_provenance_mut(bits as usize))
}

/// The bits light userdata holds.
pub(super) fn bits(data: LightUserData) -> u64 {
    data.0.addr() as u64
}

/// The entity `data` is.
pub(super) fn entity_of(data: LightUserData) -> mlua::Result<Entity> {
    Entity::from_bits(bits(data)).ok_or_else(|| {
        mlua::Error::runtime("that's a field's handle, not an entity, whose fields it reaches")
    })
}

/// The field of `fields`, in the order of their handles' bits, that `key` is the handle of.
pub(super) fn field_of<'f>(fields: &'f [FieldRef], key: &Lua_) -> mlua::Result<&'f FieldRef> {
    let at = match key {
        &Lua_::LightUserData(data) if bits(data) >> 32 == 0 => {
            let bits = bits(data) as u32;
            fields
                .binary_search_by_key(&bits, |field| field.handle)
                .ok()
        }
        _ => None,
    };
    at.map(|at| &fields[at]).ok_or_else(|| {
        mlua::Error::runtime(
            "an entity's fields are reached by handles, such as `gg.component(\"Shard\").angle`",
        )
    })
}

/// The handle of field `name` of component type `id`, named `component`, which is its field `index`
/// and holds `ty`, and which the scripts may read and write as `may` says: the one `fields` has, or
/// one made, kept in the order of their handles' bits.
///
/// Its bits are the low 32, never 0, which no entity's are, as [`Entity::to_bits`] says, and are
/// those of the field itself, as [`hashed`] gives them: not its place among the handles the VM
/// made before, which would go by what the VM ran, so that a table the handles key, which Luau
/// orders by their bits' hashes, iterates alike in a lockstep host's VM, which may have loaded and
/// reloaded scripts as it ran, and in its joiner's, which starts afresh, as in a run restored from
/// a snapshot and in a replay. Two fields whose hashes are the same, about one pair in 2^32, take
/// the next bits free in the order the VM made them, which alone goes by what it ran.
pub(super) fn handle_of(
    fields: &mut Vec<FieldRef>,
    component: &str,
    id: ComponentId,
    (index, name, ty): (u32, &Ident, Field),
    (reads, writes): (bool, bool),
) -> LightUserData {
    let same =
        |field: &&FieldRef| field.component == component && field.name == *name && field.ty == ty;
    if let Some(field) = fields.iter().find(same) {
        return handle(u64::from(field.handle));
    }
    let mut bits = hashed(component, name, ty);
    while bits == 0
        || fields
            .binary_search_by_key(&bits, |field| field.handle)
            .is_ok()
    {
        bits = bits.wrapping_add(1);
    }
    let at = fields.partition_point(|field| field.handle < bits);
    let field = FieldRef {
        handle: bits,
        component: Ident::Owned(component.to_owned()),
        id: Some(id),
        index,
        name: name.clone(),
        ty,
        reads,
        writes,
    };
    fields.insert(at, field);
    handle(u64::from(bits))
}

/// A hash of field `name` of the component type named `component`, which holds `ty`: 32-bit
/// FNV-1a, of the names' bytes, each ended by a 0, which no name holds, and of what it holds.
fn hashed(component: &str, name: &str, ty: Field) -> u32 {
    let held = match ty {
        Field::Bool => 0,
        Field::F32 => 1,
        Field::F64 => 2,
        Field::Int { signed: false } => 3,
        Field::Int { signed: true } => 4,
        Field::Vec2 => 5,
        Field::Vec3 => 6,
        Field::Vec4 => 7,
        Field::Quat => 8,
        Field::Entity => 9,
        Field::MaybeEntity => 10,
        Field::MaybeVec3 => 11,
        Field::DVec3 => 12,
    };
    let names = component.bytes().chain([0]).chain(name.bytes()).chain([0]);
    names.chain([held]).fold(0x811c_9dc5, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// `gg.component(name)`: a table of the handles of component type `name`'s fields, by name, those
/// of the kinds scripts reach; for a mod's scripts, of a type they may read or write, as
/// [`Shared::may`] says, or a refusal, `mod.ungranted`, of the script loading.
pub(super) fn component(lua: &Lua, name: LuaString) -> mlua::Result<Table> {
    shared(|shared| {
        let world = shared.world.borrow();
        let name = name.to_str()?;
        if shared.may(&name) == (false, false) {
            let what = format!("`gg.component(\"{}\")`", &*name);
            let message = ungranted(&format!("read:{}` or `write:{}", &*name, &*name), &what);
            if let Some(loading) = shared.loading.borrow_mut().as_mut() {
                let error = Error::new("mod.ungranted", format!("{}: {message}", loading.path));
                loading.refused = Some(error);
            }
            return Err(mlua::Error::external(Error::new("mod.ungranted", message)));
        }
        let Some(id) = world.component_named(&name) else {
            let names = crate::reflect::list(world.component_types());
            return Err(mlua::Error::runtime(format!(
                "there's no component type `{}`; the component types are {names}",
                &*name
            )));
        };
        let fields = (world.fields_of(id).iter()).map(|field| (&field.name, field.ty.clone()));
        let table = handles(lua, shared, &name, id, fields)?;
        table.set_readonly(true);
        Ok(table)
    })
}

/// A table of the handles of `fields`, each a name and a type, of component type `id`, which is
/// named `name`: its fields, in their order.
fn handles<'f>(
    lua: &Lua,
    shared: &Shared,
    name: &str,
    id: ComponentId,
    fields: impl Iterator<Item = (&'f Ident, Type)>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let may = shared.may(name);
    let mut refs = shared.fields.borrow_mut();
    for (index, (field, ty)) in fields.enumerate() {
        let Some(ty) = Field::of(&ty) else {
            continue;
        };
        let index = u32::try_from(index).expect("fewer than 2^32 fields");
        let handle = handle_of(&mut refs, name, id, (index, field, ty), may);
        table.raw_set(&**field, handle)?;
    }
    Ok(table)
}

/// `gg.declare(name, fields, options)`: declares the component type the script loading is, `name`,
/// of `fields`, each a name and a default, riding on the type `options`' `on` names, if it names
/// one, as [`Scripts`] says, and returns the script's table, of its fields' handles, which its tick
/// goes in. A script loading again declares the type it declared, its fields as they were or
/// otherwise, which the type takes once it has loaded. A mod's scripts' types are their own, and
/// a type they ride on is one they may read.
pub(super) fn declare(
    lua: &Lua,
    (name, fields, options): (LuaString, Table, Option<Table>),
) -> mlua::Result<Table> {
    shared(|shared| {
        let name = name.to_str()?.to_owned();
        let mut loading = shared.loading.borrow_mut();
        let Some(loading) = loading.as_mut() else {
            return Err(mlua::Error::runtime(
                "a script declares its component as it loads",
            ));
        };
        let refuse = |loading: &mut Loading, code: &'static str, what: String| {
            let error = Error::new(code, format!("{}: {what}", loading.path));
            let message = error.to_string();
            loading.refused = Some(error);
            Err(mlua::Error::runtime(message))
        };
        if let Some(declaring) = &loading.declared {
            let declared = &declaring.component;
            let what = format!("it declares `{name}`, but `{declared}` already: a script is one");
            return refuse(loading, "script.bad", what);
        }
        if let Some(before) = loading.before.as_ref().filter(|before| **before != name) {
            let what = format!(
                "it declares `{name}`, where it declared `{before}`, which a reload doesn't \
                 rename: a run started afresh does"
            );
            return refuse(loading, "script.renamed", what);
        }
        let mut declared: Vec<(Ident, Kind, Value)> = Vec::new();
        for pair in fields.pairs::<Lua_, Lua_>() {
            let (field, default) = pair?;
            let Lua_::String(field) = field else {
                let what = format!("`{name}` has a field named by a {}", field.type_name());
                return refuse(loading, "script.bad", what);
            };
            let field = field.to_str()?.to_owned();
            if field == "tick" {
                let what = format!("`{name}.tick` is the script's tick, not a field");
                return refuse(loading, "script.bad", what);
            }
            let (kind, default) = match default {
                Lua_::Boolean(b) => (Kind::Bool, Value::Bool(b)),
                Lua_::Integer(n) => (Kind::Number, Value::F64(n as f64)),
                Lua_::Number(x) => (Kind::Number, Value::F64(x)),
                Lua_::Vector(v) => (Kind::Vector, Value::Vec3(Vec3::new(v.x(), v.y(), v.z()))),
                Lua_::LightUserData(data) if bits(data) == 0 => (Kind::Entity, Value::Option(None)),
                other => {
                    let what = format!(
                        "`{name}.{field}` defaults to a {}, where a field's default is a number, \
                         a bool, a vector or `gg.entity`",
                        other.type_name()
                    );
                    return refuse(loading, "script.bad", what);
                }
            };
            declared.push((field.into(), kind, default));
        }
        // By name, as a table's pairs come in no order its text gives.
        declared.sort_by(|a, b| a.0.cmp(&b.0));
        let mut on = None;
        for pair in options.iter().flat_map(Table::pairs::<Lua_, Lua_>) {
            match pair? {
                (Lua_::String(key), Lua_::String(ridden)) if key.as_bytes() == b"on" => {
                    on = Some(ridden.to_str()?.to_owned());
                }
                _ => {
                    let what = format!(
                        "`{name}`'s options are `on`, the name of the component type it rides on"
                    );
                    return refuse(loading, "script.bad", what);
                }
            }
        }
        if let Some(ridden) = &on {
            if *ridden == name || shared.world.borrow().component_named(ridden).is_none() {
                let what = format!("`{name}` rides on `{ridden}`, which isn't another type");
                return refuse(loading, "script.bad", what);
            }
            if !shared.may(ridden).0 {
                let what = format!("`{name}` riding on `{ridden}`");
                return refuse(
                    loading,
                    "mod.ungranted",
                    ungranted(&format!("read:{ridden}"), &what),
                );
            }
        }
        let own_doc = doc(shared.held.as_ref(), &loading.path);
        let mut world = shared.world.borrow_mut();
        let (id, changed) = match world.component_named(&name) {
            // Another script's, which a mod's would take as its own.
            Some(id) if shared.held.is_some() && world.doc_of(id) != own_doc => {
                let what = format!("`{name}` is another script's component type");
                return refuse(loading, "reflect.name_taken", what);
            }
            None => match world.declare(&name, &own_doc, declared.clone()) {
                Ok(id) => (id, false),
                Err(error) => {
                    let message = format!("{}: {error:#}", loading.path);
                    let message = error.with_message(message);
                    let text = message.to_string();
                    loading.refused = Some(message);
                    return Err(mlua::Error::runtime(text));
                }
            },
            Some(id) => match world.declared(id) {
                Some(known) => (id, !known.declares(&declared)),
                None => {
                    let what = format!("`{name}` is a Rust component type's name");
                    return refuse(loading, "reflect.name_taken", what);
                }
            },
        };
        drop(world);
        if let Some(held) = &shared.held
            && !held.own.borrow().contains(&name)
        {
            held.own.borrow_mut().push(name.clone());
        }
        let own = declared.iter().map(|(field, kind, _)| (field, kind.ty()));
        let table = handles(lua, shared, &name, id, own)?;
        loading.declared = Some(Declaring {
            id,
            component: name,
            table: table.clone(),
            fields: declared,
            changed,
            on,
        });
        Ok(table)
    })
}

/// `entity[field]`: the field whose handle is `key`, of `entity`.
pub(super) fn read(_: &Lua, (entity, key): (LightUserData, Lua_)) -> mlua::Result<Lua_> {
    shared(|shared| {
        let entity = entity_of(entity)?;
        let fields = shared.fields.borrow();
        let field = field_of(&fields, &key)?;
        if !field.reads {
            let (component, name) = (&field.component, &field.name);
            let what = format!("reading `{component}.{name}`");
            return Err(denied(&format!("read:{component}"), &what));
        }
        let world = shared.world.borrow();
        let reached = world.field_at(entity, missing(field)?, field.index);
        let reached = reached.map_err(|error| reaching(&world, entity, field, &error))?;
        Ok(to_lua(reached, field.ty))
    })
}

/// `entity[field] = value`: sets the field whose handle is `key`, of `entity`, to `value`.
pub(super) fn write(
    _: &Lua,
    (entity, key, value): (LightUserData, Lua_, Lua_),
) -> mlua::Result<()> {
    shared(|shared| {
        entities::ticking(shared, "writes fields")?;
        let entity = entity_of(entity)?;
        let fields = shared.fields.borrow();
        let field = field_of(&fields, &key)?;
        if !field.writes {
            let (component, name) = (&field.component, &field.name);
            let what = format!("writing `{component}.{name}`");
            return Err(denied(&format!("write:{component}"), &what));
        }
        let mut world = shared.world.borrow_mut();
        let at = (entity, missing(field)?, field.index);
        let set = set(&mut world, at, field.ty, &value);
        set.map_err(|error| reaching(&world, entity, field, &error))
    })
}

/// `tostring(entity)`: the entity as the world prints it.
pub(super) fn describe(_: &Lua, data: LightUserData) -> mlua::Result<String> {
    Ok(match Entity::from_bits(bits(data)) {
        Some(entity) => format!("entity {entity}"),
        None if bits(data) == 0 => "gg.entity".to_owned(),
        None => "a field's handle".to_owned(),
    })
}

/// The id of the component type `field` is of, in the world scripts run in.
pub(super) fn missing(field: &FieldRef) -> mlua::Result<ComponentId> {
    field.id.ok_or_else(|| {
        mlua::Error::runtime(format!(
            "this world has no component type `{}` with field `{}` as it was",
            field.component, field.name
        ))
    })
}

/// The error that `field` of `entity` can't be reached in `world`, as `error` says, of its code,
/// such as `ecs.no_entity` for an entity despawned.
pub(super) fn reaching(
    world: &World,
    entity: Entity,
    field: &FieldRef,
    error: &Error,
) -> mlua::Error {
    mlua::Error::external(Error::new(
        error.code(),
        format!(
            "can't reach `{}.{}` of entity {}: {error:#}",
            field.component,
            field.name,
            named(world, entity)
        ),
    ))
}

/// How diagnostics name `entity` of `world`: by its ID while it's alive, and else by its handle.
pub(super) fn named(world: &World, entity: Entity) -> String {
    world
        .id(entity)
        .map_or(entity.to_string(), |id| id.to_string())
}

/// The value of `field`, which holds `ty`, as a script reads it: without allocating.
fn to_lua(field: &dyn Reflect, ty: Field) -> Lua_ {
    if ty == Field::MaybeEntity {
        let any: &dyn Any = field;
        let held = any
            .downcast_ref::<Option<Entity>>()
            .expect("an option of an entity");
        return held.map_or(Lua_::Nil, |entity| {
            Lua_::LightUserData(handle(entity.to_bits()))
        });
    }
    if ty == Field::MaybeVec3 {
        let any: &dyn Any = field;
        let held = any.downcast_ref::<Option<Vec3>>();
        let held = held.expect("an option of a vector");
        return held.map_or(Lua_::Nil, |v| Lua_::Vector(vector3_of(v)));
    }
    match field.to_value() {
        Value::Bool(b) => Lua_::Boolean(b),
        Value::Int(n) => Lua_::Number(n as f64),
        Value::UInt(n) => Lua_::Number(n as f64),
        Value::F32(x) => Lua_::Number(x.into()),
        Value::F64(x) => Lua_::Number(x),
        Value::Vec2(v) => Lua_::Vector(Vector::new(v.x, v.y, 0.0, 0.0)),
        Value::Vec3(v) => Lua_::Vector(Vector::new(v.x, v.y, v.z, 0.0)),
        Value::Vec4(v) => Lua_::Vector(Vector::new(v.x, v.y, v.z, v.w)),
        Value::DVec3(v) => Lua_::Vector(vector3_of(v.as_vec3())),
        Value::Quat(q) => Lua_::Vector(vector_of(q)),
        Value::Entity(entity) => Lua_::LightUserData(handle(entity.to_bits())),
        _ => unreachable!("scripts reach only fields of the types `Field` names"),
    }
}

/// Sets field `index` of `entity`'s component `id`, which holds `ty`, to `value`, a script's,
/// refused as [`World::field_to_set`] says: without allocating, but to fail.
pub(super) fn set(
    world: &mut World,
    (entity, id, index): (Entity, ComponentId, u32),
    ty: Field,
    value: &Lua_,
) -> Result<()> {
    match ty {
        Field::MaybeEntity => {
            let held = match value {
                Lua_::Nil => None,
                _ => Some(entity_in(value)?),
            };
            let any: &mut dyn Any = world.field_to_set(entity, id, index, None)?;
            *any.downcast_mut::<Option<Entity>>()
                .expect("an option of an entity") = held;
            Ok(())
        }
        Field::MaybeVec3 => {
            let held = match value {
                Lua_::Nil => None,
                _ => Some(vec3_of(vector_in(value)?)),
            };
            let checked = held.map(Value::Vec3);
            let any: &mut dyn Any = world.field_to_set(entity, id, index, checked.as_ref())?;
            *any.downcast_mut::<Option<Vec3>>()
                .expect("an option of a vector") = held;
            Ok(())
        }
        _ => {
            let value = value_of(ty, value)?;
            let field = world.field_to_set(entity, id, index, Some(&value))?;
            field.set_value(&value)
        }
    }
}

/// `value`, a script's, as a field that holds `ty` takes it, as reflection's value: nil for an
/// option's none. The bounds of the field aren't checked, which setting it or building its
/// component does.
///
/// Fails with `reflect.wrong_kind` for a value of another kind, and `reflect.out_of_range` for a
/// number that isn't an integer, or is negative, where the field holds an integer, or none.
pub(super) fn value_of(ty: Field, value: &Lua_) -> Result<Value> {
    let number = || match *value {
        Lua_::Number(x) => Ok(x),
        Lua_::Integer(n) => Ok(n as f64),
        _ => Err(wrong("a number", value)),
    };
    let vector = || vector_in(value);
    Ok(match ty {
        Field::MaybeEntity | Field::MaybeVec3 if value.is_nil() => Value::Option(None),
        Field::MaybeEntity => Value::Option(Some(Box::new(Value::Entity(entity_in(value)?)))),
        Field::MaybeVec3 => Value::Option(Some(Box::new(Value::Vec3(vec3_of(vector()?))))),
        Field::Bool => match *value {
            Lua_::Boolean(b) => Value::Bool(b),
            _ => return Err(wrong("a bool", value)),
        },
        Field::F32 => Value::F32(number()? as f32),
        Field::F64 => Value::F64(number()?),
        Field::Int { signed } => {
            let n = number()?;
            if n.fract() != 0.0 || !n.is_finite() {
                return Err(Error::new(
                    "reflect.out_of_range",
                    format!("{n} isn't an integer, which the field holds"),
                ));
            }
            if signed {
                Value::Int(n as i64)
            } else if n >= 0.0 {
                Value::UInt(n as u64)
            } else {
                return Err(Error::new(
                    "reflect.out_of_range",
                    format!("{n} is negative, where the field holds no sign"),
                ));
            }
        }
        Field::Vec2 => Value::Vec2(vector().map(|v| Vec2::new(v.x(), v.y()))?),
        Field::Vec3 => Value::Vec3(vector().map(|v| Vec3::new(v.x(), v.y(), v.z()))?),
        Field::Vec4 => Value::Vec4(vector().map(|v| Vec4::new(v.x(), v.y(), v.z(), v.w()))?),
        Field::DVec3 => Value::DVec3(vec3_of(vector()?).as_dvec3()),
        Field::Quat => Value::Quat(quat_of(vector()?)),
        Field::Entity => Value::Entity(entity_in(value)?),
    })
}

/// The error that `value`, a script's, is of another kind than `expected`.
fn wrong(expected: &str, value: &Lua_) -> Error {
    Error::new(
        "reflect.wrong_kind",
        format!("expected {expected}, got a {}", value.type_name()),
    )
}

/// The vector `value` is, as [`value_of`] takes one.
fn vector_in(value: &Lua_) -> Result<Vector> {
    match *value {
        Lua_::Vector(v) => Ok(v),
        _ => Err(wrong("a vector", value)),
    }
}

/// The entity `value` is, as [`value_of`] takes one.
fn entity_in(value: &Lua_) -> Result<Entity> {
    match *value {
        Lua_::LightUserData(data) => {
            Entity::from_bits(bits(data)).ok_or_else(|| wrong("an entity", value))
        }
        _ => Err(wrong("an entity", value)),
    }
}

/// `v`'s X, Y and Z, as a script's vector holds a point or a direction.
pub(super) fn vec3_of(v: Vector) -> Vec3 {
    Vec3::new(v.x(), v.y(), v.z())
}

/// `v` as a script holds a point or a direction, its W 0.
pub(super) fn vector3_of(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z, 0.0)
}

/// `q` as a script holds it, a vector: `x`, `y`, `z` and `w`.
pub(super) fn vector_of(q: Quat) -> Vector {
    Vector::new(q.x, q.y, q.z, q.w)
}

pub(super) fn quat_of(v: Vector) -> Quat {
    Quat::from_xyzw(v.x(), v.y(), v.z(), v.w())
}
