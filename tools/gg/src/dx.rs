//! Dioxus's `dx`, for `gg run`'s patches: checking that the machine's is the release of the
//! subsecond a game's build uses, building the game's development build so that it can take
//! patches, and building patches of it, reading what dx says as it builds, and mending the thunks
//! of those it builds for Windows on x86-64.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use gg_api::Failure;
use serde_json::Value;

use crate::build::Package;

/// How dx builds a game's package: from the workspace's root, as its binary, with dx's session
/// cache in a directory of gg's for the package, which patches are built from beside the build's
/// record.
#[derive(Clone)]
pub(crate) struct Dx {
    package: String,
    bin: String,
    root: PathBuf,
    cache: PathBuf,
}

/// A build of a game that patches are built against: its executable, and the record of the build
/// dx printed, its `BuildsFinished` record's `client`, as JSON, which `dx tools hotpatch` reads.
/// The record holds the environment the build ran in, which may hold secrets, such as API keys, so
/// it's kept in memory and handed to dx alone, never printed, logged or written, and this has no
/// `Debug`; dx writes it into its session cache too as it builds, which [`Dx::build`] removes.
#[derive(Clone)]
pub(crate) struct Fat {
    pub(crate) exe: PathBuf,
    client: String,
}

impl Dx {
    /// How dx builds package `name`, which cargo says is `package`.
    pub(crate) fn new(name: &str, package: &Package) -> Self {
        Self {
            package: name.to_owned(),
            bin: package.bin.clone(),
            root: package.root.clone(),
            cache: package.target.join("gg").join("dx").join(name),
        }
    }

    /// Builds the package's development build with `dx build --fat-binary`, which patches can be
    /// built against, and removes the environment dx wrote as it built, as [`forget`](Self::forget)
    /// says, whether the build succeeded or not.
    ///
    /// Fails with `gg.build` if dx can't run, or the build fails, with rustc's errors.
    pub(crate) fn build(&self) -> Result<Fat, Failure> {
        // dx makes it too, but says less if it can't.
        let _ = fs::create_dir_all(&self.cache);
        let args = ["build", "--fat-binary"];
        let mut child = self.command(&args).spawn().map_err(cant)?;
        let said = said(child.stdout.take().expect("piped"));
        let status = child.wait();
        self.forget();
        let status = status.map_err(cant)?;
        match said.fat {
            Some(fat) if status.success() => Ok(fat),
            _ => Err(self.failed(&args, status, said.errors)),
        }
    }

    /// Removes what dx's rustc wrapper writes into the session cache as a build runs, in a directory
    /// for each kind of bundle, as `windows`: each rustc's arguments and its whole environment, which
    /// may hold secrets, such as API keys. dx reads it within the build alone, and a patch's build
    /// takes the build's record from its stdin.
    fn forget(&self) {
        for dir in fs::read_dir(&self.cache).into_iter().flatten().flatten() {
            let _ = fs::remove_file(dir.path().join("rustc_wrapper_args.txt"));
        }
    }

