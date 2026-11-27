//! Dioxus's `dx`, for `gg run`'s patches: checking its release, building the game to take patches,
//! printing progress, building patches, and mending their thunks on Windows on x86-64.

use std::collections::HashSet;
use std::fs::{self, File, TryLockError};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use gg_api::Failure;
use serde_json::{Value, json};

use crate::build::{self, Package};
use crate::run::{event, millis};

/// The most often a build prints crate progress; a new stage prints at once.
const PROGRESS: Duration = Duration::from_secs(5);

/// How dx builds a game's package, with a session cache per package, which patches build from.
#[derive(Clone)]
pub(crate) struct Dx {
    package: String,
    bin: String,
    root: PathBuf,
    cache: PathBuf,
    /// Held across each build, by every package's in the target directory ([`Dx::hold`]).
    lock: PathBuf,
}

/// A build patches are built against: its executable, and `BuildsFinished`'s `client` as JSON,
/// which `dx tools hotpatch` reads. It holds the build's environment, which may hold secrets (API
/// keys): kept in memory, given to dx alone, never printed, logged or written; no `Debug`. dx also
/// writes it into its session cache; [`Dx::build`] removes that.
#[derive(Clone)]
pub(crate) struct Fat {
    pub(crate) exe: PathBuf,
    client: String,
    /// How long the build waited for the lock of dx's builds ([`Dx::hold`]): not part of its time.
    pub(crate) queued: Duration,
}

impl Fat {
    /// A build without patches, as cargo makes: its executable alone.
    pub(crate) fn unpatchable(exe: PathBuf) -> Self {
        Self {
            exe,
            client: String::new(),
            queued: Duration::ZERO,
        }
    }
}

impl Dx {
    /// dx's build of package `name`.
    pub(crate) fn new(name: &str, package: &Package) -> Self {
        Self {
            package: name.to_owned(),
            bin: package.bin.clone(),
            root: package.root.clone(),
            cache: package.target.join("gg").join("dx").join(name),
            lock: package.target.join("gg").join("dx.lock"),
        }
    }

    /// Builds with `dx build --fat-binary`, holding the target directory's builds ([`hold`]),
    /// printing `building` as [`Progress`] says, and [`forget`](Self::forget)s the environment dx
    /// wrote, success or not. After a build, [`prune`](Self::prune)s dx's archives, printing
    /// `pruned` if it removed any.
    ///
    /// Fails with `gg.build` if the lock can't be taken, dx can't run, the build fails, or no
    /// executable results (dx's own link failed).
    ///
    /// [`hold`]: Self::hold
    pub(crate) fn build(&self) -> Result<Fat, Failure> {
        let (_held, queued) = self.hold()?;
        let started = SystemTime::now();
        // dx makes it too, but says less if it can't.
        let _ = fs::create_dir_all(&self.cache);
        let args = ["build", "--fat-binary"];
        let mut child = self.command(&args).spawn().map_err(cant)?;
        let mut progress = Progress::new(Instant::now());
        let said = said(child.stdout.take().expect("piped"), |stage| {
            if let Some(building) = progress.take(stage, Instant::now()) {
                event(&building);
            }
        });
        let status = child.wait();
        self.forget();
        let status = status.map_err(cant)?;
        let fat = match said.fat {
            Some(fat) if status.success() => fat,
            _ => return Err(self.failed(&args, status, said.errors)),
        };
        // dx's linker writes an empty object file where the executable goes, which dx's own link
        // replaces. If that fails, dx logs why, says the build finished and leaves the object file.
        let mut head = Vec::new();
        let read = File::open(&fat.exe).and_then(|file| file.take(18).read_to_end(&mut head));
        let why = match read {
            Ok(_) if executable(&head) => {
                let pruned = fat.exe.parent().and_then(|dir| self.prune(dir, started));
                if let Some((archives, bytes)) = pruned.filter(|&(archives, _)| archives > 0) {
                    let why = "no package's last build in this target directory links against them";
                    event(
                        &json!({"event": "pruned", "archives": archives, "bytes": bytes, "why": why}),
                    );
                }
                return Ok(Fat { queued, ..fat });
            }
            Ok(_) => "isn't an executable".to_owned(),
            Err(error) => format!("can't be read: {error}"),
        };
        let (package, bin, exe) = (&self.package, &self.bin, fat.exe.display());
        let what = format!(
            "`dx {} -p {package} --bin {bin}` said it built {exe}, which {why}, as when dx's own \
             link of it fails",
            args.join(" ")
        );
        let message = if said.errors.is_empty() {
            format!("{what}; run by hand, it says why")
        } else {
            format!("{what}:\n\n{}", build::listed_errors(&said.errors))
        };
        Err(Failure::new("gg.build", message))
    }

