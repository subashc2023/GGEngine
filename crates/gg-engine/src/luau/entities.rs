//! What scripts make and unmake: `gg.spawn`, `gg.despawn`, `gg.add` and `gg.remove`, which change
//! the world's entities and their components at once, as Rust code's calls do, and `gg.alive`, as
//! [`Scripts`](super::Scripts) says.
//!
//! A script's tick runs for each entity in a list made as its pass starts, not over a query that
//! holds the world, so a change made as scripts run is safe at once: an entity despawned, or
//! without the component, by the time its turn comes is passed over, as Rust's
//! [`run_scripts`](crate::run_scripts) passes it over. What a VM's scripts give entities as they
//! run, an entity spawned with a script's component or given one, or given one riding on a
//! component it gained so, ticks first as that VM's scripts next run, at the next tick, whichever
//! of its passes comes later: so that when it starts doesn't go by the order of the scripts' names.
//! Another VM's scripts that run later in the tick, a mod's after the game's or another mod's,
//! tick it as they run, as their passes list it, in the order the layers run them, which is the
//! same on every peer. Their chunks change nothing of the world, entity, field, tween, clip or
//! parameter, as a chunk runs again as a lockstep joiner or a run restored from a snapshot loads it
//! (`script.loading`).
//!
//! Each change allocates, as Rust's by name do: a script spawns and despawns as Rust code does,
//! not at every tick in the thousands. Its errors are the engine's, with their codes, naming the
//! entity, by its ID while it's alive and else by its handle, and the component.

use gg_core::{Error, Result};
use mlua::{LightUserData, Lua, Table, Value as Lua_};

use super::{Field, Shared, bits, handle, named, shared, ungranted, value_of};
use crate::ecs::ComponentId;
use crate::grant::Capability;
use crate::reflect::{Ident, Reflect, Value, list};
use crate::{Entity, PrefabRoot, Transform, World};

/// Adds the functions this module serves to `gg`, the scripts' table of the engine's functions.
pub(super) fn register(lua: &Lua, gg: &Table) -> mlua::Result<()> {
    gg.raw_set("spawn", lua.create_function(spawn)?)?;
    gg.raw_set("despawn", lua.create_function(despawn)?)?;
    gg.raw_set("add", lua.create_function(add)?)?;
    gg.raw_set("remove", lua.create_function(remove)?)?;
    gg.raw_set("alive", lua.create_function(alive)?)?;
    Ok(())
}

/// Whether the VM's scripts gave `entity` component `id` as they ran this time, as the module
/// says: spawned it, gave it the component, or one riding on a component it gained so. A lookup or
/// two in a set, which is empty but where scripts change entities.
pub(super) fn fresh(shared: &Shared, entity: Entity, id: ComponentId) -> bool {
    let fresh = shared.fresh.borrow();
    !fresh.is_empty() && (fresh.contains(&(entity, None)) || fresh.contains(&(entity, Some(id))))
}

/// Notes that `entity` was given `rider`, a script's component riding on `on`, as the scripts ran,
/// which is fresh as `on` is.
pub(super) fn rides(shared: &Shared, entity: Entity, on: ComponentId, rider: ComponentId) {
    if fresh(shared, entity, on) {
        shared.fresh.borrow_mut().insert((entity, Some(rider)));
    }
}

/// `gg.spawn(components, options)`: spawns an entity with `components`, a table of each component
/// type's name to a table of its fields' values by name, those not given at their defaults, and
/// returns it. `options`' `parent` makes it the last of that entity's children, and its `prefab`,
/// a prefab's ID or path, spawns that prefab's instance with it as its root, the components given
/// in place of the prefab's and its `Transform` the identity if it's not given. A mod's scripts
/// spawn only if it may write some type, an entity of components they may write alone; one
/// without components, under a parent or of a prefab only if it may write the whole world.
fn spawn(_: &Lua, (components, options): (Lua_, Lua_)) -> mlua::Result<Lua_> {
    shared(|shared| {
        ticking(shared, "spawns")?;
        let components = table_arg("`gg.spawn`'s components", components)?;
        let options = table_arg("`gg.spawn`'s options", options)?;
        let given = components_in(components)?;
        let (mut parent, mut prefab) = (None, None);
        for pair in options.iter().flat_map(Table::pairs::<Lua_, Lua_>) {
            let (key, value) = pair?;
            let key = match &key {
                Lua_::String(key) => key.to_str()?.to_owned(),
                _ => String::new(),
            };
            match (&*key, value) {
                ("parent", Lua_::LightUserData(data)) if bits(data) == 0 => {}
                ("parent", Lua_::LightUserData(data)) => parent = Some(target(data)?),
                ("prefab", Lua_::String(name)) => prefab = Some(name.to_str()?.to_owned()),
                _ => {
                    return Err(fail(
                        "script.bad_option",
                        "a spawn's options are `parent`, an entity, and `prefab`, a prefab's ID or \
                         path"
                            .to_owned(),
                    ));
                }
            }
        }
        if let Some(held) = &shared.held {
            let writes_any = held.grant.iter().any(|c| matches!(c, Capability::Write(_)));
            if !writes_any {
                let what = "spawning an entity";
                return Err(fail("mod.ungranted", ungranted("write:<a type>", what)));
            }
            let whole = if prefab.is_some() {
                Some("spawning a prefab, whose components it gives,")
            } else if parent.is_some() {
                Some("spawning an entity under another, whose children it changes,")
            } else if given.is_empty() {
                Some("spawning an entity without components")
            } else {
                None
            };
            if let Some(what) = whole.filter(|_| !held.grant.has(&Capability::Write(None))) {
                return Err(fail("mod.ungranted", ungranted("write", what)));
            }
            if let Some((name, _)) = given.iter().find(|(name, _)| !shared.may(name).1) {
                let what = format!("spawning an entity with `{name}`");
                return Err(fail(
                    "mod.ungranted",
                    ungranted(&format!("write:{name}"), &what),
                ));
            }
        }
        let mut world = shared.world.borrow_mut();
        let mut row: Vec<(Ident, Value)> = Vec::with_capacity(given.len() + 2);
        for (name, fields) in &given {
            let about = |error: Error| {
                let message = format!("can't spawn an entity with `{name}`: {error:#}");
                mlua::Error::external(Error::new(error.code(), message))
            };
            row.push((
                name.clone().into(),
                made(&world, name, fields).map_err(about)?,
            ));
        }
        if let Some(prefab) = prefab {
            let assets = world.assets();
            let Some(found) = assets.prefab(&prefab) else {
                let message = assets.not_found("prefab", &prefab);
                return Err(fail(
                    "asset.no_prefab",
                    format!("can't spawn it: {message}"),
                ));
            };
            if !row.iter().any(|(name, _)| name == "Transform") {
                row.push(("Transform".into(), Transform::IDENTITY.to_value()));
            }
            row.push(("PrefabRoot".into(), PrefabRoot(found).to_value()));
        }
        let spawned = world.spawn_named(parent, &row).map_err(|error| {
            let under = parent.map_or(String::new(), |parent| {
                format!(" under entity {}", named(&world, parent))
            });
            let message = format!("can't spawn an entity{under}: {error:#}");
            mlua::Error::external(Error::new(error.code(), message))
        })?;
        shared.fresh.borrow_mut().insert((spawned, None));
        Ok(Lua_::LightUserData(handle(spawned.to_bits())))
    })
}

