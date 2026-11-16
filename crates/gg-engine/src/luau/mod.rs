//! Luau scripts: components that scripts declare, which act for the entity they're on at their
//! layer's ticks ([`Scripts`]); and mods, scripts a player installs ([`Mods`]).

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
use crate::grant::{Capability, Grant};
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
// Luau's interrupt and debugger hooks, which mlua reaches only through its raw API: this module
// alone may use unsafe code.
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

// A script's light userdata, a pointer, holds an entity's 64 bits.
const _: () = assert!(
    usize::BITS == 64,
    "scripts hold an entity's 64 bits in a pointer"
);

scoped_tls::scoped_thread_local! {
    /// What scripts reach as they run or load, for the functions Luau calls.
    static SHARED: Shared
}

/// The most Luau interrupts (one per call and loop turn) a call into a script takes before it
/// fails, as [`Scripts`] says. Far above a tick's work, yet a loop that never ends fails soon, even
/// in a debug build.
const BUDGET: u32 = 1_000_000;

/// A layer's Luau scripts, loaded from files in the layer's `load`. docs/LUAU.md is the reference
/// for what scripts can do; this says what the Rust side guarantees.
///
/// A script declares a component type. Where the layer's tick chooses, [`run`](Self::run) runs
/// each script's `tick` once per entity with its component, in query order, the scripts in the
/// order of their components' names. The VM stays on the thread that made it, which runs the ticks.
///
/// In a development build, a saved file reloads between ticks, through an edit that a recording
/// keeps and a replay applies there, and that both peers of a lockstep session apply at one tick.
/// A reload may change fields, not the name: each component keeps the fields whose name and kind it
/// had, the rest taking their defaults. A restored world and components an undo puts back migrate
/// alike. A changed default reaches only components added after. An undo doesn't take a reload
/// back; saving the file as it was does, dropped fields returning at their defaults.
///
/// A script keeps state in its components: no snapshot, reload or joining peer keeps a chunk's
/// locals. Once the chunk has run, every table it keeps, globals included, is frozen.
/// `script.state` refuses a script whose functions assign a local of its chunk, as its bytecode
/// shows, or whose chunk keeps a coroutine or a buffer.
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
/// `gg.spawn`, `gg.despawn`, `gg.add`, `gg.remove` and `gg.alive` change the world at once. What
/// they give an entity, or spawn, starts this VM's scripts on it at the next tick; another VM's
/// scripts that run later in the tick, a mod's, run them as they come. A chunk calls none of them,
/// nor `gg.tween`, `gg.play` or `gg.set_param`, and writes no field (`script.loading`): it runs
/// again as a lockstep joiner loads it. Failures keep the engine's codes, such as `ecs.no_entity`
/// for a despawned entity and `script.bad_argument` for an argument of another kind.
///
/// `gg.quat`, the casts, `gg.find_path`, `gg.tween`, `gg.play` and `gg.set_param` do as
/// [`World::cast_ray`], [`World::cast_shape`], [`World::find_path`], [`World::tween`],
/// [`World::play`] and [`World::set_param`] do. A `gg.set_param` call allocates nothing once its VM
/// has been given the parameter's name, among the first 64 it's given.
///
/// Scripts run alike on every platform, as the simulation must (docs/LUAU.md's sandbox lists what
/// is gone or checked):
///
/// - Luau's C++ is built, by `.cargo/config.toml`, to fuse no multiplication into an addition.
/// - The `math` functions whose precision is the platform's are libm's. The compiler is told they
///   aren't builtins, or it would call the C library's.
/// - `^` calls the platform's `pow` past any hook, so it's refused, as are `--!optimize` and
///   `--!native`.
/// - Library functions that cast a number to an integer in C, undefined past its range where
///   platforms differ, fail as an error before they run at a number past it or not finite. So does
///   `string.format` at a number its conversion would cast so, or write as a non-finite float,
///   which C libraries write each their own way. Not checked: the iterators `ipairs` and
///   `utf8.codes` return, if a script calls one itself.
/// - `math.random`'s state is outside the world, seeded from the clock: it's gone.
/// - An entity is light userdata of its 64 bits, which tables hash and compare by, not an address.
/// - The VM collects garbage at set points, not by Luau's clock: after a chunk runs, and after
///   ticks once the scripts have allocated as much as the VM holds alive. So weak tables hold
///   alike everywhere.
///
/// Each call into a script (a tick, or a chunk as it loads) takes at most a million of Luau's
/// interrupts, alike on every platform, so a loop that never ends doesn't hang the game. No script
/// code runs outside such a call: the engine checks what a chunk returns and keeps without calling
/// its metamethods.
///
/// A development build that serves the tool API debugs scripts through it, as gg-api's `debug`
/// says. A script stops inside its tick, the main thread waiting until a client resumes it. It runs
/// slower while the debugger is armed (a breakpoint placed, a pause or a step asked), and is
/// untouched otherwise.
pub struct Scripts {
    /// Shared with the run's context, which reloads and migrates through it between ticks, when
    /// only the layer's tick reaches the world.
    host: Rc<RefCell<Host>>,
}

