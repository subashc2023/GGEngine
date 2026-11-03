//! Luau's hooks, which mlua reaches through its raw API alone: the interrupt, which keeps each call
//! into a script to its budget and, in development builds, stops there as a pause asks; and, in
//! development builds, breakpoints, steps, and the stack a stop reads. The engine's side of each,
//! what to do, is [`super`]'s; this module reads and sets what it needs of Luau's state, as it
//! reads a function's upvalues for [`super`]'s check of what a script's chunk keeps.

use std::ffi::{CStr, c_int};
#[cfg(feature = "dev")]
use std::mem;
use std::{panic, process};

#[cfg(feature = "dev")]
use gg_api::StopReason;
use mlua::{Function, Lua, Value, Variadic, ffi};

use super::Interrupt;

/// Installs the hooks in `lua`'s VM.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // SAFETY: `exec_raw` runs the closure on a state of `lua`'s VM, whose callbacks every state of
    // the VM shares; mlua sets none of these but the interrupt, as `Lua::set_interrupt` asks, which
    // the engine never calls.
    unsafe {
        lua.exec_raw::<()>((), |state| {
            let callbacks = ffi::lua_callbacks(state);
            (*callbacks).interrupt = Some(interrupt);
            #[cfg(feature = "dev")]
            {
                (*callbacks).debugbreak = Some(debugbreak);
                (*callbacks).debugstep = Some(debugstep);
            }
        })
    }
}

/// Luau's interrupt, which comes at each call and each turn of a loop: counts against the budget of
/// the call into a script it comes in, failing the call past it, and stops there if a pause is
/// asked, as [`super::interrupted`] says. The GC's own, which come as it collects, count for
/// nothing.
unsafe extern "C-unwind" fn interrupt(state: *mut ffi::lua_State, gc: c_int) {
    if gc >= 0 {
        return;
    }
    match guarded(super::interrupted) {
        Interrupt::Go => {}
        #[cfg(feature = "dev")]
        // SAFETY: Luau calls the interrupt with the state that runs, between its instructions.
        Interrupt::Pause => unsafe { stop(state, StopReason::Pause) },
        Interrupt::Over => {
            // SAFETY: as above; the error unwinds to the call into the script, which mlua catches,
            // and nothing of Rust's that needs dropping is left in this frame as it's raised.
            unsafe {
                let (script, line) = position(state);
                let message = guarded(|| super::over_budget(&script, line));
                drop(script);
                ffi::lua_rawcheckstack(state, 1);
                ffi::lua_pushlstring_(state, message.as_ptr().cast(), message.len());
                drop(message);
                ffi::lua_error(state)
            }
        }
    }
}

/// Luau's hook as a breakpoint's line is reached: stops there.
#[cfg(feature = "dev")]
unsafe extern "C-unwind" fn debugbreak(state: *mut ffi::lua_State, _: *mut ffi::lua_Debug) {
    // SAFETY: Luau calls the hook with the state that runs, at the breakpoint's instruction.
    unsafe { stop(state, StopReason::Breakpoint) }
}

/// Luau's hook before each instruction while the state steps: stops where the step under way ends,
/// as [`super::stepped`] says.
#[cfg(feature = "dev")]
unsafe extern "C-unwind" fn debugstep(state: *mut ffi::lua_State, ar: *mut ffi::lua_Debug) {
    // SAFETY: Luau calls the hook with the state that runs, before an instruction, and `ar` with
    // its line.
    unsafe {
        let line = (*ar).currentline;
        let depth = ffi::lua_stackdepth(state);
        if let Some(why) = guarded(|| super::stepped(line, depth)) {
            stop(state, why);
        }
    }
}

/// Stops the script `state` runs, for `why`, there, as [`super::stopped`] says.
///
/// # Safety
///
/// `state` runs, stopped between two instructions in a hook.
#[cfg(feature = "dev")]
unsafe fn stop(state: *mut ffi::lua_State, why: StopReason) {
    // SAFETY: as the caller promises.
    unsafe {
        let depth = ffi::lua_stackdepth(state);
        let frames = frames(state);
        guarded(|| super::stopped(why, depth, frames));
    }
}