    /// Removes the session cache's rustc wrapper files: each rustc's arguments and environment,
    /// which may hold secrets (API keys). dx reads them within the build alone; a patch takes the
    /// build's record on stdin.
    fn forget(&self) {
        for dir in fs::read_dir(&self.cache).into_iter().flatten().flatten() {
            let _ = fs::remove_file(dir.path().join("rustc_wrapper_args.txt"));
        }
    }

    /// Takes the lock of dx's builds in the target directory, held until the file drops: printing
    /// `building` at stage `queued` first if another process holds it.
    ///
    /// dx links each game against an archive of its libraries, written beside the executable and
    /// named by a hash of their names, sizes and times. Games of the same dependencies share it,
    /// and one's link while another's writes it fails: `Failed to write executable`.
    ///
    /// Returns the file with how long it waited for the lock, zero if it was free.
    ///
    /// Fails with `gg.build` if the file can't be opened or locked.
    fn hold(&self) -> Result<(File, Duration), Failure> {
        let failed = |error: io::Error| {
            let message = format!("can't lock {}: {error}", self.lock.display());
            Failure::new("gg.build", message)
        };
        if let Some(dir) = self.lock.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock)
            .map_err(failed)?;
        match file.try_lock() {
            Ok(()) => return Ok((file, Duration::ZERO)),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(failed(error)),
        }
        let why = "another `gg run` builds with dx in this target directory, and two of dx's links \
                   at once can fail";
        event(&json!({"event": "building", "stage": "queued", "why": why}));
        let queued = Instant::now();
        file.lock().map_err(failed)?;
        Ok((file, queued.elapsed()))
    }

    /// Removes the archives in `dir`, beside the executable, that no package's last build linked
    /// against, with their lists; returns how many and their bytes. None if this build's archive
    /// can't be told, as when dx didn't link: then it removes nothing.
    ///
    /// dx writes `libdeps-{hash}.a`, about 1 GB, unless it's there, and never removes one: a
    /// change to any library makes another. It writes `rlibs-{hash}.txt` after each link of the
    /// archive. Holding the lock, the list written since `started` is this build's, whose hash
    /// gg records under the package's name in `gg/dx-archives`. Another package's archive stays
    /// while it's that package's record.
    fn prune(&self, dir: &Path, started: SystemTime) -> Option<(usize, u64)> {
        let mut files = Vec::new();
        let mut linked: Option<(String, SystemTime)> = None;
        for entry in fs::read_dir(dir).ok()?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some((hash, list)) = archived(&name) else {
                continue;
            };
            let written = entry.metadata().and_then(|metadata| metadata.modified());
            if list
                && let Ok(written) = written
                && written >= started
                && linked.as_ref().is_none_or(|(_, newest)| written > *newest)
            {
                linked = Some((hash.to_owned(), written));
            }
            files.push((hash.to_owned(), list, entry.path()));
        }
        let (hash, _) = linked?;
        let records = self.lock.with_file_name("dx-archives");
        let _ = fs::create_dir_all(&records);
        fs::write(records.join(&self.package), &hash).ok()?;
        let kept: HashSet<String> = (fs::read_dir(&records).ok()?.flatten())
            .filter_map(|record| fs::read_to_string(record.path()).ok())
            .map(|hash| hash.trim().to_owned())
            .collect();
        let (mut archives, mut bytes) = (0, 0);
        for (hash, list, path) in files {
            let size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
            if !kept.contains(&hash) && fs::remove_file(&path).is_ok() && !list {
                archives += 1;
                bytes += size;
            }
        }
        Some((archives, bytes))
    }

    /// Builds a patch of the modules as they are now, for a process of build `fat` whose `main` is
    /// at `aslr_reference`; returns dx's jump table, thunks mended as [`keep_rax`] says. A patch
    /// carries every edit since the build.
    ///
    /// Fails with `gg.build` if dx can't run, the patch doesn't build, or its thunks can't be
    /// mended.
    pub(crate) fn hotpatch(&self, fat: &Fat, aslr_reference: u64) -> Result<Value, Failure> {
        let decimal = aslr_reference.to_string();
        let args = ["tools", "hotpatch", "--aslr-reference", &decimal];
        let mut command = self.command(&args);
        let mut child = command.stdin(Stdio::piped()).spawn().map_err(cant)?;
        let mut stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let said = thread::scope(|scope| {
            // Written as dx's output is read, so neither waits on a full pipe; dx reads stdin to
            // its end, so dropping it closes it.
            scope.spawn(move || {
                let _ = stdin.write_all(fat.client.as_bytes());
            });
            // A patch builds one crate: no stages to print.
            said(stdout, |_| {})
        });
        let status = child.wait().map_err(cant)?;
        let table = match said.table {
            Some(table) if status.success() => table,
            _ => return Err(self.failed(&args[..2], status, said.errors)),
        };
        if cfg!(all(windows, target_arch = "x86_64"))
            && let Some(lib) = table["lib"].as_str()
        {
            keep_rax(Path::new(lib), aslr_reference).map_err(|error| {
                let message = format!("can't mend the thunks of patch {lib}: {error}");
                Failure::new("gg.build", message)
            })?;
        }
        Ok(table)
    }

    /// dx with `args`, run from the workspace's root, stdout piped: a JSON line per thing it says.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("dx");
        command
            .args(args)
            .args(["-p", &self.package, "--bin", &self.bin])
            .arg("--session-cache-dir")
            .arg(&self.cache)
            .arg("--json-output")
            // Games use no `asset!`; scanning costs 0.6 s or more a build of the showcase.
            .arg("--skip-assets");
        // A console, as cargo's build has; dx omits it otherwise.
        if cfg!(windows) {
            command.args(["--windows-subsystem", "CONSOLE"]);
        }
        // Not shown: a panic could print the build's environment.
        command
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        command
    }

    fn failed(&self, args: &[&str], status: ExitStatus, errors: Vec<String>) -> Failure {
        let (args, package, bin) = (args.join(" "), &self.package, &self.bin);
        let what = format!("dx {args} -p {package} --bin {bin}");
        let message = if errors.is_empty() {
            format!("`{what}` failed ({status}), saying no more; run by hand, it says why")
        } else {
            format!("`{what}` failed:\n\n{}", build::listed_errors(&errors))
        };
        Failure::new("gg.build", message)
    }
}

