//! Luau scripts: components that scripts declare, which act, each for the entity it's on, at its
//! layer's ticks, as [`Scripts`] says.

use std::any::Any;
use std::cell::RefCell;
use std::path::Path;
use std::{fs, mem, ptr};

use gg_core::math::{Quat, Vec2, Vec3, Vec4, libm};
use gg_core::{Error, Result};
use mlua::chunk::{ChunkMode, Compiler};
use mlua::{
    Function, LightUserData, Lua, LuaOptions, LuaString, StdLib, Table, Value as Lua_, Variadic,
    Vector,
};
use tracing::{info, warn};

use crate::ecs::{ComponentId, Kind};
use crate::reflect::{Ident, Reflect, Type, Value};
use crate::{Context, Entity, Tick, World};

// An entity's 64 bits are a script's light userdata, a pointer's.
const _: () = assert!(
    usize::BITS == 64,
    "scripts hold an entity's 64 bits in a pointer"
);

scoped_tls::scoped_thread_local! {
    /// What scripts reach as they run or load, which the functions Luau calls find here.
    static SHARED: Shared
}

/// The functions of `math` whose precision is the platform's, which libm's take the place of: the
/// compiler is told they aren't builtins, which it would otherwise call the C library's for.
const LIBM: [&str; 14] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sinh", "cosh", "tanh", "exp", "log",
    "log10", "pow",
];

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

/// A layer's Luau scripts. It loads each from a file in the layer's `load`, with the component
/// type the script declares, which entities have to run it; runs them where the layer's tick
/// chooses, each script's tick once for each entity with its component, in the order queries find
/// them, the scripts in the order of their components' names; and, in a development build, reloads
/// one as its file is saved, at a tick, through an edit that a recording keeps and a replay
/// applies there. The VM stays on the thread that made it, which runs the ticks.
///
/// A script is a component: its file declares one with `gg.declare`, its fields' names and their
/// defaults, numbers, bools, vectors, or `gg.entity` for an entity, none by default; and a `tick`
/// that runs for each entity that has it, given the entity and the tick's length in seconds. It
/// reaches components' fields, Rust's and scripts', by handles, `gg.component("Shard").angle`, and
/// its own's, `Spin.speed`, which an entity reads and writes as `entity[Shard.angle]`: a number,
/// a bool, a vector, a quaternion as a vector, or an entity. It keeps its state in its components,
/// as a game keeps its in the world: what its chunk's locals hold from tick to tick, no snapshot,
/// reload or peer joining keeps.
///
/// ```lua
/// local Shard = gg.component("Shard")
/// local Spin = gg.declare("Spin", { speed = 1.5 })
///
/// function Spin.tick(entity, dt)
///     entity[Shard.angle] = (entity[Shard.angle] + entity[Spin.speed] * dt) % (2 * math.pi)
/// end
///
/// return Spin
/// ```
///
/// Scripts run alike on every platform, as the simulation must. Luau's C++ fuses no
/// multiplication into an addition, as `.cargo/config.toml` builds it. The functions of `math`
/// whose precision is the platform's are libm's, which the compiler is told aren't builtins: it
/// would call the C library's in their place. A script that uses `^`, which calls the platform's
/// `pow` past any hook, or that sets how it compiles, with `--!optimize` or `--!native`, is
/// refused. `math.random`, whose state the VM holds outside the world and seeds from the clock,
/// isn't there, nor are `os`, `debug`, `collectgarbage`, `gcinfo`, `getfenv`, `setfenv`,
/// `loadstring`, `newproxy`, `require` and `vector.angle`. And an entity is light userdata, its 64
/// bits, which tables hash and compare by them rather than by an address.
pub struct Scripts {
    lua: Lua,
    shared: Shared,
    /// In the order of their components' names.
    scripts: Vec<Loaded>,
    /// The world whose component types the handles name, by its serial.
    serial: Option<u64>,
}

/// What the functions Luau calls reach: the world, lent in place of the one kept here as scripts
/// run or load, and what the handles of fields name.
struct Shared {
    world: RefCell<World>,
    /// By handle.
    fields: RefCell<Vec<FieldRef>>,
    /// The script loading, while one is.
    loading: RefCell<Option<Loading>>,
}

/// A field a handle names: field `index` of component type `component`.
struct FieldRef {
    component: Ident,
    /// The type's id in the world scripts last ran in; `None` if it has no such type there.
    id: Option<ComponentId>,
    index: u32,
    name: Ident,
    ty: Field,
}

/// What a field a script reaches holds.
#[derive(Clone, Copy, PartialEq)]
enum Field {
    Bool,
    F32,
    F64,
    Int { signed: bool },
    Vec2,
    Vec3,
    Vec4,
    Quat,
    Entity,
    MaybeEntity,
}

impl Field {
    /// What a field of type `ty` holds, if it's one that scripts reach.
    fn of(ty: &Type) -> Option<Self> {
        Some(match ty {
            Type::Bool => Self::Bool,
            Type::F32 => Self::F32,
            Type::F64 => Self::F64,
            &Type::Int { signed, .. } => Self::Int { signed },
            Type::Vec2 => Self::Vec2,
            Type::Vec3 => Self::Vec3,
            Type::Vec4 => Self::Vec4,
            Type::Quat => Self::Quat,
            Type::Entity => Self::Entity,
            Type::Option(inner) if **inner == Type::Entity => Self::MaybeEntity,
            _ => return None,
        })
    }
}

/// A script that has loaded.
struct Loaded {
    /// Its path from the project's root, which a reload names it by.
    path: String,
    text: String,
    /// The component type it declared, and its id in the world it last ran in.
    component: String,
    id: Option<ComponentId>,
    tick: Option<Function>,
    /// Whether it has failed since it last loaded, which warns once.
    failed: bool,
}

/// The script loading: its path, and what it declared.
struct Loading {
    path: String,
    declared: Option<(ComponentId, String, Table)>,
    /// Why it's refused, if a function it called refused it.
    refused: Option<Error>,
}