/// The script and the line the function `state` runs is at, as a stop or an error names them.
///
/// # Safety
///
/// `state` runs, stopped between two instructions in a hook.
unsafe fn position(state: *mut ffi::lua_State) -> (String, u32) {
    // SAFETY: as the caller promises; `ar` is Luau's to fill, and its strings live as long as the
    // function, which runs.
    unsafe {
        let mut ar = std::mem::zeroed::<ffi::lua_Debug>();
        if ffi::lua_getinfo(state, 0, c"sl".as_ptr(), &mut ar) == 0 {
            return (String::new(), 0);
        }
        let script = CStr::from_ptr(ar.short_src).to_string_lossy().into_owned();
        (script, u32::try_from(ar.currentline).unwrap_or(0))
    }
}

/// A function of the stack where a script has stopped, as [`frames`] reads it.
#[cfg(feature = "dev")]
pub(super) struct Frame {
    pub(super) function: String,
    pub(super) script: String,
    pub(super) line: u32,
    pub(super) locals: Vec<(String, Seen)>,
    pub(super) upvalues: Vec<(String, Seen)>,
}

/// A value a stop reads, without calling into Luau, as a metamethod would be.
#[cfg(feature = "dev")]
pub(super) enum Seen {
    Nil,
    Bool(bool),
    Number(f64),
    Vector([f32; 4]),
    /// At most [`gg_api::MOST_SHOWN`] bytes of it.
    String(String),
    /// Light userdata's bits: an entity's, a field's handle's, or `gg.entity`'s.
    Handle(u64),
    /// Any other kind, by its name.
    Other(&'static str),
}

/// The stack of the script `state` runs, from the function it runs to the call into the script,
/// Luau's functions alone, each with its locals and upvalues.
///
/// # Safety
///
/// `state` runs, stopped between two instructions in a hook.
#[cfg(feature = "dev")]
unsafe fn frames(state: *mut ffi::lua_State) -> Vec<Frame> {
    let mut frames = Vec::new();
    // SAFETY: as the caller promises. Each value read is pushed, read and popped, the function
    // whose upvalues are read popped after them; `ar`'s strings live as long as their function,
    // which is on the stack.
    unsafe {
        // The function and a value, at most.
        ffi::lua_rawcheckstack(state, 2);
        for level in 0.. {
            let mut ar = mem::zeroed::<ffi::lua_Debug>();
            if ffi::lua_getinfo(state, level, c"sln".as_ptr(), &mut ar) == 0 {
                break;
            }
            let what = CStr::from_ptr(ar.what);
            if what == c"C" {
                continue;
            }
            let function = if !ar.name.is_null() {
                CStr::from_ptr(ar.name).to_string_lossy().into_owned()
            } else if what == c"main" {
                "(chunk)".to_owned()
            } else {
                "(anonymous)".to_owned()
            };
            let script = CStr::from_ptr(ar.short_src).to_string_lossy().into_owned();
            let line = u32::try_from(ar.currentline).unwrap_or(0);
            let mut locals = Vec::new();
            for n in 1.. {
                let name = ffi::lua_getlocal(state, level, n);
                if name.is_null() {
                    break;
                }
                locals.push((
                    CStr::from_ptr(name).to_string_lossy().into_owned(),
                    seen(state),
                ));
                ffi::lua_pop(state, 1);
            }
            let mut upvalues = Vec::new();
            if ffi::lua_getinfo(state, level, c"f".as_ptr(), &mut ar) != 0 {
                for n in 1.. {
                    let name = ffi::lua_getupvalue(state, -1, n);
                    if name.is_null() {
                        break;
                    }
                    let name = CStr::from_ptr(name).to_string_lossy().into_owned();
                    upvalues.push((name, seen(state)));
                    ffi::lua_pop(state, 1);
                }
                ffi::lua_pop(state, 1);
            }
            frames.push(Frame {
                function,
                script,
                line,
                locals,
                upvalues,
            });
        }
    }
    frames
}

/// The value at the top of `state`'s stack, read without calling into Luau.
///
/// # Safety
///
/// `state`'s stack holds a value.
#[cfg(feature = "dev")]
unsafe fn seen(state: *mut ffi::lua_State) -> Seen {
    // SAFETY: as the caller promises; each read is of the type `lua_type` says, which converts
    // nothing, and the string's bytes live while it's on the stack.
    unsafe {
        match ffi::lua_type(state, -1) {
            ffi::LUA_TNIL => Seen::Nil,
            ffi::LUA_TBOOLEAN => Seen::Bool(ffi::lua_toboolean(state, -1) != 0),
            ffi::LUA_TNUMBER => Seen::Number(ffi::lua_tonumber(state, -1)),
            ffi::LUA_TINTEGER => {
                let n = ffi::lua_tointeger64(state, -1, std::ptr::null_mut());
                Seen::Number(n as f64)
            }
            ffi::LUA_TVECTOR => {
                let v = ffi::lua_tovector(state, -1);
                Seen::Vector([*v, *v.add(1), *v.add(2), *v.add(3)])
            }
            ffi::LUA_TSTRING => {
                let mut len = 0;
                let bytes = ffi::lua_tolstring(state, -1, &mut len);
                let bytes = std::slice::from_raw_parts(bytes.cast::<u8>(), len);
                let shown = &bytes[..len.min(gg_api::MOST_SHOWN)];
                Seen::String(String::from_utf8_lossy(shown).into_owned())
            }
            ffi::LUA_TLIGHTUSERDATA => {
                Seen::Handle(ffi::lua_tolightuserdata(state, -1).addr() as u64)
            }
            ffi::LUA_TTABLE => Seen::Other("table"),
            ffi::LUA_TFUNCTION => Seen::Other("function"),
            ffi::LUA_TUSERDATA => Seen::Other("userdata"),
            ffi::LUA_TTHREAD => Seen::Other("thread"),
            ffi::LUA_TBUFFER => Seen::Other("buffer"),
            _ => Seen::Other("value"),
        }
    }
}

/// Sets, or clears if not `on`, a breakpoint at `line` of `chunk`, a script's chunk, or the next line
/// with code after it, in the chunk or the functions it holds, and returns the line it's at; `None`
/// if there's none.
#[cfg(feature = "dev")]
pub(super) fn breakpoint(lua: &Lua, chunk: &Function, line: u32, on: bool) -> Option<u32> {
    let line = c_int::try_from(line).ok()?;
    let mut placed = -1;
    // SAFETY: `exec_raw` runs the closure with `chunk`, a function Luau compiled, alone on the
    // stack, which `lua_breakpoint` reads it from; it may run inside a hook, as Luau allows.
    let ran = unsafe {
        lua.exec_raw::<()>(chunk, |state| {
            placed = ffi::lua_breakpoint(state, 1, line, c_int::from(on));
        })
    };
    ran.ok()?;
    u32::try_from(placed).ok()
}

/// The values `function`'s upvalues hold, in order; of a C function's, the coroutines and buffers
/// alone, as `coroutine.wrap`'s hold its coroutine, while mlua's functions hold its own values.
pub(super) fn upvalues(lua: &Lua, function: &Function) -> mlua::Result<Variadic<Value>> {
    // SAFETY: `exec_raw` runs the closure with `function` alone on the stack, and returns what the
    // stack holds after, which is each upvalue `lua_getupvalue` pushed and kept once the function
    // is gone.
    unsafe {
        lua.exec_raw(function, |state| {
            let c = ffi::lua_iscfunction(state, 1) != 0;
            for n in 1.. {
                ffi::lua_rawcheckstack(state, 1);
                if ffi::lua_getupvalue(state, 1, n).is_null() {
                    break;
                }
                let kind = ffi::lua_type(state, -1);
                if c && kind != ffi::LUA_TTHREAD && kind != ffi::LUA_TBUFFER {
                    ffi::lua_pop(state, 1);
                }
            }
            ffi::lua_remove(state, 1);
        })
    }
}

/// Has the VM's main thread, which runs the scripts, enter the calls into them stepping, calling
/// [`debugstep`] before each instruction, or not: Luau steps only in a call it entered so.
#[cfg(feature = "dev")]
pub(super) fn singlestep(lua: &Lua, on: bool) {
    // SAFETY: `exec_raw` runs the closure on a state of `lua`'s VM, from which `lua_mainthread`
    // finds the main thread; between calls into scripts, which it runs.
    let _ = unsafe {
        lua.exec_raw::<()>((), |state| {
            ffi::lua_singlestep(ffi::lua_mainthread(state), c_int::from(on));
        })
    };
}

/// Runs `f`, the engine's side of a hook, ending the process if it panics, as a broken invariant
/// does, its message printed: a panic would unwind into Luau's C++, which doesn't expect one.
fn guarded<R>(f: impl FnOnce() -> R) -> R {
    panic::catch_unwind(panic::AssertUnwindSafe(f)).unwrap_or_else(|_| process::abort())
}