/// The hash of a file dx writes beside an executable to link it, `libdeps-{hash}.a` or
/// `rlibs-{hash}.txt`, and whether it's the list. dx's hash is 8 hexadecimal digits.
fn archived(name: &str) -> Option<(&str, bool)> {
    let (hash, list) = match name.strip_prefix("libdeps-") {
        Some(rest) => (rest.strip_suffix(".a")?, false),
        None => (name.strip_prefix("rlibs-")?.strip_suffix(".txt")?, true),
    };
    let hex = hash.len() == 8 && hash.bytes().all(|byte| byte.is_ascii_hexdigit());
    hex.then_some((hash, list))
}

fn cant(error: std::io::Error) -> Failure {
    Failure::new("gg.build", format!("can't run dx: {error}"))
}

/// Checks that dx on the PATH is release `release`, the game's subsecond release.
///
/// Fails with `gg.no_dx` if dx doesn't run, and `gg.dx_version` if it's another release.
pub(crate) fn check(release: &str) -> Result<(), Failure> {
    let install = format!("`cargo install dioxus-cli --version {release} --locked` installs it");
    let output = (Command::new("dx").arg("--version"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| {
            let message = format!("can't run dx, which builds patches: {error}; {install}");
            Failure::new("gg.no_dx", message)
        })?;
    match version(&String::from_utf8_lossy(&output.stdout)) {
        Some(theirs) if theirs == release => Ok(()),
        theirs => {
            let theirs = theirs.unwrap_or("of no release it names");
            let message = format!(
                "dx is {theirs}, and the game's subsecond {release}: patches need dx of \
                 subsecond's release; {install}"
            );
            Err(Failure::new("gg.dx_version", message))
        }
    }
}

/// `0.7.2` in `dioxus 0.7.2 (35351eb)`.
fn version(said: &str) -> Option<&str> {
    let mut words = said.split_whitespace();
    if words.next()? != "dioxus" {
        return None;
    }
    words.next()
}

/// Whether `head`, a file's first 18 bytes or fewer, begins an executable, not an object file:
/// PE (`MZ`; an object file starts with its machine), or little-endian (as on every machine gg
/// runs on) ELF or 64-bit Mach-O of executable type: `e_type` 2 (3 if position-independent) or
/// `filetype` 2; an object file's is 1.
fn executable(head: &[u8]) -> bool {
    let elf = head.starts_with(b"\x7FELF") && matches!(head.get(16..18), Some([2 | 3, 0]));
    let mach_o = head.starts_with(&0xFEED_FACFu32.to_le_bytes())
        && head.get(12..16) == Some(&2u32.to_le_bytes()[..]);
    head.starts_with(b"MZ") || elf || mach_o
}

#[derive(Default)]
struct Said {
    fat: Option<Fat>,
    table: Option<Value>,
    errors: Vec<String>,
}

fn said(stdout: impl Read, mut stage: impl FnMut(Value)) -> Said {
    let mut said = Said::default();
    for line in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
        match record(&String::from_utf8_lossy(&line)) {
            Record::Built(fat) => said.fat = Some(fat),
            Record::Patch(table) => said.table = Some(table),
            Record::Stage(at) => stage(at),
            Record::Error(error) if !build::summary(&error) && !said.errors.contains(&error) => {
                said.errors.push(error);
            }
            Record::Error(_) | Record::Other => {}
        }
    }
    said
}

