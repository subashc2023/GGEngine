//! A VM of scripts: making it, collecting its garbage, and loading and running scripts.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::{mem, ptr};

use gg_core::math::{Quat, libm};
use gg_core::{Error, Result};
use mlua::chunk::ChunkMode;
use mlua::{
    Function, LightUserData, Lua, LuaOptions, StdLib, Table, Value as Lua_, Variadic, Vector,
};
use tracing::{info, warn};

use super::api::{
    cast_box, cast_capsule, cast_ray, cast_sphere, find_path, play, print, set_param, tween,
    unit_axis, written,
};
use super::compile::{compiled, compiler};
#[cfg(feature = "dev")]
use super::debugger;
use super::fields::{
    component, declare, describe, handle, quat_of, read, vec3_of, vector_of, vector3_of, write,
};
use super::{
    BUDGET, Chunk, Declaring, Field, Held, Host, Loaded, Loading, SHARED, Shared, contacts,
    entities, hooks,
};
#[cfg(doc)]
use super::{Hosts, Scripts};
use crate::ecs::{ComponentId, Kind};
use crate::reflect::{Ident, Value, list};
use crate::{Tick, World};

/// The least garbage a VM gathers before a set point collects it, as [`Host::collect`] says, which
/// a VM whose live heap is larger lets grow to its size.
const LEAST_GARBAGE: usize = 1 << 20;

/// The globals scripts go without: what reads the clock, the VM's own state or its garbage, what
/// loads code or files, and what would let a script's chunk change how others' are compiled.
const REMOVED: [&str; 7] = [
    "collectgarbage",
    "gcinfo",
    "getfenv",
    "setfenv",
    "loadstring",
    "newproxy",
    "require",
];

impl Host {
    /// A VM for scripts, with the globals [`Scripts`] says: a game's, or, held to what `held` and
    /// `limit` say, a mod's.
    ///
    /// Fails with `script.vm` if Luau can't start, as for want of memory.
    pub(super) fn new(held: Option<(Held, usize)>) -> Result<Self> {
        let vm = |error: mlua::Error| Error::new("script.vm", format!("Luau can't start: {error}"));
        let lua = Self::vm().map_err(vm)?;
        let (held, limit) = held.unzip();
        if let Some(limit) = limit {
            lua.set_memory_limit(limit).map_err(vm)?;
        }
        let mut host = Self {
            shared: Shared {
                world: RefCell::new(World::default()),
                fields: RefCell::new(Vec::new()),
                loading: RefCell::new(None),
                budget: Cell::new(BUDGET),
                held,
                params: RefCell::new(Vec::new()),
                fresh: RefCell::new(HashSet::new()),
                #[cfg(feature = "dev")]
                hooked: debugger::Hooked::new(lua.clone()),
            },
            lua,
            scripts: Vec::new(),
            resolved: None,
            limit,
            goal: 0,
        };
        host.collect(true);
        Ok(host)
    }

