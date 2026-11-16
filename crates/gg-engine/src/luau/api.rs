//! The rest of what scripts call: `print`, `tostring`, casts, paths, tweens, clips, params.

use std::fmt::Write as _;

use gg_core::Error;
use gg_core::math::{Quat, Vec2, Vec3, Vec4};
use mlua::{Function, LightUserData, Lua, LuaString, Table, Value as Lua_, Vector};
use tracing::info;

use super::fields::{
    bits, entity_of, field_of, handle, missing, named, quat_of, reaching, vec3_of, vector3_of,
};
#[cfg(doc)]
use super::{Mods, Shared};
use super::{denied, entities, shared};
use crate::grant::Capability;
use crate::reflect::{Reflect, Value};
use crate::{Cast, Curve, Ease, Hit3D, ParamValue, Playback, Shape3D, Tween, TweenValue, World};

/// The most bytes of a line a script prints, past which it's cut, saying so: so that a mod's
/// lines, each up to as many bytes as its VM may hold, can't fill the log.
pub(super) const PRINTED: usize = 4096;

/// `print(...)`: logs what it's given, as `tostring` writes each, between tabs, cut at
/// [`PRINTED`] bytes, as [`printed`] cuts it; a mod's, naming it, unless it has printed as much as
/// it may this second of ticks, as [`Mods`] says. `luau` is Luau's own `tostring`.
pub(super) fn print(lua: &Lua, luau: &Function, values: &[Lua_]) -> mlua::Result<()> {
    shared(|shared| {
        if shared.held.as_ref().is_some_and(|held| !held.prints()) {
            return Ok(());
        }
        let line = printed(lua, luau, values)?;
        match &shared.held {
            Some(held) => info!("mod" = held.name, "{line}"),
            None => info!("{line}"),
        }
        Ok(())
    })
}

/// The line `print` logs of `values`: each as `tostring` writes it, as [`written`] says, `luau`
/// being Luau's own, between tabs, but cut at [`PRINTED`] bytes, at a character's start, saying so
/// and how many bytes the line had; of a string past them, only as many of its bytes as fit are
/// read.
pub(super) fn printed(lua: &Lua, luau: &Function, values: &[Lua_]) -> mlua::Result<String> {
    let mut line = String::new();
    let mut bytes = 0;
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            line.push('\t');
            bytes += 1;
        }
        let text = match value {
            Lua_::String(string) => string.clone(),
            value => written(lua, luau, value.clone())?,
        };
        let text = text.as_bytes();
        bytes += text.len();
        let room = (PRINTED + 1).saturating_sub(line.len()).min(text.len());
        line.push_str(&String::from_utf8_lossy(&text[..room]));
    }
    if bytes > PRINTED {
        let mut end = PRINTED.min(line.len());
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        write!(line, " [cut at {PRINTED} bytes of {bytes}]").expect("a string takes any text");
    }
    Ok(line)
}

/// `tostring(value)`: as `luau`, Luau's own, writes it, a `__tostring` metamethod's text where it
/// has one, but a table, function, thread, buffer or userdata without one by its type's name
/// alone, as `table`, or userdata by its metatable's `__type`, if it's a string, where Luau's writes
/// its address, which differs from run to run and from peer to peer, so that a script whose state
/// took it would desync them. Not a count of the objects in the order the VM made them, which
/// would be the same within a run: a lockstep joiner's VM, or a run restored from a snapshot,
/// starts afresh, having made none of those its host's did before, and a tick run again, as
/// rollback runs it, makes them again. The rest Luau's writes, as its builtin, which takes nil,
/// booleans, numbers and strings, writes them where it can, alike.
pub(super) fn written(lua: &Lua, luau: &Function, value: Lua_) -> mlua::Result<LuaString> {
    let has = |field: &Lua_| !field.is_nil();
    let name = match &value {
        Lua_::Table(table) => {
            let to = table
                .metatable()
                .map(|metatable| metatable.raw_get("__tostring"));
            match to.transpose()? {
                Some(to) if has(&to) => None,
                _ => Some(Lua_::Nil),
            }
        }
        Lua_::UserData(data) => match data.metatable() {
            Ok(metatable) if has(&metatable.get("__tostring")?) => None,
            Ok(metatable) => Some(metatable.get("__type")?),
            Err(_) => Some(Lua_::Nil),
        },
        Lua_::Function(_) | Lua_::Thread(_) | Lua_::Buffer(_) => Some(Lua_::Nil),
        _ => None,
    };
    match name {
        Some(Lua_::String(name)) => Ok(name),
        Some(_) => lua.create_string(value.type_name()),
        None => luau.call(value),
    }
}