/// What a `--json-output` line says, of what gg reads.
enum Record {
    Built(Fat),
    Patch(Value),
    /// A stage, with `building`'s fields: `stage`, snake case; at the start, `crates`, dx's count;
    /// after each crate, `compiled` of `crates` (fresh ones included) and the last's name, `crate`.
    Stage(Value),
    /// rustc's error as rendered, or dx's as plain text; one dx ends on, or logs and carries on
    /// past, as when its own link fails.
    Error(String),
    Other,
}

struct Progress {
    started: Instant,
    printed: Option<(String, Instant)>,
}

impl Progress {
    fn new(started: Instant) -> Self {
        Self {
            started,
            printed: None,
        }
    }

    /// The `building` event for `stage` at `now`, if due: at each new stage, and within one every
    /// [`PROGRESS`], so the event after a long wait names the crate that took that long.
    fn take(&mut self, mut stage: Value, now: Instant) -> Option<Value> {
        let name = stage["stage"].as_str().unwrap_or_default().to_owned();
        let due = (self.printed.as_ref())
            .is_none_or(|(last, at)| *last != name || now.duration_since(*at) >= PROGRESS);
        if !due {
            return None;
        }
        self.printed = Some((name, now));
        stage["event"] = json!("building");
        stage["ms"] = json!(millis(now.duration_since(self.started)));
        Some(stage)
    }
}

/// What `line` says: an object whose `json` is one of dx's records as a JSON string, such as
/// `{"BuildUpdate":{"stage":"Linking"}}`; without it, a log line whose `ERROR` `message` says what
/// failed.
fn record(line: &str) -> Record {
    let Ok(line) = serde_json::from_str::<Value>(line) else {
        return Record::Other;
    };
    let Some(Ok(record)) = line["json"].as_str().map(serde_json::from_str::<Value>) else {
        return match (line["level"].as_str(), line["message"].as_str()) {
            (Some("ERROR"), Some(message)) => Record::Error(plain(message)),
            _ => Record::Other,
        };
    };
    if let Some(client) = record.pointer("/BuildsFinished/client")
        && let Some(exe) = client["exe"].as_str()
    {
        let exe = PathBuf::from(exe);
        return Record::Built(Fat {
            exe,
            client: client.to_string(),
            queued: Duration::ZERO,
        });
    }
    // Its `artifacts` hold the environment again: dropped.
    if let Some(table) = record.pointer("/Hotpatch/jump_table") {
        return Record::Patch(table.clone());
    }
    if let Some(stage) = record.pointer("/BuildUpdate/stage").and_then(stage) {
        return Record::Stage(stage);
    }
    if let Some(message) = record.pointer("/RustcOutput/message")
        && (message["level"].as_str()).is_some_and(|level| level.starts_with("error"))
    {
        let rendered = message["rendered"].as_str().or(message["message"].as_str());
        return Record::Error(plain(rendered.unwrap_or_default()));
    }
    match record.pointer("/Error/message").and_then(Value::as_str) {
        Some(message) => Record::Error(plain(message)),
        None => Record::Other,
    }
}

/// The [`Record::Stage`] for dx's `BuildStage` `variant`, `"Linking"` or
/// `{"Compiling":{"current":3,"total":9,"krate":"gg_core"}}`; only starting's and compiling's
/// fields are kept. `None` if it's neither.
fn stage(variant: &Value) -> Option<Value> {
    let (name, fields) = match variant {
        Value::String(name) => (name, &Value::Null),
        Value::Object(variant) if variant.len() == 1 => variant.iter().next()?,
        _ => return None,
    };
    let mut snake = String::with_capacity(name.len() + 4);
    for (at, c) in name.char_indices() {
        if at > 0 && c.is_ascii_uppercase() {
            snake.push('_');
        }
        snake.push(c.to_ascii_lowercase());
    }
    let mut stage = json!({"stage": snake});
    match name.as_str() {
        "Starting" => stage["crates"] = fields["crate_count"].clone(),
        "Compiling" => {
            stage["compiled"] = fields["current"].clone();
            stage["crates"] = fields["total"].clone();
            stage["crate"] = fields["krate"].clone();
        }
        _ => {}
    }
    Some(stage)
}

pub(crate) fn plain(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        // `ESC [`, parameters, a final character `@` to `~`; other escapes are 1 character.
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    plain.truncate(plain.trim_end().len());
    plain
}