    pub(super) fn vm() -> mlua::Result<Lua> {
        let libs = StdLib::COROUTINE
            | StdLib::TABLE
            | StdLib::STRING
            | StdLib::UTF8
            | StdLib::BIT
            | StdLib::MATH
            | StdLib::BUFFER
            | StdLib::VECTOR;
        let lua = Lua::new_with(libs, LuaOptions::default())?;
        lua.set_compiler(compiler());
        let globals = lua.globals();
        for name in REMOVED {
            globals.raw_remove(name)?;
        }
        // Both write as Luau's `tostring` does, but for what holds an address, as `written` says.
        let luau: Function = globals.raw_get("tostring")?;
        let to = luau.clone();
        let print = move |lua: &Lua, values: Variadic<Lua_>| print(lua, &to, &values);
        globals.raw_set("print", lua.create_function(print)?)?;
        let tostring = move |lua: &Lua, value: Lua_| written(lua, &luau, value);
        globals.raw_set("tostring", lua.create_function(tostring)?)?;
        let math: Table = globals.raw_get("math")?;
        math.raw_remove("random")?;
        math.raw_remove("randomseed")?;
        type Unary = fn(f64) -> f64;
        let unary: [(&str, Unary); 11] = [
            ("sin", libm::sin),
            ("cos", libm::cos),
            ("tan", libm::tan),
            ("asin", libm::asin),
            ("acos", libm::acos),
            ("atan", libm::atan),
            ("sinh", libm::sinh),
            ("cosh", libm::cosh),
            ("tanh", libm::tanh),
            ("exp", libm::exp),
            ("log10", libm::log10),
        ];
        for (name, f) in unary {
            math.raw_set(name, lua.create_function(move |_, x: f64| Ok(f(x)))?)?;
        }
        let atan2 = lua.create_function(|_, (y, x): (f64, f64)| Ok(libm::atan2(y, x)))?;
        math.raw_set("atan2", atan2)?;
        let pow = lua.create_function(|_, (x, y): (f64, f64)| Ok(libm::pow(x, y)))?;
        math.raw_set("pow", pow)?;
        // As Luau's own, but libm's: base 2 and 10 by their own functions.
        let log = lua.create_function(|_, (x, base): (f64, Option<f64>)| {
            Ok(match base {
                None => libm::log(x),
                Some(2.0) => libm::log2(x),
                Some(10.0) => libm::log10(x),
                Some(base) => libm::log(x) / libm::log(base),
            })
        })?;
        math.raw_set("log", log)?;
        let vector: Table = globals.raw_get("vector")?;
        vector.raw_remove("angle")?;
        hooks::check_integers(&lua)?;

        let entities = lua.create_table()?;
        entities.raw_set("__index", lua.create_function(read)?)?;
        entities.raw_set("__newindex", lua.create_function(write)?)?;
        entities.raw_set("__tostring", lua.create_function(describe)?)?;
        // Shared by every script's entities, as Luau's sandbox freezes its libraries' and vectors'.
        entities.set_readonly(true);
        lua.set_type_metatable::<LightUserData>(Some(entities));

        let gg = lua.create_table()?;
        gg.raw_set("component", lua.create_function(component)?)?;
        gg.raw_set("declare", lua.create_function(declare)?)?;
        gg.raw_set("entity", handle(0))?;
        let quat = lua.create_table()?;
        quat.raw_set("identity", vector_of(Quat::IDENTITY))?;
        let rotation = |rotate: fn(f32) -> Quat| {
            lua.create_function(move |_, angle: f64| Ok(vector_of(rotate(angle as f32))))
        };
        quat.raw_set("from_rotation_x", rotation(Quat::from_rotation_x)?)?;
        quat.raw_set("from_rotation_y", rotation(Quat::from_rotation_y)?)?;
        quat.raw_set("from_rotation_z", rotation(Quat::from_rotation_z)?)?;
        let axis_angle = lua.create_function(|_, (axis, angle): (Vector, f64)| {
            let axis = unit_axis(vec3_of(axis), angle)?;
            Ok(vector_of(Quat::from_axis_angle(axis, angle as f32)))
        })?;
        quat.raw_set("from_axis_angle", axis_angle)?;
        let mul = lua.create_function(|_, (a, b): (Vector, Vector)| {
            Ok(vector_of(quat_of(a) * quat_of(b)))
        })?;
        quat.raw_set("mul", mul)?;
        let rotate = lua.create_function(|_, (q, v): (Vector, Vector)| {
            Ok(vector3_of(quat_of(q) * vec3_of(v)))
        })?;
        quat.raw_set("rotate", rotate)?;
        quat.set_readonly(true);
        gg.raw_set("quat", quat)?;
        gg.raw_set("cast_ray", lua.create_function(cast_ray)?)?;
        gg.raw_set("cast_sphere", lua.create_function(cast_sphere)?)?;
        gg.raw_set("cast_box", lua.create_function(cast_box)?)?;
        gg.raw_set("cast_capsule", lua.create_function(cast_capsule)?)?;
        gg.raw_set("find_path", lua.create_function(find_path)?)?;
        gg.raw_set("tween", lua.create_function(tween)?)?;
        gg.raw_set("play", lua.create_function(play)?)?;
        gg.raw_set("set_param", lua.create_function(set_param)?)?;
        contacts::install(&lua, &gg)?;
        entities::register(&lua, &gg)?;
        gg.set_readonly(true);
        globals.raw_set("gg", gg)?;
        lua.sandbox(true)?;
        hooks::install(&lua)?;
        // Collected at set points alone, as [`collect`](Self::collect) does: Luau paces its own
        // collections by the clock.
        lua.gc_stop();
        Ok(lua)
    }

