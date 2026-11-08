//! Luau scripts: components that scripts declare, which act, each for the entity it's on, at its
//! layer's ticks, as [`Scripts`] says; and mods, scripts a player installs, as [`Mods`] says.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
#[cfg(feature = "dev")]
use std::ffi::c_int;
use std::path::Path;
use std::rc::{Rc, Weak};
#[cfg(feature = "dev")]
use std::sync::Arc;
use std::{io, mem, ptr};

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
use crate::grant::{Capability, Grant};
use crate::reflect::{Ident, Reflect, Type, Value, list};
use crate::{
    Cast, Context, Curve, Ease, Entity, Hit3D, Playback, Shape3D, Tick, Tween, TweenValue, World,
};

mod bytecode;
#[cfg(feature = "dev")]
mod debugger;
// Luau's hooks, its interrupt and its debugger's, which mlua reaches through its raw API alone:
// this module alone may use unsafe code for them.
#[allow(unsafe_code)]
mod hooks;
mod mods;

#[cfg(feature = "dev")]
pub(crate) use debugger::Debugger;
pub use mods::Mods;
pub(crate) use mods::{ModText, RunMods};

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
/// a bool, a vector, a quaternion as a vector, or an entity. A write that [`World::set_field`]
/// would refuse, such as of a number outside the field's bounds, fails as an error does. It keeps
/// its state in its components, as a game keeps its in the world, since no snapshot, reload or peer
/// joining keeps what its chunk's locals hold: once its chunk has run, every table the chunk keeps,
/// its globals among them, is frozen, and a script is refused, `script.state`, whose functions
/// assign a local of its chunk, as its compiled bytecode shows, or whose chunk keeps a coroutine or
/// a buffer.
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
/// `gg.declare`'s third argument, `{ on = "Shard" }`, has the component ride on another: each
/// entity with a `Shard` is given its own, at its defaults, as the scripts run, before its tick.
///
/// `gg.quat` makes and multiplies rotations, and turns a vector by one (`gg.quat.rotate(q, v)`).
/// `gg.cast_ray(from, toward, distance, exclude)` casts through the world's 3D colliders, as
/// [`World::cast_ray`] does, and `gg.cast_sphere`, `gg.cast_box` and `gg.cast_capsule` sweep a
/// shape so, as [`World::cast_shape`] does, their sizes and a box's or a capsule's rotation first,
/// as `gg.cast_sphere(radius, from, toward, distance, exclude)`: each gives the entity hit, where,
/// the surface's normal there and how far, or nil, without end for a distance of nil, through the
/// colliders of `exclude` if it's given. A mod's scripts cast only if it may read the whole world.
///
/// `gg.tween(entity, field, to, seconds, options)` eases the field whose handle is `field` from
/// what it holds to `to` over `seconds`, as [`World::tween`] does, in place of a tween of that
/// field; `options`, if it's given, a table of `curve`, `ease` and `playback`, their variants'
/// names, as `{ curve = "Back", ease = "Out", playback = "PingPong" }`, and `delay`, the seconds
/// before it starts. A mod's scripts tween only fields they may write, and only if they may write
/// `Tweens`.
///
/// `gg.play(entity, clip, fade)` has `entity`, a model's root, play its model's clip named `clip`,
/// cross-fading from what it plays over `fade` seconds, or cutting to it without one, as
/// [`World::play`] does. A mod's scripts play clips only if they may write `Animator`.
///
/// Scripts run alike on every platform, as the simulation must. Luau's C++ fuses no
/// multiplication into an addition, as `.cargo/config.toml` builds it. The functions of `math`
/// whose precision is the platform's are libm's, which the compiler is told aren't builtins: it
/// would call the C library's in their place. A script that uses `^`, which calls the platform's
/// `pow` past any hook, or that sets how it compiles, with `--!optimize` or `--!native`, is
/// refused. The library functions that make an integer of a number, by a C cast that's undefined
/// past the integer's range, where platforms differ, as `string.sub`'s positions, `bit32`'s and
/// `buffer`'s arguments are, fail as an error does at a number past that range or that isn't
/// finite, before they run, as does `string.format` at a number its conversion would cast so, or
/// write as a float that isn't finite, which C libraries write each their own way; but for the
/// iterators that `ipairs` and `utf8.codes` return, which a script that calls one itself gives a
/// number unchecked. `math.random`, whose state the VM holds outside the world and seeds from the
/// clock, isn't there, nor are `os`, `debug`, `collectgarbage`, `gcinfo`, `getfenv`, `setfenv`,
/// `loadstring`, `newproxy`, `require` and `vector.angle`. And an entity is light userdata, its 64
/// bits, which tables hash and compare by them rather than by an address, and whose metatable is
/// frozen. The VM collects its garbage at set points, after a chunk runs and after the ticks once
/// the scripts have allocated as much as the VM holds alive, rather than when Luau's clock paces
/// it, so that what a weak table holds is alike everywhere.
///
/// Each call into a script, a tick or its chunk as it loads, takes at most a million of Luau's
/// interrupts, which come at each call and each turn of a loop, past which it fails, alike on every
/// platform, so that a loop that never ends fails rather than hanging the game. No script code runs
/// outside such a call: the engine checks what a chunk returns and keeps without calling its
/// metamethods.
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

/// What runs a layer's scripts, or a mod's.
struct Host {
    lua: Lua,
    shared: Shared,
    /// In the order of their components' names.
    scripts: Vec<Loaded>,
    /// The world whose component types the handles name, by its serial, and those types, by their
    /// revision, which a script's reload in another VM moves on, as it declares its type again.
    resolved: Option<(u64, u64)>,
    /// The memory the VM may hold, if it's held to a limit, as a mod's is.
    limit: Option<usize>,
    /// What the VM may hold before the next set point collects its garbage.
    goal: usize,
}