    /// Builds a patch of the package's modules, as they are now, for a process of build `fat` whose
    /// `main` is at `aslr_reference`, with `dx tools hotpatch`, and returns its jump table, as dx
    /// printed it, having mended its thunks on Windows on x86-64, as [`keep_rax`] says. A patch
    /// carries every edit since the build.
    ///
    /// Fails with `gg.build` if dx can't run, the patch doesn't build, with rustc's errors, or its
    /// thunks can't be mended.
    pub(crate) fn hotpatch(&self, fat: &Fat, aslr_reference: u64) -> Result<Value, Failure> {
        let decimal = aslr_reference.to_string();
        let args = ["tools", "hotpatch", "--aslr-reference", &decimal];
        let mut command = self.command(&args);
        let mut child = command.stdin(Stdio::piped()).spawn().map_err(cant)?;
        let mut stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let said = thread::scope(|scope| {
            // The build's record, written as dx's output is read, so that neither waits on the
            // other's full pipe; dropping it then closes dx's stdin, which dx reads to its end.
            scope.spawn(move || {
                let _ = stdin.write_all(fat.client.as_bytes());
            });
            said(stdout)
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

    /// dx with `args` for the package, run from the workspace's root, its stdout piped, where it
    /// prints a line of JSON for each thing it says.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("dx");
        command
            .args(args)
            .args(["-p", &self.package, "--bin", &self.bin])
            .arg("--session-cache-dir")
            .arg(&self.cache)
            .arg("--json-output");
        // A console, as the game's cargo build has, which dx leaves out of a desktop app otherwise.
        if cfg!(windows) {
            command.args(["--windows-subsystem", "CONSOLE"]);
        }
        // Its stderr isn't shown, as what it says there, as when it panics, could hold the
        // environment the build ran in.
        command
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        command
    }

    /// The failure of dx with `args`, which ended with `status`, having said `errors`.
    fn failed(&self, args: &[&str], status: ExitStatus, errors: Vec<String>) -> Failure {
        let (args, package, bin) = (args.join(" "), &self.package, &self.bin);
        let what = format!("dx {args} -p {package} --bin {bin}");
        let message = if errors.is_empty() {
            format!("`{what}` failed ({status}), saying no more; run by hand, it says why")
        } else {
            format!("`{what}` failed:\n\n{}", errors.join("\n\n"))
        };
        Failure::new("gg.build", message)
    }
}

fn cant(error: std::io::Error) -> Failure {
    Failure::new("gg.build", format!("can't run dx: {error}"))
}

/// Checks that dx is on the PATH, and is release `release` of Dioxus, the release of the subsecond
/// the game's build uses, as patches need.
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

/// The release `dx --version` names, as `0.7.2` in `dioxus 0.7.2 (35351eb)`.
fn version(said: &str) -> Option<&str> {
    let mut words = said.split_whitespace();
    if words.next()? != "dioxus" {
        return None;
    }
    words.next()
}

/// What dx said on its stdout, until it ended: the build's record, the patch's table, and its
/// errors, each once.
#[derive(Default)]
struct Said {
    fat: Option<Fat>,
    table: Option<Value>,
    errors: Vec<String>,
}

/// What dx says on `stdout`, read to its end.
fn said(stdout: impl Read) -> Said {
    let mut said = Said::default();
    for line in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
        match record(&String::from_utf8_lossy(&line)) {
            Record::Built(fat) => said.fat = Some(fat),
            Record::Patch(table) => said.table = Some(table),
            Record::Error(error) if !said.errors.contains(&error) => said.errors.push(error),
            Record::Error(_) | Record::Other => {}
        }
    }
    said
}

/// What a line dx printed with `--json-output` says, of what gg reads.
enum Record {
    /// A build finished.
    Built(Fat),
    /// A patch was built: its jump table.
    Patch(Value),
    /// An error, rustc's, as rustc renders it, or dx's own, as plain text.
    Error(String),
    /// Anything else, such as how far the build has come, or a warning.
    Other,
}

/// What `line` says: it's an object whose `json`, if it has one, is one of dx's records, as a
/// string of JSON, such as `{"BuildUpdate":{"stage":"Linking"}}`.
fn record(line: &str) -> Record {
    let Ok(line) = serde_json::from_str::<Value>(line) else {
        return Record::Other;
    };
    let Some(Ok(record)) = line["json"].as_str().map(serde_json::from_str::<Value>) else {
        return Record::Other;
    };
    if let Some(client) = record.pointer("/BuildsFinished/client")
        && let Some(exe) = client["exe"].as_str()
    {
        let exe = PathBuf::from(exe);
        return Record::Built(Fat {
            exe,
            client: client.to_string(),
        });
    }
    // Its `artifacts` beside, which hold the environment again, are dropped here.
    if let Some(table) = record.pointer("/Hotpatch/jump_table") {
        return Record::Patch(table.clone());
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

/// `text` without the ANSI escape sequences that color it, and without the whitespace that ends it.
fn plain(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            plain.push(c);
            continue;
        }
        // `ESC [`, parameters, and a final character from `@` to `~`; other escapes are one
        // character long.
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

/// Mends the patch library at `lib`, which dx built for Windows on x86-64 for a process whose
/// `main` is at `aslr_reference`, returning how many thunks it mended. The library reaches the
/// executable's functions through thunks that dx writes as `movabs rax, imm64; jmp rax`, 12 bytes
/// in a slot of 16, which loses what RAX held; but `__chkstk`, which every function whose frame is
/// past a page calls as it starts, takes the frame's size in RAX, so through its thunk it probes as
/// far as the address instead, and the game overflows its stack at the first such call. Each thunk
/// becomes `jmp [rip+0]` and the address, 14 bytes, which loses nothing, as dx writes them for
/// x86-64 elsewhere.
///
/// dx's `create_undefined_symbol_stub`, in packages/cli/src/build/patch.rs, writes them so in 0.7.2,
/// as in 0.7.10 and on main at 0.8.0-alpha.3; once its thunks keep RAX, this goes.
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

/// Mends the thunks in PE image `image`, as [`keep_rax`] says, returning how many; `None` if it
/// isn't an image for x86-64. A thunk is dx's 12 bytes at a slot 8 bytes aligned in a code section,
/// whose address is within 4 GiB of `aslr_reference`, as one in the executable is, and whose next 2
/// bytes, which the mended thunk takes, are padding: zeros or `int3`s.
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

/// Where in PE image `image` its code sections' bytes are; `None` if it isn't an image for x86-64.
fn code_sections(image: &[u8]) -> Option<Vec<Range<usize>>> {
    let bytes = |at: usize, len: usize| image.get(at..at.checked_add(len)?);
    let u16_at = |at: usize| Some(u16::from_le_bytes(bytes(at, 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes(at, 4)?.try_into().ok()?) as usize);
    // The PE header, where the DOS header's last field says, names the machine, counts the
    // sections and sizes the optional header, after which the sections' headers come.
    let pe = u32_at(0x3C)?;
    if bytes(pe, 4)? != b"PE\0\0" || u16_at(pe + 4)? != 0x8664 {
        return None;
    }
    let headers = pe + 24 + usize::from(u16_at(pe + 20)?);
    let mut sections = Vec::new();
    for header in (0..u16_at(pe + 6)?).map(|n| headers + 40 * usize::from(n)) {
        // A section that holds code, as its characteristics' IMAGE_SCN_CNT_CODE says: where its
        // raw data is, and how long.
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
        // A build's record, kept as JSON for the patches; its environment is a stand-in here.
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

        // A patch's table, its addresses exact however large, without the artifacts beside it.
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

        // rustc's errors, as it renders them but for their colors, and not its warnings.
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

        // Anything else.
        let linking = line(r#"{"BuildUpdate":{"stage":"Linking"}}"#);
        let message = r#"{"timestamp":"   1.000s","level":"INFO","message":"Compiling"}"#;
        for other in [linking.as_str(), message, "not JSON", ""] {
            assert!(matches!(record(other), Record::Other), "{other}");
        }
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
        }
        .forget();
        let left: Vec<String> = (fs::read_dir(&windows).expect("a directory").flatten())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let _ = fs::remove_dir_all(&cache);
        assert_eq!(left, ["link_args.json"]);
    }

    /// A PE image for x86-64 with a code section holding `code` and a data section holding `data`.
    fn image(code: &[u8], data: &[u8]) -> Vec<u8> {
        let mut image = vec![0; 0x200];
        image[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        image[0x40..0x44].copy_from_slice(b"PE\0\0");
        image[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes());
        image[0x46..0x48].copy_from_slice(&2u16.to_le_bytes());
        // With no optional header, the sections' headers start at 0x58.
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
        // A slot of 16 bytes as dx writes it, and as it's mended.
        let thunk = |to: u64| {
            [
                &[0x48, 0xB8][..],
                &to.to_le_bytes(),
                &[0xFF, 0xE0, 0, 0, 0, 0],
            ]
            .concat()
        };
        let mended = |to: u64| [&[0xFF, 0x25, 0, 0, 0, 0][..], &to.to_le_bytes(), &[0, 0]].concat();
        // Two thunks into the executable, the second padded with `int3`s; one far from it, as none
        // of dx's is; and one with no padding past it, as no slot of dx's is.
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
        // An image for another machine, and what isn't an image.
        let mut arm = image(&code, &[]);
        arm[0x44..0x46].copy_from_slice(&0xAA64u16.to_le_bytes());
        assert_eq!(mend(&mut arm, main), None);
        assert_eq!(mend(&mut [0; 64], main), None);
    }
}
