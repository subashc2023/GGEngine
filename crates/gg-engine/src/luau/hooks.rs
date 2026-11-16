//! Luau's hooks, reached through mlua's raw API alone: the interrupt, which holds each call into a
//! script to its budget (and, in development builds, stops it on a pause), and, in development
//! builds, breakpoints, steps and the stack a stop reads. What to do at each is [`super`]'s.
//!
//! It also reads function upvalues for [`super`]'s check of what a chunk keeps, and checks the
//! integers Luau's libraries read ([`CHECKED`]).

use std::ffi::{CStr, c_int};
#[cfg(feature = "dev")]
use std::mem;
use std::{panic, process};

use Cast::{Int, Long, LongNumber};
#[cfg(feature = "dev")]
use gg_api::StopReason;
use mlua::{Function, Lua, Value, Variadic, ffi};

use super::Interrupt;

/// How a library function makes an integer of a number: a C cast (Luau's `luai_num2int` and
/// `luai_num2unsigned`), undefined past the type's range. x86-64 and ARM64 give other integers,
/// and NaN others again.
#[derive(Clone, Copy)]
enum Cast {
    /// To `int`, as `luaL_checkinteger`; a string is converted to a number first.
    Int,
    /// To `long long`, as `luaL_checkunsigned` before it keeps the low 32 bits; a string is
    /// converted first.
    Long,
    /// To `long long`, as `string.pack`'s integer options; a string isn't converted.
    LongNumber,
}

impl Cast {
    /// Whether `n` casts alike everywhere: finite, and within the type's range once its fraction
    /// is cut.
    fn defined(self, n: f64) -> bool {
        match self {
            Self::Int => n > -2_147_483_649.0 && n < 2_147_483_648.0,
            Self::Long | Self::LongNumber => {
                (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&n)
            }
        }
    }
}

/// What of its arguments a [`Checked`] function reads as integers.
enum Reads {
    /// The arguments at `at` (positions from 1) and from `rest`'s position on, each with its cast;
    /// only with `least` arguments (`table.insert`'s second is a value when there are two).
    Args {
        at: &'static [(c_int, Cast)],
        rest: Option<(c_int, Cast)>,
        least: c_int,
    },
    /// What `string.format`'s format string says ([`formatted`]).
    Format,
}

/// The arguments at `at` ([`Reads::Args`]).
const fn at(at: &'static [(c_int, Cast)]) -> Reads {
    Reads::Args {
        at,
        rest: None,
        least: 0,
    }
}

/// Every argument from `from` on, cast as `cast` ([`Reads::Args`]).
const fn from(from: c_int, cast: Cast) -> Reads {
    Reads::Args {
        at: &[],
        rest: Some((from, cast)),
        least: 0,
    }
}

/// A library function whose integer arguments are checked before it runs. `library` is `None` for
/// a global. `builtin`: the compiler may still call it as one (its fast path casts nothing).
struct Checked {
    library: Option<&'static CStr>,
    name: &'static CStr,
    reads: Reads,
    builtin: bool,
}

const fn checked(library: Option<&'static CStr>, name: &'static CStr, reads: Reads) -> Checked {
    Checked {
        library,
        name,
        reads,
        builtin: false,
    }
}