/// The end of each cast's arguments, as a script gives them: where it starts, which way it goes,
/// how far, none without end, and an entity whose colliders it passes through, if any but
/// `gg.entity`.
type CastArgs = (Vector, Vector, Option<f64>, Option<LightUserData>);

/// What a cast gives a script: the entity it hit, where, the way the surface faces there and how
/// far along the cast, or nil for each, as it hit nothing.
type CastHit = (Lua_, Lua_, Lua_, Lua_);

/// `gg.cast_ray(from, toward, distance, exclude)`: what a ray hits first of the world's 3D
/// colliders, as [`World::cast_ray`] says.
pub(super) fn cast_ray(_: &Lua, args: CastArgs) -> mlua::Result<CastHit> {
    cast(args, |world, cast| world.cast_ray(cast))
}

/// `gg.cast_sphere(radius, from, toward, distance, exclude)`: what a sphere swept along a line hits
/// first, as [`World::cast_shape`] says.
pub(super) fn cast_sphere(_: &Lua, (radius, args): (f64, CastArgs)) -> mlua::Result<CastHit> {
    let shape = Shape3D::Sphere {
        radius: radius as f32,
    };
    cast(args, |world, cast| {
        world.cast_shape(shape, Quat::IDENTITY, cast)
    })
}

/// `gg.cast_box(size, rotation, from, toward, distance, exclude)`: what a box, turned, swept along
/// a line hits first, as [`World::cast_shape`] says.
pub(super) fn cast_box(
    _: &Lua,
    (size, rotation, args): (Vector, Vector, CastArgs),
) -> mlua::Result<CastHit> {
    let shape = Shape3D::Box {
        size: vec3_of(size),
    };
    cast(args, |world, cast| {
        world.cast_shape(shape, quat_of(rotation), cast)
    })
}

/// `gg.cast_capsule(height, radius, rotation, from, toward, distance, exclude)`: what a capsule,
/// upright along its Y and turned, swept along a line hits first, as [`World::cast_shape`] says.
pub(super) fn cast_capsule(
    _: &Lua,
    (height, radius, rotation, args): (f64, f64, Vector, CastArgs),
) -> mlua::Result<CastHit> {
    let shape = Shape3D::Capsule {
        height: height as f32,
        radius: radius as f32,
    };
    cast(args, |world, cast| {
        world.cast_shape(shape, quat_of(rotation), cast)
    })
}

/// What `run` hits of the world scripts reach with the cast `args` give, for a script: a mod's
/// only if it may read the whole world, as where each collider is tells of every entity with one.
fn cast(
    (from, toward, distance, exclude): CastArgs,
    run: impl FnOnce(&World, Cast) -> Option<Hit3D>,
) -> mlua::Result<CastHit> {
    shared(|shared| {
        if let Some(held) = &shared.held
            && !held.grant.has(&Capability::Read(None))
        {
            let what = "a cast, which reads where every collider is,";
            return Err(denied("read", what));
        }
        let mut cast = Cast::new(vec3_of(from).as_dvec3(), vec3_of(toward));
        if let Some(distance) = distance {
            cast = cast.within(distance as f32);
        }
        if let Some(exclude) = exclude.filter(|&data| bits(data) != 0) {
            cast = cast.excluding(entity_of(exclude)?);
        }
        let world = shared.world.borrow();
        Ok(match run(&world, cast) {
            Some(hit) => (
                Lua_::LightUserData(handle(hit.entity.to_bits())),
                Lua_::Vector(vector3_of(hit.point.as_vec3())),
                Lua_::Vector(vector3_of(hit.normal)),
                Lua_::Number(f64::from(hit.distance)),
            ),
            None => (Lua_::Nil, Lua_::Nil, Lua_::Nil, Lua_::Nil),
        })
    })
}

/// The most parameters' names a VM keeps for `gg.set_param`, as [`Shared`]'s `params` says: as many
/// as a game's state machines name many times over, few enough that searching them in turn costs
/// little, and that a script's names, which past them it may give without end, hold little.
pub(super) const PARAMS: usize = 64;

/// `gg.find_path(from, to)`: the corners of the shortest path over the world's navmesh from `from`
/// to `to`, as [`World::find_path`] says, as an array of vectors, or nil where no path joins them;
/// for a mod's script only if its mod may read the whole world, as the navmesh tells where every
/// static collider is.
pub(super) fn find_path(lua: &Lua, (from, to): (Vector, Vector)) -> mlua::Result<Lua_> {
    shared(|shared| {
        if let Some(held) = &shared.held
            && !held.grant.has(&Capability::Read(None))
        {
            let what = "a path, which reads where every static collider is,";
            return Err(denied("read", what));
        }
        let mut world = shared.world.borrow_mut();
        let Some(corners) = world.find_path(vec3_of(from), vec3_of(to)) else {
            return Ok(Lua_::Nil);
        };
        let path = lua.create_table_with_capacity(corners.len(), 0)?;
        for (i, corner) in (1..).zip(corners) {
            path.raw_set(i, Lua_::Vector(vector3_of(corner)))?;
        }
        Ok(Lua_::Table(path))
    })
}

