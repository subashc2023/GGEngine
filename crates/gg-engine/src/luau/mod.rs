//! Luau scripts: components that scripts declare, which act, each for the entity it's on, at its
//! layer's ticks, as [`Scripts`] says.

use std::any::Any;
use std::cell::{Cell, RefCell};
#[cfg(feature = "dev")]
use std::ffi::c_int;
use std::path::Path;
use std::rc::{Rc, Weak};
#[cfg(feature = "dev")]
use std::sync::Arc;
use std::{fs, mem, ptr};

#[cfg(feature = "dev")]
use gg_api::StopReason;
use gg_core::math::{Quat, Vec2, Vec3, Vec4, libm};
use gg_core::{Error, Result};
use mlua::chunk::{ChunkMode, Compiler};
use mlua::{
    Function, LightUserData, Lua, LuaOptions, LuaString, StdLib, Table, Value as Lua_, Variadic,
    Vector,
};
use tracing::{info, warn};

use crate::ecs::{ComponentId, Kind};
use crate::reflect::{Ident, Reflect, Type, Value, list};
use crate::{Context, Entity, Tick, World};

#[cfg(feature = "dev")]
mod debugger;
// Luau's hooks, its interrupt and its debugger's, which mlua reaches through its raw API alone:
// this module alone may use unsafe code for them.
#[allow(unsafe_code)]
mod hooks;

#[cfg(feature = "dev")]
pub(crate) use debugger::Debugger;

// An entity's 64 bits are a script's light userdata, a pointer's.
const _: () = assert!(
    usize::BITS == 64,
    "scripts hold an entity's 64 bits in a pointer"
);

scoped_tls::scoped_thread_local! {
    /// What scripts reach as they run or load, which the functions Luau calls find here.
    static SHARED: Shared
}

/// The most of Luau's interrupts, which come at each call and each turn of a loop, that a call into
/// a script, a tick or a chunk as it loads, takes before it fails, as [`Scripts`] says: far more
/// than a tick's work takes, and few enough that a loop that never ends fails soon, even in a debug
/// build.
const BUDGET: u32 = 1_000_000;

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
/// one as its file is saved, between ticks, through an edit that a recording keeps and a replay
/// applies there, and that both peers of a lockstep session apply at one tick. The VM stays on the
/// thread that made it, which runs the ticks.
///
/// A reload may change the component's fields, but not its name: each component of it keeps the
/// fields of a name and a kind it had, the rest taking their defaults; and a world from before,
/// as the tool API's `stop` brings back, does so as it comes back, as do the components an undo
/// puts back. A changed default reaches the components added after.
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
///
/// Each call into a script, a tick or its chunk as it loads, takes at most a million of Luau's
/// interrupts, which come at each call and each turn of a loop, past which it fails, alike on every
/// platform, so that a loop that never ends fails rather than hanging the game.
///
/// A development build that serves the tool API debugs the scripts through it, as gg-api's `debug`
/// says: breakpoints, by a script's path and a line, a pause and steps. A script stops inside its
/// tick, the main thread waiting there until a client has it go on; it runs slower while the
/// debugger is armed, a breakpoint placed or a pause or a step asked, and does nothing otherwise.
pub struct Scripts {
    /// Shared with the context of the run, which reloads and migrates through it, as edits and
    /// restores reach the world between ticks, where the layer's tick reaches it alone.
    host: Rc<RefCell<Host>>,
}

/// What runs a layer's scripts.
struct Host {
    lua: Lua,
    shared: Shared,
    /// In the order of their components' names.
    scripts: Vec<Loaded>,
    /// The world whose component types the handles name, by its serial.
    serial: Option<u64>,
}

/// The hosts of the layers' scripts, which a context reloads a script through as an edit asks, and
/// migrates a world it brings back through, as [`Scripts`] says; and the run's debugger, which it
/// gives each, in a run that serves the tool API.
#[derive(Default)]
pub(crate) struct Hosts {
    hosts: Vec<Weak<RefCell<Host>>>,
    #[cfg(feature = "dev")]
    debugger: Option<Arc<Debugger>>,
}

/// What the functions Luau calls and its hooks reach: the world, lent in place of the one kept here
/// as scripts run or load, what the handles of fields name, the budget of the call into a script
/// running, and what the VM keeps for the debugger.
struct Shared {
    world: RefCell<World>,
    /// By handle.
    fields: RefCell<Vec<FieldRef>>,
    /// The script loading, while one is.
    loading: RefCell<Option<Loading>>,
    /// The interrupts left to the call into a script that runs, of its [`BUDGET`].
    budget: Cell<u32>,
    #[cfg(feature = "dev")]
    hooked: debugger::Hooked,
}