/// The library functions scripts reach that cast a number to an integer. Each fails on an
/// argument that wouldn't cast alike everywhere ([`Cast::defined`]), as `^` is refused. The other
/// casting functions are iterators, which [`super::Scripts`] lists.
///
/// The compiler calls none as a builtin (its fast path would skip the check), except
/// `table.insert` and `tonumber`, whose fast paths take only calls that cast nothing.
static CHECKED: [Checked; 64] = [
    Checked {
        builtin: true,
        ..checked(None, c"tonumber", at(&[(2, Int)]))
    },
    checked(None, c"error", at(&[(2, Int)])),
    checked(None, c"select", at(&[(1, Int)])),
    checked(None, c"unpack", at(&[(2, Int), (3, Int)])),
    checked(Some(c"math"), c"ldexp", at(&[(2, Int)])),
    checked(Some(c"string"), c"byte", at(&[(2, Int), (3, Int)])),
    checked(Some(c"string"), c"char", from(1, Int)),
    checked(Some(c"string"), c"find", at(&[(3, Int)])),
    checked(Some(c"string"), c"format", Reads::Format),
    checked(Some(c"string"), c"gsub", at(&[(4, Int)])),
    checked(Some(c"string"), c"match", at(&[(3, Int)])),
    checked(Some(c"string"), c"pack", from(2, LongNumber)),
    checked(Some(c"string"), c"rep", at(&[(2, Int)])),
    checked(Some(c"string"), c"sub", at(&[(2, Int), (3, Int)])),
    checked(Some(c"string"), c"unpack", at(&[(3, Int)])),
    checked(Some(c"table"), c"concat", at(&[(3, Int), (4, Int)])),
    checked(Some(c"table"), c"create", at(&[(1, Int)])),
    checked(Some(c"table"), c"find", at(&[(3, Int)])),
    Checked {
        builtin: true,
        ..checked(
            Some(c"table"),
            c"insert",
            Reads::Args {
                at: &[(2, Int)],
                rest: None,
                least: 3,
            },
        )
    },
    checked(Some(c"table"), c"move", at(&[(2, Int), (3, Int), (4, Int)])),
    checked(Some(c"table"), c"remove", at(&[(2, Int)])),
    checked(Some(c"table"), c"unpack", at(&[(2, Int), (3, Int)])),
    checked(Some(c"utf8"), c"char", from(1, Int)),
    checked(Some(c"utf8"), c"codepoint", at(&[(2, Int), (3, Int)])),
    checked(Some(c"utf8"), c"len", at(&[(2, Int), (3, Int)])),
    checked(Some(c"utf8"), c"offset", at(&[(2, Int), (3, Int)])),
    checked(Some(c"bit32"), c"arshift", at(&[(1, Long), (2, Int)])),
    checked(Some(c"bit32"), c"band", from(1, Long)),
    checked(Some(c"bit32"), c"bnot", at(&[(1, Long)])),
    checked(Some(c"bit32"), c"bor", from(1, Long)),
    checked(Some(c"bit32"), c"btest", from(1, Long)),
    checked(Some(c"bit32"), c"bxor", from(1, Long)),
    checked(Some(c"bit32"), c"byteswap", at(&[(1, Long)])),
    checked(Some(c"bit32"), c"countlz", at(&[(1, Long)])),
    checked(Some(c"bit32"), c"countrz", at(&[(1, Long)])),
    checked(
        Some(c"bit32"),
        c"extract",
        at(&[(1, Long), (2, Int), (3, Int)]),
    ),
    checked(Some(c"bit32"), c"lrotate", at(&[(1, Long), (2, Int)])),
    checked(Some(c"bit32"), c"lshift", at(&[(1, Long), (2, Int)])),
    checked(
        Some(c"bit32"),
        c"replace",
        at(&[(1, Long), (2, Long), (3, Int), (4, Int)]),
    ),
    checked(Some(c"bit32"), c"rrotate", at(&[(1, Long), (2, Int)])),
    checked(Some(c"bit32"), c"rshift", at(&[(1, Long), (2, Int)])),
    checked(
        Some(c"buffer"),
        c"copy",
        at(&[(2, Int), (4, Int), (5, Int)]),
    ),
    checked(Some(c"buffer"), c"create", at(&[(1, Int)])),
    checked(
        Some(c"buffer"),
        c"fill",
        at(&[(2, Int), (3, Long), (4, Int)]),
    ),
    checked(Some(c"buffer"), c"readbits", at(&[(2, Long), (3, Int)])),
    checked(Some(c"buffer"), c"readf32", at(&[(2, Int)])),
    checked(Some(c"buffer"), c"readf64", at(&[(2, Int)])),
    checked(Some(c"buffer"), c"readi16", at(&[(2, Int)])),
    checked(Some(c"buffer"), c"readi32", at(&[(2, Int)])),
    checked(Some(c"buffer"), c"readi8", at(&[(2, Int)])),
    checked(Some(c"buffer"), c"readstring", at(&[(2, Int), (3, Int)])),
    checked(Some(c"buffer"), c"readu16", at(&[(2, Int)])),
    checked(Some(c"buffer"), c"readu32", at(&[(2, Int)])),
    checked(Some(c"buffer"), c"readu8", at(&[(2, Int)])),
    checked(
        Some(c"buffer"),
        c"writebits",
        at(&[(2, Long), (3, Int), (4, Long)]),
    ),
    checked(Some(c"buffer"), c"writef32", at(&[(2, Int)])),
    checked(Some(c"buffer"), c"writef64", at(&[(2, Int)])),
    checked(Some(c"buffer"), c"writei16", at(&[(2, Int), (3, Long)])),
    checked(Some(c"buffer"), c"writei32", at(&[(2, Int), (3, Long)])),
    checked(Some(c"buffer"), c"writei8", at(&[(2, Int), (3, Long)])),
    checked(Some(c"buffer"), c"writestring", at(&[(2, Int), (4, Int)])),
    checked(Some(c"buffer"), c"writeu16", at(&[(2, Int), (3, Long)])),
    checked(Some(c"buffer"), c"writeu32", at(&[(2, Int), (3, Long)])),
    checked(Some(c"buffer"), c"writeu8", at(&[(2, Int), (3, Long)])),
];

