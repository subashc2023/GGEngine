// Ported from dioxus-cli 0.7.2 (https://github.com/DioxusLabs/dioxus, packages/cli):
// src/build/patch.rs, src/build/request.rs and src/cli/link.rs, the macOS path alone.
// Copyright (c) Jonathan Kelley, the crate's author as its Cargo.toml names him; the crate ships no
// LICENSE file. Licensed under the MIT license or the Apache License, Version 2.0, at your option.

//! `gg run`'s patches built in process on macOS, as `dx tools hotpatch` builds them, without its
//! fixed cost of about 0.35 s a patch: rustc compiles the game's crate as the build recorded it,
//! with gg as its linker ([`link`]), and gg links the patch and makes dx's jump table. After a
//! restart in place, the table also maps that restart's patch's functions by name ([`Restart`]).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use object::write::{MachOBuildVersion, StandardSection, Symbol as Written, SymbolSection};
use object::{
    Architecture, BinaryFormat, Endianness, Object, ObjectSymbol, SymbolFlags, SymbolKind,
    SymbolScope, macho,
};
use serde_json::{Map, Value, json};

use crate::build;
use crate::dx::plain;

/// The environment variable naming the file gg, run as rustc's linker, writes its arguments to.
const LINK_ARGS: &str = "GG_PATCH_LINK_ARGS";

/// The symbol whose address in the executable and in the running process gives the slide. Mach-O's
/// symbol table prefixes C names with `_`.
const MAIN: &str = "_main";

/// An executable's symbol table, by name, as a patch's stub and jump table need it; dx's
/// `HotpatchModuleCache`.
pub(crate) struct Symbols {
    architecture: Architecture,
    table: HashMap<String, Symbol>,
}

struct Symbol {
    address: u64,
    kind: SymbolKind,
    undefined: bool,
    weak: bool,
    size: u64,
    /// Mach-O's `n_desc`, which a data symbol's stub copies.
    n_desc: Option<u16>,
}

impl Symbols {
    /// Reads executable `exe`'s symbols.
    ///
    /// # Errors
    ///
    /// - `exe` can't be read, or isn't an object file `object` reads.
    pub(crate) fn read(exe: &Path) -> Result<Self, String> {
        let bytes =
            fs::read(exe).map_err(|error| format!("can't read {}: {error}", exe.display()))?;
        Self::parse(&bytes)
            .map_err(|error| format!("can't read the symbols of {}: {error}", exe.display()))
    }

    /// A later symbol of a name replaces an earlier one, as in dx's cache.
    fn parse(bytes: &[u8]) -> object::Result<Self> {
        let file = object::File::parse(bytes)?;
        let mut table = HashMap::new();
        for symbol in file.symbols() {
            let Ok(name) = symbol.name() else {
                continue;
            };
            let n_desc = match symbol.flags() {
                SymbolFlags::MachO { n_desc } => Some(n_desc),
                _ => None,
            };
            let symbol = Symbol {
                address: symbol.address(),
                kind: symbol.kind(),
                undefined: symbol.is_undefined(),
                weak: symbol.is_weak(),
                size: symbol.size(),
                n_desc,
            };
            table.insert(name.to_owned(), symbol);
        }
        Ok(Self {
            architecture: file.architecture(),
            table,
        })
    }
}

/// The patch library of the last restart in place, as the process loaded it. The layers that
/// restart built call `HotFn` with its functions' addresses, which the executable lacks for a
/// layer type the restart added. So each later patch maps them too, by name ([`jump_table`]).
#[derive(Clone)]
pub(crate) struct Restart {
    lib: PathBuf,
    /// How far the process loaded `lib` from its link's addresses, modulo 2^64
    /// (`gg_api::Restarted::loaded`).
    loaded: u64,
    /// `lib`'s functions by name, at their link's addresses: read by the first patch that needs
    /// them, then kept.
    functions: Arc<OnceLock<Result<HashMap<String, u64>, String>>>,
}

impl Restart {
    /// The restart whose patch is `lib`, which the process loaded `loaded` from its link's
    /// addresses.
    pub(crate) fn new(lib: PathBuf, loaded: u64) -> Self {
        Self {
            lib,
            loaded,
            functions: Arc::default(),
        }
    }

    fn functions(&self) -> Result<&HashMap<String, u64>, String> {
        let read = self.functions.get_or_init(|| {
            let lib = &self.lib;
            let bytes =
                fs::read(lib).map_err(|error| format!("can't read {}: {error}", lib.display()))?;
            functions(&bytes)
                .map_err(|error| format!("can't read the symbols of {}: {error}", lib.display()))
        });
        read.as_ref().map_err(|error| {
            format!("can't map the functions of the last restart in place's patch: {error}")
        })
    }
}