    /// A set point, between calls into the scripts: collects the VM's garbage, whole, `now`, or
    /// once it holds as much as its [`goal`](Self::goal), which is then what it holds alive and as
    /// much again, or [`LEAST_GARBAGE`] more if that's more, but no more than halfway to its limit.
    /// A collection's pause grows with what the VM holds alive, which collecting in steps would
    /// spread, once a game's scripts hold enough that it shows.
    fn collect(&mut self, now: bool) {
        if !now && self.lua.used_memory() < self.goal {
            return;
        }
        let _zone = gg_core::zone!();
        // It fails only as a finalizer would, and scripts make none.
        let _ = self.lua.gc_collect();
        // A collection sets Luau's own pace again.
        self.lua.gc_stop();
        let alive = self.lua.used_memory();
        let goal = alive + alive.max(LEAST_GARBAGE);
        self.goal = (self.limit).map_or(goal, |limit| goal.min(alive.midpoint(limit.max(alive))));
    }

    /// Loads the script at `name`, a path from the project's root, whose text is `text`, in
    /// `world`, and returns the name of the component type it declares, as [`Scripts::load`]
    /// says.
    pub(super) fn load(&mut self, world: &mut World, name: &str, text: String) -> Result<String> {
        let chunk = self.run_chunk(world, name, &text, None);
        self.resolve(world);
        // A set point, so that what a weak table the chunk keeps holds is alike everywhere.
        self.collect(true);
        let chunk = chunk?;
        #[cfg(feature = "dev")]
        self.shared.hooked.loaded(name, chunk.function);
        self.scripts.retain(|loaded| loaded.path != name);
        let at = self
            .scripts
            .partition_point(|loaded| loaded.component < chunk.component);
        let component = chunk.component.clone();
        self.scripts.insert(
            at,
            Loaded {
                path: name.to_owned(),
                text,
                component: chunk.component,
                fields: chunk.fields,
                id: Some(chunk.id),
                on_id: (chunk.on.as_deref()).and_then(|on| world.component_named(on)),
                on: chunk.on,
                tick: chunk.tick,
                #[cfg(not(feature = "dev"))]
                chunk: chunk.function,
                failed: false,
            },
        );
        Ok(component)
    }

    /// The script loaded at `path`, if there's one.
    pub(super) fn loaded(&self, path: &str) -> Option<usize> {
        self.scripts.iter().position(|loaded| loaded.path == path)
    }

    pub(super) fn run(&mut self, world: &mut World) {
        let _zone = gg_core::zone!();
        if self.scripts.is_empty() {
            return;
        }
        match self.resolved {
            // Its types changed in place, as another VM's script reloads, whose fields may be
            // elsewhere now, or gone.
            Some((serial, revision)) if serial == world.serial() => {
                if revision != world.types_revision() {
                    self.resolve(world);
                }
            }
            _ => self.migrate(world),
        }
        let dt = Tick::DT.as_secs_f64();
        let mut list = world.take_list();
        let Self {
            shared, scripts, ..
        } = self;
        // A mod's calls this tick share one budget.
        shared.budget.set(BUDGET);
        shared.fresh.borrow_mut().clear();
        lent(shared, world, || {
            for script in scripts.iter_mut() {
                let Some(id) = script.id else {
                    continue;
                };
                if let Some(on) = script.on_id {
                    let mut world = shared.world.borrow_mut();
                    world.list_with(on, &mut list);
                    for &entity in &list {
                        if !world.has(entity, id) {
                            let added = world.add_default(entity, &script.component);
                            added.expect("an entity alive is given a component it hasn't");
                            entities::rides(shared, entity, on, id);
                        }
                    }
                }
                let Some(tick) = &script.tick else {
                    continue;
                };
                shared.world.borrow().list_with(id, &mut list);
                for &entity in &list {
                    // Gone, or without the component, since the list was made; or given it as the
                    // scripts ran, as `entities` says.
                    if !shared.world.borrow().has(entity, id) || entities::fresh(shared, entity, id)
                    {
                        continue;
                    }
                    if shared.held.is_none() {
                        shared.budget.set(BUDGET);
                    }
                    #[cfg(feature = "dev")]
                    shared.hooked.call();
                    let ran = tick.call::<()>((handle(entity.to_bits()), dt));
                    if let Err(error) = ran
                        && !mem::replace(&mut script.failed, true)
                    {
                        let world = shared.world.borrow();
                        let entity = world.id(entity).map(|id| id.to_string());
                        // The engine's own error's code, as of a despawned entity reached.
                        let cause = error.downcast_ref::<Error>().map(Error::code);
                        warn!(
                            code = "script.error",
                            script = script.path,
                            entity,
                            cause,
                            "{error}; it runs on, for this entity and others, as it does"
                        );
                    }
                }
            }
        });
        world.keep_list(list);
        if let Some(held) = &self.shared.held {
            held.ticked();
        }
        self.collect(false);
    }