/// The names (`library.name`, or a global's) of the [`CHECKED`] functions not to compile as
/// builtins.
pub(super) fn not_builtins() -> impl Iterator<Item = String> {
    CHECKED
        .iter()
        .filter(|checked| !checked.builtin)
        .map(|checked| {
            let name = checked.name.to_string_lossy();
            match checked.library {
                Some(library) => format!("{}.{name}", library.to_string_lossy()),
                None => name.into_owned(),
            }
        })
}

/// Replaces each [`CHECKED`] function in `lua`'s libraries with a C closure ([`refusing`]) that
/// checks its integer arguments and then calls it.
///
/// Fails if one isn't a C function in its library.
pub(super) fn check_integers(lua: &Lua) -> mlua::Result<()> {
    // SAFETY: `exec_raw` runs the closure on a state of `lua`'s VM with an empty stack. The
    // closure keeps to three values (library table, function, number), for which
    // `lua_rawcheckstack` makes room, and returns the index of a function that isn't there, or
    // nothing. Each closure made holds the function and its index in `CHECKED`, a static, as
    // upvalues, and a static name, which Luau keeps.
    let missing = unsafe {
        lua.exec_raw::<Option<u32>>((), |state| {
            ffi::lua_rawcheckstack(state, 3);
            for (index, checked) in (0..).zip(&CHECKED) {
                match checked.library {
                    Some(library) => {
                        ffi::lua_rawgetfield(state, ffi::LUA_GLOBALSINDEX, library.as_ptr());
                    }
                    None => ffi::lua_pushvalue(state, ffi::LUA_GLOBALSINDEX),
                }
                if ffi::lua_type(state, -1) == ffi::LUA_TTABLE {
                    ffi::lua_rawgetfield(state, -1, checked.name.as_ptr());
                }
                if ffi::lua_iscfunction(state, -1) == 0 {
                    ffi::lua_settop(state, 0);
                    ffi::lua_pushnumber(state, f64::from(index));
                    return;
                }
                ffi::lua_pushnumber(state, f64::from(index));
                ffi::lua_pushcclosurek(state, refusing, checked.name.as_ptr(), 2, None);
                ffi::lua_rawsetfield(state, -2, checked.name.as_ptr());
                ffi::lua_pop(state, 1);
            }
        })
    }?;
    match missing {
        Some(index) => Err(mlua::Error::runtime(format!(
            "the library function `{}` isn't Luau's",
            CHECKED[index as usize].name.to_string_lossy()
        ))),
        None => Ok(()),
    }
}

/// A [`CHECKED`] function's replacement: fails on an integer argument that wouldn't cast alike
/// everywhere, else calls the function. Its upvalues are the function and its index in `CHECKED`.
unsafe extern "C-unwind" fn refusing(state: *mut ffi::lua_State) -> c_int {
    // SAFETY: Luau calls the closure `check_integers` made, with its two upvalues, the second an
    // index in `CHECKED`. The function it calls is a library's and reads no upvalue of its own, so
    // it runs in the closure's frame as in its own. An error raised leaves nothing of Rust's in
    // this frame that needs dropping.
    unsafe {
        let index = ffi::lua_tonumberx(state, ffi::lua_upvalueindex(2), std::ptr::null_mut());
        let Some(checked) = CHECKED.get(index as usize) else {
            // `check_integers` made it with its place.
            process::abort()
        };
        let refused = match checked.reads {
            Reads::Args { at, rest, least } => args(state, at, rest, least),
            Reads::Format => formatted(state),
        };
        if let Some((arg, what)) = refused {
            let name = checked.name.to_string_lossy();
            let message = format!("invalid argument #{arg} to '{name}' ({what})");
            drop((name, what));
            ffi::lua_rawcheckstack(state, 2);
            ffi::luaL_where(state, 1);
            ffi::lua_pushlstring_(state, message.as_ptr().cast(), message.len());
            drop(message);
            ffi::lua_concat(state, 2);
            ffi::lua_error(state);
        }
        let Some(function) = ffi::lua_tocfunction(state, ffi::lua_upvalueindex(1)) else {
            // `check_integers` made it of a C function.
            process::abort()
        };
        function(state)
    }
}