/// What an interrupt does, as [`interrupted`] says.
enum Interrupt {
    Go,
    /// Fails the call, past its budget.
    Over,
    /// Stops there, as a pause asks.
    #[cfg(feature = "dev")]
    Pause,
}

/// Counts an interrupt against the budget of the call into a script that runs, which fails past it,
/// and, in a development build, stops there if a pause is asked, as [`Scripts`] says.
fn interrupted() -> Interrupt {
    if !SHARED.is_set() {
        return Interrupt::Go;
    }
    SHARED.with(|shared| {
        let left = shared.budget.get();
        if left == 0 {
            return Interrupt::Over;
        }
        shared.budget.set(left - 1);
        #[cfg(feature = "dev")]
        if shared.hooked.pausing() {
            return Interrupt::Pause;
        }
        Interrupt::Go
    })
}

/// The error of a call into a script that ran past its budget at `line` of `script`.
fn over_budget(script: &str, line: u32) -> String {
    format!(
        "{script}:{line}: the call into the script ran past its budget of {BUDGET} of Luau's \
         interrupts, which come at each call and each turn of a loop, as a loop that never ends \
         would"
    )
}

/// Whether the step under way ends at `line`, at `depth` of the stack, and why it stops there if it
/// does, as [`debugger::Hooked::stepped`] says.
#[cfg(feature = "dev")]
fn stepped(line: c_int, depth: c_int) -> Option<StopReason> {
    if !SHARED.is_set() {
        return None;
    }
    SHARED.with(|shared| shared.hooked.stepped(line, depth))
}

/// Has the script that runs stop where `frames` says, at `depth` of the stack, for `why`, until a
/// client of the debugger says what it does next, as [`debugger::Hooked::stop`] says.
#[cfg(feature = "dev")]
fn stopped(why: StopReason, depth: c_int, frames: Vec<hooks::Frame>) {
    if !SHARED.is_set() {
        return;
    }
    SHARED.with(|shared| {
        let frames = debugger::shown(frames, &shared.world.borrow());
        shared.hooked.stop(why, depth, frames);
    });
}

/// A field a handle names: field `name` of component type `component`, which holds `ty`.
struct FieldRef {
    component: Ident,
    /// The type's id in the world scripts last ran in, and the field's index there; `None` if it
    /// has no such type, or no such field, there.
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
    /// The component type it declared, its fields, and its id in the world it last ran in.
    component: String,
    fields: Vec<(Ident, Kind, Value)>,
    id: Option<ComponentId>,
    tick: Option<Function>,
    /// Whether it has failed since it last loaded, which warns once.
    failed: bool,
}

/// The script loading: its path, the component it declared before, if it's reloading, and what it
/// declares.
struct Loading {
    path: String,
    before: Option<String>,
    declared: Option<Declaring>,
    /// Why it's refused, if a function it called refused it.
    refused: Option<Error>,
}

/// The component type a script loading declared: its id, its name, the script's table, its
/// fields, and whether they're other than the world's type of its name has, which the world's
/// type takes once the script has loaded.
struct Declaring {
    id: ComponentId,
    component: String,
    table: Table,
    fields: Vec<(Ident, Kind, Value)>,
    changed: bool,
}

/// A script's chunk, run: the component it declared, its fields, its tick, and, in a development
/// build, the chunk, which breakpoints are placed in.
struct Chunk {
    component: String,
    fields: Vec<(Ident, Kind, Value)>,
    id: ComponentId,
    tick: Option<Function>,
    #[cfg(feature = "dev")]
    function: Function,
}

impl Scripts {
    /// A VM for scripts, with the globals [`Scripts`] says.
    ///
    /// Fails with `script.vm` if Luau can't start, as for want of memory.
    pub fn new() -> Result<Self> {
        let vm = |error: mlua::Error| Error::new("script.vm", format!("Luau can't start: {error}"));
        let lua = Host::vm().map_err(vm)?;
        let host = Host {
            shared: Shared {
                world: RefCell::new(World::default()),
                fields: RefCell::new(Vec::new()),
                loading: RefCell::new(None),
                budget: Cell::new(BUDGET),
                #[cfg(feature = "dev")]
                hooked: debugger::Hooked::new(lua.clone()),
            },
            lua,
            scripts: Vec::new(),
            serial: None,
        };
        Ok(Self {
            host: Rc::new(RefCell::new(host)),
        })
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
        let loaded = self
            .host
            .borrow_mut()
            .load(cx.world_mut(), &name, text.clone());
        let component = loaded?;
        cx.hosts.add(&self.host);
        #[cfg(feature = "dev")]
        cx.scripts.watch(name, path, text);
        Ok(component)
    }