/// What runs a layer's scripts, or a mod's.
struct Host {
    lua: Lua,
    shared: Shared,
    /// In the order of their components' names.
    scripts: Vec<Loaded>,
    /// The world (its serial) whose component types the handles name, and their revision, which
    /// another VM's reload moves on by declaring its type again.
    resolved: Option<(u64, u64)>,
    /// The VM's memory limit, a mod's.
    limit: Option<usize>,
    /// What the VM may hold before the next set point collects garbage.
    goal: usize,
}

/// What a mod's scripts are held to ([`Mods`]). `own`: the component types its scripts declared.
/// `prints` is what's left this second of ticks, `unprinted` what it went without.
struct Held {
    name: String,
    grant: Grant,
    own: RefCell<Vec<String>>,
    prints: Cell<u32>,
    unprinted: Cell<u32>,
    ticks: Cell<u32>,
}

/// The most lines a mod prints a second of ticks.
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

    /// Counts a print, and says whether it prints.
    fn prints(&self) -> bool {
        let left = self.prints.get();
        if left == 0 {
            self.unprinted.set(self.unprinted.get() + 1);
        }
        self.prints.set(left.saturating_sub(1));
        left > 0
    }

    /// Counts a tick. As a second of them ends, prints start again and a warning,
    /// `mod.unprinted`, counts those that went unprinted.
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

/// The layers' script hosts, which a context reloads scripts and migrates a restored world
/// through, as [`Scripts`] says; and the run's debugger, given to each, if the run serves the tool
/// API.
#[derive(Default)]
pub(crate) struct Hosts {
    hosts: Vec<Weak<RefCell<Host>>>,
    #[cfg(feature = "dev")]
    debugger: Option<Arc<Debugger>>,
}

/// What the functions Luau calls and its hooks reach. `world` is lent in place of the one kept
/// here as scripts run or load.
struct Shared {
    world: RefCell<World>,
    /// In the order of their handles' bits.
    fields: RefCell<Vec<FieldRef>>,
    loading: RefCell<Option<Loading>>,
    /// The interrupts left of the running call's [`BUDGET`], or, a mod's, of its calls this tick.
    budget: Cell<u32>,
    /// `None` for a game's own scripts, which reach everything.
    held: Option<Held>,
    /// The first [`PARAMS`] names scripts gave `gg.set_param`: the Luau string (held so its
    /// address stays its own) and its text. A call naming one again finds the text by the interned
    /// string's address; reading a Luau string's text through mlua allocates.
    params: RefCell<Vec<(LuaString, String)>>,
    /// What the scripts gave entities this run, which no tick runs for until they run again
    /// ([`entities`]): each spawned entity with `None`, each given component with its type and
    /// those riding on it. Never iterated, so its order counts for nothing; cleared in place, so it
    /// allocates only as it outgrows its past.
    fresh: RefCell<HashSet<(Entity, Option<ComponentId>)>>,
    #[cfg(feature = "dev")]
    hooked: debugger::Hooked,
}

impl Shared {
    /// Whether the scripts may read, and may write, components of type `component`: a mod's may
    /// those its grant names and its own.
    fn may(&self, component: &str) -> (bool, bool) {
        match &self.held {
            None => (true, true),
            Some(held) if held.own.borrow().iter().any(|own| own == component) => (true, true),
            Some(held) => (held.grant.reads(component), held.grant.writes(component)),
        }
    }