    /// Reloads the script loaded at `path`, its text now `text`, in `world`, as [`Hosts::reload`]
    /// says; nothing changes if its text is as it was.
    pub(super) fn reload(&mut self, world: &mut World, path: &str, text: String) -> Result<()> {
        let at = self.loaded(path).expect("the script at `path` has loaded");
        if self.scripts[at].text == text {
            return Ok(());
        }
        let declared = self.scripts[at].component.clone();
        let chunk = self.run_chunk(world, path, &text, Some(&declared));
        // Its handles found again, as the chunk ran, and the fields as they are now.
        self.resolve(world);
        self.collect(true);
        let chunk = chunk?;
        #[cfg(feature = "dev")]
        self.shared.hooked.loaded(path, chunk.function);
        let loaded = &mut self.scripts[at];
        loaded.text = text;
        loaded.fields = chunk.fields;
        loaded.id = Some(chunk.id);
        loaded.on_id = (chunk.on.as_deref()).and_then(|on| world.component_named(on));
        loaded.on = chunk.on;
        loaded.tick = chunk.tick;
        #[cfg(not(feature = "dev"))]
        {
            loaded.chunk = chunk.function;
        }
        loaded.failed = false;
        info!(script = path, "Script reloaded");
        Ok(())
    }

    /// Has the types that the scripts declare in `world` hold their fields as the scripts declare
    /// them, each that doesn't, as a world from before a reload doesn't, declared again, as a
    /// reload declares it; and finds again the types and fields the handles name, as
    /// [`resolve`](Self::resolve) does.
    pub(super) fn migrate(&mut self, world: &mut World) {
        for script in &self.scripts {
            let Some(id) = world.component_named(&script.component) else {
                continue;
            };
            // A Rust type of its name, which the script then doesn't run for.
            if world
                .declared(id)
                .is_none_or(|known| known.declares(&script.fields))
            {
                continue;
            }
            let (path, component) = (&script.path, &script.component);
            let held = self.shared.held.as_ref();
            let fields = script.fields.clone();
            if let Err(error) = redeclare(world, held, path, id, component, fields) {
                warn!(code = error.code(), script = path, "{error:#}");
            }
        }
        self.resolve(world);
    }