/// What a script gives `gg.tween`: the entity, the handle of its field, where the field goes, in
/// how many seconds, and the table of the rest, if any.
type TweenArgs = (LightUserData, Lua_, Lua_, f64, Option<Table>);

/// `gg.tween(entity, field, to, seconds, options)`: starts a tween of `entity`'s field whose handle
/// is `field` from what it holds to `to`, as the scripts' doc says.
pub(super) fn tween(_: &Lua, (entity, key, to, seconds, options): TweenArgs) -> mlua::Result<()> {
    shared(|shared| {
        entities::ticking(shared, "starts tweens")?;
        let entity = entity_of(entity)?;
        let fields = shared.fields.borrow();
        let field = field_of(&fields, &key)?;
        let (component, name) = (&field.component, &field.name);
        if !field.writes {
            let what = format!("tweening `{component}.{name}`");
            let capability = format!("write:{component}");
            return Err(denied(&capability, &what));
        }
        if let Some(held) = &shared.held
            && !held.grant.writes("Tweens")
        {
            let what = format!("tweening `{component}.{name}`, which its entity's `Tweens` holds,");
            return Err(denied("write:Tweens", &what));
        }
        let mut world = shared.world.borrow_mut();
        let reached = world.field_at(entity, missing(field)?, field.index);
        let from = reached.map_err(|error| reaching(&world, entity, field, &error))?;
        let from = TweenValue::of(&from.to_value()).ok_or_else(|| {
            mlua::Error::runtime(format!(
                "`{component}.{name}` isn't a number, a vector or a rotation, which tweens ease"
            ))
        })?;
        let to = match (from, &to) {
            (TweenValue::Number(_), &Lua_::Number(x)) => TweenValue::Number(x),
            (TweenValue::Number(_), &Lua_::Integer(n)) => TweenValue::Number(n as f64),
            (TweenValue::Vec2(_), Lua_::Vector(v)) => TweenValue::Vec2(Vec2::new(v.x(), v.y())),
            (TweenValue::Vec3(_), Lua_::Vector(v)) => TweenValue::Vec3(vec3_of(*v).as_dvec3()),
            (TweenValue::Vec4(_), Lua_::Vector(v)) => {
                TweenValue::Vec4(Vec4::new(v.x(), v.y(), v.z(), v.w()))
            }
            (TweenValue::Quat(_), Lua_::Vector(v)) => TweenValue::Quat(quat_of(*v)),
            (TweenValue::Number(_), to) => {
                let got = to.type_name();
                let message = format!("`{component}.{name}` goes to a number, not a {got}");
                return Err(mlua::Error::runtime(message));
            }
            (_, to) => {
                let got = to.type_name();
                let message = format!("`{component}.{name}` goes to a vector, not a {got}");
                return Err(mlua::Error::runtime(message));
            }
        };
        let mut tween = Tween::new(component, name, from, to, seconds as f32);
        if let Some(options) = options {
            for pair in options.pairs::<Lua_, Lua_>() {
                let (key, value) = pair?;
                let key = key.to_string()?;
                let named = |kind: &str| match &value {
                    Lua_::String(text) => {
                        Ok(Value::Variant(text.to_str()?.to_owned().into(), Vec::new()))
                    }
                    other => Err(mlua::Error::runtime(format!(
                        "a tween's `{key}` is the name of a {kind}, as a string, not a {}",
                        other.type_name()
                    ))),
                };
                let refused =
                    |error: Error| mlua::Error::runtime(format!("a tween's `{key}`: {error:#}"));
                match &*key {
                    "curve" => {
                        tween.curve = Curve::from_value(&named("Curve")?).map_err(refused)?
                    }
                    "ease" => tween.ease = Ease::from_value(&named("Ease")?).map_err(refused)?,
                    "playback" => {
                        tween.playback =
                            Playback::from_value(&named("Playback")?).map_err(refused)?;
                    }
                    "delay" => match value {
                        Lua_::Number(x) => tween.time = -(x as f32),
                        Lua_::Integer(n) => tween.time = -(n as f32),
                        ref other => {
                            return Err(mlua::Error::runtime(format!(
                                "a tween's `delay` is a number of seconds, not a {}",
                                other.type_name()
                            )));
                        }
                    },
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "a tween has no `{other}`: its options are `curve`, `ease`, \
                             `playback` and `delay`"
                        )));
                    }
                }
            }
        }
        let started = world.tween(entity, tween);
        started.map_err(|error| reaching(&world, entity, field, &error))
    })
}