    /// Fails with `mod.ungranted` unless the scripts are a game's or their mod has `capability`,
    /// which `what` needs. Both are made for a mod's alone, so a game's calls allocate nothing.
    fn needs(
        &self,
        capability: impl FnOnce() -> Capability,
        what: impl FnOnce() -> String,
    ) -> mlua::Result<()> {
        let Some(held) = &self.held else {
            return Ok(());
        };
        let capability = capability();
        if held.grant.has(&capability) {
            return Ok(());
        }
        Err(denied(&capability.to_string(), &what()))
    }
}

enum Interrupt {
    Go,
    /// Fails the call, past its budget.
    Over,
    #[cfg(feature = "dev")]
    Pause,
}

/// Counts an interrupt against the running call's budget, which fails past it, and, in a
/// development build, stops there if a pause is asked. Script code outside any call into a script,
/// with no budget, fails at once.
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

/// The error of a call that ran past its budget (a mod's: its mod's for the tick) at `line` of
/// `script`, or of script code that ran there outside any call into a script.
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

/// Why the step under way stops at `line`, at stack `depth`, if it does:
/// [`debugger::Hooked::stepped`].
#[cfg(feature = "dev")]
fn stepped(line: c_int, depth: c_int) -> Option<StopReason> {
    if !SHARED.is_set() {
        return None;
    }
    SHARED.with(|shared| shared.hooked.stepped(line, depth))
}

/// Stops the running script at `frames`, stack `depth`, for `why`, until a debugger client says
/// what next: [`debugger::Hooked::stop`].
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

/// A field a handle names: field `name` of component type `component`, holding `ty`, and whether
/// the scripts may read and write it ([`Shared::may`]).
struct FieldRef {
    /// The bits of its handle, from [`handle_of`].
    handle: u32,
    component: Ident,
    /// The type's id in the world scripts last ran in, and the field's index there; `None` if that
    /// world has no such type or field.
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
    /// An f64 vector, as a translation: a script's f32 vector, stepping 2 mm 16 km from the origin
    /// (docs/LUAU.md).
    DVec3,
    Quat,
    Entity,
    MaybeEntity,
    /// A vector or nil, as a [`NavAgent`](crate::NavAgent)'s destination.
    MaybeVec3,
}

impl Field {
    /// The kind of a field of type `ty`, if scripts reach it.
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

struct Loaded {
    /// Its path from the project's root, which a reload names it by.
    path: String,
    text: String,
    /// The component type it declared, its fields, and its id in the world it last ran in.
    component: String,
    fields: Vec<(Ident, Kind, Value)>,
    id: Option<ComponentId>,
    /// The type it rides on (`gg.declare`'s `on`) and its id there.
    on: Option<String>,
    on_id: Option<ComponentId>,
    tick: Option<Function>,
    /// Its [`Chunk`]'s function, which a development build's debugger keeps instead.
    #[cfg(not(feature = "dev"))]
    chunk: Function,
    /// Whether it has failed since it last loaded, which warns once.
    failed: bool,
}

struct Loading {
    path: String,
    /// The component it declared before, if it's reloading.
    before: Option<String>,
    declared: Option<Declaring>,
    /// Why a function it called refused it.
    refused: Option<Error>,
}

/// The component type a loading script declared. `changed`: its fields differ from the world's
/// type of that name, which takes them once the script has loaded.
struct Declaring {
    id: ComponentId,
    component: String,
    table: Table,
    fields: Vec<(Ident, Kind, Value)>,
    changed: bool,
    on: Option<String>,
}

/// A script's chunk, run. Its function lives as long as the script in every build, so the VM holds
/// as much in all: a development build's debugger keeps it to place breakpoints.
struct Chunk {
    component: String,
    fields: Vec<(Ident, Kind, Value)>,
    id: ComponentId,
    on: Option<String>,
    tick: Option<Function>,
    function: Function,
}

impl Scripts {
    /// A VM for scripts.
    ///
    /// Fails with `script.vm` if Luau can't start, as for want of memory.
    pub fn new() -> Result<Self> {
        Ok(Self {
            host: Rc::new(RefCell::new(Host::new(None)?)),
        })
    }