    /// Runs the chunk of the script at `path`, whose text is `text`, in `world`, which it may
    /// declare its component in, as its first load does, or declare again with other fields; or,
    /// reloading, where it declared `before`.
    fn run_chunk(
        &mut self,
        world: &mut World,
        path: &str,
        text: &str,
        before: Option<&str>,
    ) -> Result<Chunk> {
        let _zone = gg_core::zone!();
        let bytecode = compiled(path, text)?;
        let env = self.environment().map_err(|error| bad(path, &error))?;
        *self.shared.loading.borrow_mut() = Some(Loading {
            path: path.to_owned(),
            before: before.map(str::to_owned),
            declared: None,
            refused: None,
        });
        let (lua, shared) = (&self.lua, &self.shared);
        let ran = lent(shared, world, || {
            // The engine's compiler's, from the text, which is all a script may be.
            let chunk = lua.load(&bytecode[..]).set_name(format!("={path}"));
            let chunk = chunk
                .set_mode(ChunkMode::Binary)
                .set_environment(env.clone());
            let function = chunk.into_function()?;
            shared.budget.set(BUDGET);
            #[cfg(feature = "dev")]
            shared.hooked.call();
            let returned = function.call::<Lua_>(())?;
            Ok((function, returned))
        });
        let loading = self.shared.loading.borrow_mut().take();
        let loading = loading.expect("the script's loading is kept until it's run");
        let (function, returned) = match ran {
            Ok(ran) => ran,
            Err(error) => return Err(loading.refused.unwrap_or_else(|| bad(path, &error))),
        };
        let Some(Declaring {
            id,
            component,
            table,
            fields,
            changed,
            on,
        }) = loading.declared
        else {
            return Err(Error::new(
                "script.bad",
                format!("{path} declares no component, which a script does with `gg.declare`"),
            ));
        };
        let taken = (self.scripts.iter())
            .find(|loaded| loaded.component == component && loaded.path != path);
        if let Some(other) = taken {
            return Err(Error::new(
                "reflect.name_taken",
                format!(
                    "{path} declares `{component}`, which {} declares",
                    other.path
                ),
            ));
        }
        let Lua_::Table(returned) = returned else {
            return Err(returns(path, &component));
        };
        // By identity: `equals` would call the tables' `__eq`, the script's code, outside any call
        // into it.
        if !ptr::eq(returned.to_pointer(), table.to_pointer()) {
            return Err(returns(path, &component));
        }
        let tick = match table.raw_get::<Lua_>("tick") {
            Ok(Lua_::Function(tick)) => Some(tick),
            Ok(Lua_::Nil) => None,
            Ok(other) => {
                return Err(Error::new(
                    "script.bad",
                    format!(
                        "{path}: `{component}.tick` is a {}, where it's a function or nil",
                        other.type_name()
                    ),
                ));
            }
            Err(error) => return Err(bad(path, &error)),
        };
        settle(&self.lua, path, &env, &table)?;
        if changed {
            let held = self.shared.held.as_ref();
            redeclare(world, held, path, id, &component, fields.clone())?;
        }
        Ok(Chunk {
            component,
            fields,
            id,
            on,
            tick,
            function,
        })
    }

    /// A script's own globals, which read the VM's where it has none of its own, as Luau's sandbox
    /// gives each thread: safe, so that the compiler's imports and builtins go fast, which the
    /// VM's being read-only keeps true.
    fn environment(&self) -> mlua::Result<Table> {
        let env = self.lua.create_table()?;
        let meta = self.lua.create_table()?;
        meta.raw_set("__index", self.lua.globals())?;
        meta.set_readonly(true);
        env.set_metatable(Some(meta))?;
        env.set_safeenv(true);
        Ok(env)
    }

    /// Finds again in `world` the component types that the handles and the scripts name, and the
    /// handles' fields, by name, as another world, restored or loaded, may give them other ids,
    /// and a type declared again its fields other places.
    fn resolve(&mut self, world: &World) {
        for field in self.shared.fields.get_mut() {
            let found = world.component_named(&field.component).and_then(|id| {
                let mut fields = world.fields_of(id).iter();
                let at = fields.position(|found| {
                    found.name == field.name && Field::of(&found.ty) == Some(field.ty)
                })?;
                Some((id, at))
            });
            field.id = found.map(|(id, _)| id);
            if let Some((_, at)) = found {
                field.index = u32::try_from(at).expect("fewer than 2^32 fields");
            }
        }
        for script in &mut self.scripts {
            script.id = world
                .component_named(&script.component)
                .filter(|&id| world.declared(id).is_some());
            script.on_id = (script.on.as_deref()).and_then(|on| world.component_named(on));
        }
        self.resolved = Some((world.serial(), world.types_revision()));
    }
}