/// `gg.play(entity, clip, fade)`: has `entity`, a model's root, play its clip `clip`, cross-fading
/// over `fade` seconds, or at once without them, as the scripts' doc says.
pub(super) fn play(
    _: &Lua,
    (entity, clip, fade): (LightUserData, String, Option<f64>),
) -> mlua::Result<()> {
    shared(|shared| {
        entities::ticking(shared, "plays clips")?;
        let entity = entity_of(entity)?;
        if let Some(held) = &shared.held
            && !held.grant.writes("Animator")
        {
            let what = format!("playing clip {clip:?}, which its root's `Animator` holds,");
            return Err(denied("write:Animator", &what));
        }
        let mut world = shared.world.borrow_mut();
        let played = world.play(entity, &clip, fade.unwrap_or(0.0) as f32);
        played.map_err(|error| {
            let message = format!(
                "can't play clip {clip:?} on entity {}: {error:#}",
                named(&world, entity)
            );
            mlua::Error::external(Error::new(error.code(), message))
        })
    })
}

/// `gg.set_param(entity, name, value)`: sets parameter `name` of `entity`'s state machine to
/// `value`, a number or a boolean, as the scripts' doc says.
pub(super) fn set_param(
    _: &Lua,
    (entity, name, value): (LightUserData, LuaString, Lua_),
) -> mlua::Result<()> {
    shared(|shared| {
        entities::ticking(shared, "sets parameters")?;
        let entity = entity_of(entity)?;
        if let Some(held) = &shared.held
            && !held.grant.writes("AnimationStates")
        {
            let what = format!(
                "setting parameter \"{}\", which its entity's `AnimationStates` holds,",
                name.display()
            );
            return Err(denied("write:AnimationStates", &what));
        }
        let value = match value {
            Lua_::Number(x) => ParamValue::Float(x as f32),
            Lua_::Integer(n) => ParamValue::Float(n as f32),
            Lua_::Boolean(set) => ParamValue::Bool(set),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "parameter \"{}\" is set to a number or a boolean, not a {}",
                    name.display(),
                    other.type_name()
                )));
            }
        };
        let mut params = shared.params.borrow_mut();
        let at = name.to_pointer();
        let known = params.iter().position(|(held, _)| held.to_pointer() == at);
        let unheld;
        let name = match known {
            Some(known) => params[known].1.as_str(),
            None if params.len() < PARAMS => {
                let text = name.to_str()?.to_owned();
                params.push((name, text));
                params[params.len() - 1].1.as_str()
            }
            None => {
                unheld = name.to_str()?;
                &*unheld
            }
        };
        let mut world = shared.world.borrow_mut();
        let set = world.set_param(entity, name, value);
        set.map_err(|error| {
            let message = format!(
                "can't set parameter \"{name}\" of entity {}: {error:#}",
                named(&world, entity)
            );
            mlua::Error::external(Error::new(error.code(), message))
        })
    })
}

/// The smallest of an axis' components, in absolute value, that is one: below it, a rotation's
/// axis is no direction.
const AXIS_LEAST: f32 = 1e-6;

/// `axis`, of the rotation of `angle` radians that `gg.quat.from_axis_angle` makes, as a unit
/// vector: scaled by its largest component first, so that an axis of huge components doesn't
/// overflow as its length is found.
///
/// Fails with `script.bad_argument` for an axis of a component that isn't finite, or of none past
/// [`AXIS_LEAST`], and for an angle that isn't finite.
pub(super) fn unit_axis(axis: Vec3, angle: f64) -> mlua::Result<Vec3> {
    let refuse =
        |message: String| mlua::Error::external(Error::new("script.bad_argument", message));
    if !angle.is_finite() {
        return Err(refuse(format!(
            "`gg.quat.from_axis_angle`'s angle is a finite number of radians, not {angle}"
        )));
    }
    if !axis.is_finite() {
        return Err(refuse(format!(
            "`gg.quat.from_axis_angle`'s axis is a vector of finite components, not {axis}"
        )));
    }
    let largest = axis.abs().max_element();
    if largest < AXIS_LEAST {
        return Err(refuse(format!(
            "`gg.quat.from_axis_angle`'s axis is a direction, not {axis}, which is as good as zero"
        )));
    }
    Ok((axis / largest).normalize())
}