/// The first argument of `at` and `rest` ([`Reads::Args`]) that wouldn't cast alike everywhere,
/// and why.
///
/// # Safety
///
/// `state` runs a C function, whose arguments these are.
unsafe fn args(
    state: *mut ffi::lua_State,
    at: &[(c_int, Cast)],
    rest: Option<(c_int, Cast)>,
    least: c_int,
) -> Option<(c_int, String)> {
    // SAFETY: as the caller promises.
    unsafe {
        let top = ffi::lua_gettop(state);
        if top < least {
            return None;
        }
        let rest = rest
            .into_iter()
            .flat_map(|(from, cast)| (from..=top).map(move |arg| (arg, cast)));
        for (arg, cast) in at.iter().copied().chain(rest) {
            if let Some(n) = number(state, arg, cast)
                && !cast.defined(n)
            {
                return Some((
                    arg,
                    format!("{n} has no integer that every platform casts it to"),
                ));
            }
        }
        None
    }
}

/// Argument `arg` as the number a library reads it as under `cast`, if it is one.
///
/// # Safety
///
/// `state` runs a C function.
unsafe fn number(state: *mut ffi::lua_State, arg: c_int, cast: Cast) -> Option<f64> {
    // SAFETY: as the caller promises; `arg` is one of its arguments, and `lua_tonumberx` converts
    // nothing on the stack.
    unsafe {
        if arg > ffi::lua_gettop(state)
            || (matches!(cast, Cast::LongNumber) && ffi::lua_type(state, arg) != ffi::LUA_TNUMBER)
        {
            return None;
        }
        let mut is = 0;
        let n = ffi::lua_tonumberx(state, arg, &mut is);
        (is != 0).then_some(n)
    }
}

/// The first `string.format` argument that would be written differently on another platform, and
/// why:
///
/// - an integer conversion's argument that doesn't cast alike everywhere: `%c` casts to `int`,
///   `%d` and `%i` to `long long`, `%o`, `%u`, `%x` and `%X` to `long long` if negative, else
///   `unsigned long long`;
/// - a float conversion's (`%e`, `%E`, `%f`, `%g`, `%G`) argument that isn't finite: C libraries
///   write it each their own way.
///
/// The format string is read as Luau's `str_format` does, skipping flags, width and precision.
/// A `%*` argument is first replaced by what the scripts' `tostring` writes ([`named`]); Luau's
/// would write an address.
///
/// # Safety
///
/// `state` runs a C function.
unsafe fn formatted(state: *mut ffi::lua_State) -> Option<(c_int, String)> {
    // SAFETY: as the caller promises; the format string's bytes live while it's on the stack, and
    // `lua_tolstring` converts nothing of a string.
    unsafe {
        if ffi::lua_type(state, 1) != ffi::LUA_TSTRING {
            return None;
        }
        let mut len = 0;
        let format = ffi::lua_tolstring(state, 1, &mut len);
        let format = std::slice::from_raw_parts(format.cast::<u8>(), len);
        let mut arg = 1;
        let mut i = 0;
        while i < format.len() {
            if format[i] != b'%' {
                i += 1;
                continue;
            }
            i += 1;
            match format.get(i) {
                Some(b'%') => {
                    i += 1;
                    continue;
                }
                Some(b'*') => {
                    i += 1;
                    arg += 1;
                    named(state, arg);
                    continue;
                }
                _ => {}
            }
            arg += 1;
            while format.get(i).is_some_and(|c| b"-+ #0".contains(c)) {
                i += 1;
            }
            let digits = |i: &mut usize| {
                for _ in 0..2 {
                    if format.get(*i).is_some_and(u8::is_ascii_digit) {
                        *i += 1;
                    }
                }
            };
            digits(&mut i);
            if format.get(i) == Some(&b'.') {
                i += 1;
                digits(&mut i);
            }
            let &conversion = format.get(i)?;
            i += 1;
            let Some(n) = number(state, arg, Cast::Long) else {
                continue;
            };
            let defined = match conversion {
                b'c' => Cast::Int.defined(n),
                b'd' | b'i' => Cast::Long.defined(n),
                b'o' | b'u' | b'x' | b'X' => {
                    Cast::Long.defined(n) || (0.0..18_446_744_073_709_551_616.0).contains(&n)
                }
                b'e' | b'E' | b'f' | b'g' | b'G' => {
                    if !n.is_finite() {
                        let what = format!("{n} isn't finite, which C libraries write otherwise");
                        return Some((arg, what));
                    }
                    true
                }
                _ => true,
            };
            if !defined {
                return Some((
                    arg,
                    format!("{n} has no integer that every platform casts it to"),
                ));
            }
        }
        None
    }
}