/// Mends patch library `lib`, which dx built for Windows on x86-64 for a process whose `main` is at
/// `aslr_reference`; returns how many thunks. dx's thunks to the executable's functions,
/// `movabs rax, imm64; jmp rax` (12 bytes in a slot of 16), lose RAX, but `__chkstk`, called by
/// every function with a frame past a page, takes the frame size in RAX: through a thunk it probes
/// up to the address and the game overflows its stack. Each becomes `jmp [rip+0]` and the address
/// (14 bytes), as dx writes them elsewhere.
///
/// dx's `create_undefined_symbol_stub`, in packages/cli/src/build/patch.rs, writes them so in 0.7.2,
/// 0.7.10 and main at 0.8.0-alpha.3; once its thunks keep RAX, this goes.
fn keep_rax(lib: &Path, aslr_reference: u64) -> io::Result<usize> {
    let mut image = fs::read(lib)?;
    let mended = mend(&mut image, aslr_reference).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "it isn't a PE image for x86-64")
    })?;
    if mended > 0 {
        fs::write(lib, &image)?;
    }
    Ok(mended)
}

/// Mends the thunks in PE image `image`; returns how many, or `None` if it isn't for x86-64. A
/// thunk is dx's 12 bytes at an 8-aligned slot in a code section, targeting within 4 GiB of
/// `aslr_reference`, and followed by 2 padding bytes (zeros or `int3`s), which the mended thunk
/// takes.
fn mend(image: &mut [u8], aslr_reference: u64) -> Option<usize> {
    let mut mended = 0;
    for code in code_sections(image)? {
        let mut at = code.start;
        while at + 14 <= code.end {
            let slot = &image[at..at + 14];
            let address = u64::from_le_bytes(slot[2..10].try_into().expect("8 bytes"));
            let thunk = slot[..2] == [0x48, 0xB8] && slot[10..12] == [0xFF, 0xE0];
            let padded = slot[12..].iter().all(|&byte| byte == 0 || byte == 0xCC);
            if thunk && padded && address.abs_diff(aslr_reference) < 1 << 32 {
                image[at..at + 6].copy_from_slice(&[0xFF, 0x25, 0, 0, 0, 0]);
                image[at + 6..at + 14].copy_from_slice(&address.to_le_bytes());
                mended += 1;
            }
            at += 8;
        }
    }
    Some(mended)
}