/// The functions object file `bytes` defines, by name, at their addresses in it.
fn functions(bytes: &[u8]) -> object::Result<HashMap<String, u64>> {
    let file = object::File::parse(bytes)?;
    let functions = (file.symbols())
        .filter(|symbol| symbol.kind() == SymbolKind::Text && symbol.is_definition())
        .filter_map(|symbol| Some((symbol.name().ok()?.to_owned(), symbol.address())));
    Ok(functions.collect())
}

/// Why a patch didn't build.
pub(crate) enum Failed {
    /// rustc failed, with its errors, plain, each once.
    Rustc(ExitStatus, Vec<String>),
    /// Anything else, as a message.
    Other(String),
}

impl From<String> for Failed {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

/// What a patch builds from: a build and where its pieces go.
pub(crate) struct Build<'a> {
    /// dx's `BuildsFinished` client record, as JSON. It holds the build's environment, which may
    /// hold secrets: given to rustc and the linker alone, never in a message.
    pub(crate) client: &'a str,
    pub(crate) exe: &'a Path,
    pub(crate) symbols: &'a Symbols,
    /// The last restart in place, while the process runs the layers it built.
    pub(crate) restart: Option<&'a Restart>,
    /// The workspace's root, where cargo ran rustc.
    pub(crate) root: &'a Path,
    /// Where rustc's link arguments are written: a file gg alone writes and reads.
    pub(crate) link_args: &'a Path,
}

/// Builds a patch of the game's crate as it is now, for a process whose `main` is at
/// `aslr_reference`; returns dx's jump table as JSON, which `subsecond::apply_patch` takes.
///
/// # Errors
///
/// - The build's record doesn't read.
/// - rustc fails, or doesn't run the linker.
/// - The stub, the link or the jump table fails.
/// - The last restart in place's patch can't be read.
pub(crate) fn hotpatch(build: &Build, aslr_reference: u64) -> Result<Value, Failed> {
    let started = SystemTime::now();
    let client: Value = serde_json::from_str(build.client)
        .map_err(|error| format!("dx's record of the build doesn't read: {error}"))?;
    let rustc_args = strings(&client["rustc_args"]);
    let envs: Vec<(String, String)> = (client["rustc_envs"].as_array().into_iter().flatten())
        .filter_map(|pair| Some((pair[0].as_str()?.to_owned(), pair[1].as_str()?.to_owned())))
        .collect();
    let Some((rustc, args)) = rustc_args.split_first() else {
        return Err(Failed::Other(
            "dx's record of the build has no rustc command".to_owned(),
        ));
    };
    let gg = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|error| format!("can't find gg's own executable, rustc's linker: {error}"))?;
    let _ = fs::remove_file(build.link_args);

    // As dx runs it: cargo's environment alone, its arguments, gg as the linker. cargo ran rustc
    // by its path, as here, which skips rustup's proxy.
    let mut command = Command::new(rustc);
    command
        .args(args)
        .arg(format!("-Clinker={}", gg.display()))
        .current_dir(build.root)
        .env_clear()
        .envs(envs.iter().map(|(key, value)| (key, value)))
        .env(LINK_ARGS, build.link_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("can't run rustc ({rustc}): {error}"))?;
    let errors = errors(child.stderr.take().expect("piped"));
    let status = (child.wait()).map_err(|error| format!("can't run rustc ({rustc}): {error}"))?;
    if !status.success() {
        return Err(Failed::Rustc(status, errors));
    }
    let linked = fs::read_to_string(build.link_args)
        .map_err(|error| format!("rustc built the patch's crate but never linked it: {error}"))?;
    let linked: Vec<String> = linked.split('\n').map(str::to_owned).collect();

    let mut objects: Vec<PathBuf> = (linked.iter())
        .filter(|arg| arg.ends_with(".rcgu.o"))
        .map(PathBuf::from)
        .collect();
    objects.sort();
    // dx's bundle, where dx puts a patch too: a dylib written beside cargo's executable takes
    // macOS about 0.15 s more to load.
    let bundle = Path::new(client["path"].as_str().unwrap_or_default());
    let dir = match build.exe.parent() {
        _ if bundle.is_dir() => bundle.join("Contents").join("MacOS"),
        Some(dir) => dir.to_owned(),
        None => PathBuf::new(),
    };
    let stub = stub(build.symbols, &undefined(&objects)?, aslr_reference)?;
    let stub_file = dir.join("stub.o");
    fs::write(&stub_file, stub)
        .map_err(|error| format!("can't write {}: {error}", stub_file.display()))?;
    objects.push(stub_file);
    // Libraries the game ships beside it, from the bundle, not the target directory.
    let frameworks = bundle.join("Contents").join("Frameworks");
    let dylibs = (strings(&client["link_args"]).into_iter())
        .filter(|arg| arg.ends_with(".dylib") || arg.ends_with(".so"))
        .filter_map(|arg| Some(frameworks.join(Path::new(&arg).file_name()?)));

    let name = build.exe.file_name().unwrap_or_default().to_string_lossy();
    let ms = started
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let lib = dir.join(format!("lib{name}-patch-{ms}.dylib"));
    let output = Command::new("cc")
        .args(&objects)
        .args(dylibs)
        .args(thin_link_args(&linked))
        .arg("-o")
        .arg(&lib)
        .env_clear()
        .envs(envs.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("can't run cc, the patch's linker: {error}"));
    // rustc's output, which gg wrote as its linker; and the objects, which the next patch's rustc
    // writes again, with the bitcode `-Csave-temps` writes beside each, which dx leaves to pile up.
    // dx found that a load of a later patch fails while rustc's output stays.
    if let Some(at) = linked.iter().position(|arg| arg == "-o")
        && let Some(out) = linked.get(at + 1)
    {
        let _ = fs::remove_file(out);
    }
    for object in &objects {
        let _ = fs::remove_file(object);
        for bitcode in ["bc", "no-opt.bc"] {
            let _ = fs::remove_file(object.with_extension(bitcode));
        }
    }
    let output = output?;
    if !output.status.success() || !lib.exists() {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = plain(&said);
        return Err(Failed::Other(format!(
            "`cc`'s link of patch {} failed ({}):\n\n{}",
            lib.display(),
            output.status,
            build::fit(&said, 2_000)
        )));
    }
    let bytes = fs::read(&lib).map_err(|error| format!("can't read {}: {error}", lib.display()))?;
    let restart = (build.restart)
        .map(|restart| Ok::<_, String>((restart.functions()?, restart.loaded)))
        .transpose()?;
    let table = jump_table(&bytes, &lib, build.symbols, restart, aslr_reference)?;
    Ok(table)
}

