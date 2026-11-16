//! Luau scripts: components that scripts declare, which act, each for the entity it's on, at its
//! layer's ticks, as [`Scripts`] says; and mods, scripts a player installs, as [`Mods`] says.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
#[cfg(feature = "dev")]
use std::ffi::c_int;
use std::fmt::Write as _;
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
    Cast, Context, Curve, Ease, Entity, Hit3D, ParamValue, Playback, Shape3D, Tick, Tween,
    TweenValue, World,
};

mod bytecode;
mod contacts;
#[cfg(feature = "dev")]
mod debugger;
mod entities;
// Luau's hooks, its interrupt and its debugger's, which mlua reaches through its raw API alone:
// this module alone may use unsafe code for them.
#[allow(unsafe_code)]
mod hooks;
mod mods;
#[cfg(test)]
mod reference;

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
/// puts back. A changed default reaches the components added after. An undo doesn't take a reload
/// back: saving the file as it was does, the fields the reload dropped coming back at their
/// defaults.
///
/// A script is a component: its file declares one with `gg.declare`, its fields' names and their
/// defaults, numbers, bools, vectors, or `gg.entity` for an entity, none by default; and a `tick`
/// that runs for each entity that has it, given the entity and the tick's length in seconds. It
/// reaches components' fields, Rust's and scripts', by handles, `gg.component("Shard").angle`, and
/// its own's, `Spin.speed`, which an entity reads and writes as `entity[Shard.angle]`: a number,
/// a bool, a vector, a quaternion as a vector, or an entity, nil for an entity or a vector a field
/// may hold none of. A write that [`World::set_field`]
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
/// A tick changes the world's entities at once, as Rust code does, by their components' type
/// names: `gg.spawn`, `gg.despawn`, `gg.add`, `gg.remove` and `gg.alive`, whose arguments are in
/// docs/LUAU.md. What these give an entity, or the entity they spawn, starts the VM's scripts on it
/// at the next tick, though another VM's that run later in the tick, a mod's, run them as they
/// come; a chunk calls none of them, nor `gg.tween`, `gg.play`, `gg.set_param` or writes a field
/// (`script.loading`), as it runs again as a lockstep joiner loads it; and they fail with the engine's errors and codes, such as `ecs.no_entity` for an
/// entity despawned, as a field read or written through a handle to one fails too, and
/// `script.bad_argument` for an argument of another kind. A mod's scripts give and take the
/// components they may write, and spawn only if they may write some type, an entity of components
/// they may write; one without components, under a parent or of a prefab, and a despawn, only if
/// they may write the whole world.
///
/// `gg.quat`, the casts, `gg.find_path`, `gg.tween`, `gg.play` and `gg.set_param` do as
/// [`World::cast_ray`], [`World::cast_shape`], [`World::find_path`], [`World::tween`],
/// [`World::play`] and [`World::set_param`] do, for the entity they're given, and what a mod's
/// scripts need of each is as docs/LUAU.md lists, which is the reference of everything scripts
/// reach, each `gg` function's arguments, results and errors among it. A call that sets
/// `gg.set_param`'s parameter allocates nothing once its VM has been given its name, among the
/// first 64 names it's given.
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

/// The most bytes of a line a script prints, past which it's cut, saying so: so that a mod's
/// lines, each up to as many bytes as its VM may hold, can't fill the log.
const PRINTED: usize = 4096;

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
    /// In the order of their handles' bits.
    fields: RefCell<Vec<FieldRef>>,
    /// The script loading, while one is.
    loading: RefCell<Option<Loading>>,
    /// The interrupts left to the call into a script that runs, of its [`BUDGET`], or, a mod's, to
    /// its calls this tick.
    budget: Cell<u32>,
    /// `None` for a game's own scripts, which reach everything.
    held: Option<Held>,
    /// The parameters' names that scripts have given `gg.set_param` in calls it took, the first
    /// [`PARAMS`] of them, each the Luau string, held so that its address stays its own, and its
    /// text: so that a call that names one again, as calls at each tick do, finds its text by the
    /// address of the string Luau interned, where reading a Luau string's text through mlua
    /// allocates, as a call naming one past them does.
    params: RefCell<Vec<(LuaString, String)>>,
    /// What the scripts gave entities as they ran this time, which no tick runs for until they run
    /// again, as [`entities`] says: each entity they spawned, with `None`, and each component they
    /// gave one, with its type, and those riding on it. A set, never iterated, so its order counts
    /// for nothing, and cleared in place, so it allocates only as it holds more than ever before.
    fresh: RefCell<HashSet<(Entity, Option<ComponentId>)>>,
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
    /// The bits of its handle, as [`handle_of`] gives them.
    handle: u32,
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
    Int {
        signed: bool,
    },
    Vec2,
    Vec3,
    Vec4,
    /// An f64 vector, as an entity's translation: a script's vector, whose f32s step 2 mm 16 km
    /// from the origin, as LUAU.md says.
    DVec3,
    Quat,
    Entity,
    MaybeEntity,
    /// A vector or none, as a [`NavAgent`](crate::NavAgent)'s destination is: nil for none.
    MaybeVec3,
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
            Type::DVec3 => Self::DVec3,
            Type::Quat => Self::Quat,
            Type::Entity => Self::Entity,
            Type::Option(inner) if **inner == Type::Entity => Self::MaybeEntity,
            Type::Option(inner) if **inner == Type::Vec3 => Self::MaybeVec3,
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