fn code_sections(image: &[u8]) -> Option<Vec<Range<usize>>> {
    let bytes = |at: usize, len: usize| image.get(at..at.checked_add(len)?);
    let u16_at = |at: usize| Some(u16::from_le_bytes(bytes(at, 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes(at, 4)?.try_into().ok()?) as usize);
    // The DOS header's last field gives the PE header, then the section headers follow.
    let pe = u32_at(0x3C)?;
    if bytes(pe, 4)? != b"PE\0\0" || u16_at(pe + 4)? != 0x8664 {
        return None;
    }
    let headers = pe + 24 + usize::from(u16_at(pe + 20)?);
    let mut sections = Vec::new();
    for header in (0..u16_at(pe + 6)?).map(|n| headers + 40 * usize::from(n)) {
        // IMAGE_SCN_CNT_CODE.
        if u32_at(header + 36)? & 0x20 != 0 {
            let start = u32_at(header + 20)?;
            let end = start.saturating_add(u32_at(header + 16)?).min(image.len());
            sections.push(start..end);
        }
    }
    Some(sections)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A line dx prints, with record `record`.
    fn line(record: &str) -> String {
        json!({"timestamp": "   7.138s", "level": "INFO", "json": record}).to_string()
    }

    #[test]
    fn dx_names_its_release_in_its_version() {
        assert_eq!(version("dioxus 0.7.2 (35351eb)\n"), Some("0.7.2"));
        assert_eq!(version("dx 0.7.2"), None);
        assert_eq!(version(""), None);
    }

    #[test]
    fn the_lines_dx_prints_give_the_build_the_patch_and_the_errors() {
        // A build's record; its environment is a stand-in.
        let client = json!({
            "path": "C:\\dev\\GGEngine\\target\\dx\\gg-sandbox\\debug\\windows\\app",
            "exe": "C:\\dev\\GGEngine\\target\\x86_64-pc-windows-msvc\\desktop-dev\\gg-sandbox.exe",
            "rustc_args": ["--crate-name", "gg_sandbox"],
            "rustc_envs": [["API_KEY", "stand-in"]],
            "link_args": [],
            "assets": null,
        });
        let built = line(&json!({"BuildsFinished": {"client": client}}).to_string());
        let Record::Built(fat) = record(&built) else {
            panic!("a build");
        };
        let exe = "C:\\dev\\GGEngine\\target\\x86_64-pc-windows-msvc\\desktop-dev\\gg-sandbox.exe";
        assert_eq!(fat.exe, PathBuf::from(exe));
        let kept: Value = serde_json::from_str(&fat.client).expect("JSON");
        assert_eq!(kept, client);

        // A patch's table, addresses exact however large, without its artifacts.
        let patched = line(
            r#"{"Hotpatch":{"jump_table":{"lib":"C:\\t\\libgg-sandbox-patch-1.dll",
            "map":{"23552":23552,"140737488355328":18446744073709551615},"aslr_reference":26064,
            "new_base_address":26064,"ifunc_count":0},"artifacts":{"rustc_envs":[]}}}"#,
        );
        let Record::Patch(table) = record(&patched) else {
            panic!("a patch");
        };
        assert_eq!(table["map"]["140737488355328"].as_u64(), Some(u64::MAX));
        assert_eq!(table["map"]["23552"].as_u64(), Some(23552));
        assert_eq!(table["lib"], "C:\\t\\libgg-sandbox-patch-1.dll");
        assert_eq!(table.get("artifacts"), None);

        // rustc's errors, not its warnings.
        let rustc = |level: &str, rendered: &str| {
            let message = json!({"message": "mismatched types", "code": {"code": "E0308"},
                "level": level, "spans": [], "children": [], "rendered": rendered});
            line(&json!({"RustcOutput": {"message": message}}).to_string())
        };
        let rendered = "\u{1b}[0m\u{1b}[1m\u{1b}[38;5;9merror[E0308]\u{1b}[0m\u{1b}[0m\u{1b}[1m: \
                        mismatched types\u{1b}[0m\n --> apps/sandbox/src/main.rs:2:5\n\n";
        let Record::Error(error) = record(&rustc("error", rendered)) else {
            panic!("an error");
        };
        assert_eq!(
            error,
            "error[E0308]: mismatched types\n --> apps/sandbox/src/main.rs:2:5"
        );
        let warning = "warning: failed to connect to jobserver from environment variable";
        assert!(matches!(record(&rustc("warning", warning)), Record::Other));

        // dx's own.
        let failed = json!({"Error": {"message": "\u{1b}[31mERROR\u{1b}[0m dx tools: Cargo build \
            failed - no output location."}});
        let Record::Error(error) = record(&line(&failed.to_string())) else {
            panic!("an error");
        };
        assert_eq!(
            error,
            "ERROR dx tools: Cargo build failed - no output location."
        );

        // Its log's errors, which it carries on past, as when its link fails.
        let lld = "rust-lld: error: could not open 'kernel32.lib': no such file or directory";
        let logged = json!({"timestamp": "  10.212s", "level": "ERROR",
            "message": format!("Failed to generate fat binary: {lld}\n{lld}\n"),
            "telemetry": r#"{"event":"hotpatch_fat_binary_generation_failed"}"#});
        let Record::Error(error) = record(&logged.to_string()) else {
            panic!("an error");
        };
        assert_eq!(
            error,
            format!("Failed to generate fat binary: {lld}\n{lld}")
        );

        // Stages, with crate counts.
        let stages = [
            (
                r#"{"Starting":{"patch":false,"crate_count":530}}"#,
                json!({"stage": "starting", "crates": 530}),
            ),
            (
                r#"{"Compiling":{"current":212,"total":530,"krate":"gg_engine"}}"#,
                json!({"stage": "compiling", "compiled": 212, "crates": 530, "crate": "gg_engine"}),
            ),
            (r#""Linking""#, json!({"stage": "linking"})),
            (
                r#"{"CopyingAssets":{"current":0,"total":2,"path":"a.png"}}"#,
                json!({"stage": "copying_assets"}),
            ),
        ];
        for (stage, fields) in stages {
            let update = line(&format!(r#"{{"BuildUpdate":{{"stage":{stage}}}}}"#));
            let Record::Stage(said) = record(&update) else {
                panic!("a stage: {stage}");
            };
            assert_eq!(said, fields);
        }

        // Anything else.
        let message = r#"{"timestamp":"   1.000s","level":"INFO","message":"Compiling"}"#;
        let warned = r#"{"timestamp":"   2.000s","level":"WARN","message":"No assets found"}"#;
        let shapeless = line(r#"{"BuildUpdate":{"stage":{}}}"#);
        for other in [message, warned, shapeless.as_str(), "not JSON", ""] {
            assert!(matches!(record(other), Record::Other), "{other}");
        }
    }

    #[test]
    fn an_executable_is_told_from_the_object_file_dx_leaves_where_its_link_failed() {
        // What dx left on Windows on x86-64 when its link failed: an empty COFF object file.
        let coff = [
            0x64, 0x86, 0, 0, 0, 0, 0, 0, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0,
        ];
        assert!(!executable(&coff[..18]));
        let pe = b"MZ\x90\0\x03\0\0\0\x04\0\0\0\xFF\xFF\0\0\xB8\0";
        assert!(executable(pe));
        // ELF: object, executable, position-independent.
        let elf = |e_type: u8| {
            let mut head = [0; 18];
            head[..7].copy_from_slice(b"\x7FELF\x02\x01\x01");
            head[16] = e_type;
            head
        };
        assert!(!executable(&elf(1)));
        assert!(executable(&elf(2)));
        assert!(executable(&elf(3)));
        // Mach-O for ARM64: object, executable.
        let mach_o = |filetype: u32| {
            let mut head = [0; 18];
            head[..4].copy_from_slice(&0xFEED_FACFu32.to_le_bytes());
            head[4..8].copy_from_slice(&0x0100_000Cu32.to_le_bytes());
            head[12..16].copy_from_slice(&filetype.to_le_bytes());
            head
        };
        assert!(!executable(&mach_o(1)));
        assert!(executable(&mach_o(2)));
        // Too little to tell by.
        assert!(!executable(&[]));
        assert!(!executable(&elf(2)[..16]));
    }

    #[test]
    fn a_build_prints_each_stage_and_its_progress_through_crates_now_and_then() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let compiled = |n: u32| json!({"stage": "compiling", "compiled": n, "crates": 9});
        let mut progress = Progress::new(start);
        let printed: Vec<_> = [
            (json!({"stage": "starting", "crates": 9}), at(0)),
            (compiled(1), at(100)),
            // Within PROGRESS of the one at 100.
            (compiled(2), at(200)),
            (compiled(3), at(5_099)),
            (compiled(4), at(5_100)),
            (compiled(5), at(9_000)),
            // A crate that took long.
            (compiled(6), at(40_000)),
            (json!({"stage": "linking"}), at(40_500)),
        ]
        .into_iter()
        .filter_map(|(stage, now)| progress.take(stage, now))
        .map(|event| {
            assert_eq!(event["event"], "building");
            (
                event["stage"].clone(),
                event["compiled"].clone(),
                event["ms"].clone(),
            )
        })
        .collect();
        let want = [
            ("starting", None, 0),
            ("compiling", Some(1), 100),
            ("compiling", Some(4), 5_100),
            ("compiling", Some(6), 40_000),
            ("linking", None, 40_500),
        ]
        .map(|(stage, compiled, ms)| (json!(stage), json!(compiled), json!(ms)));
        assert_eq!(printed, want);
    }

    #[test]
    fn the_environment_dx_writes_as_it_builds_is_removed() {
        let cache = std::env::temp_dir().join(format!("gg-dx-cache-{}", std::process::id()));
        let windows = cache.join("windows");
        fs::create_dir_all(&windows).expect("a directory");
        for file in ["rustc_wrapper_args.txt", "link_args.json"] {
            fs::write(windows.join(file), "{}").expect("written");
        }
        let (package, bin) = ("game".to_owned(), "game".to_owned());
        let (root, cache) = (cache.clone(), cache);
        Dx {
            package,
            bin,
            root,
            cache: cache.clone(),
            lock: cache.join("dx.lock"),
        }
        .forget();
        let left: Vec<String> = (fs::read_dir(&windows).expect("a directory").flatten())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let _ = fs::remove_dir_all(&cache);
        assert_eq!(left, ["link_args.json"]);
    }

    /// Two packages' builds in one target directory take turns: the second waits for the first's
    /// lock to drop.
    #[test]
    fn builds_in_one_target_directory_take_turns() {
        let target = std::env::temp_dir().join(format!("gg-dx-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&target);
        let dx = |name: &str| Dx {
            package: name.to_owned(),
            bin: name.to_owned(),
            root: target.clone(),
            cache: target.join("gg").join("dx").join(name),
            lock: target.join("gg").join("dx.lock"),
        };
        let (rts, fps) = (dx("rts"), dx("fps"));
        let (held, free) = rts.hold().expect("the lock");
        let (sent, got) = std::sync::mpsc::channel();
        let waiting = thread::spawn(move || {
            let held = fps.hold();
            let _ = sent.send(());
            held.ok().map(|(_, queued)| queued)
        });
        let blocked = got.recv_timeout(Duration::from_millis(300)).is_err();
        drop(held);
        let took = got.recv_timeout(Duration::from_secs(5)).is_ok();
        let queued = waiting.join().expect("no panic");
        let _ = fs::remove_dir_all(&target);
        assert!(blocked, "the second waits while the first holds it");
        assert!(took, "then takes it");
        assert_eq!(free, Duration::ZERO, "a free lock queues no time");
        assert!(
            queued.is_some_and(|queued| queued >= Duration::from_millis(250)),
            "the wait is the queue's: {queued:?}"
        );
    }

    /// A build keeps the archive it linked against and another package's record, removing the
    /// rest with their lists; one that can't tell its own removes nothing.
    #[test]
    fn a_build_removes_the_archives_no_packages_last_build_links_against() {
        assert_eq!(archived("libdeps-0216e4f8.a"), Some(("0216e4f8", false)));
        assert_eq!(archived("rlibs-0216e4f8.txt"), Some(("0216e4f8", true)));
        assert_eq!(archived("libdeps-game.a"), None);
        assert_eq!(archived("libdeps-0216e4f8.rlib"), None);
        let target = std::env::temp_dir().join(format!("gg-dx-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&target);
        let dir = target.join("desktop-dev");
        let records = target.join("gg").join("dx-archives");
        fs::create_dir_all(&dir).expect("a directory");
        fs::create_dir_all(&records).expect("a directory");
        let rts = Dx {
            package: "rts".to_owned(),
            bin: "rts".to_owned(),
            root: target.clone(),
            cache: target.join("gg").join("dx").join("rts"),
            lock: target.join("gg").join("dx.lock"),
        };
        let started = SystemTime::now();
        let archive = |hash: &str, linked: SystemTime| {
            fs::write(dir.join(format!("libdeps-{hash}.a")), [0; 100]).expect("written");
            let list = File::create(dir.join(format!("rlibs-{hash}.txt"))).expect("written");
            list.set_modified(linked).expect("dated");
        };
        let before = started - Duration::from_secs(60);
        let after = started + Duration::from_secs(1);
        // fps's record, two no build links against any more, and this build's.
        for hash in ["0000aaaa", "1111bbbb", "2222cccc"] {
            archive(hash, before);
        }
        archive("3333dddd", after);
        fs::write(records.join("fps"), "0000aaaa").expect("written");
        for other in ["libdeps.a", "gg-rts"] {
            fs::write(dir.join(other), "").expect("written");
        }
        let left = || {
            let mut left: Vec<String> = (fs::read_dir(&dir).expect("a directory").flatten())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            left.sort();
            left
        };
        let untold = rts.prune(&dir, after + Duration::from_secs(1));
        let all = left().len();
        let pruned = rts.prune(&dir, started);
        let (left, record) = (left(), fs::read_to_string(records.join("rts")));
        let _ = fs::remove_dir_all(&target);
        assert_eq!(untold, None, "no list written since it started");
        assert_eq!(all, 10);
        assert_eq!(pruned, Some((2, 200)));
        let archives = ["libdeps-0000aaaa.a", "libdeps-3333dddd.a", "libdeps.a"];
        let lists = ["rlibs-0000aaaa.txt", "rlibs-3333dddd.txt"];
        assert_eq!(left, [&["gg-rts"][..], &archives, &lists].concat());
        assert_eq!(record.ok().as_deref(), Some("3333dddd"));
    }

    /// A PE image for x86-64 with a code section of `code` and a data section of `data`.
    fn image(code: &[u8], data: &[u8]) -> Vec<u8> {
        let mut image = vec![0; 0x200];
        image[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        image[0x40..0x44].copy_from_slice(b"PE\0\0");
        image[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes());
        image[0x46..0x48].copy_from_slice(&2u16.to_le_bytes());
        // With no optional header, section headers start at 0x58.
        let sections = [
            (0x200u32, code, 0x6000_0020u32),
            (0x200 + code.len() as u32, data, 0xC000_0040),
        ];
        for (n, (at, bytes, characteristics)) in sections.into_iter().enumerate() {
            let header = 0x58 + 40 * n;
            image[header + 16..header + 20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            image[header + 20..header + 24].copy_from_slice(&at.to_le_bytes());
            image[header + 36..header + 40].copy_from_slice(&characteristics.to_le_bytes());
        }
        [&image[..], code, data].concat()
    }

    #[test]
    fn dxs_thunks_are_mended_to_keep_rax() {
        let main = 0x7FF7_6100_1000u64;
        // A 16-byte slot as dx writes it, and as mended.
        let thunk = |to: u64| {
            [
                &[0x48, 0xB8][..],
                &to.to_le_bytes(),
                &[0xFF, 0xE0, 0, 0, 0, 0],
            ]
            .concat()
        };
        let mended = |to: u64| [&[0xFF, 0x25, 0, 0, 0, 0][..], &to.to_le_bytes(), &[0, 0]].concat();
        // Two thunks into the executable, the second padded with `int3`s; one far from it; one
        // with no padding.
        let mut padded = thunk(main - 0x100);
        padded[12..].fill(0xCC);
        let mut tight = thunk(main + 8);
        tight[12..].copy_from_slice(&[0x55, 0x48, 0x89, 0xE5]);
        let code = [thunk(main + 0x29D0), padded, thunk(0x1234), tight.clone()].concat();
        // And one in a section of data.
        let data = thunk(main + 16);
        let mut patched = image(&code, &data);
        assert_eq!(mend(&mut patched, main), Some(2));
        let mut padded = mended(main - 0x100);
        padded[14..].fill(0xCC);
        let want = [mended(main + 0x29D0), padded, thunk(0x1234), tight, data].concat();
        assert_eq!(patched[0x200..], want);
        // Another machine's image, and a non-image.
        let mut arm = image(&code, &[]);
        arm[0x44..0x46].copy_from_slice(&0xAA64u16.to_le_bytes());
        assert_eq!(mend(&mut arm, main), None);
        assert_eq!(mend(&mut [0; 64], main), None);
    }
}