/// The strings of JSON array `value`; none if it isn't one.
fn strings(value: &Value) -> Vec<String> {
    (value.as_array().into_iter().flatten())
        .filter_map(|arg| Some(arg.as_str()?.to_owned()))
        .collect()
}

/// rustc's errors on `stderr`, plain, each once, but its summary: JSON diagnostics, as cargo's
/// arguments ask for, and text from a line starting `error` on, as an internal compiler error's.
fn errors(stderr: impl Read) -> Vec<String> {
    let mut errors = Vec::new();
    let mut text: Option<String> = None;
    for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
        let line = String::from_utf8_lossy(&line);
        let error = match serde_json::from_str::<Value>(&line) {
            Ok(message) => {
                let level = message["level"].as_str().unwrap_or_default();
                let rendered = message["rendered"].as_str().or(message["message"].as_str());
                match rendered {
                    Some(rendered) if level.starts_with("error") => plain(rendered),
                    _ => continue,
                }
            }
            Err(_) => {
                let line = plain(&line);
                match &mut text {
                    Some(text) => text.extend(["\n", line.as_str()]),
                    None if line.trim_start().starts_with("error") => text = Some(line),
                    None => {}
                }
                continue;
            }
        };
        if !build::summary(&error) && !errors.contains(&error) {
            errors.push(error);
        }
    }
    errors.extend(text);
    errors
}

/// The symbols `objects` leave undefined, sorted.
///
/// # Errors
///
/// - An object can't be read.
fn undefined(objects: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut undefined = HashSet::new();
    let mut defined = HashSet::new();
    for path in objects {
        let failed =
            |error: &dyn std::fmt::Display| format!("can't read {}: {error}", path.display());
        let bytes = fs::read(path).map_err(|error| failed(&error))?;
        let file = object::File::parse(&*bytes).map_err(|error| failed(&error))?;
        for symbol in file.symbols() {
            let name = symbol.name().map_err(|error| failed(&error))?;
            if symbol.is_undefined() {
                undefined.insert(name.to_owned());
            } else if symbol.is_global() {
                defined.insert(name.to_owned());
            }
        }
    }
    let mut undefined: Vec<String> = undefined.difference(&defined).cloned().collect();
    // dx's order is a hash set's; sorted, two builds of a patch lay out alike.
    undefined.sort();
    Ok(undefined)
}