/// Freezes what a script's chunk keeps once it has run, as [`Scripts`] says: every table that
/// `env`, its globals, and `component`, its component's table, reach, through tables' keys, values
/// and metatables and functions' upvalues, `env` and `component` among them.
///
/// Fails with `script.state` if they reach a coroutine or a buffer.
fn settle(lua: &Lua, path: &str, env: &Table, component: &Table) -> Result<()> {
    let state = |kept: &str| {
        Error::new(
            "script.state",
            format!(
                "{path}: its chunk keeps a {kept}, whose state no snapshot, reload or peer joining \
                 keeps: keep it in a component's fields"
            ),
        )
    };
    let mut seen = HashSet::new();
    let mut left = vec![Lua_::Table(env.clone()), Lua_::Table(component.clone())];
    while let Some(value) = left.pop() {
        match value {
            Lua_::Table(table) => {
                if !seen.insert(table.to_pointer()) {
                    continue;
                }
                left.extend(table.metatable().map(Lua_::Table));
                let pairs = table.for_each::<Lua_, Lua_>(|key, value| {
                    left.extend([key, value]);
                    Ok(())
                });
                pairs.map_err(|error| bad(path, &error))?;
                table.set_readonly(true);
            }
            Lua_::Function(function) => {
                if seen.insert(function.to_pointer()) {
                    let upvalues = hooks::upvalues(lua, &function);
                    left.extend(upvalues.map_err(|error| bad(path, &error))?);
                }
            }
            Lua_::Thread(_) => return Err(state("coroutine")),
            Lua_::Buffer(_) => return Err(state("buffer")),
            _ => {}
        }
    }
    Ok(())
}

/// Runs `f` with `world` lent to the functions Luau calls, in place of `shared`'s, which it takes
/// back after, even as a panic unwinds.
pub(super) fn lent<R>(shared: &Shared, world: &mut World, f: impl FnOnce() -> R) -> R {
    struct Back<'a> {
        world: &'a mut World,
        shared: &'a Shared,
    }
    impl Drop for Back<'_> {
        fn drop(&mut self) {
            mem::swap(self.world, &mut self.shared.world.borrow_mut());
        }
    }
    mem::swap(world, &mut shared.world.borrow_mut());
    let _back = Back { world, shared };
    SHARED.set(shared, f)
}

/// The error that the script at `path` failed as Luau says, `script.bad`.
pub(super) fn bad(path: &str, error: &mlua::Error) -> Error {
    Error::new("script.bad", format!("{path}: {error}"))
}

/// The error that the script at `path` doesn't return the component it declared.
fn returns(path: &str, component: &str) -> Error {
    Error::new(
        "script.bad",
        format!("{path} returns other than `{component}`, as `gg.declare` returns it, as it ends"),
    )
}

/// The doc of the component type the script at `path` declares, a game's or, as `held` says, a
/// mod's, which names its mod, as no game's script's doc does.
pub(super) fn doc(held: Option<&Held>, path: &str) -> String {
    match held {
        Some(held) => format!("Declared by mod `{}`'s `{path}`.", held.name),
        None => format!("Declared by `{path}`."),
    }
}

/// Declares `component`, type `id` of `world`, which the script at `path` declared, again, of
/// `fields`, as [`Scripts`] says: logs the fields added and dropped, and warns, `script.field_reset`,
/// of those whose kinds changed.
///
/// Fails as [`World::redeclare`] does, changing nothing.
fn redeclare(
    world: &mut World,
    held: Option<&Held>,
    path: &str,
    id: ComponentId,
    component: &str,
    fields: Vec<(Ident, Kind, Value)>,
) -> Result<()> {
    let known = world.declared(id).expect("a type a script declared");
    let kind = |name: &Ident| {
        let mut known = known.fields.iter();
        known.find_map(|(field, kind)| (field == name).then_some(*kind))
    };
    let added = fields.iter().filter(|(name, ..)| kind(name).is_none());
    let added = list(added.map(|(name, ..)| name));
    let dropped = (known.fields.iter()).filter(|(name, _)| !fields.iter().any(|(f, ..)| f == name));
    let dropped = list(dropped.map(|(name, _)| name));
    let reset: Vec<&Ident> = (fields.iter())
        .filter(|(name, of, _)| kind(name).is_some_and(|kind| kind != *of))
        .map(|(name, ..)| name)
        .collect();
    let reset = (!reset.is_empty()).then(|| list(reset));
    world.redeclare(id, &doc(held, path), fields)?;
    info!(
        script = path,
        component,
        added,
        dropped,
        "Each `{component}` holds its fields as the script declares them now"
    );
    if let Some(reset) = reset {
        warn!(
            code = "script.field_reset",
            script = path,
            component,
            "`{component}`'s {reset} hold another kind now: each `{component}` has them at their \
             defaults"
        );
    }
    Ok(())
}