/// `gg.despawn(entity)`: despawns `entity` and its descendants, as [`World::despawn`] does; for a
/// mod's scripts only if it may write the whole world, as they take every component of each.
fn despawn(_: &Lua, entity: Lua_) -> mlua::Result<()> {
    shared(|shared| {
        ticking(shared, "despawns")?;
        let entity = entity_arg("`gg.despawn`'s entity", entity)?;
        if let Some(held) = &shared.held
            && !held.grant.has(&Capability::Write(None))
        {
            let what = "despawning an entity, which takes all of its components and descendants,";
            return Err(fail("mod.ungranted", ungranted("write", what)));
        }
        let mut world = shared.world.borrow_mut();
        world.despawn(entity).map_err(|error| {
            let message = format!("can't despawn entity {}: {error:#}", named(&world, entity));
            mlua::Error::external(Error::new(error.code(), message))
        })
    })
}

/// `gg.add(entity, component, fields)`: gives `entity` a `component`, which it hasn't, its fields
/// those in `fields` by name, and the rest at their defaults.
fn add(_: &Lua, (entity, component, fields): (Lua_, Lua_, Lua_)) -> mlua::Result<()> {
    shared(|shared| {
        ticking(shared, "gives components")?;
        let entity = entity_arg("`gg.add`'s entity", entity)?;
        let component = name_arg("`gg.add`'s component", &component)?;
        let fields = table_arg("`gg.add`'s fields", fields)?;
        let fields = fields_in(&component, fields)?;
        writes(shared, &component, "giving an entity")?;
        let mut world = shared.world.borrow_mut();
        let about = |world: &World, error: Error| {
            let entity = named(world, entity);
            let message = format!("can't give entity {entity} a `{component}`: {error:#}");
            mlua::Error::external(Error::new(error.code(), message))
        };
        let value = made(&world, &component, &fields).map_err(|error| about(&world, error))?;
        let added = world.add_named(entity, &component, &value);
        added.map_err(|error| about(&world, error))?;
        let id = world.component_named(&component).expect("added");
        shared.fresh.borrow_mut().insert((entity, Some(id)));
        Ok(())
    })
}

/// `gg.remove(entity, component)`: takes `entity`'s `component` away.
fn remove(_: &Lua, (entity, component): (Lua_, Lua_)) -> mlua::Result<()> {
    shared(|shared| {
        ticking(shared, "takes components away")?;
        let entity = entity_arg("`gg.remove`'s entity", entity)?;
        let component = name_arg("`gg.remove`'s component", &component)?;
        writes(shared, &component, "taking away")?;
        let mut world = shared.world.borrow_mut();
        world.remove_named(entity, &component).map_err(|error| {
            let entity = named(&world, entity);
            let message = format!("can't take `{component}` away from entity {entity}: {error:#}");
            mlua::Error::external(Error::new(error.code(), message))
        })
    })
}

/// `gg.alive(entity)`: whether `entity` is in the world, which a handle held to an entity
/// despawned since isn't, as no other entity in its slot is it; false for `gg.entity` and nil.
fn alive(_: &Lua, entity: Lua_) -> mlua::Result<bool> {
    shared(|shared| {
        let entity = match entity {
            Lua_::Nil => None,
            Lua_::LightUserData(data) => Entity::from_bits(bits(data)),
            other => return Err(argument("`gg.alive`'s entity", "an entity", &other)),
        };
        Ok(entity.is_some_and(|entity| shared.world.borrow().contains(entity)))
    })
}

/// The error of `code` that `message` says, as a script's call fails with it.
fn fail(code: &'static str, message: String) -> mlua::Error {
    mlua::Error::external(Error::new(code, message))
}

/// The error, `script.bad_argument`, that a call's argument `what` is `got`, where it's `expected`.
fn argument(what: &str, expected: &str, got: &Lua_) -> mlua::Error {
    let message = format!("{what} is {expected}, not a {}", got.type_name());
    fail("script.bad_argument", message)
}

