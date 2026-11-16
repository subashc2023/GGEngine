//! Compiling a script as the engine has Luau compile it, and what it refuses before.

use gg_core::{Error, Result};
use mlua::chunk::Compiler;

#[cfg(doc)]
use super::Scripts;
use super::host::bad;
use super::{bytecode, hooks};

/// The functions of `math` whose precision is the platform's, which libm's take the place of: the
/// compiler is told they aren't builtins, which it would otherwise call the C library's for.
const LIBM: [&str; 14] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sinh", "cosh", "tanh", "exp", "log",
    "log10", "pow",
];

/// The compiler of scripts: at optimization level 1, whatever a script asks, with libm's math in
/// place of the builtins the C library's would be, the library functions whose integer arguments
/// are checked called as they're checked, and Luau's full debug information, the names of locals
/// and upvalues among it, which the debugger shows, as [`Scripts`] says.
pub(super) fn compiler() -> Compiler {
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
pub(super) fn compiled(path: &str, text: &str) -> Result<Vec<u8>> {
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
pub(super) fn check(text: &str) -> Result<()> {
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