/// An object file defining each of `undefined` that the executable defines, at its address in a
/// process whose `main` is at `aslr_reference`: dx's `create_undefined_symbol_stub`. A function is
/// a jump to it; a thread-local, its initializer's address; anything else, an absolute symbol.
///
/// # Errors
///
/// - The executable has no `_main`, or `aslr_reference` is below it.
/// - The executable is for neither ARM64 nor x86-64.
fn stub(symbols: &Symbols, undefined: &[String], aslr_reference: u64) -> Result<Vec<u8>, String> {
    let architecture = symbols.architecture;
    if !matches!(architecture, Architecture::Aarch64 | Architecture::X86_64) {
        return Err(format!(
            "patches are built for ARM64 and x86-64, not {architecture:?}"
        ));
    }
    let mut obj = object::write::Object::new(BinaryFormat::MachO, architecture, Endianness::Little);
    let mut version = MachOBuildVersion::default();
    version.platform = macho::PLATFORM_MACOS;
    // 11.0.0, as `xxxx.yy.zz` in nibbles.
    (version.minos, version.sdk) = (11 << 16, 11 << 16);
    obj.set_macho_build_version(version);
    let main = (symbols.table.get(MAIN))
        .ok_or("the build has no `_main` symbol")?
        .address;
    if aslr_reference < main {
        return Err(format!(
            "the process's `main` is at {aslr_reference:#x}, below the build's, {main:#x}"
        ));
    }
    let slide = aslr_reference - main;
    let text = obj.section_id(StandardSection::Text);
    for name in undefined {
        // Imports have no address until dyld binds them.
        let Some(symbol) = symbols.table.get(name).filter(|symbol| !symbol.undefined) else {
            continue;
        };
        // The writer prefixes the `_` that the objects' names carry.
        let unprefixed = name.get(1..).unwrap_or_default().as_bytes().to_vec();
        let address = symbol.address + slide;
        match symbol.kind {
            SymbolKind::Text => {
                let jump = match architecture {
                    // `jmp [rip+0]`, then the address.
                    Architecture::X86_64 => {
                        [&[0xFF, 0x25, 0, 0, 0, 0][..], &address.to_le_bytes()].concat()
                    }
                    // `ldr x16, #8; br x16`, then the address.
                    _ => [
                        &[0x50, 0, 0, 0x58, 0x00, 0x02, 0x1F, 0xD6][..],
                        &address.to_le_bytes(),
                    ]
                    .concat(),
                };
                let at = obj.append_section_data(text, &jump, 8);
                obj.add_symbol(Written {
                    name: unprefixed,
                    value: at,
                    size: jump.len() as u64,
                    kind: SymbolKind::Text,
                    scope: SymbolScope::Linkage,
                    weak: false,
                    section: SymbolSection::Section(text),
                    flags: SymbolFlags::None,
                });
            }
            // Rust's thread-local accessors read the initializer's address: one already run
            // isn't run again, and one a patch adds is.
            SymbolKind::Tls => {
                let tls = obj.section_id(StandardSection::Tls);
                let size = if symbol.size == 0 { 8 } else { symbol.size };
                let mut init = vec![0; usize::try_from(size).unwrap_or(8)];
                for (byte, value) in init.iter_mut().zip(address.to_le_bytes()) {
                    *byte = value;
                }
                let at = obj.append_section_data(tls, &init, size.min(8).next_power_of_two());
                obj.add_symbol(Written {
                    name: unprefixed,
                    value: at,
                    size,
                    kind: SymbolKind::Tls,
                    scope: SymbolScope::Linkage,
                    weak: false,
                    section: SymbolSection::Section(tls),
                    flags: SymbolFlags::None,
                });
            }
            kind => {
                obj.add_symbol(Written {
                    name: unprefixed,
                    value: address,
                    size: 0,
                    // Mach-O's statics read as unknown.
                    kind: if kind == SymbolKind::Unknown {
                        SymbolKind::Data
                    } else {
                        kind
                    },
                    scope: SymbolScope::Linkage,
                    weak: symbol.weak,
                    section: SymbolSection::Absolute,
                    flags: symbol
                        .n_desc
                        .map_or(SymbolFlags::None, |n_desc| SymbolFlags::MachO { n_desc }),
                });
            }
        }
    }
    obj.write()
        .map_err(|error| format!("can't write the patch's stub: {error}"))
}

/// The arguments of the patch's link that come from rustc's link of the executable, `linked`:
/// dx's `thin_link_args` for the Darwin flavor. The patch is a dylib; frameworks, libraries,
/// search paths and the target carry over; the archives don't, as the executable defines them.
fn thin_link_args(linked: &[String]) -> Vec<String> {
    let mut args = vec!["-Wl,-dylib".to_owned()];
    for (at, arg) in linked.iter().enumerate() {
        let valued = ["-framework", "-arch", "-L", "-target", "-isysroot"];
        if valued.contains(&arg.as_str())
            && let Some(value) = linked.get(at + 1)
        {
            args.extend([arg.clone(), value.clone()]);
        }
        if arg.starts_with("-l") || arg.starts_with("-m") || arg.starts_with("-nodefaultlibs") {
            args.push(arg.clone());
        }
    }
    // dx repeats these last.
    for flag in ["-target", "-isysroot"] {
        if let Some(at) = linked.iter().position(|arg| arg == flag)
            && let Some(value) = linked.get(at + 1)
        {
            args.extend([flag.to_owned(), value.clone()]);
        }
    }
    args
}