/// The entity a call's argument `what`, `value`, is, as [`target`] finds it.
///
/// Fails with `script.bad_argument` for a value that isn't light userdata, and as `target` does.
fn entity_arg(what: &str, value: Lua_) -> mlua::Result<Entity> {
    match value {
        Lua_::LightUserData(data) => target(data),
        other => Err(argument(what, "an entity", &other)),
    }
}

/// The table a call's argument `what`, `value`, is, or none for nil.
///
/// Fails with `script.bad_argument` for a value of another kind.
fn table_arg(what: &str, value: Lua_) -> mlua::Result<Option<Table>> {
    match value {
        Lua_::Nil => Ok(None),
        Lua_::Table(table) => Ok(Some(table)),
        other => Err(argument(what, "a table or nil", &other)),
    }
}

/// The name a call's argument `what`, `value`, is.
///
/// Fails with `script.bad_argument` for a value that isn't a string, or isn't UTF-8.
fn name_arg(what: &str, value: &Lua_) -> mlua::Result<String> {
    let name = match value {
        Lua_::String(name) => name.to_str().ok().map(|name| name.to_owned()),
        _ => None,
    };
    name.ok_or_else(|| argument(what, "a component type's name", value))
}

/// Fails with `script.loading` as a script's chunk runs, where a script's call that `does`
/// changes no entity, as the module says, refusing the script loading.
pub(super) fn ticking(shared: &Shared, does: &str) -> mlua::Result<()> {
    let mut loading = shared.loading.borrow_mut();
    let Some(loading) = loading.as_mut() else {
        return Ok(());
    };
    let message = format!(
        "a script {does} in its tick, not as its chunk runs, which runs again as a lockstep joiner \
         or a run restored from a snapshot loads it"
    );
    let error = Error::new("script.loading", format!("{}: {message}", loading.path));
    loading.refused = Some(error);
    Err(fail("script.loading", message))
}

/// Fails with `mod.ungranted` unless the scripts may write `component`, which `what` it needs.
fn writes(shared: &Shared, component: &str, what: &str) -> mlua::Result<()> {
    if shared.may(component).1 {
        return Ok(());
    }
    let what = format!("{what} `{component}`");
    Err(fail(
        "mod.ungranted",
        ungranted(&format!("write:{component}"), &what),
    ))
}

/// The entity `data` is, which a script gives a call to change it.
///
/// Fails with `ecs.no_entity` for `gg.entity`, which is none, and with `reflect.wrong_kind` for a
/// field's handle.
fn target(data: LightUserData) -> mlua::Result<Entity> {
    if bits(data) == 0 {
        let message = "`gg.entity` is no entity, but a field's default of none".to_owned();
        return Err(fail("ecs.no_entity", message));
    }
    Entity::from_bits(bits(data)).ok_or_else(|| {
        let message = "that's a field's handle, not an entity".to_owned();
        fail("reflect.wrong_kind", message)
    })
}

/// A spawn's `components`, each a type's name and the values of its fields that it gives, as
/// [`fields_in`] reads them, in the order of the names, so that what a spawn refuses first is the
/// same whatever order Luau holds its table in.
fn components_in(components: Option<Table>) -> mlua::Result<Vec<(String, Fields)>> {
    let mut given = Vec::new();
    for pair in components.iter().flat_map(Table::pairs::<Lua_, Lua_>) {
        let (name, fields) = pair?;
        let Lua_::String(name) = name else {
            let message = format!(
                "a spawn's components are named by their types' names, not by a {}",
                name.type_name()
            );
            return Err(fail("reflect.unknown_type", message));
        };
        let name = name.to_str()?.to_owned();
        let fields = match fields {
            Lua_::Table(fields) => fields_in(&name, Some(fields))?,
            other => {
                let message = format!(
                    "a spawn's `{name}` is a table of its fields' values, not a {}",
                    other.type_name()
                );
                return Err(fail("reflect.wrong_kind", message));
            }
        };
        given.push((name, fields));
    }
    given.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(given)
}

/// The values a script gives a component's fields, each by the field's name.
type Fields = Vec<(String, Lua_)>;

/// The values `fields` gives `component`'s fields, each by its name, in the order of the names.
fn fields_in(component: &str, fields: Option<Table>) -> mlua::Result<Fields> {
    let mut given = Vec::new();
    for pair in fields.iter().flat_map(Table::pairs::<Lua_, Lua_>) {
        let (name, value) = pair?;
        let Lua_::String(name) = name else {
            let message = format!(
                "`{component}`'s fields are named by strings, not by a {}",
                name.type_name()
            );
            return Err(fail("reflect.no_field", message));
        };
        given.push((name.to_str()?.to_owned(), value));
    }
    given.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(given)
}