/// Replaces argument `arg`, if it's a table or userdata without `__tostring`, a function, a
/// thread or a buffer, with what the scripts' `tostring` writes: userdata's metatable's `__type`
/// if a string, else its type's name. Luau's own, which `%*` calls, writes the address;
/// [`super`]'s `written` says why that's replaced.
///
/// # Safety
///
/// `state` runs a C function.
unsafe fn named(state: *mut ffi::lua_State, arg: c_int) {
    // SAFETY: as the caller promises; `arg` is checked to be one of its arguments;
    // `lua_rawcheckstack` makes room for the metafield `luaL_getmetafield` pushes, which is popped
    // or, a `__type` string, takes the argument's place, as does a type name, a static string.
    unsafe {
        if arg > ffi::lua_gettop(state) {
            return;
        }
        ffi::lua_rawcheckstack(state, 1);
        let ty = ffi::lua_type(state, arg);
        let has = |field: &CStr| {
            let found = ffi::luaL_getmetafield(state, arg, field.as_ptr());
            if found != ffi::LUA_TNIL {
                ffi::lua_pop(state, 1);
            }
            found != ffi::LUA_TNIL
        };
        let opaque = match ty {
            ffi::LUA_TTABLE | ffi::LUA_TUSERDATA => !has(c"__tostring"),
            ffi::LUA_TFUNCTION | ffi::LUA_TTHREAD | ffi::LUA_TBUFFER => true,
            _ => false,
        };
        if !opaque {
            return;
        }
        if ty == ffi::LUA_TUSERDATA
            && ffi::luaL_getmetafield(state, arg, c"__type".as_ptr()) != ffi::LUA_TNIL
        {
            if ffi::lua_type(state, -1) == ffi::LUA_TSTRING {
                ffi::lua_replace(state, arg);
                return;
            }
            ffi::lua_pop(state, 1);
        }
        ffi::lua_pushstring(state, ffi::lua_typename(state, ty));
        ffi::lua_replace(state, arg);
    }
}

/// Installs the hooks in `lua`'s VM.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // SAFETY: `exec_raw` runs the closure on a state of `lua`'s VM, whose callbacks every state of
    // the VM shares. mlua sets only the interrupt, through `Lua::set_interrupt`, which the engine
    // never calls.
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

/// Luau's interrupt, at each call and each loop turn: counts against the budget of the call into a
/// script it comes in, failing the call past it, and stops there if a pause is asked
/// ([`super::interrupted`]). The GC's interrupts, during collection, count for nothing.
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
            // SAFETY: as above. The error unwinds to the call into the script, which mlua catches;
            // nothing of Rust's that needs dropping is left in this frame when it's raised.
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

/// Luau's hook at a breakpoint's line: stops there.
#[cfg(feature = "dev")]
unsafe extern "C-unwind" fn debugbreak(state: *mut ffi::lua_State, _: *mut ffi::lua_Debug) {
    // SAFETY: Luau calls the hook with the state that runs, at the breakpoint's instruction.
    unsafe { stop(state, StopReason::Breakpoint) }
}

/// Luau's hook before each instruction while the state steps: stops where the step ends
/// ([`super::stepped`]).
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

/// Stops the script `state` runs here, for `why` ([`super::stopped`]).
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

/// The script and line the running function is at, as a stop or an error names them.
///
/// # Safety
///
/// `state` runs, stopped between two instructions in a hook.
unsafe fn position(state: *mut ffi::lua_State) -> (String, u32) {
    // SAFETY: as the caller promises. `ar` is Luau's to fill; its strings live as long as the
    // running function.
    unsafe {
        let mut ar = std::mem::zeroed::<ffi::lua_Debug>();
        if ffi::lua_getinfo(state, 0, c"sl".as_ptr(), &mut ar) == 0 {
            return (String::new(), 0);
        }
        let script = CStr::from_ptr(ar.short_src).to_string_lossy().into_owned();
        (script, u32::try_from(ar.currentline).unwrap_or(0))
    }
}