/// The field of `fields`, in the order of their handles' bits, that `key` is the handle of.
fn field_of<'f>(fields: &'f [FieldRef], key: &Lua_) -> mlua::Result<&'f FieldRef> {
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
fn handle_of(
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

/// The error that a mod's scripts aren't granted what `capability` names, which `what` needs.
fn ungranted(capability: &str, what: &str) -> String {
    format!("the mod isn't granted `{capability}`, which {what} needs")
}

/// [`ungranted`] as the error a script's call fails with, `mod.ungranted`, so that a tick's
/// `script.error` names it as its `cause`.
fn denied(capability: &str, what: &str) -> mlua::Error {
    let message = ungranted(capability, what);
    mlua::Error::external(Error::new("mod.ungranted", message))
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
fn write(_: &Lua, (entity, key, value): (LightUserData, Lua_, Lua_)) -> mlua::Result<()> {
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
fn describe(_: &Lua, data: LightUserData) -> mlua::Result<String> {
    Ok(match Entity::from_bits(bits(data)) {
        Some(entity) => format!("entity {entity}"),
        None if bits(data) == 0 => "gg.entity".to_owned(),
        None => "a field's handle".to_owned(),
    })
}

/// `print(...)`: logs what it's given, as `tostring` writes each, between tabs, cut at
/// [`PRINTED`] bytes, as [`printed`] cuts it; a mod's, naming it, unless it has printed as much as
/// it may this second of ticks, as [`Mods`] says. `luau` is Luau's own `tostring`.
fn print(lua: &Lua, luau: &Function, values: &[Lua_]) -> mlua::Result<()> {
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
fn printed(lua: &Lua, luau: &Function, values: &[Lua_]) -> mlua::Result<String> {
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
fn written(lua: &Lua, luau: &Function, value: Lua_) -> mlua::Result<LuaString> {
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

/// The id of the component type `field` is of, in the world scripts run in.
fn missing(field: &FieldRef) -> mlua::Result<ComponentId> {
    field.id.ok_or_else(|| {
        mlua::Error::runtime(format!(
            "this world has no component type `{}` with field `{}` as it was",
            field.component, field.name
        ))
    })
}

/// The error that `field` of `entity` can't be reached in `world`, as `error` says, of its code,
/// such as `ecs.no_entity` for an entity despawned.
fn reaching(world: &World, entity: Entity, field: &FieldRef, error: &Error) -> mlua::Error {
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
fn named(world: &World, entity: Entity) -> String {
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
fn set(
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
fn value_of(ty: Field, value: &Lua_) -> Result<Value> {
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
const PARAMS: usize = 64;

/// `gg.find_path(from, to)`: the corners of the shortest path over the world's navmesh from `from`
/// to `to`, as [`World::find_path`] says, as an array of vectors, or nil where no path joins them;
/// for a mod's script only if its mod may read the whole world, as the navmesh tells where every
/// static collider is.
fn find_path(lua: &Lua, (from, to): (Vector, Vector)) -> mlua::Result<Lua_> {
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
fn tween(_: &Lua, (entity, key, to, seconds, options): TweenArgs) -> mlua::Result<()> {
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
fn play(_: &Lua, (entity, clip, fade): (LightUserData, String, Option<f64>)) -> mlua::Result<()> {
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
fn set_param(_: &Lua, (entity, name, value): (LightUserData, LuaString, Lua_)) -> mlua::Result<()> {
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
fn unit_axis(axis: Vec3, angle: f64) -> mlua::Result<Vec3> {
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
mod tests;