/// A `component` of `world`'s, at its default, as [`World::default_of`] makes it, but for
/// `fields`, each a field's name and the script's value it takes in place of its default, of the
/// kinds a script's handles reach. A type without a default, as one of an entity's field that's
/// no option is, is made of the fields given and the others' defaults.
///
/// Fails with `reflect.unknown_type` for no component type of its name, and `reflect.read_only`
/// for one the world keeps, as [`World::unlinked_type`] says; with `reflect.no_field` for a field
/// the type hasn't; with `reflect.wrong_kind` for a field of a kind scripts don't reach, or a value
/// of another kind, as [`value_of`] does; with `reflect.no_default` for a field not given that has
/// no default; and as `default_of` does otherwise. Its bounds are checked as it's built.
fn made(world: &World, component: &str, fields: &[(String, Lua_)]) -> Result<Value> {
    let id = world.unlinked_type(component)?;
    let described = world.fields_of(id);
    let names = || list(described.iter().map(|field| &field.name));
    let mut given = Vec::with_capacity(fields.len());
    for (name, value) in fields {
        let Some(field) = described.iter().find(|field| field.name == *name) else {
            return Err(Error::new(
                "reflect.no_field",
                format!(
                    "`{component}` has no field `{name}`; its fields are {}",
                    names()
                ),
            ));
        };
        let Some(ty) = Field::of(&field.ty) else {
            return Err(Error::new(
                "reflect.wrong_kind",
                format!(
                    "`{component}.{name}` holds what scripts don't reach, which takes its default"
                ),
            ));
        };
        let converted = value_of(ty, value).map_err(|error| {
            let message = format!("`{component}.{name}`: {error:#}");
            error.with_message(message)
        })?;
        given.push((&field.name, converted));
    }
    let mut value = match world.default_of(component) {
        Ok(value) => value,
        // A field of no default, which those given may give.
        Err(error) if error.code() == "reflect.no_default" && !described.is_empty() => {
            let mut own = Vec::with_capacity(described.len());
            for field in described {
                let value = match given.iter().find(|(name, _)| **name == field.name) {
                    Some((_, value)) => value.clone(),
                    None => world.types().field_default(field).map_err(|error| {
                        let message = format!(
                            "`{component}.{}` has no default, so a script gives it: {error:#}",
                            field.name
                        );
                        Error::new("reflect.no_default", message)
                    })?,
                };
                own.push((field.name.clone(), value));
            }
            return Ok(Value::Struct(own));
        }
        Err(error) => return Err(error),
    };
    if given.is_empty() {
        return Ok(value);
    }
    let Value::Struct(own) = &mut value else {
        unreachable!("a type of named fields, which the fields given are, has a struct's default");
    };
    for (name, converted) in given {
        let (_, slot) = (own.iter_mut())
            .find(|(field, _)| field == name)
            .expect("a default has each of its fields");
        *slot = converted;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use gg_core::math::{Quat, Vec3};
    use mlua::{FromLuaMulti, IntoLuaMulti};

    use super::super::{BUDGET, Held, Host, Scripts, lent};
    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::grant::Grant;
    use crate::{Component, Context};

    /// A Rust component that scripts give and take, with a field of a kind they don't reach.
    #[derive(Clone, Component)]
    struct Body {
        mass: f32,
        steps: u32,
        at: Vec3,
        of: Option<Entity>,
        label: String,
    }

    /// A Rust component without a default, as its entity's field has none.
    #[derive(Clone, Component)]
    struct Follow {
        target: Entity,
        gap: f32,
    }

    /// Lays a `Kid` under its entity at each tick, given a `Body` whose fields it sets, some as it
    /// spawns it and one after.
    const HATCH: &str = r#"
local Body = gg.component("Body")
local Hatch = gg.declare("Hatch", { laid = 0 })

function Hatch.tick(entity, dt)
    local laid = entity[Hatch.laid] + 1
    entity[Hatch.laid] = laid
    local kid = gg.spawn({
        Body = { mass = laid, at = vector.create(laid, 0, 0), of = entity },
        Kid = {},
    }, { parent = entity })
    kid[Body.steps] = 100
end

return Hatch
"#;

    /// Takes its entity's `Body` away at its first tick, gives it another at its second, and
    /// despawns it at its third.
    const KID: &str = r#"
local Kid = gg.declare("Kid", { age = 0 })

function Kid.tick(entity, dt)
    local age = entity[Kid.age] + 1
    entity[Kid.age] = age
    if age == 1 then
        gg.remove(entity, "Body")
    elseif age == 2 then
        gg.add(entity, "Body", { steps = 7, mass = 2 })
    elseif age == 3 then
        gg.despawn(entity)
    end
end

return Kid
"#;

    /// A context whose project's root is `dir`, with `Body` registered, and a host that has loaded
    /// `scripts`, each a path in `dir` and a text, which it writes there.
    fn loaded(dir: &Scratch, scripts: &[(&str, &str)]) -> (Context, Scripts) {
        fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
        let mut cx = Context::seeded(1).root(&dir.0);
        cx.world_mut().register::<Body>().expect("described");
        cx.world_mut().register::<Follow>().expect("described");
        let mut host = Scripts::new().expect("Luau starts");
        for (name, text) in scripts {
            let path = dir.0.join(name);
            fs::write(&path, text).expect("written");
            host.load(&mut cx, &path).expect("the script loads");
        }
        (cx, host)
    }

    /// Runs `text` in `host`'s VM, given `args`, with `world` lent, as a tick runs.
    fn call<R: FromLuaMulti>(
        host: &mut Host,
        world: &mut World,
        text: &str,
        args: impl IntoLuaMulti,
    ) -> mlua::Result<R> {
        let Host { lua, shared, .. } = host;
        lent(shared, world, || {
            shared.budget.set(BUDGET);
            lua.load(text).call(args)
        })
    }

    /// The code of the engine's error that `ran` failed with.
    fn code<R>(ran: mlua::Result<R>) -> &'static str {
        let Err(error) = ran else {
            panic!("refused");
        };
        let code = error.downcast_ref::<Error>().map(Error::code);
        code.unwrap_or_else(|| panic!("an engine's error: {error}"))
    }

    /// The handle of `entity`, as a script holds it.
    fn held(entity: Entity) -> LightUserData {
        handle(entity.to_bits())
    }

    /// The entity a script returned.
    fn entity(data: LightUserData) -> Entity {
        Entity::from_bits(bits(data)).expect("an entity")
    }

    /// Each entity of `world`, with its ID, in the order queries find them.
    fn ids(world: &World) -> Vec<(Entity, String)> {
        let entities = world.entities(None).expect("listed");
        entities
            .map(|entity| (entity, world.id(entity).expect("alive").to_string()))
            .collect()
    }

    /// A script spawns an entity under another with components whose fields it gives and sets
    /// after, takes one away, gives one, and despawns it, at once; what it spawns, or gives a
    /// script's component, ticks first at the next tick, whichever script's pass comes later.
    #[test]
    fn a_script_spawns_gives_takes_away_and_despawns() {
        let dir = Scratch::new("luau-entities");
        let scripts = [("scripts/hatch.luau", HATCH), ("scripts/kid.luau", KID)];
        let (mut cx, mut host) = loaded(&dir, &scripts);
        let hatch = cx.world_mut().spawn(());
        cx.world_mut().add_default(hatch, "Hatch").expect("added");
        host.run(cx.world_mut());
        let world = cx.world();
        let kids: Vec<Entity> = world.children(hatch).expect("alive").collect();
        let [first] = kids[..] else {
            panic!("one kid, not {kids:?}");
        };
        let body = world.get::<Body>(first).expect("its body");
        let made = (body.mass, body.steps, body.at, body.of);
        assert_eq!(made, (1.0, 100, Vec3::X, Some(hatch)));
        let age = |world: &World, kid| world.get_field(kid, "Kid", "age").expect("a kid");
        assert_eq!(
            age(world, first),
            Value::F64(0.0),
            "it starts at the next tick"
        );
        host.run(cx.world_mut());
        let world = cx.world();
        assert_eq!(age(world, first), Value::F64(1.0));
        assert!(world.get::<Body>(first).is_none(), "taken away");
        host.run(cx.world_mut());
        let body = cx.world().get::<Body>(first).expect("given again");
        assert_eq!((body.mass, body.steps, body.of), (2.0, 7, None));
        host.run(cx.world_mut());
        let world = cx.world();
        assert!(!world.contains(first), "despawned");
        assert_eq!(world.children(hatch).expect("alive").count(), 3);
    }

    /// A component riding on one a script gave an entity is given it as its pass comes, but ticks
    /// first at the next tick, as the one it rides on does.
    #[test]
    fn a_rider_on_what_a_script_gave_starts_at_the_next_tick() {
        let dir = Scratch::new("luau-entities-rider");
        let ride = "local Ride = gg.declare(\"Ride\", { rode = 0 }, { on = \"Body\" })\n\
            function Ride.tick(entity, dt)\n\
                entity[Ride.rode] = entity[Ride.rode] + 1\n\
            end\n\
            return Ride\n";
        let scripts = [
            ("scripts/hatch.luau", HATCH),
            ("scripts/kid.luau", KID),
            ("scripts/ride.luau", ride),
        ];
        let (mut cx, mut host) = loaded(&dir, &scripts);
        let hatch = cx.world_mut().spawn(());
        cx.world_mut().add_default(hatch, "Hatch").expect("added");
        host.run(cx.world_mut());
        let world = cx.world();
        let kid = world.children(hatch).expect("alive").next().expect("a kid");
        let rode = world.get_field(kid, "Ride", "rode").expect("given");
        assert_eq!(rode, Value::F64(0.0));
        host.run(cx.world_mut());
        let rode = cx.world().get_field(kid, "Ride", "rode").expect("kept");
        assert_eq!(rode, Value::F64(1.0));
    }

    /// Spawns, gives, takes away and despawns from scripts go alike on a world run again from a
    /// copy, as rollback runs it, and on a copy run beside it, as a SyncTest runs it: the same
    /// entities, in the same slots, of the same IDs, and the same hash.
    #[test]
    fn what_scripts_spawn_and_despawn_goes_alike_when_ticks_run_again() {
        let dir = Scratch::new("luau-entities-again");
        let scripts = [("scripts/hatch.luau", HATCH), ("scripts/kid.luau", KID)];
        let (mut cx, mut host) = loaded(&dir, &scripts);
        let hatch = cx.world_mut().spawn(());
        cx.world_mut().add_default(hatch, "Hatch").expect("added");
        for _ in 0..2 {
            host.run(cx.world_mut());
        }
        let snapshot = cx.world().clone();
        let mut copy = snapshot.clone();
        let mut ran = Vec::new();
        for _ in 0..5 {
            host.run(cx.world_mut());
            ran.push((cx.world().hash(), ids(cx.world())));
        }
        cx.world_mut().rewind(&snapshot);
        for (tick, ran) in ran.iter().enumerate() {
            host.run(cx.world_mut());
            let again = (cx.world().hash(), ids(cx.world()));
            assert_eq!(&again, ran, "tick {tick} run again");
            host.run(&mut copy);
            let beside = (copy.hash(), ids(&copy));
            assert_eq!(&beside, ran, "tick {tick} on a copy");
        }
        assert!(cx.world().difference(&copy).is_none());
    }

    /// A spawn takes a prefab, spawning its instance under the entity it returns, which takes the
    /// `Transform` given; and a parent, of whose children it's the last. Despawning the parent
    /// despawns them all, and a handle held to one of them is found to be of none, as a field's
    /// read through it fails, `ecs.no_entity`.
    #[test]
    fn a_spawn_takes_a_prefab_and_a_parent() {
        let dir = Scratch::new("luau-entities-prefab");
        let (mut cx, scripts) = loaded(&dir, &[("scripts/kid.luau", KID)]);
        let pair = "(entities: {\"0000000000000a01\": (Name: (\"Pair\")), \"0000000000000a02\": \
                    (Name: (\"half\"), Parent: (\"0000000000000a01\"))})\n";
        fs::create_dir_all(dir.0.join("prefabs")).expect("a directory");
        let file = dir.0.join("prefabs").join("pair.prefab");
        fs::write(&file, pair).expect("written");
        cx.add_prefab("pair", &file).expect("loaded");
        let parent = cx.world_mut().spawn(());
        let host = &mut *scripts.host.borrow_mut();
        let text = "local parent = ...\n\
            return gg.spawn({ Transform = { translation = vector.create(1, 2, 3) } }, \
                { prefab = \"prefabs/pair.prefab\", parent = parent })";
        let root: LightUserData = call(host, cx.world_mut(), text, held(parent)).expect("spawned");
        let root = entity(root);
        let world = cx.world();
        assert_eq!(world.parent(root).expect("alive"), Some(parent));
        let transform = world.get::<Transform>(root).expect("its own");
        assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
        let halves: Vec<Entity> = world.children(root).expect("alive").collect();
        assert_eq!(halves.len(), 1, "the prefab's");
        let half = halves[0];
        let text = "local parent, half = ...\n\
            gg.despawn(parent)\n\
            return gg.alive(half), gg.alive(gg.entity)";
        let alive: (bool, bool) =
            call(host, cx.world_mut(), text, (held(parent), held(half))).expect("despawned");
        assert_eq!(alive, (false, false));
        assert!(!cx.world().contains(root) && !cx.world().contains(half));
        let text = "local half = ...\n\
            return half[gg.component(\"Transform\").translation]";
        let read = call::<Lua_>(host, cx.world_mut(), text, held(half));
        assert_eq!(code(read), "ecs.no_entity");
    }

    /// What a script's call to change the world's entities refuses, it refuses with the engine's
    /// error and its code, naming the entity and the component, and changing nothing.
    #[test]
    fn what_a_script_s_changes_refuse_fails_with_a_code() {
        let dir = Scratch::new("luau-entities-refused");
        let (mut cx, scripts) = loaded(&dir, &[("scripts/kid.luau", KID)]);
        let world = cx.world_mut();
        let body = Body {
            mass: 1.0,
            steps: 0,
            at: Vec3::ZERO,
            of: None,
            label: String::new(),
        };
        let bodied = world.spawn(body);
        let bare = world.spawn(());
        let gone = world.spawn(());
        world.despawn(gone).expect("alive");
        let host = &mut *scripts.host.borrow_mut();
        let refusals = [
            ("gg.add(bare, \"Nope\")", "reflect.unknown_type"),
            ("gg.add(bare, \"Body\", { nope = 1 })", "reflect.no_field"),
            ("gg.add(bare, \"Body\", { [1] = 1 })", "reflect.no_field"),
            (
                "gg.add(bare, \"Body\", { label = \"a\" })",
                "reflect.wrong_kind",
            ),
            (
                "gg.add(bare, \"Body\", { mass = true })",
                "reflect.wrong_kind",
            ),
            (
                "gg.add(bare, \"Body\", { steps = -1 })",
                "reflect.out_of_range",
            ),
            ("gg.add(bodied, \"Body\")", "reflect.has_component"),
            ("gg.add(bare, \"Parent\")", "reflect.read_only"),
            ("gg.add(bare, \"PrefabRoot\")", "reflect.read_only"),
            ("gg.add(gone, \"Body\")", "ecs.no_entity"),
            ("gg.remove(bare, \"Body\")", "reflect.missing_component"),
            ("gg.remove(gone, \"Body\")", "ecs.no_entity"),
            ("gg.remove(bare, \"Nope\")", "reflect.unknown_type"),
            ("gg.despawn(gone)", "ecs.no_entity"),
            ("gg.despawn(gg.entity)", "ecs.no_entity"),
            (
                "gg.spawn({ Body = { steps = 1.5 } })",
                "reflect.out_of_range",
            ),
            (
                "gg.spawn({ Transform = { rotation = vector.create(0, 0, 0, 0) } })",
                "reflect.not_normalized",
            ),
            ("gg.spawn({ Nope = {} })", "reflect.unknown_type"),
            ("gg.spawn({ Body = 1 })", "reflect.wrong_kind"),
            ("gg.spawn({ PrefabRoot = {} })", "reflect.read_only"),
            ("gg.spawn(nil, { parent = gone })", "ecs.no_entity"),
            ("gg.spawn(nil, { prefab = \"nope\" })", "asset.no_prefab"),
            ("gg.spawn(nil, { colour = 1 })", "script.bad_option"),
            ("gg.add(bare, \"ModelRoot\")", "reflect.read_only"),
            ("gg.remove(bare, \"ModelRoot\")", "reflect.read_only"),
            ("gg.spawn({ ModelRoot = {} })", "reflect.read_only"),
            (
                "gg.add(bare, \"Follow\", { gap = 1 })",
                "reflect.no_default",
            ),
            ("gg.spawn({ Follow = {} })", "reflect.no_default"),
            ("gg.despawn(nil)", "script.bad_argument"),
            ("gg.despawn(1)", "script.bad_argument"),
            ("gg.spawn({}, 5)", "script.bad_argument"),
            ("gg.spawn(\"Body\")", "script.bad_argument"),
            ("gg.add(bare, 5)", "script.bad_argument"),
            ("gg.add(bare, \"Body\", 5)", "script.bad_argument"),
            ("gg.remove(nil, \"Body\")", "script.bad_argument"),
            ("gg.alive(5)", "script.bad_argument"),
        ];
        let before = ids(cx.world());
        for (text, expected) in refusals {
            let text = format!("local bodied, bare, gone = ...\n{text}");
            let args = (held(bodied), held(bare), held(gone));
            let ran = call::<()>(host, cx.world_mut(), &text, args);
            assert!(ran.is_err(), "{text} is refused");
            assert_eq!(code(ran), expected, "{text}");
            assert_eq!(ids(cx.world()), before, "{text} changes nothing");
        }
        let text = "local bodied = ...\ngg.add(bodied, \"Body\")";
        let error = call::<()>(host, cx.world_mut(), text, held(bodied)).expect_err("refused");
        let id = cx.world().id(bodied).expect("alive");
        let message = error.to_string();
        assert!(
            message.contains(&format!("entity {id}")) && message.contains("`Body`"),
            "{message}"
        );
        let text = "local gone = ...\ngg.despawn(gone)";
        let error = call::<()>(host, cx.world_mut(), text, held(gone)).expect_err("refused");
        assert!(error.to_string().contains(&gone.to_string()), "{error}");
    }

    /// A component type without a default, as one with an entity's field that's no option is, is
    /// given made of the fields a script gives and the others' defaults.
    #[test]
    fn a_component_without_a_default_is_made_of_the_fields_given() {
        let dir = Scratch::new("luau-entities-no-default");
        let (mut cx, scripts) = loaded(&dir, &[("scripts/kid.luau", KID)]);
        let world = cx.world_mut();
        let [leader, follower] = [world.spawn(()), world.spawn(())];
        let host = &mut *scripts.host.borrow_mut();
        let text = "local leader, follower = ...\n\
            gg.add(follower, \"Follow\", { target = leader, gap = 2 })\n\
            return gg.spawn({ Follow = { target = follower } })";
        let args = (held(leader), held(follower));
        let spawned: LightUserData = call(host, cx.world_mut(), text, args).expect("given");
        let world = cx.world();
        let follow = world.get::<Follow>(follower).expect("given");
        assert_eq!((follow.target, follow.gap), (leader, 2.0));
        let follow = world
            .get::<Follow>(entity(spawned))
            .expect("spawned with it");
        assert_eq!((follow.target, follow.gap), (follower, 0.0));
    }

    /// A script's chunk changes no entity, as it runs again as a lockstep joiner or a restored run
    /// loads it: one that does is refused, `script.loading`.
    #[test]
    fn a_script_s_chunk_spawns_nothing() {
        let dir = Scratch::new("luau-entities-chunk");
        let (mut cx, mut host) = loaded(&dir, &[]);
        let text = "gg.spawn()\nreturn gg.declare(\"Early\", {})";
        let path = dir.0.join("scripts").join("early.luau");
        fs::write(&path, text).expect("written");
        let refused = host.load(&mut cx, &path).expect_err("refused");
        assert_eq!(refused.code(), "script.loading", "{refused}");
        assert_eq!(cx.world().entities(None).expect("listed").count(), 0);
    }

    /// A mod's scripts spawn and despawn as its grant lets them: only if it may write some type, an
    /// entity with the components they may write; one without any, under a parent or of a prefab,
    /// and a despawn, only if they may write the whole world.
    #[test]
    fn a_mod_s_scripts_change_entities_as_its_grant_lets_them() {
        let mut world = World::default();
        world.register::<Body>().expect("described");
        let modded = |grant: &[&str]| {
            let grant = Grant::parse(grant).expect("capabilities");
            let kept = Held::new("modded".to_owned(), grant);
            Host::new(Some((kept, 1 << 24))).expect("Luau starts")
        };
        let mut reader = modded(&["read"]);
        for text in ["gg.spawn()", "gg.spawn({ Body = {} })"] {
            let ran = call::<()>(&mut reader, &mut world, text, ());
            assert_eq!(code(ran), "mod.ungranted", "{text}, read alone");
        }
        assert_eq!(world.entities(None).expect("listed").count(), 0);
        let mut whole = modded(&["write"]);
        let text = "local parent = gg.spawn()\nreturn gg.spawn(nil, { parent = parent })";
        call::<LightUserData>(&mut whole, &mut world, text, ()).expect("granted");
        let mut host = modded(&["read", "write:Body"]);
        let text = "return gg.spawn({ Body = {} })";
        let spawned: LightUserData = call(&mut host, &mut world, text, ()).expect("granted");
        let spawned = entity(spawned);
        assert!(world.get::<Body>(spawned).is_some());
        let text = "local spawned = ...\ngg.remove(spawned, \"Body\")\ngg.add(spawned, \"Body\")";
        call::<()>(&mut host, &mut world, text, held(spawned)).expect("granted");
        let refusals = [
            "gg.spawn()",
            "gg.spawn({}, { parent = spawned })",
            "gg.spawn({ Body = {} }, { parent = spawned })",
            "gg.spawn({ Transform = {} })",
            "gg.spawn(nil, { prefab = \"any\" })",
            "gg.despawn(spawned)",
            "gg.add(spawned, \"Transform\")",
        ];
        for text in refusals {
            let text = format!("local spawned = ...\n{text}");
            let ran = call::<()>(&mut host, &mut world, &text, held(spawned));
            assert_eq!(code(ran), "mod.ungranted", "{text}");
        }
    }

    /// A script's chunk writes nothing at all, not only entities and components: a field, a tween,
    /// a clip and a parameter are refused too, `script.loading`, as each runs again as a lockstep
    /// joiner or a restored run loads the chunk.
    #[test]
    fn a_script_s_chunk_writes_no_field_tween_clip_or_parameter() {
        let dir = Scratch::new("luau-entities-chunk-writes");
        let (mut cx, mut host) = loaded(&dir, &[]);
        let writes = [
            "gg.tween(gg.entity, gg.entity, 1, 1)",
            "gg.play(gg.entity, \"Walk\")",
            "gg.set_param(gg.entity, \"speed\", 1)",
            "gg.entity[gg.entity] = 1",
            "gg.despawn(gg.entity)",
            "gg.add(gg.entity, \"Body\")",
            "gg.remove(gg.entity, \"Body\")",
        ];
        for (i, write) in writes.iter().enumerate() {
            let text = format!("{write}\nreturn gg.declare(\"Early{i}\", {{}})");
            let path = dir.0.join("scripts").join(format!("early{i}.luau"));
            fs::write(&path, text).expect("written");
            let refused = host.load(&mut cx, &path).expect_err("refused");
            assert_eq!(refused.code(), "script.loading", "{write}: {refused}");
        }
        // What a chunk may do, read, still loads.
        let text = "local Body = gg.component(\"Body\")\n\
                    local _ = gg.quat.from_axis_angle(vector.create(0, 1, 0), 1)\n\
                    return gg.declare(\"Late\", {})";
        let path = dir.0.join("scripts").join("late.luau");
        fs::write(&path, text).expect("written");
        host.load(&mut cx, &path).expect("loads");
    }

    /// The errors of `gg.play` and `gg.set_param` keep their engine codes, as a tick's
    /// `script.error` names them as its `cause`.
    #[test]
    fn play_and_set_param_keep_the_codes_of_the_engine_s_errors() {
        let dir = Scratch::new("luau-entities-play-codes");
        let (mut cx, scripts) = loaded(&dir, &[]);
        let bare = cx.world_mut().spawn(());
        let gone = cx.world_mut().spawn(());
        cx.world_mut().despawn(gone).expect("despawned");
        let host = &mut *scripts.host.borrow_mut();
        let play = "gg.play(..., \"Walk\", 0.5)";
        let param = "gg.set_param(..., \"speed\", 1)";
        let refusals = [
            (play, bare, "animation.no_clip"),
            (play, gone, "animation.no_clip"),
            (param, bare, "reflect.missing_component"),
            (param, gone, "ecs.no_entity"),
        ];
        for (text, entity, expected) in refusals {
            let ran = call::<()>(host, cx.world_mut(), text, held(entity));
            assert_eq!(code(ran), expected, "{text}");
        }
    }

    /// A mod's field read or write that its grant doesn't allow fails as a tick runs with
    /// `mod.ungranted`, as the other refusals do, where it was text only.
    #[test]
    fn a_field_read_or_written_ungranted_fails_with_its_code() {
        let mut world = World::default();
        world.register::<Body>().expect("described");
        let body = world.spawn(Body {
            mass: 1.0,
            steps: 0,
            at: Vec3::ZERO,
            of: None,
            label: String::new(),
        });
        let modded = |grant: &[&str]| {
            let grant = Grant::parse(grant).expect("capabilities");
            let kept = Held::new("modded".to_owned(), grant);
            Host::new(Some((kept, 1 << 24))).expect("Luau starts")
        };
        let text = "local body = ...\nlocal Body = gg.component(\"Body\")\n";
        let mut reader = modded(&["read:Body"]);
        let write = format!("{text}body[Body.steps] = 3");
        let ran = call::<()>(&mut reader, &mut world, &write, held(body));
        assert_eq!(code(ran), "mod.ungranted", "written, read alone");
        let mut writer = modded(&["write:Body"]);
        let read = format!("{text}return body[Body.steps]");
        let ran = call::<Lua_>(&mut writer, &mut world, &read, held(body));
        assert_eq!(code(ran), "mod.ungranted", "read, written alone");
        let tween = format!("{text}gg.tween(body, Body.mass, 2, 1)");
        let ran = call::<()>(&mut reader, &mut world, &tween, held(body));
        assert_eq!(code(ran), "mod.ungranted", "tweened, read alone");
        let ran = call::<Lua_>(
            &mut modded(&["read:Transform"]),
            &mut world,
            text,
            held(body),
        );
        assert_eq!(code(ran), "mod.ungranted", "a type it has no grant on");
    }

    /// `gg.quat.from_axis_angle` takes an axis of any length as its direction, and refuses one that
    /// is none: `script.bad_argument`.
    #[test]
    fn an_axis_angle_rotation_normalizes_its_axis_and_refuses_one_that_is_none() {
        let mut host = Host::new(None).expect("Luau starts");
        let mut world = World::default();
        let text = "return gg.quat.from_axis_angle(vector.create(0, 5, 0), math.pi)";
        let q: mlua::Vector = call(&mut host, &mut world, text, ()).expect("a rotation");
        let q = Quat::from_xyzw(q.x(), q.y(), q.z(), q.w());
        assert!(q.is_normalized(), "{q}");
        assert!(q.angle_between(Quat::from_rotation_y(std::f32::consts::PI)) < 1e-5);
        let text = "return gg.quat.from_axis_angle(vector.create(3e30, 0, 3e30), 1)";
        let q: mlua::Vector = call(&mut host, &mut world, text, ()).expect("a rotation");
        assert!(Quat::from_xyzw(q.x(), q.y(), q.z(), q.w()).is_normalized());
        for text in [
            "gg.quat.from_axis_angle(vector.create(0, 0, 0), 1)",
            "gg.quat.from_axis_angle(vector.create(1e-9, 0, 0), 1)",
            "gg.quat.from_axis_angle(vector.create(0/0, 1, 0), 1)",
            "gg.quat.from_axis_angle(vector.create(1, math.huge, 0), 1)",
            "gg.quat.from_axis_angle(vector.create(0, 1, 0), 0/0)",
        ] {
            let ran = call::<()>(&mut host, &mut world, text, ());
            assert_eq!(code(ran), "script.bad_argument", "{text}");
        }
    }
}