    /// Loads the script at `path`, declaring its component type in `cx`'s world, and returns the
    /// type's name, which [`World::add_default`] takes to give an entity the script. A development
    /// build reloads the file as it's saved ([`Scripts`]), unless the run reads its game's package
    /// ([`App::project`](crate::App::project)), which holds it. Loading the same path again takes
    /// the script's own place.
    ///
    /// # Errors
    ///
    /// - `script.unreadable`: the file can't be read, isn't a regular file or isn't UTF-8.
    /// - `script.pow`, `script.header`: refused as [`Scripts`] says.
    /// - `script.bad`: it doesn't compile, fails as its chunk runs, or returns other than the
    ///   component `gg.declare` returned.
    /// - `reflect.name_taken`: another has the component type.
    /// - Any error of declaring the type.
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

    /// Runs each script's tick once per entity with its component, as [`Scripts`] says. A failing
    /// script warns `script.error` once until it reloads, and runs on for the other entities.
    /// Allocates nothing the scripts don't.
    pub fn run(&mut self, world: &mut World) {
        self.host.borrow_mut().run(world);
    }
}

impl Hosts {
    /// Keeps `host` unless kept already, giving it the run's debugger if any.
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

    /// Has `debugger` debug every host's scripts, those loaded already included.
    #[cfg(feature = "dev")]
    pub(crate) fn debug(&mut self, debugger: Arc<Debugger>) {
        for host in self.hosts() {
            host.borrow().shared.hooked.give(&debugger);
        }
        self.debugger = Some(debugger);
    }

    /// Places the breakpoints again in each host's scripts if they've changed, as a frame starts,
    /// between ticks.
    #[cfg(feature = "dev")]
    pub(crate) fn place(&self) {
        for host in self.hosts() {
            host.borrow().shared.hooked.place();
        }
    }

    /// Whether a script has stopped since this last answered, so no frame's time counts the stop.
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

    /// Reloads the script at `path` with `text` in `world`, through the host that loaded it,
    /// between ticks, as an edit asks. It runs from the next tick; fields change as [`Scripts`]
    /// says. Logs `Script reloaded`, or why it's refused, the script running as it did.
    ///
    /// # Errors
    ///
    /// - `script.unknown`: no layer loaded a script at `path`.
    /// - `script.renamed`: it declares another component.
    /// - Any other failure of a script's load.
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

    /// Has the types the scripts declare in `world`, restored or brought back, hold their fields
    /// as the scripts declare them now ([`Scripts`]).
    pub(crate) fn migrate(&self, world: &mut World) {
        for host in self.hosts() {
            host.borrow_mut().migrate(world);
        }
    }

    /// Each script loaded: its path and its text as last applied. A lockstep host sends them with
    /// its world to its joiner.
    pub(crate) fn applied(&self) -> Vec<(String, String)> {
        let mut applied = Vec::new();
        for host in self.hosts() {
            let host = host.borrow();
            let scripts = host.scripts.iter();
            applied.extend(scripts.map(|loaded| (loaded.path.clone(), loaded.text.clone())));
        }
        applied
    }

    /// Reloads each of `scripts` (path, text) as [`reload`](Self::reload) does; those whose text
    /// is unchanged stay. A lockstep joiner runs its host's, which come with the host's world, in
    /// place of its own files'.
    pub(crate) fn take(&self, world: &mut World, scripts: &[(String, String)]) {
        for (path, text) in scripts {
            let _ = self.reload(world, path, text.clone());
        }
    }
}

/// The message that a mod isn't granted `capability`, which `what` needs.
fn ungranted(capability: &str, what: &str) -> String {
    format!("the mod isn't granted `{capability}`, which {what} needs")
}

/// [`ungranted`] as the `mod.ungranted` error a script's call fails with, which a tick's
/// `script.error` names as its `cause`.
fn denied(capability: &str, what: &str) -> mlua::Error {
    let message = ungranted(capability, what);
    mlua::Error::external(Error::new("mod.ungranted", message))
}

/// Runs `f` on what the functions Luau calls reach; they run only as scripts run or load.
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