/// Patch `bytes`'s jump table, at path `lib`, for a process whose `main` is at `aslr_reference`:
/// each of the executable's addresses to the patch's of the same name, as dx's
/// `create_native_jump_table` makes it.
///
/// With `restart`, the last restart in place's functions by name and how far the process loaded
/// its patch, it maps where the process has those too. subsecond adds the executable's slide to
/// every key, so they go in less it. A key the executable gives stays.
///
/// # Errors
///
/// - The patch doesn't read, or it or the executable has no `_main`.
fn jump_table(
    bytes: &[u8],
    lib: &Path,
    symbols: &Symbols,
    restart: Option<(&HashMap<String, u64>, u64)>,
    aslr_reference: u64,
) -> Result<Value, String> {
    let file = object::File::parse(bytes)
        .map_err(|error| format!("can't read patch {}: {error}", lib.display()))?;
    let patched = file.symbol_map();
    let patched: HashMap<&str, u64> = (patched.symbols().iter())
        .map(|symbol| (symbol.name(), symbol.address()))
        .collect();
    let mut map = Map::new();
    for (name, address) in &patched {
        if let Some(old) = symbols.table.get(*name) {
            map.insert(old.address.to_string(), json!(address));
        }
    }
    let new_base = *patched
        .get(MAIN)
        .ok_or_else(|| format!("patch {} has no `_main` symbol", lib.display()))?;
    let base = symbols
        .table
        .get(MAIN)
        .ok_or("the build has no `_main` symbol")?;
    if let Some((functions, loaded)) = restart {
        let slide = aslr_reference.wrapping_sub(base.address);
        let moved = loaded.wrapping_sub(slide);
        for (name, address) in &patched {
            if let Some(old) = functions.get(*name) {
                let key = old.wrapping_add(moved).to_string();
                map.entry(key).or_insert_with(|| json!(address));
            }
        }
    }
    Ok(json!({
        "lib": lib,
        "map": map,
        "aslr_reference": base.address,
        "new_base_address": new_base,
        "ifunc_count": 0,
    }))
}

/// As rustc's linker for a patch, when [`LINK_ARGS`] names a file: writes the arguments there, one
/// a line, and an empty object file where the executable goes, for rustc's steps after a link.
/// gg links the patch itself, from the arguments. `None` if gg isn't run so.
pub(crate) fn link() -> Option<ExitCode> {
    let file = std::env::var_os(LINK_ARGS)?;
    let args = std::env::args_os().map(|arg| arg.to_string_lossy().into_owned());
    match linked(Path::new(&file), expand(args.collect())) {
        Ok(()) => Some(ExitCode::SUCCESS),
        Err(error) => {
            // rustc shows its linker's stderr in the error it gives.
            let _ = writeln!(io::stderr(), "gg, as the linker of a patch: {error}");
            Some(ExitCode::FAILURE)
        }
    }
}

fn linked(file: &Path, args: Vec<String>) -> io::Result<()> {
    fs::write(file, args.join("\n"))?;
    let out = output(&args)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no `-o` among them"))?;
    // Mach-O for the host, as rustc expects of a link on macOS.
    let architecture = if cfg!(target_arch = "x86_64") {
        Architecture::X86_64
    } else {
        Architecture::Aarch64
    };
    let empty = object::write::Object::new(BinaryFormat::MachO, architecture, Endianness::Little);
    let bytes = empty.write().map_err(io::Error::other)?;
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(out, bytes)
}

/// The path after `-o` in `args`.
fn output(args: &[String]) -> Option<&Path> {
    let at = args.iter().position(|arg| arg == "-o")?;
    args.get(at + 1).map(Path::new)
}

/// `args` with each `@file` replaced by the arguments it holds, one a line, as rustc writes them
/// when a command line is too long: a backslash escapes the character after it. A file that can't
/// be read stays as it is.
fn expand(args: Vec<String>) -> Vec<String> {
    let mut expanded = Vec::with_capacity(args.len());
    for arg in args {
        let Some(read) = arg
            .strip_prefix('@')
            .and_then(|file| fs::read_to_string(file).ok())
        else {
            expanded.push(arg);
            continue;
        };
        for line in read.lines() {
            let mut unescaped = String::with_capacity(line.len());
            let mut chars = line.chars();
            while let Some(c) = chars.next() {
                unescaped.push(if c == '\\' {
                    chars.next().unwrap_or(c)
                } else {
                    c
                });
            }
            expanded.push(unescaped);
        }
    }
    expanded
}

#[cfg(test)]
mod tests {
    use object::ObjectSection;
    use object::write::Object as Writer;

    use super::*;

