//! Luau scripts: components that scripts declare, which act, each for the entity it's on, at its
//! layer's ticks, as [`Scripts`] says; and mods, scripts a player installs, as [`Mods`] says.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
#[cfg(feature = "dev")]
use std::ffi::c_int;
use std::path::Path;
use std::rc::{Rc, Weak};
#[cfg(feature = "dev")]
use std::sync::Arc;
use std::{io, ptr};

#[cfg(feature = "dev")]
use gg_api::StopReason;
use gg_core::{Error, Result};
use mlua::{Function, Lua, LuaString, Table};
use tracing::warn;

use crate::ecs::{ComponentId, Kind};
use crate::grant::Grant;
use crate::reflect::{Ident, Type, Value};
use crate::{Context, Entity, Tick, World};

#[cfg(doc)]
use api::PARAMS;
#[cfg(doc)]
use fields::handle_of;

mod api;
mod bytecode;
mod compile;
mod contacts;
#[cfg(feature = "dev")]
mod debugger;
mod entities;
mod fields;
// Luau's hooks, its interrupt and its debugger's, which mlua reaches through its raw API alone:
// this module alone may use unsafe code for them.
#[allow(unsafe_code)]
mod hooks;
mod host;
mod mods;
#[cfg(test)]
mod reference;

#[cfg(feature = "dev")]
pub(crate) use compile::compiles;
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
            cx.dev.scripts.watch(name, path, text);
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

    /// Never: a build without `dev` stops no script.
    #[cfg(not(feature = "dev"))]
    pub(crate) fn took_stop(&self) -> bool {
        false
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

#[cfg(feature = "dev")]
pub(crate) use watch::{Files, look};

#[cfg(feature = "dev")]
mod watch;

#[cfg(test)]
mod tests;
