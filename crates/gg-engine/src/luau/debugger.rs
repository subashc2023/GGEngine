//! The debugger of a run's scripts, which the main thread, as it runs them and stops in them, and the
//! tool API's connections' threads, as gg-api's `debug` asks of it, share; and what each host's VM
//! keeps for it, as [`Scripts`](super::Scripts) says.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use gg_api::{Breakpoint, DebugAction, Debugging, StackFrame, Stop, StopReason, Variable};
use gg_core::{Error, Result};
use mlua::{Function, Lua};
use serde_json::Value as Json;
use tracing::{debug, info};

use super::hooks::{self, Frame, Seen};
use crate::{Entity, World};

/// How long a stop waits at a time for what to do: between waits, on Windows, it answers what other
/// threads send the game's windows and dispatches at most `PUMPED` messages posted to them, as
/// `windows_wait::pump` says, so that Windows takes the window for one that waits rather than one
/// that hangs.
const LOOK: Duration = Duration::from_millis(15);
#[cfg(target_os = "windows")]
const PUMPED: usize = 64;

/// The debugger of a run's scripts, which the context gives each host of the layers' scripts, and
/// the tool API's connections' threads ask, as gg-api's `debug` says.
#[derive(Default)]
pub(crate) struct Debugger {
    state: Mutex<State>,
    /// Notified as a script stops, as a stop is told what to do, and as the breakpoints change.
    changed: Condvar,
    /// Whether a pause is asked, which interrupts and calls into scripts read without the lock.
    pausing: AtomicBool,
    /// The breakpoints' revision, which each change moves on, and which hosts read without the
    /// lock as they look whether to place them again.
    revision: AtomicU64,
    /// Set as a script stops, until the main loop takes it to restart its clock.
    stopped: AtomicBool,
}

#[derive(Default)]
struct State {
    /// By script: each line asked, in order, and where it's placed.
    breakpoints: BTreeMap<String, Vec<(u32, Option<u32>)>>,
    /// The scripts the hosts have loaded.
    scripts: BTreeSet<String>,
    stop: Option<Stop>,
    stops: u64,
    asked: Option<DebugAction>,
    /// What the stop does next, once a client has said.
    go: Option<DebugAction>,
}

/// What a stop does next.
enum At {
    /// Places the breakpoints again, as they've changed.
    Place,
    /// Goes on, as `debug`'s action says.
    Go(DebugAction),
}

impl Debugger {
    /// Sets `script`'s breakpoints at `lines`, in place of those it had, for the hosts to place as
    /// they next look: as a frame starts, and at a stop.
    pub(crate) fn set(&self, script: String, mut lines: Vec<u32>) {
        lines.sort_unstable();
        lines.dedup();
        let mut state = self.lock();
        if lines.is_empty() {
            state.breakpoints.remove(&script);
        } else {
            let lines = lines.into_iter().map(|line| (line, None)).collect();
            state.breakpoints.insert(script, lines);
        }
        self.revision.fetch_add(1, Ordering::Relaxed);
        self.changed.notify_all();
    }

    /// Does `action`, as gg-api's `debug` says: a pause asks the next line a script runs to stop,
    /// and anything else has the stop go on.
    ///
    /// Fails with `script.not_stopped` for a continue or a step while no script has stopped.
    pub(crate) fn act(&self, action: DebugAction) -> Result<()> {
        let mut state = self.lock();
        if action == DebugAction::Pause {
            if state.stop.is_none() {
                state.asked = Some(action);
                self.pausing.store(true, Ordering::Relaxed);
            }
            return Ok(());
        }
        if state.stop.is_none() {
            return Err(Error::new(
                "script.not_stopped",
                "no script has stopped, so there's nothing to go on from: a breakpoint or a pause \
                 stops one",
            ));
        }
        state.stop = None;
        state.asked = (action != DebugAction::Continue).then_some(action);
        state.go = Some(action);
        self.changed.notify_all();
        Ok(())
    }