    /// Runs each script's tick once for each entity with its component, as [`Scripts`] says. A
    /// script that fails warns once, `script.error`, until it reloads, and runs on for the other
    /// entities. Allocates nothing that the scripts themselves don't.
    pub fn run(&mut self, world: &mut World) {
        self.host.borrow_mut().run(world);
    }
}

impl Hosts {
    /// Keeps `host`, unless it's kept already, giving it the run's debugger, if it has one.
    fn add(&mut self, host: &Rc<RefCell<Host>>) {
        self.hosts.retain(|kept| kept.strong_count() > 0);
        if !self
            .hosts
            .iter()
            .any(|kept| ptr::eq(kept.as_ptr(), Rc::as_ptr(host)))
        {
            self.hosts.push(Rc::downgrade(host));
        }
        #[cfg(feature = "dev")]
        if let Some(debugger) = &self.debugger {
            host.borrow().shared.hooked.give(debugger);
        }
    }

    fn hosts(&self) -> impl Iterator<Item = Rc<RefCell<Host>>> + '_ {
        self.hosts.iter().filter_map(Weak::upgrade)
    }

    /// Has `debugger` debug the scripts of the run, each host's, from those loaded already.
    #[cfg(feature = "dev")]
    pub(crate) fn debug(&mut self, debugger: Arc<Debugger>) {
        for host in self.hosts() {
            host.borrow().shared.hooked.give(&debugger);
        }
        self.debugger = Some(debugger);
    }

    /// Places the breakpoints again in each host's scripts, if they've changed since they were
    /// last placed, as a frame starts, between ticks.
    #[cfg(feature = "dev")]
    pub(crate) fn place(&self) {
        for host in self.hosts() {
            host.borrow().shared.hooked.place();
        }
    }

    /// Whether a script has stopped since this last answered, so that no frame's time counts the
    /// stop.
    #[cfg(feature = "dev")]
    pub(crate) fn took_stop(&self) -> bool {
        self.debugger
            .as_ref()
            .is_some_and(|debugger| debugger.took_stop())
    }

    /// Reloads the script at `path`, its text now `text`, in `world`, through the host that loaded
    /// it, between ticks, as an edit asks: it runs from the next tick, and its component's fields
    /// change, as [`Scripts`] says. Logs `Script reloaded`, or why it's refused, the script running
    /// as it did.
    ///
    /// Fails with `script.unknown` if no layer loaded a script at `path`; `script.renamed` for one
    /// that declares another component; and as a script's load fails otherwise.
    pub(crate) fn reload(&self, world: &mut World, path: &str, text: String) -> Result<()> {
        let host = self
            .hosts()
            .find(|host| host.borrow().loaded(path).is_some());
        let reloaded = match host {
            Some(host) => host.borrow_mut().reload(world, path, text),
            None => Err(Error::new(
                "script.unknown",
                format!("no layer loaded a script at {path}, so none reloads"),
            )),
        };
        if let Err(error) = &reloaded {
            warn!(
                code = error.code(),
                script = path,
                "{error:#}; the script runs as it did"
            );
        }
        reloaded
    }

    /// Has the types that the scripts declare in `world`, which has been restored or brought back,
    /// hold their fields as the scripts declare them now, as [`Scripts`] says.
    pub(crate) fn migrate(&self, world: &mut World) {
        for host in self.hosts() {
            host.borrow_mut().migrate(world);
        }
    }

    /// Each script loaded, its path and its text as last applied, which a lockstep host's world
    /// goes to its joiner with.
    pub(crate) fn applied(&self) -> Vec<(String, String)> {
        let mut applied = Vec::new();
        for host in self.hosts() {
            let host = host.borrow();
            let scripts = host.scripts.iter();
            applied.extend(scripts.map(|loaded| (loaded.path.clone(), loaded.text.clone())));
        }
        applied
    }

    /// Reloads each of `scripts`, each a path and a text, as [`reload`](Self::reload) does, those
    /// whose texts are as they were staying as they are: a lockstep joiner's, as they come with its
    /// host's world, which it runs in place of its own files'.
    pub(crate) fn take(&self, world: &mut World, scripts: &[(String, String)]) {
        for (path, text) in scripts {
            let _ = self.reload(world, path, text.clone());
        }
    }
}