/// What a mod's scripts are held to, as [`Mods`] says: the mod's name; its grant; the component
/// types its scripts declared, which are its own; and its prints, those left to it this second of
/// ticks and those it went without, with the ticks of the second.
struct Held {
    name: String,
    grant: Grant,
    own: RefCell<Vec<String>>,
    prints: Cell<u32>,
    unprinted: Cell<u32>,
    ticks: Cell<u32>,
}

/// The most lines a mod prints a second of ticks, past which its prints go unprinted.
const PRINTS: u32 = 32;

/// The ticks of a second.
const TICKS_A_SECOND: u32 = (1_000_000_000 / Tick::DT.as_nanos()) as u32;

impl Held {
    fn new(name: String, grant: Grant) -> Self {
        Self {
            name,
            grant,
            own: RefCell::new(Vec::new()),
            prints: Cell::new(PRINTS),
            unprinted: Cell::new(0),
            ticks: Cell::new(0),
        }
    }

    /// Whether a print prints, as it's the mod's this second of ticks, counting it.
    fn prints(&self) -> bool {
        let left = self.prints.get();
        if left == 0 {
            self.unprinted.set(self.unprinted.get() + 1);
        }
        self.prints.set(left.saturating_sub(1));
        left > 0
    }

    /// Counts a tick of the mod's, after which, as a second of them ends, its prints start again,
    /// warning of those that went unprinted, `mod.unprinted`.
    fn ticked(&self) {
        let ticks = self.ticks.get() + 1;
        self.ticks.set(ticks % TICKS_A_SECOND);
        if ticks < TICKS_A_SECOND {
            return;
        }
        self.prints.set(PRINTS);
        let unprinted = self.unprinted.replace(0);
        if unprinted > 0 {
            warn!(
                code = "mod.unprinted",
                "mod" = self.name,
                unprinted,
                "The mod printed more than {PRINTS} lines in a second of ticks: the {unprinted} \
                 past them went unprinted"
            );
        }
    }
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
/// running, what a mod's scripts are held to, and what the VM keeps for the debugger.
struct Shared {
    world: RefCell<World>,
    /// By handle.
    fields: RefCell<Vec<FieldRef>>,
    /// The script loading, while one is.
    loading: RefCell<Option<Loading>>,
    /// The interrupts left to the call into a script that runs, of its [`BUDGET`], or, a mod's, to
    /// its calls this tick.
    budget: Cell<u32>,
    /// `None` for a game's own scripts, which reach everything.
    held: Option<Held>,
    #[cfg(feature = "dev")]
    hooked: debugger::Hooked,
}

impl Shared {
    /// Whether the scripts may read, and may write, the components of type `component`: a mod's,
    /// those its grant names and its own.
    fn may(&self, component: &str) -> (bool, bool) {
        match &self.held {
            None => (true, true),
            Some(held) if held.own.borrow().iter().any(|own| own == component) => (true, true),
            Some(held) => (held.grant.reads(component), held.grant.writes(component)),
        }
    }
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
/// and, in a development build, stops there if a pause is asked, as [`Scripts`] says. Script code
/// that runs outside any call into a script, where it would have no budget, fails at once.
fn interrupted() -> Interrupt {
    if !SHARED.is_set() {
        return Interrupt::Over;
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

/// The error of a call into a script that ran past its budget at `line` of `script`, or, a mod's,
/// past its mod's for the tick; or of script code that ran there outside any call into a script.
fn over_budget(script: &str, line: u32) -> String {
    if !SHARED.is_set() {
        return format!(
            "{script}:{line}: script code ran outside a call into a script, where alone it has a \
             budget of Luau's interrupts"
        );
    }
    let held = SHARED.with(|shared| shared.held.is_some());
    let what = if held {
        "the mod's calls this tick ran past their budget"
    } else {
        "the call into the script ran past its budget"
    };
    format!(
        "{script}:{line}: {what} of {BUDGET} of Luau's interrupts, which come at each call and \
         each turn of a loop, as a loop that never ends would"
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

/// A field a handle names: field `name` of component type `component`, which holds `ty`; and
/// whether the scripts may read it and write it, as [`Shared::may`] says.
struct FieldRef {
    component: Ident,
    /// The type's id in the world scripts last ran in, and the field's index there; `None` if it
    /// has no such type, or no such field, there.
    id: Option<ComponentId>,
    index: u32,
    name: Ident,
    ty: Field,
    reads: bool,
    writes: bool,
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
    /// The component type it rides on, as `gg.declare`'s `on` names it, and its id there.
    on: Option<String>,
    on_id: Option<ComponentId>,
    tick: Option<Function>,
    /// Its chunk, as [`Chunk`] says, which a development build's debugger keeps in its place.
    #[cfg(not(feature = "dev"))]
    chunk: Function,
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
/// fields, whether they're other than the world's type of its name has, which the world's type
/// takes once the script has loaded, and the type it rides on.
struct Declaring {
    id: ComponentId,
    component: String,
    table: Table,
    fields: Vec<(Ident, Kind, Value)>,
    changed: bool,
    on: Option<String>,
}

/// A script's chunk, run: the component it declared, its fields, the type it rides on, its tick,
/// and the chunk, which lives as long as the script, so that its VM holds as much in every build: a
/// development build's debugger keeps it, placing breakpoints in it, and the script any other's.
struct Chunk {
    component: String,
    fields: Vec<(Ident, Kind, Value)>,
    id: ComponentId,
    on: Option<String>,
    tick: Option<Function>,
    function: Function,
}

impl Scripts {
    /// A VM for scripts, with the globals [`Scripts`] says.
    ///
    /// Fails with `script.vm` if Luau can't start, as for want of memory.
    pub fn new() -> Result<Self> {
        Ok(Self {
            host: Rc::new(RefCell::new(Host::new(None)?)),
        })
    }

    /// Loads the script at `path`, declaring the component type it declares in `cx`'s world, and
    /// returns that type's name, which an entity is given to run the script, as
    /// [`World::add_default`] gives it. A development build watches its file, reloading it as it's
    /// saved, as [`Scripts`] says, but in a run that reads its game's package, as
    /// [`App::project`](crate::App::project) says, which reads it there. A script loaded again, at
    /// the same path, takes its own place.
    ///
    /// Fails with `script.unreadable` if the file can't be read, isn't a regular file or isn't
    /// UTF-8; `script.pow` and
    /// `script.header` for a script refused as [`Scripts`] says; `script.bad` for one that
    /// doesn't compile, that fails as its chunk runs, or that returns other than the component it
    /// declares, as `gg.declare` returns it; `reflect.name_taken` for a component type another has;
    /// and as declaring it does.
    pub fn load(&mut self, cx: &mut Context, path: impl AsRef<Path>) -> Result<String> {
        let path = path.as_ref();
        let name = crate::assets::name_of(cx.root.as_deref(), path).to_string();
        let read = cx.files.read(path, u64::MAX).and_then(|bytes| {
            String::from_utf8(bytes).map_err(|_| io::Error::other("it isn't UTF-8"))
        });
        let text = read.map_err(|error| {
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
        if !cx.reads_package() {
            cx.scripts.watch(name, path, text);
        }
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
    /// A VM for scripts, with the globals [`Scripts`] says: a game's, or, held to what `held` and
    /// `limit` say, a mod's.
    ///
    /// Fails with `script.vm` if Luau can't start, as for want of memory.
    fn new(held: Option<(Held, usize)>) -> Result<Self> {
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
            let axis = Vec3::new(axis.x(), axis.y(), axis.z());
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
        gg.raw_set("tween", lua.create_function(tween)?)?;
        gg.raw_set("play", lua.create_function(play)?)?;
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
    fn load(&mut self, world: &mut World, name: &str, text: String) -> Result<String> {
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
    fn loaded(&self, path: &str) -> Option<usize> {
        self.scripts.iter().position(|loaded| loaded.path == path)
    }

    fn run(&mut self, world: &mut World) {
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
                        }
                    }
                }
                let Some(tick) = &script.tick else {
                    continue;
                };
                shared.world.borrow().list_with(id, &mut list);
                for &entity in &list {
                    // Gone, or without the component, since the list was made.
                    if !shared.world.borrow().has(entity, id) {
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
        if let Some(held) = &self.shared.held {
            held.ticked();
        }
        self.collect(false);
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
/// place of the builtins the C library's would be, the library functions whose integer arguments
/// are checked called as they're checked, and Luau's full debug information, the names of locals
/// and upvalues among it, which the debugger shows, as [`Scripts`] says.
fn compiler() -> Compiler {
    let libm = LIBM.map(|name| format!("math.{name}"));
    Compiler::new()
        .set_optimization_level(1)
        .set_debug_level(2)
        .set_disabled_builtins(libm.into_iter().chain(hooks::not_builtins()))
}

/// Checks that `text` compiles, as a script loads it, as [`compiled`] says.
#[cfg(feature = "dev")]
pub(crate) fn compiles(path: &str, text: &str) -> Result<()> {
    compiled(path, text).map(drop)
}

/// `text`, the script at `path`'s, compiled: refused as [`check`] refuses it; as it doesn't
/// compile, `script.bad`, where the message is the compiler's; or, `script.state`, if one of its
/// functions assigns a local of its chunk, as [`Scripts`] says.
fn compiled(path: &str, text: &str) -> Result<Vec<u8>> {
    check(text).map_err(|error| {
        let message = format!("{path}:{error}");
        error.with_message(message)
    })?;
    let bytecode = compiler()
        .compile(text)
        .map_err(|error| bad(path, &error))?;
    let assigned = bytecode::assigned(&bytecode).map_err(|why| {
        let what = format!("{path}: the engine can't read Luau's bytecode: {why}");
        Error::new("script.vm", what)
    })?;
    if let Some(bytecode::Assigned { line, local }) = assigned {
        return Err(Error::new(
            "script.state",
            format!(
                "{path}:{line}: a function assigns `{local}`, a local of the script's chunk, whose \
                 value no snapshot, reload or peer joining keeps: keep it in a component's fields"
            ),
        ));
    }
    Ok(bytecode)
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

/// The doc of the component type the script at `path` declares, a game's or, as `held` says, a
/// mod's, which names its mod, as no game's script's doc does.
fn doc(held: Option<&Held>, path: &str) -> String {
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
/// and holds `ty`, and which the scripts may read and write as `may` says: the one `fields` has, or
/// one made.
fn handle_of(
    fields: &mut Vec<FieldRef>,
    component: &str,
    id: ComponentId,
    (index, name, ty): (u32, &Ident, Field),
    (reads, writes): (bool, bool),
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
            reads,
            writes,
        });
        fields.len() - 1
    });
    handle(at as u64 + 1)
}

/// The error that a mod's scripts aren't granted what `capability` names, which `what` needs.
fn ungranted(capability: &str, what: &str) -> String {
    format!("the mod isn't granted `{capability}`, which {what} needs")
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
/// of the kinds scripts reach; for a mod's scripts, of a type they may read or write, as
/// [`Shared::may`] says, or a refusal, `mod.ungranted`, of the script loading.
fn component(lua: &Lua, name: LuaString) -> mlua::Result<Table> {
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
            return Err(mlua::Error::runtime(message));
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
fn declare(
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
fn read(_: &Lua, (entity, key): (LightUserData, Lua_)) -> mlua::Result<Lua_> {
    shared(|shared| {
        let entity = entity_of(entity)?;
        let fields = shared.fields.borrow();
        let field = &fields[field_of(&key)?];
        if !field.reads {
            let (component, name) = (&field.component, &field.name);
            let what = format!("reading `{component}.{name}`");
            return Err(mlua::Error::runtime(ungranted(
                &format!("read:{component}"),
                &what,
            )));
        }
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
        if !field.writes {
            let (component, name) = (&field.component, &field.name);
            let what = format!("writing `{component}.{name}`");
            return Err(mlua::Error::runtime(ungranted(
                &format!("write:{component}"),
                &what,
            )));
        }
        let mut world = shared.world.borrow_mut();
        let at = (entity, missing(field)?, field.index);
        let set = set(&mut world, at, field.ty, &value);
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

/// `print(...)`: logs what it's given, as `tostring` writes each, between tabs; a mod's, naming
/// it, unless it has printed as much as it may this second of ticks, as [`Mods`] says.
fn print(_: &Lua, values: Variadic<Lua_>) -> mlua::Result<()> {
    shared(|shared| {
        if shared.held.as_ref().is_some_and(|held| !held.prints()) {
            return Ok(());
        }
        let texts: Vec<String> = values
            .iter()
            .map(|value| value.to_string())
            .collect::<mlua::Result<_>>()?;
        match &shared.held {
            Some(held) => info!("mod" = held.name, "{}", texts.join("\t")),
            None => info!("{}", texts.join("\t")),
        }
        Ok(())
    })
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

/// Sets field `index` of `entity`'s component `id`, which holds `ty`, to `value`, a script's,
/// refused as [`World::field_to_set`] says: without allocating, but to fail.
fn set(
    world: &mut World,
    (entity, id, index): (Entity, ComponentId, u32),
    ty: Field,
    value: &Lua_,
) -> Result<()> {
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
    let to_entity = || match *value {
        Lua_::LightUserData(data) => {
            Entity::from_bits(bits(data)).ok_or_else(|| wrong("an entity"))
        }
        _ => Err(wrong("an entity")),
    };
    let value = match ty {
        Field::MaybeEntity => {
            let held = match value {
                Lua_::Nil => None,
                _ => Some(to_entity()?),
            };
            let any: &mut dyn Any = world.field_to_set(entity, id, index, None)?;
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
        Field::Entity => Value::Entity(to_entity()?),
    };
    let field = world.field_to_set(entity, id, index, Some(&value))?;
    field.set_value(&value)
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
fn cast_ray(_: &Lua, args: CastArgs) -> mlua::Result<CastHit> {
    cast(args, |world, cast| world.cast_ray(cast))
}

/// `gg.cast_sphere(radius, from, toward, distance, exclude)`: what a sphere swept along a line hits
/// first, as [`World::cast_shape`] says.
fn cast_sphere(_: &Lua, (radius, args): (f64, CastArgs)) -> mlua::Result<CastHit> {
    let shape = Shape3D::Sphere {
        radius: radius as f32,
    };
    cast(args, |world, cast| {
        world.cast_shape(shape, Quat::IDENTITY, cast)
    })
}

/// `gg.cast_box(size, rotation, from, toward, distance, exclude)`: what a box, turned, swept along
/// a line hits first, as [`World::cast_shape`] says.
fn cast_box(_: &Lua, (size, rotation, args): (Vector, Vector, CastArgs)) -> mlua::Result<CastHit> {
    let shape = Shape3D::Box {
        size: vec3_of(size),
    };
    cast(args, |world, cast| {
        world.cast_shape(shape, quat_of(rotation), cast)
    })
}

/// `gg.cast_capsule(height, radius, rotation, from, toward, distance, exclude)`: what a capsule,
/// upright along its Y and turned, swept along a line hits first, as [`World::cast_shape`] says.
fn cast_capsule(
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
            return Err(mlua::Error::runtime(ungranted("read", what)));
        }
        let mut cast = Cast::new(vec3_of(from), vec3_of(toward));
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
                Lua_::Vector(vector3_of(hit.point)),
                Lua_::Vector(vector3_of(hit.normal)),
                Lua_::Number(f64::from(hit.distance)),
            ),
            None => (Lua_::Nil, Lua_::Nil, Lua_::Nil, Lua_::Nil),
        })
    })
}

/// What a script gives `gg.tween`: the entity, the handle of its field, where the field goes, in
/// how many seconds, and the table of the rest, if any.
type TweenArgs = (LightUserData, Lua_, Lua_, f64, Option<Table>);

/// `gg.tween(entity, field, to, seconds, options)`: starts a tween of `entity`'s field whose handle
/// is `field` from what it holds to `to`, as the scripts' doc says.
fn tween(_: &Lua, (entity, key, to, seconds, options): TweenArgs) -> mlua::Result<()> {
    shared(|shared| {
        let entity = entity_of(entity)?;
        let fields = shared.fields.borrow();
        let field = &fields[field_of(&key)?];
        let (component, name) = (&field.component, &field.name);
        if !field.writes {
            let what = format!("tweening `{component}.{name}`");
            let capability = format!("write:{component}");
            return Err(mlua::Error::runtime(ungranted(&capability, &what)));
        }
        if let Some(held) = &shared.held
            && !held.grant.writes("Tweens")
        {
            let what = format!("tweening `{component}.{name}`, which its entity's `Tweens` holds,");
            return Err(mlua::Error::runtime(ungranted("write:Tweens", &what)));
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
            (TweenValue::Vec3(_), Lua_::Vector(v)) => TweenValue::Vec3(vec3_of(*v)),
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
fn play(_: &Lua, (entity, clip, fade): (LightUserData, String, Option<f64>)) -> mlua::Result<()> {
    shared(|shared| {
        let entity = entity_of(entity)?;
        if let Some(held) = &shared.held
            && !held.grant.writes("Animator")
        {
            let what = format!("playing clip {clip:?}, which its root's `Animator` holds,");
            return Err(mlua::Error::runtime(ungranted("write:Animator", &what)));
        }
        let mut world = shared.world.borrow_mut();
        let played = world.play(entity, &clip, fade.unwrap_or(0.0) as f32);
        played.map_err(|error| mlua::Error::runtime(format!("{error:#}")))
    })
}

/// `v`'s X, Y and Z, as a script's vector holds a point or a direction.
fn vec3_of(v: Vector) -> Vec3 {
    Vec3::new(v.x(), v.y(), v.z())
}

/// `v` as a script holds a point or a direction, its W 0.
fn vector3_of(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z, 0.0)
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
    use std::fs;

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

    /// The library functions that make an integer of a number fail at one past the integer's range,
    /// or that isn't finite, which platforms cast otherwise, called as functions or as methods, and
    /// `string.format` at one its conversion would cast so or write as a float that isn't finite;
    /// within the range they work as Luau's own, `table.insert`'s value however large.
    #[test]
    fn library_functions_refuse_numbers_that_platforms_make_other_integers_of() {
        let dir = Scratch::new("luau-integers");
        let script = "return gg.declare(\"Checked\", {\n\
            sub = string.sub(\"abcd\", 2, -2) == \"bc\" and (\"abcd\"):sub(3) == \"cd\",\n\
            band = bit32.band(1099511627781, 7),\n\
            format = string.format(\"%d %x %5.1f %s\", -3, 255, 2.3, 1e300)\n\
                == \"-3 ff   2.3 \" .. tostring(1e300),\n\
            insert = (function() local t = {} table.insert(t, 1e300) return t[1] end)(),\n\
            unpack = select(2, unpack({ 7, 8, 9 }, 1, 3)),\n\
            buffer = buffer.readu8(buffer.fromstring(\"\\5\"), 0),\n\
        })\n";
        let (cx, _host) = loaded(&dir, &[("scripts/checked.luau", script)]);
        let world = cx.world();
        let checked = |name| default(world, "Checked", name);
        assert_eq!(checked("sub"), Some(Value::Bool(true)));
        assert_eq!(checked("band"), Some(Value::F64(5.0)));
        assert_eq!(checked("format"), Some(Value::Bool(true)));
        assert_eq!(checked("insert"), Some(Value::F64(1e300)));
        assert_eq!(checked("unpack"), Some(Value::F64(8.0)));
        assert_eq!(checked("buffer"), Some(Value::F64(5.0)));
        for (name, call) in [
            ("sub", "string.sub(\"abc\", 1e10)"),
            ("method", "(\"abc\"):sub(0/0)"),
            ("char", "string.char(65, 1/0)"),
            ("band", "bit32.band(1, 1e19)"),
            ("insert", "table.insert({}, 1e10, 1)"),
            ("select", "select(-1e10, 1, 2)"),
            ("buffer", "buffer.readu8(buffer.create(1), 0/0)"),
            ("write", "buffer.writeu32(buffer.create(4), 0, 1e19)"),
            ("ldexp", "math.ldexp(1, 1e10)"),
            ("format-d", "string.format(\"%d\", 1e19)"),
            ("format-c", "string.format(\"%%%c\", -1e10)"),
            ("format-f", "string.format(\"%5.1f\", 0/0)"),
        ] {
            let text = format!("local _ = {call}\nreturn gg.declare(\"Refused\", {{}})");
            let message = refused(&format!("integer-{name}"), &text).to_string();
            let why = ["every platform casts it to", "C libraries write"];
            assert!(
                why.iter().any(|why| message.contains(why)),
                "{name}: {message}"
            );
        }
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

    /// A reload that moves its type's fields reaches the handles of them that every other VM holds,
    /// another layer's or a mod's, as it reaches its own: each finds its field where it is now, and
    /// one of a field gone fails as an error does.
    #[test]
    fn a_reload_reaches_the_handles_other_vms_hold_of_its_fields() {
        let dir = Scratch::new("luau-reload-elsewhere");
        let first = stepper(1, "by = 1");
        let (mut cx, mut steps) = loaded(&dir, &[("scripts/step.luau", &first)]);
        let path = dir.0.join("scripts/copy.luau");
        fs::write(&path, COPY).expect("written");
        let mut copies = Scripts::new().expect("Luau starts");
        copies.load(&mut cx, &path).expect("the script loads");
        let world = cx.world_mut();
        let mover = world.spawn(Mover::new());
        world.add_default(mover, "Step").expect("added");
        world.add_default(mover, "Copy").expect("added");
        let by = Value::F64(5.0);
        world.set_field(mover, "Step", "by", &by).expect("set");
        let speed = |cx: &Context| cx.world().get::<Mover>(mover).expect("a mover").speed;
        // `after` sorts first, where `by` was, which holds an entity now.
        reload(&mut cx, stepper(1, "after = gg.entity, by = 2")).expect("reloaded");
        steps.run(cx.world_mut());
        copies.run(cx.world_mut());
        assert_eq!(speed(&cx), 5.0);
        reload(&mut cx, stepper(1, "after = gg.entity")).expect("reloaded");
        cx.world_mut()
            .get_mut::<Mover>(mover)
            .expect("a mover")
            .speed = 1.0;
        copies.run(cx.world_mut());
        assert_eq!(speed(&cx), 1.0, "its field gone");
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

    /// No script code runs outside a call into a script, where alone its budget holds: a chunk is
    /// told from what it returns by identity, its tables' `__eq`, which never returns, uncalled, and
    /// a script's function called outside any call into a script fails at its first interrupt.
    #[test]
    fn no_script_code_runs_outside_a_call_into_a_script() {
        let eq = "local mt = { __eq = function() while true do end end }\n\
            local X = setmetatable(gg.declare(\"X\", {}), mt)\n\
            return setmetatable({}, mt)\n";
        let eq = refused("eq", eq);
        assert_eq!(eq.code(), "script.bad");
        assert!(eq.to_string().contains("returns other than `X`"), "{eq}");
        let turns = "local Turns = gg.declare(\"Turns\", {})\n\
            function Turns.tick(entity, dt)\n\
                for i = 1, 2 do end\n\
            end\n\
            return Turns\n";
        let dir = Scratch::new("luau-outside");
        let (_cx, host) = loaded(&dir, &[("scripts/turns.luau", turns)]);
        let tick = host.host.borrow().scripts[0].tick.clone().expect("a tick");
        let outside = tick.call::<()>(()).expect_err("it has no budget");
        let message = outside.to_string();
        assert!(
            message.contains("scripts/turns.luau:3: script code ran outside a call into a script"),
            "{message}"
        );
    }

    /// A script's write is refused as `World::set_field` refuses one, with its code, the field left
    /// as it was: of a number outside its field's bounds, a collider's density below 0, or that
    /// isn't finite there; and of `Parent`, which the world keeps, and of whose field, a tuple
    /// struct's, no script finds a handle anyway.
    #[test]
    fn a_script_s_write_is_refused_as_a_field_set_by_name_is() {
        let dir = Scratch::new("luau-checked");
        let script = "local Box = gg.component(\"BoxCollider2D\")\n\
            local Dense = gg.declare(\"Dense\", {\n\
                to = 1,\n\
                parentless = next(gg.component(\"Parent\")) == nil,\n\
            })\n\
            function Dense.tick(entity, dt)\n\
                entity[Box.density] = entity[Dense.to]\n\
            end\n\
            return Dense\n";
        let (mut cx, mut host) = loaded(&dir, &[("scripts/dense.luau", script)]);
        let world = cx.world_mut();
        let parentless = default(world, "Dense", "parentless");
        assert_eq!(parentless, Some(Value::Bool(true)));
        let dense = world.spawn(crate::BoxCollider2D::default());
        world.add_default(dense, "Dense").expect("added");
        let collider = world.component_named("BoxCollider2D").expect("registered");
        let mut fields = world.fields_of(collider).iter();
        let density = fields.position(|field| field.name == "density");
        let density = u32::try_from(density.expect("a density")).expect("few fields");
        for to in [-1.0, f64::NAN] {
            let world = cx.world_mut();
            world
                .set_field(dense, "Dense", "to", &Value::F64(to))
                .expect("set");
            host.run(world);
            let kept = world
                .get::<crate::BoxCollider2D>(dense)
                .expect("a collider");
            assert_eq!(kept.density, 1.0);
            let by_name =
                world.set_field(dense, "BoxCollider2D", "density", &Value::F32(to as f32));
            let scripted = set(
                world,
                (dense, collider, density),
                Field::F32,
                &Lua_::Number(to),
            );
            let codes = [scripted, by_name.map(drop)].map(|refused| refused.expect_err("refused"));
            assert_eq!(codes[0].code(), "reflect.out_of_range", "{:?}", codes[0]);
            assert_eq!(codes[0].code(), codes[1].code());
        }
        let world = cx.world_mut();
        let [parent, child] = [(); 2].map(|()| world.spawn(()));
        world.set_parent(child, Some(parent)).expect("parented");
        let id = world.component_named("Parent").expect("registered");
        let by_name = world.set_field(child, "Parent", "0", &Value::Entity(dense));
        let other = Lua_::LightUserData(handle(dense.to_bits()));
        let scripted = set(world, (child, id, 0), Field::Entity, &other);
        let codes = [scripted, by_name.map(drop)].map(|refused| refused.expect_err("refused"));
        assert_eq!(codes[0].code(), "reflect.read_only", "{:?}", codes[0]);
        assert_eq!(codes[0].code(), codes[1].code());
        assert_eq!(world.parent(child).expect("alive"), Some(parent));
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

    /// Declares `name`, after `chunk`, whose tick runs `tick` then sets its mover's steps to 7.
    fn keeper(name: &str, chunk: &str, tick: &str) -> String {
        format!(
            "local Mover = gg.component(\"Mover\")\n{chunk}\n\
             local Kept = gg.declare(\"{name}\", {{}})\n\
             function Kept.tick(entity, dt)\n{tick}\nentity[Mover.steps] = 7\nend\n\
             return Kept\n"
        )
    }

    /// A script's chunk keeps nothing that changes: a script whose functions assign a local of its
    /// chunk, directly or through a function between, or whose chunk keeps a coroutine or a
    /// buffer, is refused, `script.state`, saying where; once its chunk has run, its tables and its
    /// globals are frozen, which a tick that writes them fails at; and the metatable entities share
    /// is frozen as the chunk runs. A function may assign its own locals, and its enclosing
    /// function's.
    #[test]
    fn a_script_keeps_its_state_in_its_components() {
        let between = "local count = 0\n\
            local function add() return function() count += 1 end end";
        let refusals = [
            (
                "assigned",
                keeper("K", "local count = 0", "count = count + 1"),
            ),
            ("between", keeper("K", between, "add()()")),
            (
                "coroutine",
                keeper(
                    "K",
                    "local co = coroutine.create(function() end)",
                    "local _ = co",
                ),
            ),
            (
                "wrapped",
                keeper(
                    "K",
                    "local step = coroutine.wrap(function() end)",
                    "local _ = step",
                ),
            ),
            ("buffer", keeper("K", "Bytes = buffer.create(4)", "")),
        ];
        for (name, text) in refusals {
            let refused = refused(name, &text);
            assert_eq!(refused.code(), "script.state", "{name}: {refused}");
        }
        let assigned = refused("line", &keeper("K", "local count = 0", "count = count + 1"));
        let message = assigned.to_string();
        assert!(
            message.starts_with("scripts/refused.luau:5: a function assigns `count`"),
            "{message}"
        );
        let hijack = "getmetatable(gg.entity).__tostring = nil\nreturn gg.declare(\"H\", {})";
        let hijack = refused("hijack", hijack);
        assert!(hijack.to_string().contains("readonly"), "{hijack}");
        let dir = Scratch::new("luau-state");
        let own = keeper(
            "Own",
            "local cache = { n = 0 }",
            "local n = 0\nlocal function more() n += 1 end\nmore()\nentity[Mover.angle] = n",
        );
        let table = keeper("Table", "local cache = { n = 0 }", "cache.n = 1");
        let global = keeper("Global", "Total = 0", "Total = 1");
        let scripts = [
            ("scripts/own.luau", &*own),
            ("scripts/table.luau", &*table),
            ("scripts/global.luau", &*global),
        ];
        let (mut cx, mut host) = loaded(&dir, &scripts);
        let world = cx.world_mut();
        let movers = ["Own", "Table", "Global"].map(|component| {
            let mover = world.spawn(Mover::new());
            world.add_default(mover, component).expect("added");
            mover
        });
        host.run(cx.world_mut());
        let steps = movers.map(|mover| cx.world().get::<Mover>(mover).expect("a mover").steps);
        assert_eq!(steps, [7, 0, 0]);
        let angle = cx.world().get::<Mover>(movers[0]).expect("a mover").angle;
        assert_eq!(angle, 1.0);
    }

    /// A component that rides on another is given to each entity with that one, at its defaults,
    /// as the scripts run, before its tick.
    #[test]
    fn a_component_riding_on_another_is_given_to_each_entity_with_it() {
        let dir = Scratch::new("luau-on");
        let rider = "local Mover = gg.component(\"Mover\")\n\
            local Rider = gg.declare(\"Rider\", { by = 3 }, { on = \"Mover\" })\n\
            function Rider.tick(entity, dt)\n\
                entity[Mover.steps] = entity[Mover.steps] + entity[Rider.by]\n\
            end\n\
            return Rider\n";
        let (mut cx, mut host) = loaded(&dir, &[("scripts/rider.luau", rider)]);
        let world = cx.world_mut();
        let [first, bare] = [world.spawn(Mover::new()), world.spawn(())];
        host.run(cx.world_mut());
        let later = cx.world_mut().spawn(Mover::new());
        host.run(cx.world_mut());
        let world = cx.world();
        let steps = |mover| world.get::<Mover>(mover).expect("a mover").steps;
        assert_eq!((steps(first), steps(later)), (6, 3));
        let by = world.get_field(later, "Rider", "by").expect("its own");
        assert_eq!(by, Value::F64(3.0));
        assert!(world.get_field(bare, "Rider", "by").is_err());
        for (name, options) in [
            ("on-unknown", "{ on = \"Nope\" }"),
            ("on-itself", "{ on = \"Riding\" }"),
            ("on-other", "{ under = \"Mover\" }"),
        ] {
            let text = format!("return gg.declare(\"Riding\", {{}}, {options})");
            assert_eq!(refused(name, &text).code(), "script.bad", "{name}");
        }
    }

    /// A string pattern that backtracks without end fails past its call's budget, as Luau's matcher
    /// comes to the interrupt.
    #[test]
    fn a_pattern_that_backtracks_fails_past_its_budget() {
        let text = "local a = string.rep(\"a\", 40)\n\
            local found = string.find(a, \"a*a*a*a*a*a*a*a*b\")\n\
            return gg.declare(\"Matched\", {})";
        let error = refused("pattern", text);
        assert!(error.to_string().contains("ran past its budget"), "{error}");
    }

    /// Casts a ray, a sphere and a box down, and a ray past what the first hit, keeping what each
    /// found in its own fields.
    const LOOK: &str = r#"
local Look = gg.declare("Look", {
    hit = gg.entity,
    point = vector.create(0, 0, 0),
    normal = vector.create(0, 0, 0),
    distance = -1,
    through = gg.entity,
    ball = -1,
    boxed = -1,
    missed = true,
})

local down = vector.create(0, -1, 0)

function Look.tick(entity, dt)
    local from = vector.create(0.25, 5, -0.25)
    local hit, point, normal, distance = gg.cast_ray(from, down)
    entity[Look.hit] = hit
    entity[Look.point] = point
    entity[Look.normal] = normal
    entity[Look.distance] = distance
    entity[Look.through] = gg.cast_ray(from, down, 10, hit)
    local _, _, _, ball = gg.cast_sphere(0.5, from, down)
    entity[Look.ball] = ball
    local _, _, _, boxed = gg.cast_box(vector.create(1, 1, 1), gg.quat.identity, from, down, nil, gg.entity)
    entity[Look.boxed] = boxed
    entity[Look.missed] = gg.cast_capsule(2, 0.5, gg.quat.identity, from, down, 2) == nil
end

return Look
"#;

    /// A script casts through the world's 3D colliders, as code does: what a ray hits first, its
    /// entity, where, the surface's normal and how far; past an entity it names; and a sphere, a
    /// box and a capsule swept down, nil where it hits nothing within its distance.
    #[test]
    fn a_script_casts_through_the_world_s_colliders() {
        use crate::{BodyType, BoxCollider3D, RigidBody3D, Transform, step_physics};

        let dir = Scratch::new("luau-casts");
        let (mut cx, mut host) = loaded(&dir, &[("scripts/look.luau", LOOK)]);
        let world = cx.world_mut();
        let fixed = RigidBody3D {
            body_type: BodyType::Static,
            ..RigidBody3D::default()
        };
        let at = |translation, scale| Transform {
            translation,
            scale,
            ..Transform::IDENTITY
        };
        let floor_at = at(Vec3::ZERO, Vec3::new(10.0, 1.0, 10.0));
        let floor = world.spawn((floor_at, fixed, BoxCollider3D::default()));
        let boxed_at = at(Vec3::new(0.0, 1.0, 0.0), Vec3::ONE);
        let boxed = world.spawn((boxed_at, fixed, BoxCollider3D::default()));
        step_physics(world);
        let looker = world.spawn(());
        world.add_default(looker, "Look").expect("added");
        host.run(cx.world_mut());
        let world = cx.world();
        let field = |name| world.get_field(looker, "Look", name).expect("a field");
        let some = |entity| Value::Option(Some(Box::new(Value::Entity(entity))));
        assert_eq!(field("hit"), some(boxed));
        assert_eq!(field("point"), Value::Vec3(Vec3::new(0.25, 1.5, -0.25)));
        assert_eq!(field("normal"), Value::Vec3(Vec3::Y));
        assert_eq!(field("distance"), Value::F64(3.5));
        assert_eq!(field("through"), some(floor));
        let near = |name, want: f64| match field(name) {
            Value::F64(got) => assert!((got - want).abs() < 1e-3, "{name}: {got}, not {want}"),
            other => panic!("{name}: {other:?}"),
        };
        near("ball", 3.0);
        near("boxed", 3.0);
        assert_eq!(field("missed"), Value::Bool(true));
    }

    /// Starts its entity's tweens at its first tick: its scale, eased in; its own size, looping
    /// after a wait; and its rotation; and counts the tweens refused, of a scale to a number, a
    /// curve and an option there are none of, and seconds less than none.
    const PULSE: &str = r#"
local Transform = gg.component("Transform")
local Pulse = gg.declare("Pulse", { started = false, size = 1, refused = 0 })

function Pulse.tick(entity, dt)
    if entity[Pulse.started] then
        return
    end
    entity[Pulse.started] = true
    gg.tween(entity, Transform.scale, vector.create(2, 2, 2), 0.5, { curve = "Quad", ease = "In" })
    gg.tween(entity, Pulse.size, 3, 1, { playback = "Loop", delay = 0.25 })
    gg.tween(entity, Transform.rotation, gg.quat.from_rotation_y(math.pi / 2), 1)
    local one = vector.create(1, 1, 1)
    local refusals = {
        function() gg.tween(entity, Transform.scale, 2, 1) end,
        function() gg.tween(entity, Transform.scale, one, 1, { curve = "Wobbly" }) end,
        function() gg.tween(entity, Transform.scale, one, 1, { speed = 2 }) end,
        function() gg.tween(entity, Transform.scale, one, -1) end,
    }
    for _, refusal in refusals do
        if not pcall(refusal) then
            entity[Pulse.refused] += 1
        end
    end
end

return Pulse
"#;

    /// A script tweens its entity's fields, Rust's and its own, from what they hold, with the
    /// curve, ease, playback and wait it names, as code does; and a tween it can't start is refused
    /// where it asks, starting nothing.
    #[test]
    fn a_script_tweens_its_entity_s_fields() {
        use crate::{Transform, Tweens, step_animation};

        let dir = Scratch::new("luau-tweens");
        let (mut cx, mut host) = loaded(&dir, &[("scripts/pulse.luau", PULSE)]);
        let world = cx.world_mut();
        let pulsing = world.spawn(Transform::IDENTITY);
        world.add_default(pulsing, "Pulse").expect("added");
        let mut run = |world: &mut World, ticks: usize| {
            for _ in 0..ticks {
                host.run(world);
                step_animation(world);
            }
        };
        run(cx.world_mut(), 16);
        let world = cx.world();
        let field = |name| world.get_field(pulsing, "Pulse", name).expect("a field");
        assert_eq!(field("refused"), Value::F64(4.0));
        let tweens = &world.get::<Tweens>(pulsing).expect("tweens").0;
        assert_eq!(tweens.len(), 3, "{tweens:?}");
        let transform = *world.get::<Transform>(pulsing).expect("placed");
        // A quarter second, half of the scale's, eased in: a quarter of the way.
        assert_eq!(transform.scale, Vec3::splat(1.25));
        assert_eq!(field("size"), Value::F64(1.0), "waiting");
        run(cx.world_mut(), 16);
        let world = cx.world();
        let field = |name| world.get_field(pulsing, "Pulse", name).expect("a field");
        let transform = *world.get::<Transform>(pulsing).expect("placed");
        assert_eq!(transform.scale, Vec3::splat(2.0));
        assert_eq!(field("size"), Value::F64(1.5));
        let eighth = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        assert!(transform.rotation.angle_between(eighth) < 1e-3);
        let tweens = &world.get::<Tweens>(pulsing).expect("tweens").0;
        assert_eq!(tweens.len(), 2, "the scale's ended");
    }
}