    fn macho() -> Writer<'static> {
        Writer::new(
            BinaryFormat::MachO,
            Architecture::Aarch64,
            Endianness::Little,
        )
    }

    /// A symbol `name` (unprefixed) of `kind` at `value` in `section`.
    fn symbol(name: &str, value: u64, kind: SymbolKind, section: SymbolSection) -> Written {
        Written {
            name: name.as_bytes().to_vec(),
            value,
            size: 0,
            kind,
            scope: SymbolScope::Dynamic,
            weak: false,
            section,
            flags: SymbolFlags::None,
        }
    }

    /// An object file defining `_main` and `_grow` in code and `_POLLEN` in data, and
    /// referencing `_malloc`.
    fn executable() -> Vec<u8> {
        let mut obj = macho();
        let text = obj.section_id(StandardSection::Text);
        let data = obj.section_id(StandardSection::Data);
        obj.append_section_data(text, &[0; 64], 4);
        obj.append_section_data(data, &[0; 16], 8);
        obj.add_symbol(symbol(
            "main",
            0,
            SymbolKind::Text,
            SymbolSection::Section(text),
        ));
        obj.add_symbol(symbol(
            "grow",
            32,
            SymbolKind::Text,
            SymbolSection::Section(text),
        ));
        let mut pollen = symbol("POLLEN", 8, SymbolKind::Data, SymbolSection::Section(data));
        pollen.weak = true;
        obj.add_symbol(pollen);
        obj.add_symbol(symbol(
            "malloc",
            0,
            SymbolKind::Text,
            SymbolSection::Undefined,
        ));
        obj.write().expect("written")
    }

    #[test]
    fn an_executables_symbols_are_read_by_name() {
        let symbols = Symbols::parse(&executable()).expect("read");
        assert_eq!(symbols.architecture, Architecture::Aarch64);
        let main = &symbols.table["_main"];
        let grow = &symbols.table["_grow"];
        assert_eq!((main.kind, main.undefined), (SymbolKind::Text, false));
        assert_eq!(grow.address - main.address, 32);
        let pollen = &symbols.table["_POLLEN"];
        assert!(pollen.weak && !pollen.undefined);
        assert!(pollen.n_desc.is_some());
        assert!(symbols.table["_malloc"].undefined);
        assert!(Symbols::parse(b"not an object file").is_err());
    }

    /// Symbols as an executable at 0x1_0000_0000 has them.
    fn symbols() -> Symbols {
        let symbol = |address, kind, undefined, size| Symbol {
            address,
            kind,
            undefined,
            weak: false,
            size,
            n_desc: Some(0),
        };
        let table = [
            ("_main", symbol(0x1_0000_1000, SymbolKind::Text, false, 0)),
            ("_grow", symbol(0x1_0000_2000, SymbolKind::Text, false, 0)),
            (
                "_POLLEN",
                symbol(0x1_0000_8000, SymbolKind::Unknown, false, 0),
            ),
            ("_TICKS", symbol(0x1_0000_9000, SymbolKind::Tls, false, 24)),
            ("_malloc", symbol(0, SymbolKind::Text, true, 0)),
        ];
        Symbols {
            architecture: Architecture::Aarch64,
            table: table
                .into_iter()
                .map(|(name, symbol)| (name.to_owned(), symbol))
                .collect(),
        }
    }

    #[test]
    fn the_stub_defines_the_executables_symbols_where_the_process_has_them() {
        let symbols = symbols();
        let slide = 0x4000;
        let undefined = ["_POLLEN", "_TICKS", "_grow", "_malloc", "_nowhere"].map(str::to_owned);
        let built = stub(&symbols, &undefined, 0x1_0000_1000 + slide).expect("a stub");
        let file = object::File::parse(&*built).expect("an object file");
        let defined: HashMap<String, (u64, SymbolKind, bool)> = (file.symbols())
            .filter(|symbol| !symbol.is_undefined() && symbol.kind() != SymbolKind::Section)
            .map(|symbol| {
                let name = symbol.name().expect("a name").to_owned();
                let absolute = symbol.section() == object::SymbolSection::Absolute;
                (name, (symbol.address(), symbol.kind(), absolute))
            })
            .collect();
        // An import and a name the executable lacks get nothing.
        let mut names: Vec<&str> = defined.keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["_POLLEN", "_TICKS", "_grow"]);
        // A static is absolute; Mach-O reads an absolute symbol's kind as unknown.
        assert_eq!(
            defined["_POLLEN"],
            (0x1_0000_8000 + slide, SymbolKind::Unknown, true)
        );
        assert_eq!(defined["_TICKS"].1, SymbolKind::Tls);
        // A function is a jump to its address.
        let (at, kind, absolute) = defined["_grow"];
        assert_eq!((kind, absolute), (SymbolKind::Text, false));
        let text = file.section_by_name("__text").expect("code");
        let code = text.data().expect("bytes");
        let jump = &code[usize::try_from(at - text.address()).expect("small")..][..16];
        let want = [
            &[0x50, 0, 0, 0x58, 0x00, 0x02, 0x1F, 0xD6][..],
            &(0x1_0000_2000u64 + slide).to_le_bytes(),
        ]
        .concat();
        assert_eq!(jump, want);
        // A process can't load below the build's addresses.
        assert!(stub(&symbols, &undefined, 0x1000).is_err());
        let mut no_main = symbols;
        no_main.table.remove("_main");
        assert!(stub(&no_main, &undefined, 0x1_0000_1000).is_err());
    }

    #[test]
    fn the_undefined_symbols_are_those_no_object_defines() {
        let dir =
            std::env::temp_dir().join(format!("gg-hotpatch-undefined-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("a directory");
        let mut one = macho();
        let text = one.section_id(StandardSection::Text);
        one.append_section_data(text, &[0; 8], 4);
        one.add_symbol(symbol(
            "grow",
            0,
            SymbolKind::Text,
            SymbolSection::Section(text),
        ));
        one.add_symbol(symbol(
            "bloom",
            0,
            SymbolKind::Text,
            SymbolSection::Undefined,
        ));
        let mut two = macho();
        two.add_symbol(symbol(
            "grow",
            0,
            SymbolKind::Text,
            SymbolSection::Undefined,
        ));
        two.add_symbol(symbol(
            "alloc",
            0,
            SymbolKind::Text,
            SymbolSection::Undefined,
        ));
        let paths = [dir.join("a.rcgu.o"), dir.join("b.rcgu.o")];
        for (path, obj) in paths.iter().zip([one, two]) {
            fs::write(path, obj.write().expect("written")).expect("written");
        }
        let found = undefined(&paths);
        let missing = undefined(&[dir.join("none.o")]);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(found.expect("read"), ["_alloc", "_bloom"]);
        assert!(missing.is_err());
    }

    #[test]
    fn the_patchs_link_keeps_rustcs_frameworks_libraries_paths_and_target() {
        let linked = [
            "/gg/target/debug/gg",
            "/t/rustcrtASBD/symbols.o",
            "/t/deps/gg_hive-b835.07cs.rcgu.o",
            "/t/deps/libgg_engine-e1d6.rlib",
            "-framework",
            "AppKit",
            "-lc++",
            "-arch",
            "arm64",
            "-mmacosx-version-min=11.0.0",
            "-L",
            "/t/build/zstd-sys/out",
            "-o",
            "/t/deps/gg_hive-b835",
            "-nodefaultlibs",
            "-Wl,-rpath,@executable_path",
            "-target",
            "arm64-apple-macosx",
        ]
        .map(str::to_owned);
        let want = [
            "-Wl,-dylib",
            "-framework",
            "AppKit",
            "-lc++",
            "-arch",
            "arm64",
            "-mmacosx-version-min=11.0.0",
            "-L",
            "/t/build/zstd-sys/out",
            "-nodefaultlibs",
            "-target",
            "arm64-apple-macosx",
            "-target",
            "arm64-apple-macosx",
        ];
        assert_eq!(thin_link_args(&linked), want);
        // A flag missing its value is dropped.
        assert_eq!(thin_link_args(&["-framework".to_owned()]), ["-Wl,-dylib"]);
    }

    #[test]
    fn the_jump_table_maps_the_executables_addresses_to_the_patchs() {
        let mut obj = macho();
        let text = obj.section_id(StandardSection::Text);
        obj.append_section_data(text, &[0; 64], 4);
        obj.add_symbol(symbol(
            "main",
            16,
            SymbolKind::Text,
            SymbolSection::Section(text),
        ));
        obj.add_symbol(symbol(
            "grow",
            48,
            SymbolKind::Text,
            SymbolSection::Section(text),
        ));
        obj.add_symbol(symbol(
            "added",
            32,
            SymbolKind::Text,
            SymbolSection::Section(text),
        ));
        let lib = Path::new("/t/libgg-hive-patch-1.dylib");
        let written = obj.write().expect("written");
        let table = jump_table(&written, lib, &symbols(), None, 0).expect("a table");
        assert_eq!(table["lib"], "/t/libgg-hive-patch-1.dylib");
        assert_eq!(table["aslr_reference"], 0x1_0000_1000u64);
        assert_eq!(table["new_base_address"], 16);
        assert_eq!(table["ifunc_count"], 0);
        let map = table["map"].as_object().expect("a map");
        assert_eq!(map.len(), 2);
        assert_eq!(map[&0x1_0000_1000u64.to_string()], 16);
        assert_eq!(map[&0x1_0000_2000u64.to_string()], 48);
        let mut no_main = symbols();
        no_main.table.remove("_main");
        assert!(jump_table(&written, lib, &no_main, None, 0).is_err());
    }

    /// A library defining `_main`, `_grow` and `_hatch` in code from `base` on, `_POLLEN` in data.
    fn library(base: u64) -> Vec<u8> {
        let mut obj = macho();
        let text = obj.section_id(StandardSection::Text);
        let data = obj.section_id(StandardSection::Data);
        obj.append_section_data(text, &[0; 256], 4);
        obj.append_section_data(data, &[0; 16], 8);
        for (name, at) in [("main", 0), ("grow", 16), ("hatch", 32)] {
            let section = SymbolSection::Section(text);
            obj.add_symbol(symbol(name, base + at, SymbolKind::Text, section));
        }
        let pollen = symbol("POLLEN", 0, SymbolKind::Data, SymbolSection::Section(data));
        obj.add_symbol(pollen);
        obj.write().expect("written")
    }

    /// After a restart in place, a patch maps the restart's functions by name, `_hatch` (a layer's
    /// the executable lacks) too, at where the process has them less the executable's slide. Its
    /// data doesn't map, and the executable's keys stay.
    #[test]
    fn a_patch_maps_the_last_restarts_functions_by_name() {
        let restarted = functions(&library(0x40)).expect("read");
        assert_eq!(restarted.len(), 3);
        assert_eq!((restarted["_main"], restarted["_hatch"]), (0x40, 0x60));
        assert!(!restarted.contains_key("_POLLEN"));
        let lib = Path::new("/t/libgg-hive-patch-2.dylib");
        // The executable slid by 0x4000; the restart's library loaded at 0x3_0000_0000.
        let (slide, loaded) = (0x4000, 0x3_0000_0000);
        let reference = 0x1_0000_1000 + slide;
        let restart = Some((&restarted, loaded));
        let table =
            jump_table(&library(0x80), lib, &symbols(), restart, reference).expect("a table");
        let map = table["map"].as_object().expect("a map");
        let key = |at: u64| (at + loaded - slide).to_string();
        assert_eq!(map[&key(0x60)], 0xa0);
        assert_eq!(map[&key(0x50)], 0x90);
        assert_eq!(map[&key(0x40)], 0x80);
        assert_eq!(map[&0x1_0000_2000u64.to_string()], 0x90);
        // `_main`, `_grow` and `_POLLEN` from the executable; the restart's three functions.
        assert_eq!(map.len(), 6);
    }

    #[test]
    fn rustcs_errors_are_its_json_errors_and_text_from_an_error_on() {
        let diagnostic = |level: &str, rendered: &str| {
            json!({"$message_type": "diagnostic", "message": "m", "level": level,
                "rendered": rendered})
            .to_string()
        };
        let stderr = [
            diagnostic("warning", "warning: unused variable"),
            diagnostic(
                "error",
                "\u{1b}[1merror[E0308]\u{1b}[0m: mismatched types\n",
            ),
            diagnostic("error", "error[E0308]: mismatched types"),
            json!({"artifact": "/t/deps/gg_hive-b835", "emit": "link"}).to_string(),
            diagnostic("error", "error: aborting due to 1 previous error"),
            "note: a note".to_owned(),
            "error: internal compiler error: unexpected panic".to_owned(),
            "thread 'rustc' panicked".to_owned(),
        ]
        .join("\n");
        let errors = errors(stderr.as_bytes());
        assert_eq!(
            errors,
            [
                "error[E0308]: mismatched types",
                "error: internal compiler error: unexpected panic\nthread 'rustc' panicked",
            ]
        );
    }

    #[test]
    fn as_the_linker_gg_writes_its_arguments_and_an_object_where_rustc_expects_one() {
        let dir = std::env::temp_dir().join(format!("gg-hotpatch-link-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("a directory");
        let file = dir.join("link_args.txt");
        let command = dir.join("linker-args");
        fs::write(&command, "/t/a\\ b.rcgu.o\n-o\n").expect("written");
        let out = dir.join("deps").join("gg_hive-b835");
        let args = expand(vec![
            "gg".to_owned(),
            format!("@{}", command.display()),
            out.display().to_string(),
            "@/no/such/file".to_owned(),
        ]);
        let written = linked(&file, args.clone()).map(|()| {
            let said = fs::read_to_string(&file).expect("written");
            let object = fs::read(&out).expect("written");
            (
                said,
                object::File::parse(&*object).map(|file| file.format()),
            )
        });
        let unlinked = linked(&file, vec!["gg".to_owned()]);
        let _ = fs::remove_dir_all(&dir);
        let want = [
            "gg",
            "/t/a b.rcgu.o",
            "-o",
            &out.display().to_string(),
            "@/no/such/file",
        ];
        assert_eq!(args, want);
        let (said, format) = written.expect("linked");
        assert_eq!(said, want.join("\n"));
        assert_eq!(format.ok(), Some(BinaryFormat::MachO));
        assert!(unlinked.is_err());
    }
}