/// A script's chunk, run: the component it declared, and its tick.
struct Chunk {
    component: String,
    id: ComponentId,
    tick: Option<Function>,
}

/// The scripts reloaded since the last tick, as edits queue them, by path, which the [`Scripts`]
/// that loaded each take as their next tick starts.
#[derive(Clone, Default)]
pub(crate) struct Reloads(Vec<(String, String)>);

/// Queues the reload of the script at `path`, its text now `text`, as an edit applies it.
pub(crate) fn queue(world: &mut World, path: String, text: String) {
    match world.resource_mut::<Reloads>() {
        Some(reloads) => reloads.0.push((path, text)),
        None => {
            world.insert_resource(Reloads(vec![(path, text)]));
        }
    }
}

impl Scripts {
    /// A VM for scripts, with the globals [`Scripts`] says.
    ///
    /// Fails with `script.vm` if Luau can't start, as for want of memory.
    pub fn new() -> Result<Self> {
        let vm = |error: mlua::Error| Error::new("script.vm", format!("Luau can't start: {error}"));
        let lua = Self::vm().map_err(vm)?;
        Ok(Self {
            lua,
            shared: Shared {
                world: RefCell::new(World::default()),
                fields: RefCell::new(Vec::new()),
                loading: RefCell::new(None),
            },
            scripts: Vec::new(),
            serial: None,
        })
    }