/// A function of the stack where a script stopped ([`frames`]).
#[cfg(feature = "dev")]
pub(super) struct Frame {
    pub(super) function: String,
    pub(super) script: String,
    pub(super) line: u32,
    pub(super) locals: Vec<(String, Seen)>,
    pub(super) upvalues: Vec<(String, Seen)>,
}

/// A value a stop reads without calling into Luau (no metamethods).
#[cfg(feature = "dev")]
pub(super) enum Seen {
    Nil,
    Bool(bool),
    Number(f64),
    Vector([f32; 4]),
    /// At most [`gg_api::MOST_SHOWN`] bytes of it.
    String(String),
    /// Light userdata's bits: an entity's, a field handle's or `gg.entity`'s.
    Handle(u64),
    /// Any other kind, by its name.
    Other(&'static str),
}

/// The stack of the script `state` runs, from the running function to the call into the script:
/// Luau functions only, with their locals and upvalues.
///
/// # Safety
///
/// `state` runs, stopped between two instructions in a hook.
#[cfg(feature = "dev")]
unsafe fn frames(state: *mut ffi::lua_State) -> Vec<Frame> {
    let mut frames = Vec::new();
    // SAFETY: as the caller promises. Each value read is pushed, read and popped, a function
    // whose upvalues are read popped after them; `ar`'s strings live as long as their function,
    // which is on the stack.
    unsafe {
        // At most the function and a value.
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

/// The value at the top of the stack, read without calling into Luau.
///
/// # Safety
///
/// `state`'s stack holds a value.
#[cfg(feature = "dev")]
unsafe fn seen(state: *mut ffi::lua_State) -> Seen {
    // SAFETY: as the caller promises. Each read is of the type `lua_type` gives and converts
    // nothing; the string's bytes live while it's on the stack.
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

/// Sets, or clears if not `on`, a breakpoint at `line` of a script's `chunk`, or at the next line
/// with code, in the chunk or its functions. Returns the line it's at, or `None` if there's none.
#[cfg(feature = "dev")]
pub(super) fn breakpoint(lua: &Lua, chunk: &Function, line: u32, on: bool) -> Option<u32> {
    let line = c_int::try_from(line).ok()?;
    let mut placed = -1;
    // SAFETY: `exec_raw` runs the closure with `chunk`, a function Luau compiled, alone on the
    // stack, where `lua_breakpoint` reads it; Luau allows this inside a hook.
    let ran = unsafe {
        lua.exec_raw::<()>(chunk, |state| {
            placed = ffi::lua_breakpoint(state, 1, line, c_int::from(on));
        })
    };
    ran.ok()?;
    u32::try_from(placed).ok()
}

/// The values `function`'s upvalues hold, in order. A C function's are only coroutines and
/// buffers (`coroutine.wrap` holds its coroutine); mlua's functions hold values of its own.
pub(super) fn upvalues(lua: &Lua, function: &Function) -> mlua::Result<Variadic<Value>> {
    // SAFETY: `exec_raw` runs the closure with `function` alone on the stack and returns what the
    // stack holds after: each upvalue `lua_getupvalue` pushed and kept, once the function is
    // removed.
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

/// Sets whether the VM's main thread, which runs the scripts, enters calls into them stepping
/// ([`debugstep`] before each instruction). Luau steps only in a call it entered so.
#[cfg(feature = "dev")]
pub(super) fn singlestep(lua: &Lua, on: bool) {
    // SAFETY: `exec_raw` runs the closure on a state of `lua`'s VM, from which `lua_mainthread`
    // finds the main thread; it runs between calls into scripts.
    let _ = unsafe {
        lua.exec_raw::<()>((), |state| {
            ffi::lua_singlestep(ffi::lua_mainthread(state), c_int::from(on));
        })
    };
}

/// Runs `f`, the engine's side of a hook, aborting the process if it panics (a broken invariant,
/// message printed): a panic would unwind into Luau's C++, which doesn't expect one.
fn guarded<R>(f: impl FnOnce() -> R) -> R {
    panic::catch_unwind(panic::AssertUnwindSafe(f)).unwrap_or_else(|_| process::abort())
}