impl Host {
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
        hooks::install(&lua)?;
        Ok(lua)
    }

    /// Loads the script at `name`, a path from the project's root, whose text is `text`, in
    /// `world`, and returns the name of the component type it declares, as [`Scripts::load`]
    /// says.
    fn load(&mut self, world: &mut World, name: &str, text: String) -> Result<String> {
        let chunk = self.run_chunk(world, name, &text, None);
        self.resolve(world);
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
                tick: chunk.tick,
                failed: false,
            },
        );
        Ok(component)
    }

    /// The script loaded at `path`, if there's one.
    fn loaded(&self, path: &str) -> Option<usize> {
        self.scripts.iter().position(|loaded| loaded.path == path)
    }

    fn run(&mut self, world: &mut World) {
        let _zone = gg_core::zone!();
        if self.scripts.is_empty() {
            return;
        }
        if self.serial != Some(world.serial()) {
            self.migrate(world);
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
                    shared.budget.set(BUDGET);
                    #[cfg(feature = "dev")]
                    shared.hooked.call();
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

    /// Reloads the script loaded at `path`, its text now `text`, in `world`, as [`Hosts::reload`]
    /// says; nothing changes if its text is as it was.
    fn reload(&mut self, world: &mut World, path: &str, text: String) -> Result<()> {
        let at = self.loaded(path).expect("the script at `path` has loaded");
        if self.scripts[at].text == text {
            return Ok(());
        }
        let declared = self.scripts[at].component.clone();
        let chunk = self.run_chunk(world, path, &text, Some(&declared));
        // Its handles found again, as the chunk ran, and the fields as they are now.
        self.resolve(world);
        let chunk = chunk?;
        #[cfg(feature = "dev")]
        self.shared.hooked.loaded(path, chunk.function);
        let loaded = &mut self.scripts[at];
        loaded.text = text;
        loaded.fields = chunk.fields;
        loaded.id = Some(chunk.id);
        loaded.tick = chunk.tick;
        loaded.failed = false;
        info!(script = path, "Script reloaded");
        Ok(())
    }

    /// Has the types that the scripts declare in `world` hold their fields as the scripts declare
    /// them, each that doesn't, as a world from before a reload doesn't, declared again, as a
    /// reload declares it; and finds again the types and fields the handles name, as
    /// [`resolve`](Self::resolve) does.
    fn migrate(&mut self, world: &mut World) {
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
            if let Err(error) = redeclare(world, path, id, component, script.fields.clone()) {
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
        check(text).map_err(|error| {
            let message = format!("{path}:{error}");
            error.with_message(message)
        })?;
        let env = self.environment().map_err(|error| bad(path, &error))?;
        *self.shared.loading.borrow_mut() = Some(Loading {
            path: path.to_owned(),
            before: before.map(str::to_owned),
            declared: None,
            refused: None,
        });
        let (lua, shared) = (&self.lua, &self.shared);
        let ran = lent(shared, world, || {
            let chunk = lua.load(text).set_name(format!("={path}"));
            let chunk = chunk.set_mode(ChunkMode::Text).set_environment(env);
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
        // Kept only for the debugger.
        #[cfg(not(feature = "dev"))]
        drop(function);
        let Some(Declaring {
            id,
            component,
            table,
            fields,
            changed,
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
        if changed {
            redeclare(world, path, id, &component, fields.clone())?;
        }
        Ok(Chunk {
            component,
            fields,
            id,
            tick,
            #[cfg(feature = "dev")]
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
/// place of the builtins the C library's would be, and Luau's full debug information, the names of
/// locals and upvalues among it, which the debugger shows, as [`Scripts`] says.
fn compiler() -> Compiler {
    Compiler::new()
        .set_optimization_level(1)
        .set_debug_level(2)
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

/// The doc of the component type the script at `path` declares.
fn doc(path: &str) -> String {
    format!("Declared by `{path}`.")
}

/// Declares `component`, type `id` of `world`, which the script at `path` declared, again, of
/// `fields`, as [`Scripts`] says: logs the fields added and dropped, and warns, `script.field_reset`,
/// of those whose kinds changed.
///
/// Fails as [`World::redeclare`] does, changing nothing.
fn redeclare(
    world: &mut World,
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
    world.redeclare(id, &doc(path), fields)?;
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

/// The handle of field `name` of component type `id`, named `component`, which is its field `index`
/// and holds `ty`: the one `fields` has, or one made.
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
        .position(|field| field.component == component && field.name == *name && field.ty == ty);
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
    let mut refs = shared.fields.borrow_mut();
    for (index, (field, ty)) in fields.enumerate() {
        let Some(ty) = Field::of(&ty) else {
            continue;
        };
        let index = u32::try_from(index).expect("fewer than 2^32 fields");
        let handle = handle_of(&mut refs, name, id, index, field, ty);
        table.raw_set(&**field, handle)?;
    }
    Ok(table)
}

/// `gg.declare(name, fields)`: declares the component type the script loading is, `name`, of
/// `fields`, each a name and a default, as [`Scripts`] says, and returns the script's
/// table, of its fields' handles, which its tick goes in. A script loading again declares the type
/// it declared, its fields as they were or otherwise, which the type takes once it has loaded.
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
        let mut world = shared.world.borrow_mut();
        let (id, changed) = match world.component_named(&name) {
            None => match world.declare(&name, &doc(&loading.path), declared.clone()) {
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
        let own = declared.iter().map(|(field, kind, _)| (field, kind.ty()));
        let table = handles(lua, shared, &name, id, own)?;
        loading.declared = Some(Declaring {
            id,
            component: name,
            table: table.clone(),
            fields: declared,
            changed,
        });
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
    /// look, through an edit of each, which a recording keeps, so that a replay reloads it there,
    /// and which a lockstep host offers for the tick both peers apply it before, as
    /// [`Stack::offer_script`] says: each that compiles, as [`compiles`](super::compiles) checks, a
    /// script that doesn't warning and running as it did. A lockstep joiner doesn't look, as it
    /// runs its host's scripts.
    pub(crate) fn look(cx: &mut Context, stack: &mut Stack) {
        let now = Instant::now();
        let files = &mut cx.scripts;
        if !stack.watches_files()
            || files.files.is_empty()
            || files.looked.is_some_and(|looked| now - looked < LOOK)
        {
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
            saves.push((file.name.clone(), text));
        }
        for (path, text) in saves {
            stack.offer_script(cx, path, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Component;
    use crate::dirs::tests::Scratch;
    use crate::edits::{Done, Edit};

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

    /// Steps a mover by its own field `by`, of `fields`, and by `step`.
    fn stepper(step: u32, fields: &str) -> String {
        format!(
            "local Mover = gg.component(\"Mover\")\n\
             local Step = gg.declare(\"Step\", {{ {fields} }})\n\
             function Step.tick(entity, dt)\n\
                 entity[Mover.steps] = entity[Mover.steps] + entity[Step.by] * {step}\n\
             end\n\
             return Step\n"
        )
    }

    /// Copies a stepper's `by` into its mover's speed, through a handle of another script's field.
    const COPY: &str = "local Mover = gg.component(\"Mover\")\n\
        local Step = gg.component(\"Step\")\n\
        local Copy = gg.declare(\"Copy\", {})\n\
        function Copy.tick(entity, dt)\n\
            entity[Mover.speed] = entity[Step.by]\n\
        end\n\
        return Copy\n";

    /// Reloads the script `scripts/step.luau` as an edit does, its text now `text`.
    fn reload(cx: &mut Context, text: impl Into<String>) -> Result<Done> {
        let edit = Edit::Script {
            path: "scripts/step.luau".to_owned(),
            text: text.into(),
        };
        cx.apply_edit(edit)
    }

    #[test]
    fn a_reload_runs_from_the_next_tick_its_components_keeping_the_fields_it_keeps() {
        let dir = Scratch::new("luau-reload");
        let first = stepper(1, "by = 1, on = true, gone = 3");
        let scripts = [("scripts/step.luau", &*first), ("scripts/copy.luau", COPY)];
        let (mut cx, mut host) = loaded(&dir, &scripts);
        let world = cx.world_mut();
        let mover = world.spawn(Mover::new());
        world.add_default(mover, "Step").expect("added");
        world.add_default(mover, "Copy").expect("added");
        let by = Value::F64(5.0);
        world.set_field(mover, "Step", "by", &by).expect("set");
        host.run(cx.world_mut());
        reload(&mut cx, stepper(10, "by = 1, on = true, gone = 3")).expect("reloaded");
        host.run(cx.world_mut());
        let steps = |cx: &Context| cx.world().get::<Mover>(mover).expect("a mover").steps;
        assert_eq!(steps(&cx), 55);
        // `gone` dropped, `on` of another kind, `added` added, and `by`'s default changed, which
        // now sorts second: the other script's handle of it finds it there.
        let fields = "by = 2, on = 7, added = vector.create(1, 2, 3)";
        reload(&mut cx, stepper(100, fields)).expect("reloaded");
        let world = cx.world();
        let spun = world.get_field(mover, "Step", "").expect("a stepper");
        let expected = Value::Struct(vec![
            ("added".into(), Value::Vec3(Vec3::new(1.0, 2.0, 3.0))),
            ("by".into(), Value::F64(5.0)),
            ("on".into(), Value::F64(7.0)),
        ]);
        assert_eq!(spun, expected);
        assert_eq!(default(world, "Step", "by"), Some(Value::F64(2.0)));
        host.run(cx.world_mut());
        assert_eq!(steps(&cx), 555);
        let speed = cx.world().get::<Mover>(mover).expect("a mover").speed;
        assert_eq!(speed, 5.0);
        // Refused: one that doesn't compile, one that renames its component, and one of a script
        // no layer loaded; the script runs as it did.
        let refused = |result: Result<Done>| result.expect_err("refused").code().to_owned();
        assert_eq!(refused(reload(&mut cx, "local = ")), "script.bad");
        let renamed = stepper(1, "by = 1").replace("\"Step\"", "\"Stride\"");
        assert_eq!(refused(reload(&mut cx, renamed)), "script.renamed");
        let unknown = Edit::Script {
            path: "scripts/none.luau".to_owned(),
            text: COPY.to_owned(),
        };
        assert_eq!(refused(cx.apply_edit(unknown)), "script.unknown");
        host.run(cx.world_mut());
        assert_eq!(steps(&cx), 1055);
    }

    /// A world from before a reload that changed the fields, as `stop` brings back, holds them as
    /// they are now once it's back; and so do the components an undo puts back.
    #[cfg(feature = "dev")]
    #[test]
    fn what_comes_back_from_before_a_reload_holds_its_fields_as_they_are_now() {
        let dir = Scratch::new("luau-stop");
        let first = stepper(1, "by = 1, gone = true");
        let (mut cx, mut host) = loaded(&dir, &[("scripts/step.luau", &first)]);
        let world = cx.world_mut();
        let kept = world.spawn(Mover::new());
        let deleted = world.spawn(Mover::new());
        for entity in [kept, deleted] {
            world.add_default(entity, "Step").expect("added");
            let by = Value::F64(4.0);
            world.set_field(entity, "Step", "by", &by).expect("set");
        }
        let deleted = world.id(deleted).expect("alive");
        let mut log = crate::Log::default();
        let delete = crate::Operation::Delete { entity: deleted };
        log.apply_all(world, vec![delete]).expect("deleted");
        let mut stack = crate::layer::Stack::default();
        stack.play(&mut cx, 0);
        reload(&mut cx, stepper(1, "by = 1, added = true")).expect("reloaded");
        assert_eq!(stack.stop(&mut cx), Some(0));
        let expected = Value::Struct(vec![
            ("added".into(), Value::Bool(true)),
            ("by".into(), Value::F64(4.0)),
        ]);
        let step = |world: &World, entity| world.get_field(entity, "Step", "").expect("a stepper");
        assert_eq!(step(cx.world(), kept), expected);
        log.undo(cx.world_mut()).expect("undone");
        let world = cx.world();
        let back = world.entity(deleted).expect("back");
        assert_eq!(step(world, back), expected);
        host.run(cx.world_mut());
        let steps = cx.world().get::<Mover>(kept).expect("a mover").steps;
        assert_eq!(steps, 4);
    }

    /// Counts a mover's steps, through a local, after a comment, so that a breakpoint asked at the
    /// comment's line takes the function's.
    const COUNTED: &str = "local Mover = gg.component(\"Mover\")\n\
        local Step = gg.declare(\"Step\", { by = 1, forever = false })\n\
        \n\
        -- Counts.\n\
        function Step.tick(entity, dt)\n\
            local steps = entity[Mover.steps]\n\
            steps = steps + entity[Step.by]\n\
            entity[Mover.steps] = steps\n\
            while entity[Step.forever] do end\n\
        end\n\
        \n\
        return Step\n";

    /// A context whose project's root is `dir` with two movers stepped by [`COUNTED`], whose
    /// scripts `debugger` debugs, and its host.
    #[cfg(feature = "dev")]
    fn debugged(dir: &Scratch, debugger: &Arc<Debugger>) -> (Context, Scripts, [Entity; 2]) {
        let (mut cx, host) = loaded(dir, &[("scripts/step.luau", COUNTED)]);
        cx.hosts.debug(debugger.clone());
        let world = cx.world_mut();
        let movers = [(); 2].map(|()| world.spawn(Mover::new()));
        for mover in movers {
            world.add_default(mover, "Step").expect("added");
        }
        (cx, host, movers)
    }

    /// What a stop's frame holds as `name`, a local's.
    #[cfg(feature = "dev")]
    fn local(stop: &gg_api::Stop, name: &str) -> serde_json::Value {
        let locals = &stop.frames[0].locals;
        let found = locals.iter().find(|local| local.name == name);
        found.map_or(serde_json::Value::Null, |local| local.value.clone())
    }

    /// A script stops at a breakpoint inside its tick, which waits there, with its stack and locals,
    /// until it's told to go on: a step over goes to its next line, a step out from the tick to the
    /// next entity's, and a continue to the next stop; the ticks then end as they would have.
    #[cfg(feature = "dev")]
    #[test]
    fn a_script_stops_at_a_breakpoint_and_steps_over_and_out_until_it_goes_on() {
        use gg_api::{DebugAction, StopReason};
        let dir = Scratch::new("luau-breakpoint");
        let debugger = Arc::new(Debugger::default());
        let (mut cx, mut host, movers) = debugged(&dir, &debugger);
        let ids = movers.map(|mover| cx.world().id(mover).expect("alive").to_string());
        debugger.set("scripts/step.luau".to_owned(), vec![7]);
        cx.hosts.place();
        let client = debugger.clone();
        let client = std::thread::spawn(move || {
            let mut stops = Vec::new();
            let wait = std::time::Duration::from_secs(20);
            let actions = [
                DebugAction::StepOver,
                DebugAction::StepOut,
                DebugAction::Continue,
            ];
            for action in actions {
                stops.push(client.answer(wait).stopped.expect("a stop"));
                client.act(action).expect("it goes on");
            }
            stops.push(client.answer(wait).stopped.expect("a stop"));
            client.set("scripts/step.luau".to_owned(), Vec::new());
            client.act(DebugAction::Continue).expect("it goes on");
            stops
        });
        host.run(cx.world_mut());
        let stops = client.join().expect("the client didn't panic");
        let at = |stop: &gg_api::Stop| (stop.number, stop.why, stop.frames[0].line);
        let numbers: Vec<_> = stops.iter().map(at).collect();
        assert_eq!(
            numbers,
            [
                (1, StopReason::Breakpoint, 7),
                (2, StopReason::Step, 8),
                (3, StopReason::Step, 6),
                (4, StopReason::Breakpoint, 7),
            ]
        );
        let frame = &stops[0].frames[0];
        assert_eq!(
            (&*frame.script, &*frame.function),
            ("scripts/step.luau", "tick")
        );
        assert_eq!(local(&stops[0], "entity"), serde_json::json!(ids[0]));
        assert_eq!(
            local(&stops[0], "dt"),
            serde_json::json!(Tick::DT.as_secs_f64())
        );
        assert_eq!(local(&stops[0], "steps"), serde_json::json!(0.0));
        assert_eq!(local(&stops[1], "steps"), serde_json::json!(1.0));
        assert_eq!(local(&stops[2], "entity"), serde_json::json!(ids[1]));
        let upvalues = &stops[0].frames[0].upvalues;
        let names: Vec<&str> = upvalues.iter().map(|upvalue| &*upvalue.name).collect();
        assert_eq!(names, ["Mover", "Step"]);
        assert_eq!(upvalues[0].value, serde_json::json!("(table)"));
        let world = cx.world();
        for mover in movers {
            assert_eq!(world.get::<Mover>(mover).expect("a mover").steps, 1);
        }
        let after = debugger.answer(std::time::Duration::ZERO);
        assert_eq!((after.stopped, after.asked, after.stops), (None, None, 4));
        assert!(after.breakpoints.is_empty());
        assert!(cx.hosts.took_stop(), "the main loop learns of the stop");
        let refused = debugger
            .act(DebugAction::Continue)
            .expect_err("nothing stopped");
        assert_eq!(refused.code(), "script.not_stopped");
    }

    /// A pause asked stops the next line a script runs, its tick's first; and one asked while a call
    /// runs, as an interrupt in it finds.
    #[cfg(feature = "dev")]
    #[test]
    fn a_pause_stops_the_next_line_a_script_runs() {
        use gg_api::{DebugAction, StopReason};
        let dir = Scratch::new("luau-pause");
        let debugger = Arc::new(Debugger::default());
        let (mut cx, mut host, movers) = debugged(&dir, &debugger);
        debugger.act(DebugAction::Pause).expect("asked");
        assert_eq!(
            debugger.answer(std::time::Duration::ZERO).asked,
            Some(DebugAction::Pause)
        );
        let client = debugger.clone();
        let client = std::thread::spawn(move || {
            let stop = client.answer(std::time::Duration::from_secs(20)).stopped;
            client.act(DebugAction::Continue).expect("it goes on");
            stop.expect("a stop")
        });
        host.run(cx.world_mut());
        let stop = client.join().expect("the client didn't panic");
        assert_eq!((stop.why, stop.frames[0].line), (StopReason::Pause, 6));
        let id = cx.world().id(movers[0]).expect("alive").to_string();
        assert_eq!(local(&stop, "entity"), serde_json::json!(id));
        assert_eq!(debugger.answer(std::time::Duration::ZERO).stops, 1);
        // Asked again, an interrupt in a call that runs finds it.
        debugger.act(DebugAction::Pause).expect("asked");
        let shared = &host.host.borrow().shared;
        let interrupt = lent(shared, cx.world_mut(), interrupted);
        assert!(matches!(interrupt, Interrupt::Pause));
    }

    /// A breakpoint takes the next line with code from the line asked, kept for a script that hasn't
    /// loaded until it does, and is placed again in its new functions as it reloads.
    #[cfg(feature = "dev")]
    #[test]
    fn a_breakpoint_takes_the_next_line_with_code_and_follows_its_script_as_it_reloads() {
        let dir = Scratch::new("luau-placed");
        fs::create_dir_all(dir.0.join("scripts")).expect("a directory");
        let mut cx = Context::seeded(1).root(&dir.0);
        cx.world_mut().register::<Mover>().expect("described");
        let debugger = Arc::new(Debugger::default());
        cx.hosts.debug(debugger.clone());
        debugger.set("scripts/step.luau".to_owned(), vec![12, 3, 9, 13]);
        let placed = |debugger: &Debugger| -> Vec<(u32, Option<u32>)> {
            let answer = debugger.answer(std::time::Duration::ZERO);
            let breakpoints = answer.breakpoints.iter();
            breakpoints.map(|at| (at.line, at.placed)).collect()
        };
        assert_eq!(
            placed(&debugger),
            [(3, None), (9, None), (12, None), (13, None)]
        );
        let path = dir.0.join("scripts/step.luau");
        fs::write(&path, COUNTED).expect("written");
        let mut host = Scripts::new().expect("Luau starts");
        host.load(&mut cx, &path).expect("the script loads");
        let scripts = debugger.answer(std::time::Duration::ZERO).scripts;
        assert_eq!(scripts, ["scripts/step.luau"]);
        // The comment's line takes the function's, which the chunk sets.
        assert_eq!(
            placed(&debugger),
            [(3, Some(5)), (9, Some(9)), (12, Some(12)), (13, None)]
        );
        reload(&mut cx, format!("\n{COUNTED}")).expect("reloaded");
        assert_eq!(
            placed(&debugger),
            [(3, Some(3)), (9, Some(9)), (12, Some(13)), (13, Some(13))]
        );
    }

    /// A call into a script that never returns fails once it's past its budget, saying where it
    /// was, and the game goes on: a tick, the others running as they do, and a chunk as it loads.
    #[test]
    fn a_call_into_a_script_that_never_returns_fails_past_its_budget() {
        let dir = Scratch::new("luau-budget");
        let (mut cx, mut host) = loaded(&dir, &[("scripts/step.luau", COUNTED)]);
        let world = cx.world_mut();
        let [stuck, free] = [(); 2].map(|()| world.spawn(Mover::new()));
        for mover in [stuck, free] {
            world.add_default(mover, "Step").expect("added");
        }
        let forever = Value::Bool(true);
        world
            .set_field(stuck, "Step", "forever", &forever)
            .expect("set");
        host.run(cx.world_mut());
        host.run(cx.world_mut());
        let steps = |mover| cx.world().get::<Mover>(mover).expect("a mover").steps;
        assert_eq!((steps(stuck), steps(free)), (2, 2));
        let never = refused(
            "never",
            "while true do end\nreturn gg.declare(\"Never\", {})",
        );
        assert_eq!(never.code(), "script.bad");
        let message = never.to_string();
        assert!(
            message
                .contains("scripts/refused.luau:1: the call into the script ran past its budget"),
            "{message}"
        );
        // A loop that ends, of many turns, runs.
        let looped = "local n = 0\nfor i = 1, 100000 do n += 1 end\n\
            return gg.declare(\"Looped\", { n = n })";
        let (cx, _host) = loaded(
            &Scratch::new("luau-budget-ends"),
            &[("scripts/l.luau", looped)],
        );
        assert_eq!(
            default(cx.world(), "Looped", "n"),
            Some(Value::F64(100_000.0))
        );
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