    fn vm() -> mlua::Result<Lua> {
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
        globals.raw_set("print", lua.create_function(print)?)?;
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

        let entities = lua.create_table()?;
        entities.raw_set("__index", lua.create_function(read)?)?;
        entities.raw_set("__newindex", lua.create_function(write)?)?;
        entities.raw_set("__tostring", lua.create_function(describe)?)?;
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
            let axis = Vec3::new(axis.x(), axis.y(), axis.z());
            Ok(vector_of(Quat::from_axis_angle(axis, angle as f32)))
        })?;
        quat.raw_set("from_axis_angle", axis_angle)?;
        let mul = lua.create_function(|_, (a, b): (Vector, Vector)| {
            Ok(vector_of(quat_of(a) * quat_of(b)))
        })?;
        quat.raw_set("mul", mul)?;
        quat.set_readonly(true);
        gg.raw_set("quat", quat)?;
        gg.set_readonly(true);
        globals.raw_set("gg", gg)?;
        lua.sandbox(true)?;
        Ok(lua)
    }

    /// Loads the script at `path`, declaring the component type it declares in `cx`'s world, and
    /// returns that type's name, which an entity is given to run the script, as
    /// [`World::add_default`] gives it. A development build watches its file, reloading it as it's
    /// saved, as [`Scripts`] says. A script loaded again, at the same path, takes its own place.
    ///
    /// Fails with `script.unreadable` if the file can't be read; `script.pow` and
    /// `script.header` for a script refused as [`Scripts`] says; `script.bad` for one that
    /// doesn't compile, that fails as its chunk runs, or that returns other than the component it
    /// declares, as `gg.declare` returns it; `reflect.name_taken` for a component type another has;
    /// and as declaring it does.
    pub fn load(&mut self, cx: &mut Context, path: impl AsRef<Path>) -> Result<String> {
        let path = path.as_ref();
        let name = crate::assets::name_of(cx.root.as_deref(), path).to_string();
        let text = fs::read_to_string(path).map_err(|error| {
            Error::new(
                "script.unreadable",
                format!("can't read {}: {error}", path.display()),
            )
        })?;
        let world = cx.world_mut();
        let chunk = self.run_chunk(world, &name, &text, None)?;
        let taken = self
            .scripts
            .iter()
            .find(|loaded| loaded.component == chunk.component);
        if let Some(other) = taken.filter(|loaded| loaded.path != name) {
            return Err(Error::new(
                "reflect.name_taken",
                format!(
                    "{name} declares `{}`, which {} declares",
                    chunk.component, other.path
                ),
            ));
        }
        self.scripts.retain(|loaded| loaded.path != name);
        let at = self
            .scripts
            .partition_point(|loaded| loaded.component < chunk.component);
        let component = chunk.component.clone();
        self.scripts.insert(
            at,
            Loaded {
                path: name.clone(),
                text: text.clone(),
                component: chunk.component,
                id: Some(chunk.id),
                tick: chunk.tick,
                failed: false,
            },
        );
        self.serial = Some(world.serial());
        #[cfg(feature = "dev")]
        cx.scripts.watch(name, path, text);
        Ok(component)
    }

    /// Runs each script's tick once for each entity with its component, as [`Scripts`] says,
    /// after reloading those whose reloads the world has queued since. A script that fails warns
    /// once, `script.error`, until it reloads, and runs on for the other entities. Allocates
    /// nothing that the scripts themselves don't.
    pub fn run(&mut self, world: &mut World) {
        let _zone = gg_core::zone!();
        if self.scripts.is_empty() {
            return;
        }
        self.take_reloads(world);
        if self.serial != Some(world.serial()) {
            self.resolve(world);
        }
        let dt = Tick::DT.as_secs_f64();
        let mut list = world.take_list();
        let Self {
            shared, scripts, ..
        } = self;
        lent(shared, world, || {
            for script in scripts.iter_mut() {
                let (Some(tick), Some(id)) = (&script.tick, script.id) else {
                    continue;
                };
                shared.world.borrow().list_with(id, &mut list);
                for &entity in &list {
                    // Gone, or without the component, since the list was made.
                    if !shared.world.borrow().has(entity, id) {
                        continue;
                    }
                    let ran = tick.call::<()>((handle(entity.to_bits()), dt));
                    if let Err(error) = ran
                        && !mem::replace(&mut script.failed, true)
                    {
                        let world = shared.world.borrow();
                        let entity = world.id(entity).map(|id| id.to_string());
                        warn!(
                            code = "script.error",
                            script = script.path,
                            entity,
                            "{error}; it runs on, for this entity and others, as it does"
                        );
                    }
                }
            }
        });
        world.keep_list(list);
    }

    /// Reloads each of the scripts whose reloads `world` has queued, in the order they came.
    fn take_reloads(&mut self, world: &mut World) {
        let Some(reloads) = world.resource_mut::<Reloads>() else {
            return;
        };
        if reloads.0.is_empty() {
            return;
        }
        let queued = mem::take(&mut reloads.0);
        let (ours, others): (Vec<_>, Vec<_>) = (queued.into_iter())
            .partition(|(path, _)| self.scripts.iter().any(|loaded| loaded.path == *path));
        if let Some(reloads) = world.resource_mut::<Reloads>() {
            reloads.0 = others;
        }
        for (path, text) in ours {
            self.reload(world, &path, text);
        }
    }

    /// Reloads the script at `path`, its text now `text`: refused, warning, if it doesn't load or
    /// declares its component otherwise, as for now a reload doesn't change a component's fields.
    fn reload(&mut self, world: &mut World, path: &str, text: String) {
        let Some(at) = self.scripts.iter().position(|loaded| loaded.path == path) else {
            return;
        };
        if self.scripts[at].text == text {
            return;
        }
        let declared = self.scripts[at].component.clone();
        match self.run_chunk(world, path, &text, Some(&declared)) {
            Ok(chunk) => {
                let loaded = &mut self.scripts[at];
                loaded.text = text;
                loaded.id = Some(chunk.id);
                loaded.tick = chunk.tick;
                loaded.failed = false;
                info!(script = path, "Script reloaded");
            }
            Err(error) => warn!(
                code = error.code(),
                script = path,
                "{error:#}; the script runs as it did"
            ),
        }
    }

    /// Runs the chunk of the script at `path`, whose text is `text`, in `world`, which it may
    /// declare its component in, as its first load does; or, reloading, where it declared `before`.
    fn run_chunk(
        &mut self,
        world: &mut World,
        path: &str,
        text: &str,
        before: Option<&str>,
    ) -> Result<Chunk> {
        let _zone = gg_core::zone!();
        check(text).map_err(|error| {
            let message = format!("{path}:{error}");
            error.with_message(message)
        })?;
        let env = self.environment().map_err(|error| bad(path, &error))?;
        *self.shared.loading.borrow_mut() = Some(Loading {
            path: path.to_owned(),
            declared: None,
            refused: None,
        });
        let lua = &self.lua;
        let ran = lent(&self.shared, world, || {
            let chunk = lua.load(text).set_name(format!("={path}"));
            let chunk = chunk.set_mode(ChunkMode::Text).set_environment(env);
            chunk.call::<Lua_>(())
        });
        let loading = self.shared.loading.borrow_mut().take();
        let loading = loading.expect("the script's loading is kept until it's run");
        let returned = match ran {
            Ok(returned) => returned,
            Err(error) => return Err(loading.refused.unwrap_or_else(|| bad(path, &error))),
        };
        let Some((id, component, table)) = loading.declared else {
            return Err(Error::new(
                "script.bad",
                format!("{path} declares no component, which a script does with `gg.declare`"),
            ));
        };
        if let Some(before) = before.filter(|&before| before != component) {
            return Err(Error::new(
                "script.fields_changed",
                format!(
                    "{path} declares `{component}`, where it declared `{before}`, which a reload \
                     doesn't change"
                ),
            ));
        }
        let Lua_::Table(returned) = returned else {
            return Err(returns(path, &component));
        };
        if !returned.equals(&table).map_err(|error| bad(path, &error))? {
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
        table.set_readonly(true);
        Ok(Chunk {
            component,
            id,
            tick,
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

    /// Finds again in `world` the component types that the handles and the scripts name, by
    /// name, as another world, restored or loaded, may give them other ids.
    fn resolve(&mut self, world: &World) {
        for field in self.shared.fields.get_mut() {
            field.id = world.component_named(&field.component).filter(|&id| {
                let found = world.fields_of(id).get(field.index as usize);
                found.is_some_and(|found| {
                    found.name == field.name && Field::of(&found.ty) == Some(field.ty)
                })
            });
        }
        for script in &mut self.scripts {
            script.id = world
                .component_named(&script.component)
                .filter(|&id| world.declared(id).is_some());
        }
        self.serial = Some(world.serial());
    }
}

/// Runs `f` with `world` lent to the functions Luau calls, in place of `shared`'s, which it takes
/// back after, even as a panic unwinds.
fn lent<R>(shared: &Shared, world: &mut World, f: impl FnOnce() -> R) -> R {
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

/// The compiler of scripts: at optimization level 1, whatever a script asks, with libm's math in
/// place of the builtins the C library's would be, as [`Scripts`] says.
fn compiler() -> Compiler {
    Compiler::new()
        .set_optimization_level(1)
        .set_debug_level(1)
        .set_disabled_builtins(LIBM.map(|name| format!("math.{name}")))
}

/// Checks that `text` compiles, as a script loads it: refused as [`check`] refuses it, or as it
/// doesn't compile, `script.bad`, where the message is the compiler's.
#[cfg(feature = "dev")]
pub(crate) fn compiles(path: &str, text: &str) -> Result<()> {
    check(text).map_err(|error| {
        let message = format!("{path}:{error}");
        error.with_message(message)
    })?;
    match compiler().compile(text) {
        Ok(_) => Ok(()),
        Err(error) => Err(bad(path, &error)),
    }
}

/// Checks what the compiler doesn't of `text`, a script's: that it uses no `^`, with
/// `script.pow`, and sets nothing of how it compiles, with `script.header`, either error's message
/// starting with the line it's on, then `: `. Strings, comments, and the text of interpolated
/// strings are skipped.
fn check(text: &str) -> Result<()> {
    let bytes = text.as_bytes();
    let mut line = 1;
    let mut i = 0;
    // The braces opened in each interpolated string's expression we're in, the innermost last.
    let mut nested: Vec<u32> = Vec::new();
    // Whether we're in an interpolated string's text, outside its braces.
    let mut quoted = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            line += 1;
        }
        if quoted {
            match c {
                b'\\' => {
                    i += 1;
                    line += usize::from(bytes.get(i) == Some(&b'\n'));
                }
                b'`' => quoted = false,
                b'{' => {
                    nested.push(0);
                    quoted = false;
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        match c {
            b'^' => {
                return Err(Error::new(
                    "script.pow",
                    format!(
                        "{line}: `^` raises to a power with the platform's `pow`, whose results \
                         differ from platform to platform; `x * x`, or `math.pow`, libm's, gives \
                         the same everywhere"
                    ),
                ));
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                if let Some(level) = long_bracket(bytes, i) {
                    i = skip_long(bytes, i, level, &mut line);
                    continue;
                }
                let end = bytes[i..].iter().position(|&c| c == b'\n');
                let comment = &bytes[i..end.map_or(bytes.len(), |end| i + end)];
                if let Some(hot) = comment.strip_prefix(b"!")
                    && (hot.starts_with(b"optimize") || hot.starts_with(b"native"))
                {
                    return Err(Error::new(
                        "script.header",
                        format!(
                            "{line}: `--!optimize` and `--!native` set how a script compiles, \
                             which the engine sets so that scripts run alike on every platform"
                        ),
                    ));
                }
                i += comment.len();
                continue;
            }
            b'[' => {
                if let Some(level) = long_bracket(bytes, i) {
                    i = skip_long(bytes, i, level, &mut line);
                    continue;
                }
            }
            b'"' | b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != c && bytes[i] != b'\n' {
                    if bytes[i] == b'\\' {
                        i += 1;
                        line += usize::from(bytes.get(i) == Some(&b'\n'));
                    }
                    i += 1;
                }
            }
            b'`' => quoted = true,
            b'{' => {
                if let Some(open) = nested.last_mut() {
                    *open += 1;
                }
            }
            b'}' => match nested.last_mut() {
                Some(0) => {
                    nested.pop();
                    quoted = true;
                }
                Some(open) => *open -= 1,
                None => {}
            },
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// The level of the long bracket that opens at `at`, `[[` being 0 and `[==[` 2, if one does.
fn long_bracket(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'[') {
        return None;
    }
    let level = bytes[at + 1..].iter().take_while(|&&c| c == b'=').count();
    (bytes.get(at + 1 + level) == Some(&b'[')).then_some(level)
}

/// Where the long string or comment of `level` that opens at `at` ends, past its close, counting
/// its lines into `line`.
fn skip_long(bytes: &[u8], at: usize, level: usize, line: &mut usize) -> usize {
    let mut i = at + level + 2;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            *line += 1;
        }
        if bytes[i] == b']'
            && bytes[i + 1..].iter().take(level).all(|&c| c == b'=')
            && bytes.get(i + 1 + level) == Some(&b']')
        {
            return i + level + 2;
        }
        i += 1;
    }
    bytes.len()
}

/// The error that the script at `path` failed as Luau says, `script.bad`.
fn bad(path: &str, error: &mlua::Error) -> Error {
    Error::new("script.bad", format!("{path}: {error}"))
}

/// The error that the script at `path` doesn't return the component it declared.
fn returns(path: &str, component: &str) -> Error {
    Error::new(
        "script.bad",
        format!("{path} returns other than `{component}`, as `gg.declare` returns it, as it ends"),
    )
}

/// Light userdata of `bits`: an entity's, a field's handle's, or `gg.entity`'s, 0.
fn handle(bits: u64) -> LightUserData {
    LightUserData(ptr::without_provenance_mut(bits as usize))
}

/// The bits light userdata holds.
fn bits(data: LightUserData) -> u64 {
    data.0.addr() as u64
}

/// The entity `data` is.
fn entity_of(data: LightUserData) -> mlua::Result<Entity> {
    Entity::from_bits(bits(data)).ok_or_else(|| {
        mlua::Error::runtime("that's a field's handle, not an entity, whose fields it reaches")
    })
}

/// The index of the field `key` is the handle of.
fn field_of(key: &Lua_) -> mlua::Result<usize> {
    match key {
        &Lua_::LightUserData(data) if bits(data) != 0 && bits(data) >> 32 == 0 => {
            Ok(bits(data) as usize - 1)
        }
        _ => Err(mlua::Error::runtime(
            "an entity's fields are reached by handles, such as `gg.component(\"Shard\").angle`",
        )),
    }
}

/// The handle of field `index` of component type `id`, named `component`, which `name` is, holding
/// `ty`: the one `fields` has, or one made.
fn handle_of(
    fields: &mut Vec<FieldRef>,
    component: &str,
    id: ComponentId,
    index: u32,
    name: &Ident,
    ty: Field,
) -> LightUserData {
    let at = fields
        .iter()
        .position(|field| field.component == component && field.index == index);
    let at = at.unwrap_or_else(|| {
        fields.push(FieldRef {
            component: Ident::Owned(component.to_owned()),
            id: Some(id),
            index,
            name: name.clone(),
            ty,
        });
        fields.len() - 1
    });
    handle(at as u64 + 1)
}

/// What the functions Luau calls reach, as they run only as scripts run or load.
fn shared<R>(f: impl FnOnce(&Shared) -> mlua::Result<R>) -> mlua::Result<R> {
    if !SHARED.is_set() {
        return Err(mlua::Error::runtime(
            "a script reaches the world only as it runs or loads",
        ));
    }
    SHARED.with(f)
}

/// `gg.component(name)`: a table of the handles of component type `name`'s fields, by name, those
/// of the kinds scripts reach.
fn component(lua: &Lua, name: LuaString) -> mlua::Result<Table> {
    shared(|shared| {
        let world = shared.world.borrow();
        let name = name.to_str()?;
        let Some(id) = world.component_named(&name) else {
            let names = crate::reflect::list(world.component_types());
            return Err(mlua::Error::runtime(format!(
                "there's no component type `{}`; the component types are {names}",
                &*name
            )));
        };
        let table = handles(lua, shared, &world, &name, id)?;
        table.set_readonly(true);
        Ok(table)
    })
}

/// A table of the handles of component type `id`'s fields, which is named `name`.
fn handles(
    lua: &Lua,
    shared: &Shared,
    world: &World,
    name: &str,
    id: ComponentId,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let mut fields = shared.fields.borrow_mut();
    for (index, field) in world.fields_of(id).iter().enumerate() {
        let Some(ty) = Field::of(&field.ty) else {
            continue;
        };
        let handle = handle_of(&mut fields, name, id, index as u32, &field.name, ty);
        table.raw_set(&*field.name, handle)?;
    }
    Ok(table)
}

/// `gg.declare(name, fields)`: declares the component type the script loading is, `name`, of
/// `fields`, each a name and a default, as [`Scripts`] says, and returns the script's
/// table, of its fields' handles, which its tick goes in. A script loading again declares the type
/// it declared, its fields as they were: for now, a reload doesn't change them.
fn declare(lua: &Lua, (name, fields): (LuaString, Table)) -> mlua::Result<Table> {
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
        if let Some((_, declared, _)) = &loading.declared {
            let what = format!("it declares `{name}`, but `{declared}` already: a script is one");
            return refuse(loading, "script.bad", what);
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
        let mut world = shared.world.borrow_mut();
        let id = match world.component_named(&name) {
            None => {
                let doc = format!("Declared by `{}`.", loading.path);
                match world.declare(&name, &doc, declared) {
                    Ok(id) => id,
                    Err(error) => {
                        let message = format!("{}: {error:#}", loading.path);
                        let message = error.with_message(message);
                        let text = message.to_string();
                        loading.refused = Some(message);
                        return Err(mlua::Error::runtime(text));
                    }
                }
            }
            Some(id) => {
                let same = world.declared(id).is_some_and(|known| {
                    let fields = world.fields_of(id);
                    known.fields.len() == declared.len()
                        && (known.fields.iter().zip(fields).zip(&declared)).all(
                            |(((field, kind), described), (name, given, default))| {
                                field == name
                                    && kind == given
                                    && described.default.as_ref() == Some(default)
                            },
                        )
                });
                if world.declared(id).is_none() {
                    let what = format!("`{name}` is a Rust component type's name");
                    return refuse(loading, "reflect.name_taken", what);
                }
                if !same {
                    let what = format!(
                        "`{name}`'s fields, their names, kinds or defaults, aren't as they were, \
                         which for now a reload doesn't change: a run started afresh takes them"
                    );
                    return refuse(loading, "script.fields_changed", what);
                }
                id
            }
        };
        let table = handles(lua, shared, &world, &name, id)?;
        loading.declared = Some((id, name, table.clone()));
        Ok(table)
    })
}

/// `entity[field]`: the field whose handle is `key`, of `entity`.
fn read(_: &Lua, (entity, key): (LightUserData, Lua_)) -> mlua::Result<Lua_> {
    shared(|shared| {
        let entity = entity_of(entity)?;
        let fields = shared.fields.borrow();
        let field = &fields[field_of(&key)?];
        let world = shared.world.borrow();
        let reached = world.field_at(entity, missing(field)?, field.index);
        let reached = reached.map_err(|error| reaching(&world, entity, field, &error))?;
        Ok(to_lua(reached, field.ty))
    })
}

/// `entity[field] = value`: sets the field whose handle is `key`, of `entity`, to `value`.
fn write(_: &Lua, (entity, key, value): (LightUserData, Lua_, Lua_)) -> mlua::Result<()> {
    shared(|shared| {
        let entity = entity_of(entity)?;
        let fields = shared.fields.borrow();
        let field = &fields[field_of(&key)?];
        let mut world = shared.world.borrow_mut();
        let id = missing(field)?;
        let set = match world.field_at_mut(entity, id, field.index) {
            Ok(reached) => set(reached, field.ty, &value),
            Err(error) => Err(error),
        };
        set.map_err(|error| reaching(&world, entity, field, &error))
    })
}

/// `tostring(entity)`: the entity as the world prints it.
fn describe(_: &Lua, data: LightUserData) -> mlua::Result<String> {
    Ok(match Entity::from_bits(bits(data)) {
        Some(entity) => format!("entity {entity}"),
        None if bits(data) == 0 => "gg.entity".to_owned(),
        None => "a field's handle".to_owned(),
    })
}

/// `print(...)`: logs what it's given, as `tostring` writes each, between tabs.
fn print(_: &Lua, values: Variadic<Lua_>) -> mlua::Result<()> {
    let texts: Vec<String> = values
        .iter()
        .map(|value| value.to_string())
        .collect::<mlua::Result<_>>()?;
    info!("{}", texts.join("\t"));
    Ok(())
}

/// The id of the component type `field` is of, in the world scripts run in.
fn missing(field: &FieldRef) -> mlua::Result<ComponentId> {
    field.id.ok_or_else(|| {
        mlua::Error::runtime(format!(
            "this world has no component type `{}` with field `{}` as it was",
            field.component, field.name
        ))
    })
}

/// The error that `field` of `entity` can't be reached in `world`, as `error` says.
fn reaching(world: &World, entity: Entity, field: &FieldRef, error: &Error) -> mlua::Error {
    let entity = world
        .id(entity)
        .map_or(entity.to_string(), |id| id.to_string());
    mlua::Error::runtime(format!(
        "can't reach `{}.{}` of entity {entity}: {error:#}",
        field.component, field.name
    ))
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
    match field.to_value() {
        Value::Bool(b) => Lua_::Boolean(b),
        Value::Int(n) => Lua_::Number(n as f64),
        Value::UInt(n) => Lua_::Number(n as f64),
        Value::F32(x) => Lua_::Number(x.into()),
        Value::F64(x) => Lua_::Number(x),
        Value::Vec2(v) => Lua_::Vector(Vector::new(v.x, v.y, 0.0, 0.0)),
        Value::Vec3(v) => Lua_::Vector(Vector::new(v.x, v.y, v.z, 0.0)),
        Value::Vec4(v) => Lua_::Vector(Vector::new(v.x, v.y, v.z, v.w)),
        Value::Quat(q) => Lua_::Vector(vector_of(q)),
        Value::Entity(entity) => Lua_::LightUserData(handle(entity.to_bits())),
        _ => unreachable!("scripts reach only fields of the types `Field` names"),
    }
}

/// Sets `field`, which holds `ty`, to `value`, a script's: without allocating, but to fail.
fn set(field: &mut dyn Reflect, ty: Field, value: &Lua_) -> Result<()> {
    let wrong = |expected: &str| {
        Error::new(
            "reflect.wrong_kind",
            format!("expected {expected}, got a {}", value.type_name()),
        )
    };
    let number = || match *value {
        Lua_::Number(x) => Ok(x),
        Lua_::Integer(n) => Ok(n as f64),
        _ => Err(wrong("a number")),
    };
    let vector = || match *value {
        Lua_::Vector(v) => Ok(v),
        _ => Err(wrong("a vector")),
    };
    let entity = || match *value {
        Lua_::LightUserData(data) => {
            Entity::from_bits(bits(data)).ok_or_else(|| wrong("an entity"))
        }
        _ => Err(wrong("an entity")),
    };
    let value = match ty {
        Field::MaybeEntity => {
            let held = match value {
                Lua_::Nil => None,
                _ => Some(entity()?),
            };
            let any: &mut dyn Any = field;
            *any.downcast_mut::<Option<Entity>>()
                .expect("an option of an entity") = held;
            return Ok(());
        }
        Field::Bool => match *value {
            Lua_::Boolean(b) => Value::Bool(b),
            _ => return Err(wrong("a bool")),
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
        Field::Quat => Value::Quat(quat_of(vector()?)),
        Field::Entity => Value::Entity(entity()?),
    };
    field.set_value(&value)
}

/// `q` as a script holds it, a vector: `x`, `y`, `z` and `w`.
fn vector_of(q: Quat) -> Vector {
    Vector::new(q.x, q.y, q.z, q.w)
}

fn quat_of(v: Vector) -> Quat {
    Quat::from_xyzw(v.x(), v.y(), v.z(), v.w())
}

#[cfg(feature = "dev")]
pub(crate) use watch::{Files, look};

/// Development builds' watch of the script files the layers loaded, which reloads one as it's
/// saved, through an edit, as scene files are.
#[cfg(feature = "dev")]
mod watch {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant, SystemTime};

    use tracing::warn;

    use crate::Context;
    use crate::edits::Edit;
    use crate::layer::Stack;

    /// How long a look waits after the last, as scene files' does.
    const LOOK: Duration = Duration::from_millis(200);

    /// The script files the layers loaded.
    #[derive(Default)]
    pub(crate) struct Files {
        files: Vec<File>,
        looked: Option<Instant>,
    }

    struct File {
        /// Its path from the project's root, which its edits name it by.
        name: String,
        path: PathBuf,
        stamp: Option<(SystemTime, u64)>,
        /// Its text as last read.
        text: String,
    }

    impl Files {
        /// Watches the file at `path`, a script's, named `name`, whose text is `text`.
        pub(crate) fn watch(&mut self, name: String, path: &Path, text: String) {
            self.files.retain(|file| file.name != name);
            self.files.push(File {
                name,
                path: path.to_owned(),
                stamp: stamp(path),
                text,
            });
        }
    }

    fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
        fs::metadata(path)
            .and_then(|meta| Ok((meta.modified()?, meta.len())))
            .ok()
    }

    /// Between ticks, as a frame starts, reloads the scripts whose files were saved since the last
    /// look, through an edit of each, which a recording keeps, so that a replay reloads it there:
    /// each that compiles, as [`compiles`](super::compiles) checks, a script that doesn't warning
    /// and running as it did. A lockstep peer doesn't for now, warning `script.lockstep`.
    pub(crate) fn look(cx: &mut Context, stack: &mut Stack) {
        let now = Instant::now();
        let files = &mut cx.scripts;
        if files.files.is_empty() || files.looked.is_some_and(|looked| now - looked < LOOK) {
            return;
        }
        files.looked = Some(now);
        let mut saves = Vec::new();
        for file in &mut files.files {
            let stamp = stamp(&file.path);
            if stamp.is_none() || stamp == file.stamp {
                continue;
            }
            // Being written, or held by its writer: a later look reads it, its stamp unchanged.
            let Ok(text) = fs::read_to_string(&file.path) else {
                continue;
            };
            file.stamp = stamp;
            if text == file.text {
                continue;
            }
            file.text.clone_from(&text);
            if let Err(error) = super::compiles(&file.name, &text) {
                warn!(
                    code = error.code(),
                    script = file.name,
                    "{error:#}; the script runs as it did"
                );
                continue;
            }
            if stack.is_peer() {
                warn!(
                    code = "script.lockstep",
                    script = file.name,
                    "a lockstep peer doesn't reload scripts yet, so that both run the same; the \
                     script runs as it did, until a run started afresh"
                );
                continue;
            }
            saves.push((file.name.clone(), text));
        }
        for (path, text) in saves {
            if let Err(error) = stack.edit(cx, Edit::Script { path, text }) {
                warn!(code = error.code(), "{error:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Component;
    use crate::dirs::tests::Scratch;
    use crate::edits::Edit;

    /// A Rust component that scripts reach, with a field of each kind they do.
    #[derive(Clone, Component)]
    struct Mover {
        angle: f32,
        speed: f64,
        steps: u32,
        on: bool,
        at: Vec3,
        turn: Quat,
        target: Option<Entity>,
        /// Of a kind scripts don't reach, which has no handle.
        label: String,
    }

    impl Mover {
        fn new() -> Self {
            Self {
                angle: 0.25,
                speed: 1.0,
                steps: 0,
                on: false,
                at: Vec3::ZERO,
                turn: Quat::IDENTITY,
                target: None,
                label: String::new(),
            }
        }
    }

    /// Spins a mover, and counts its ticks in its own fields.
    const SPIN: &str = r#"
local Mover = gg.component("Mover")
local Spin = gg.declare("Spin", {
    rate = 2,
    count = 0,
    on = true,
    toward = vector.create(1, 2, 3),
    other = gg.entity,
})

function Spin.tick(entity, dt)
    entity[Mover.angle] = entity[Mover.angle] + entity[Spin.rate] * dt
    entity[Mover.steps] = entity[Mover.steps] + 1
    entity[Mover.on] = entity[Spin.on]
    entity[Mover.at] = entity[Mover.at] + entity[Spin.toward]
    entity[Mover.turn] = gg.quat.from_rotation_z(entity[Mover.angle])
    entity[Mover.target] = entity
    entity[Spin.count] = entity[Spin.count] + 1
    entity[Spin.other] = entity[Mover.target]
end

return Spin
"#;

    /// A context whose project's root is `dir`, with `Mover` registered, and a host that has
    /// loaded `scripts`, each a path in `dir` and a text, which it writes there.
    fn loaded(dir: &Scratch, scripts: &[(&str, &str)]) -> (Context, Scripts) {
        fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
        let mut cx = Context::seeded(1).root(&dir.0);
        cx.world_mut().register::<Mover>().expect("described");
        let mut host = Scripts::new().expect("Luau starts");
        for (name, text) in scripts {
            let path = dir.0.join(name);
            fs::write(&path, text).expect("written");
            host.load(&mut cx, &path).expect("the script loads");
        }
        (cx, host)
    }

    /// What loading `text` as the script `scripts/refused.luau` fails with.
    fn refused(name: &str, text: &str) -> Error {
        let dir = Scratch::new(&format!("luau-refused-{name}"));
        fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
        let mut cx = Context::seeded(1).root(&dir.0);
        cx.world_mut().register::<Mover>().expect("described");
        let path = dir.0.join("scripts/refused.luau");
        fs::write(&path, text).expect("written");
        let mut host = Scripts::new().expect("Luau starts");
        host.load(&mut cx, &path).expect_err("refused")
    }

    /// The default of field `name` of component type `component`.
    fn default(world: &World, component: &str, name: &str) -> Option<Value> {
        match world.default_of(component).expect("a default") {
            Value::Struct(fields) => fields.into_iter().find(|(field, _)| field == name),
            _ => None,
        }
        .map(|(_, value)| value)
    }

    #[test]
    fn a_script_runs_for_each_entity_with_its_component_reaching_rust_s_fields_and_its_own() {
        let dir = Scratch::new("luau-runs");
        let (mut cx, mut host) = loaded(&dir, &[("scripts/spin.luau", SPIN)]);
        let world = cx.world_mut();
        let spun = world.spawn(Mover::new());
        let still = world.spawn(Mover::new());
        world.add_default(spun, "Spin").expect("added");
        let names: Vec<String> = match world.default_of("Spin").expect("a default") {
            Value::Struct(fields) => fields.into_iter().map(|(name, _)| name.into()).collect(),
            _ => Vec::new(),
        };
        assert_eq!(names, ["count", "on", "other", "rate", "toward"]);
        for _ in 0..4 {
            host.run(cx.world_mut());
        }
        let world = cx.world();
        let mover = world.get::<Mover>(spun).expect("a mover");
        let mut angle = 0.25f32;
        for _ in 0..4 {
            angle = (f64::from(angle) + 2.0 * Tick::DT.as_secs_f64()) as f32;
        }
        assert_eq!(mover.angle.to_bits(), angle.to_bits());
        assert_eq!(mover.steps, 4);
        assert!(mover.on);
        assert_eq!(mover.at, Vec3::new(4.0, 8.0, 12.0));
        assert_eq!(mover.turn, Quat::from_rotation_z(angle));
        assert_eq!(mover.target, Some(spun));
        let count = world.get_field(spun, "Spin", "count").expect("a count");
        assert_eq!(count, Value::F64(4.0));
        let other = world.get_field(spun, "Spin", "other").expect("an entity");
        assert_eq!(other, Value::Option(Some(Box::new(Value::Entity(spun)))));
        let untouched = world.get::<Mover>(still).expect("a mover");
        assert_eq!((untouched.angle, untouched.steps), (0.25, 0));
    }

    #[test]
    fn a_script_that_fails_for_one_entity_runs_on_for_the_others() {
        let dir = Scratch::new("luau-fails");
        let script = "local Mover = gg.component(\"Mover\")\n\
            local Step = gg.declare(\"Step\", {})\n\
            function Step.tick(entity, dt)\n\
                entity[Mover.steps] = entity[Mover.steps] + 1\n\
            end\n\
            return Step\n";
        let (mut cx, mut host) = loaded(&dir, &[("scripts/step.luau", script)]);
        let world = cx.world_mut();
        let bare = world.spawn(());
        let mover = world.spawn(Mover::new());
        world.add_default(bare, "Step").expect("added");
        world.add_default(mover, "Step").expect("added");
        host.run(cx.world_mut());
        host.run(cx.world_mut());
        assert_eq!(cx.world().get::<Mover>(mover).expect("a mover").steps, 2);
    }

    #[test]
    fn scripts_go_without_what_would_run_otherwise_on_another_platform_or_in_another_run() {
        let dir = Scratch::new("luau-sandbox");
        let script = "return gg.declare(\"Gone\", {\n\
            removed = os == nil and debug == nil and collectgarbage == nil and gcinfo == nil\n\
                and getfenv == nil and setfenv == nil and loadstring == nil and require == nil\n\
                and newproxy == nil,\n\
            random = math.random == nil and math.randomseed == nil and vector.angle == nil,\n\
            sin = math.sin(1.2345),\n\
            pow = math.pow(2, 0.5),\n\
            log = math.log(10, 3),\n\
        })\n";
        let (cx, _host) = loaded(&dir, &[("scripts/gone.luau", script)]);
        let world = cx.world();
        assert_eq!(default(world, "Gone", "removed"), Some(Value::Bool(true)));
        assert_eq!(default(world, "Gone", "random"), Some(Value::Bool(true)));
        let sin = default(world, "Gone", "sin");
        assert_eq!(sin, Some(Value::F64(libm::sin(1.2345))));
        let pow = default(world, "Gone", "pow");
        assert_eq!(pow, Some(Value::F64(libm::pow(2.0, 0.5))));
        let log = default(world, "Gone", "log");
        assert_eq!(log, Some(Value::F64(libm::log(10.0) / libm::log(3.0))));
    }

    #[test]
    fn a_script_that_raises_to_a_power_or_sets_how_it_compiles_is_refused() {
        let pow = refused("pow", "local x = 2\nlocal y = x ^ 3");
        assert_eq!(pow.code(), "script.pow");
        assert!(
            pow.to_string().starts_with("scripts/refused.luau:2: "),
            "{pow}"
        );
        assert_eq!(
            refused("compound", "local x = 2\nx ^= 2").code(),
            "script.pow"
        );
        assert_eq!(
            refused("interpolated", "local s = `{2 ^ 3}`").code(),
            "script.pow"
        );
        assert_eq!(
            refused("optimize", "--!optimize 2\nreturn 1").code(),
            "script.header"
        );
        assert_eq!(
            refused("native", "--!native\nreturn 1").code(),
            "script.header"
        );
        let quoted = "--!strict\n-- a ^ in a comment\n--[==[ and ^ ]==]\n\
            local s = \"^\" .. '\\'^' .. [[^]] .. `^{\"}\"}^{ {1} }^`\n";
        check(quoted).expect("no `^` outside strings and comments");
    }

    #[test]
    fn a_script_that_doesn_t_declare_its_component_as_a_script_does_is_refused() {
        assert_eq!(refused("syntax", "local = 1").code(), "script.bad");
        assert_eq!(refused("none", "return {}").code(), "script.bad");
        let other = "local One = gg.declare(\"One\", {})\nreturn {}";
        assert_eq!(refused("other", other).code(), "script.bad");
        let twice = "gg.declare(\"One\", {})\nreturn gg.declare(\"Two\", {})";
        assert_eq!(refused("twice", twice).code(), "script.bad");
        let rust = "return gg.declare(\"Mover\", {})";
        assert_eq!(refused("rust", rust).code(), "reflect.name_taken");
        let tick = "return gg.declare(\"Ticked\", { tick = 1 })";
        assert_eq!(refused("tick", tick).code(), "script.bad");
        let unknown = "local Nope = gg.component(\"Nope\")\nreturn gg.declare(\"Fine\", {})";
        assert_eq!(refused("unknown", unknown).code(), "script.bad");
    }

    #[test]
    fn a_reload_an_edit_queues_runs_from_the_next_tick_unless_it_changes_the_fields() {
        let dir = Scratch::new("luau-reload");
        let script = |step: u32, fields: &str| {
            format!(
                "local Mover = gg.component(\"Mover\")\n\
                 local Step = gg.declare(\"Step\", {{ {fields} }})\n\
                 function Step.tick(entity, dt)\n\
                     entity[Mover.steps] = entity[Mover.steps] + {step}\n\
                 end\n\
                 return Step\n"
            )
        };
        let first = script(1, "by = 1");
        let (mut cx, mut host) = loaded(&dir, &[("scripts/step.luau", &first)]);
        let world = cx.world_mut();
        let mover = world.spawn(Mover::new());
        world.add_default(mover, "Step").expect("added");
        host.run(cx.world_mut());
        let edit = |cx: &mut Context, text: String| {
            let edit = Edit::Script {
                path: "scripts/step.luau".to_owned(),
                text,
            };
            cx.apply_edit(edit).expect("queued");
        };
        edit(&mut cx, script(10, "by = 1"));
        host.run(cx.world_mut());
        assert_eq!(cx.world().get::<Mover>(mover).expect("a mover").steps, 11);
        // Its fields changed, and then it doesn't compile: both refused, as it runs as it did.
        edit(&mut cx, script(100, "by = 2"));
        host.run(cx.world_mut());
        edit(&mut cx, "local = ".to_owned());
        host.run(cx.world_mut());
        assert_eq!(cx.world().get::<Mover>(mover).expect("a mover").steps, 31);
        let by = default(cx.world(), "Step", "by");
        assert_eq!(by, Some(Value::F64(1.0)));
    }

    #[test]
    fn an_edit_reloading_a_script_reads_as_it_writes() {
        let edit = Edit::Script {
            path: "scripts/spin.luau".to_owned(),
            text: "return 1\n".to_owned(),
        };
        assert_eq!(Edit::from_value(&edit.to_value()).expect("an edit"), edit);
    }

    #[test]
    fn a_declared_component_copies_hashes_and_saves_as_a_rust_one_does() {
        let dir = Scratch::new("luau-saved");
        let (mut cx, mut host) = loaded(&dir, &[("scripts/spin.luau", SPIN)]);
        let world = cx.world_mut();
        let spun = world.spawn(Mover::new());
        world.add_default(spun, "Spin").expect("added");
        host.run(cx.world_mut());
        let world = cx.world();
        let copy = world.clone();
        assert_eq!(copy.hash(), world.hash());
        let whole = |world: &World| world.get_field(spun, "Spin", "").expect("a spin");
        assert_eq!(whole(&copy), whole(world));
        let mut changed = world.clone();
        let count = Value::F64(9.0);
        changed
            .set_field(spun, "Spin", "count", &count)
            .expect("set");
        assert_ne!(changed.hash(), world.hash());
        let mut text = String::new();
        world.save_whole(&mut text).expect("saved");
        changed
            .load_whole(text.lines().enumerate(), true)
            .expect("loads");
        assert_eq!(changed.hash(), world.hash());
        assert_eq!(whole(&changed), whole(world));
    }
}