    /// The debugger as it is once a script has stopped, or `wait` has passed, whichever comes
    /// first.
    pub(crate) fn answer(&self, wait: Duration) -> Debugging {
        let until = Instant::now() + wait;
        let mut state = self.lock();
        while state.stop.is_none() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let (waited, _) = (self.changed.wait_timeout(state, left)).expect("never poisoned");
            state = waited;
        }
        let breakpoints = state.breakpoints.iter().flat_map(|(script, lines)| {
            lines.iter().map(|&(line, placed)| Breakpoint {
                script: script.clone(),
                line,
                placed,
            })
        });
        Debugging {
            stopped: state.stop.clone(),
            asked: state.asked,
            stops: state.stops,
            breakpoints: breakpoints.collect(),
            scripts: state.scripts.iter().cloned().collect(),
        }
    }

    /// Whether a script has stopped since this last answered, which the main loop asks so that no
    /// frame's time counts the stop.
    pub(crate) fn took_stop(&self) -> bool {
        self.stopped.swap(false, Ordering::Relaxed)
    }

    /// Whether a script has stopped, and waits, which `ping` says, so that a tool doesn't take the
    /// game for hung.
    pub(crate) fn stopped(&self) -> bool {
        self.lock().stop.is_some()
    }

    fn pausing(&self) -> bool {
        self.pausing.load(Ordering::Relaxed)
    }

    fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// The lines asked of `script`'s breakpoints.
    fn lines(&self, script: &str) -> Vec<u32> {
        let state = self.lock();
        let lines = state.breakpoints.get(script).into_iter().flatten();
        lines.map(|&(line, _)| line).collect()
    }

    /// Notes where `script`'s breakpoints at `lines` were placed, `placed`, line by line, unless
    /// they've changed since.
    fn placed(&self, script: &str, lines: &[u32], placed: &[Option<u32>]) {
        let mut state = self.lock();
        let Some(kept) = state.breakpoints.get_mut(script) else {
            return;
        };
        if kept.iter().map(|&(line, _)| line).eq(lines.iter().copied()) {
            for ((_, at), &placed) in kept.iter_mut().zip(placed) {
                *at = placed;
            }
        }
    }

    /// Notes that a host has loaded `script`.
    fn loaded(&self, script: &str) {
        self.lock().scripts.insert(script.to_owned());
    }

    /// Has a script stop where `frames` says, for `why`: answers the requests waiting for a stop,
    /// then waits for a client to say what it does next, placing the breakpoints again through
    /// `place` as they change meanwhile, and returns that.
    fn stop(&self, why: StopReason, frames: Vec<StackFrame>, place: impl Fn()) -> DebugAction {
        let (script, line) = (frames.first()).map_or((String::new(), 0), |frame| {
            (frame.script.clone(), frame.line)
        });
        let why_named = match why {
            StopReason::Breakpoint => "a breakpoint",
            StopReason::Step => "a step",
            StopReason::Pause => "a pause",
        };
        {
            let mut state = self.lock();
            state.stops += 1;
            let number = state.stops;
            state.stop = Some(Stop {
                number,
                why,
                frames,
            });
            state.asked = None;
            state.go = None;
            self.pausing.store(false, Ordering::Relaxed);
            self.stopped.store(true, Ordering::Relaxed);
            self.changed.notify_all();
        }
        info!(
            script,
            line,
            why = why_named,
            "Script stopped at {script}:{line}, for {why_named}; the game waits until a client has \
             it go on"
        );
        let mut placed = self.revision();
        let go = loop {
            match self.wait(placed) {
                At::Place => {
                    placed = self.revision();
                    place();
                }
                At::Go(go) => break go,
            }
        };
        debug!(script, line, "Script went on");
        go
    }

    /// Waits for what a stop does next: to place the breakpoints again, once their revision isn't
    /// `placed`, or to go on, once a client has said.
    fn wait(&self, placed: u64) -> At {
        let mut state = self.lock();
        loop {
            if let Some(go) = state.go.take() {
                return At::Go(go);
            }
            if self.revision() != placed {
                return At::Place;
            }
            let (waited, _) = (self.changed.wait_timeout(state, LOOK)).expect("never poisoned");
            state = waited;
            #[cfg(target_os = "windows")]
            {
                drop(state);
                crate::windows_wait::pump(PUMPED);
                state = self.lock();
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("never poisoned")
    }
}

/// What a host's VM keeps for the debugger: the run's, once the context has given it one, as a run
/// that serves the tool API does; a handle on the VM, which places breakpoints; each script's chunk,
/// with the breakpoints placed in it; and the step under way.
///
/// Luau steps, calling its hook before each instruction, only in a call it entered stepping: a call
/// into a script runs stepping while the debugger is armed, a breakpoint placed or a pause or a step
/// asked, so that a stop in it can step on. A step from where a pause broke into a call that entered
/// otherwise, as one running as it's asked, goes on to the next line a script runs after that call.
pub(super) struct Hooked {
    debugger: OnceCell<Arc<Debugger>>,
    lua: Lua,
    chunks: RefCell<Vec<Chunked>>,
    /// The breakpoints' revision as last placed, and whether any is placed.
    placed: Cell<u64>,
    breaks: Cell<bool>,
    /// The calls into scripts made, ticks and chunks, which tell a step that the call it started in
    /// has ended.
    calls: Cell<u64>,
    stepping: Cell<Option<Stepping>>,
    /// Whether the VM's main thread enters calls stepping, as last set.
    steps: Cell<bool>,
}

/// A script's chunk, by the script's path, and the lines the breakpoints are placed at in it.
struct Chunked {
    path: String,
    chunk: Function,
    lines: Vec<u32>,
}

/// A step under way: how, and the call into a script, the depth of the stack and the line it
/// started at.
#[derive(Clone, Copy)]
struct Stepping {
    how: DebugAction,
    call: u64,
    depth: c_int,
    line: c_int,
}

impl Hooked {
    pub(super) fn new(lua: Lua) -> Self {
        Self {
            debugger: OnceCell::new(),
            lua,
            chunks: RefCell::new(Vec::new()),
            placed: Cell::new(0),
            breaks: Cell::new(false),
            calls: Cell::new(0),
            stepping: Cell::new(None),
            steps: Cell::new(false),
        }
    }

    /// Takes `debugger`, the run's, unless it has one, telling it of the scripts loaded and placing
    /// their breakpoints.
    pub(super) fn give(&self, debugger: &Arc<Debugger>) {
        if self.debugger.set(debugger.clone()).is_err() {
            return;
        }
        self.placed.set(debugger.revision());
        for chunked in self.chunks.borrow_mut().iter_mut() {
            debugger.loaded(&chunked.path);
            self.place_in(debugger, chunked);
        }
        self.count();
    }

    /// Keeps `chunk`, the chunk of the script at `path`, which has just loaded, in place of the one
    /// it had, placing its breakpoints in it.
    pub(super) fn loaded(&self, path: &str, chunk: Function) {
        let mut chunks = self.chunks.borrow_mut();
        let at = match chunks.iter().position(|chunked| chunked.path == path) {
            Some(at) => {
                let old = &mut chunks[at];
                for &line in &old.lines {
                    hooks::breakpoint(&self.lua, &old.chunk, line, false);
                }
                old.chunk = chunk;
                old.lines.clear();
                at
            }
            None => {
                chunks.push(Chunked {
                    path: path.to_owned(),
                    chunk,
                    lines: Vec::new(),
                });
                chunks.len() - 1
            }
        };
        if let Some(debugger) = self.debugger.get() {
            debugger.loaded(path);
            self.place_in(debugger, &mut chunks[at]);
        }
        drop(chunks);
        self.count();
    }

    /// Places the breakpoints again, if they've changed since they were last placed.
    pub(super) fn place(&self) {
        let Some(debugger) = self.debugger.get() else {
            return;
        };
        let revision = debugger.revision();
        if self.placed.replace(revision) == revision {
            return;
        }
        for chunked in self.chunks.borrow_mut().iter_mut() {
            self.place_in(debugger, chunked);
        }
        self.count();
    }

    fn place_in(&self, debugger: &Debugger, chunked: &mut Chunked) {
        for &line in &chunked.lines {
            hooks::breakpoint(&self.lua, &chunked.chunk, line, false);
        }
        let lines = debugger.lines(&chunked.path);
        let placed: Vec<Option<u32>> = (lines.iter())
            .map(|&line| hooks::breakpoint(&self.lua, &chunked.chunk, line, true))
            .collect();
        chunked.lines = placed.iter().flatten().copied().collect();
        debugger.placed(&chunked.path, &lines, &placed);
    }

    /// Notes whether any breakpoint is placed, once they've been placed.
    fn count(&self) {
        let chunks = self.chunks.borrow();
        self.breaks
            .set(chunks.iter().any(|chunked| !chunked.lines.is_empty()));
    }

    /// Counts a call into a script, as it's made; as a pause is asked, has the call stop at its
    /// first line, unless a pause is under way already; and has the VM's main thread enter it
    /// stepping while the debugger is armed, as [`Hooked`] says.
    pub(super) fn call(&self) {
        self.calls.set(self.calls.get() + 1);
        let pausing = self.pausing();
        let paused = (self.stepping.get()).is_some_and(|step| step.how == DebugAction::Pause);
        if pausing && !paused {
            self.stepping.set(Some(Stepping {
                how: DebugAction::Pause,
                call: self.calls.get(),
                depth: 0,
                line: 0,
            }));
        }
        let armed = self.breaks.get() || self.stepping.get().is_some();
        if self.steps.replace(armed) != armed {
            hooks::singlestep(&self.lua, armed);
        }
    }

    /// Whether a pause is asked.
    pub(super) fn pausing(&self) -> bool {
        self.debugger
            .get()
            .is_some_and(|debugger| debugger.pausing())
    }

    /// Whether the step under way ends at `line`, at `depth` of the stack, before its instruction
    /// runs, and why it stops there if it does: a step into at any other line or depth; over, at
    /// another line of its function or once it has returned; out, once it has returned; and a pause
    /// at once. Any of them stops once the call into a script it started in has ended, at the next
    /// line a script runs.
    pub(super) fn stepped(&self, line: c_int, depth: c_int) -> Option<StopReason> {
        let step = self.stepping.get()?;
        let ends = self.calls.get() != step.call
            || match step.how {
                DebugAction::StepInto => depth != step.depth || line != step.line,
                DebugAction::StepOver => {
                    depth < step.depth || (depth == step.depth && line != step.line)
                }
                DebugAction::StepOut => depth < step.depth,
                DebugAction::Pause | DebugAction::Continue => true,
            };
        let why = match step.how {
            DebugAction::Pause => StopReason::Pause,
            _ => StopReason::Step,
        };
        ends.then_some(why)
    }

    /// Has the script stop where `frames` says, at `depth` of the stack, for `why`, until a client
    /// says what it does next, as [`Debugger`] says, the breakpoints placed again as they change
    /// meanwhile; and starts the step it says, if it says one.
    pub(super) fn stop(&self, why: StopReason, depth: c_int, frames: Vec<StackFrame>) {
        self.stepping.set(None);
        let Some(debugger) = self.debugger.get() else {
            return;
        };
        let line = (frames.first()).map_or(0, |frame| c_int::try_from(frame.line).unwrap_or(0));
        let how = debugger.stop(why, frames, || self.place());
        if how != DebugAction::Continue {
            self.stepping.set(Some(Stepping {
                how,
                call: self.calls.get(),
                depth,
                line,
            }));
        }
    }
}

/// `frames`, as a stop read them, as the debugger shows them, entities by their IDs in `world`.
pub(super) fn shown(frames: Vec<Frame>, world: &World) -> Vec<StackFrame> {
    let variables = |seen: Vec<(String, Seen)>| {
        let shown = seen.into_iter().map(|(name, seen)| Variable {
            name,
            value: json(seen, world),
        });
        shown.collect()
    };
    let frames = frames.into_iter().map(|frame| StackFrame {
        function: frame.function,
        script: frame.script,
        line: frame.line,
        locals: variables(frame.locals),
        upvalues: variables(frame.upvalues),
    });
    frames.collect()
}

/// `seen` as gg-api's [`Variable`] says it's shown.
fn json(seen: Seen, world: &World) -> Json {
    match seen {
        Seen::Nil => Json::Null,
        Seen::Bool(b) => Json::Bool(b),
        Seen::Number(x) => float(x),
        Seen::Vector(v) => Json::Array(v.iter().map(|&x| float(x.into())).collect()),
        Seen::String(text) => Json::String(text),
        Seen::Handle(0) => Json::Null,
        Seen::Handle(bits) if bits >> 32 == 0 => Json::String("(handle)".to_owned()),
        Seen::Handle(bits) => {
            let id = Entity::from_bits(bits).and_then(|entity| world.id(entity));
            id.map_or(Json::Null, |id| Json::String(id.to_string()))
        }
        Seen::Other(kind) => Json::String(format!("({kind})")),
    }
}

/// `x` as JSON: a number, or for one that isn't finite, which JSON has none for, `NaN`, `inf` or
/// `-inf`, as a value's float is.
fn float(x: f64) -> Json {
    serde_json::Number::from_f64(x).map_or_else(
        || {
            let name = match (x.is_nan(), x > 0.0) {
                (true, _) => "NaN",
                (false, true) => "inf",
                (false, false) => "-inf",
            };
            Json::String(name.to_owned())
        },
        Json::Number,
    )
}
