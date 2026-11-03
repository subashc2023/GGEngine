//! Assets' ID files, `<file>.meta` beside each texture, model and font under a game's asset root:
//! what names an asset beyond its file's name, which a game's development build writes and reads,
//! as ARCHITECTURE.md's Project state describes; their text, and the scan that finds them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{Failure, UNBUILT};

/// An asset's ID: a 64-bit number, never 0, written as 16 lowercase hex digits, as an entity's is,
/// such as `5f3c9a1e07b2d846`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(NonZeroU64);

impl AssetId {
    /// An ID drawn at random, from `RandomState`, whose keys std seeds from the OS's randomness, as
    /// a world's seed is drawn; drawn again until its text holds one of `a`, `b`, `c`, `d` and `f`,
    /// so that it never reads as a JSON number, whose text may be digits alone and an exponent's
    /// `e`, as `gg set` reads its value as JSON first.
    pub fn draw() -> Self {
        loop {
            // std steps its hashers' keys for each `RandomState`, so that each draw gets others.
            // About one draw in 400, (11/16)^16, has none of the letters.
            let drawn = NonZeroU64::new(RandomState::new().hash_one(())).map(Self);
            if let Some(id) = drawn
                && id.to_string().contains(['a', 'b', 'c', 'd', 'f'])
            {
                return id;
            }
        }
    }

    /// The number it is.
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl fmt::Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Exactly as `Display` writes one, so that each ID has one text: 16 lowercase hex digits, not all
/// zero, digits alone too.
///
/// Fails with `asset.bad_id` for any other text.
impl FromStr for AssetId {
    type Err = Failure;

    fn from_str(text: &str) -> Result<Self, Failure> {
        let hex = |b: u8| matches!(b, b'0'..=b'9' | b'a'..=b'f');
        let n = (text.len() == 16 && text.bytes().all(hex))
            .then(|| u64::from_str_radix(text, 16).ok())
            .flatten()
            .and_then(NonZeroU64::new);
        n.map(Self).ok_or_else(|| {
            let message = format!(
                "`{text}` isn't an asset's ID: an ID is 16 lowercase hex digits, not all zero, \
                 such as `5f3c9a1e07b2d846`"
            );
            Failure::new("asset.bad_id", message)
        })
    }
}

/// What an asset is, by its file's extension, whatever its case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    /// A PNG or a JPEG, `.png`, `.jpg` or `.jpeg`, as `Context::add_texture` loads.
    Texture,
    /// A glTF, `.gltf` or `.glb`, as `Context::add_model` loads.
    Model,
    /// A TrueType or OpenType font, `.ttf` or `.otf`, as `Context::add_font` loads.
    Font,
}

/// The kinds, by the names the API gives them, as `assets`' `kind` takes them.
pub const KINDS: [&str; 3] = ["texture", "model", "font"];

impl AssetKind {
    /// The kind of the file at `path`, by its extension, whatever its case; `None` for any other
    /// file, a shader, a scene and an ID file among them.
    pub fn of(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "png" | "jpg" | "jpeg" => Some(Self::Texture),
            "gltf" | "glb" => Some(Self::Model),
            "ttf" | "otf" => Some(Self::Font),
            _ => None,
        }
    }
}

/// An asset's ID file, `<file>.meta` beside it, as it reads: canonical RON, a part to a line, which
/// [`text`](Self::text) writes, as ARCHITECTURE.md's Project state describes. `id` is required; a
/// part added later, as import settings will be, takes its default where a file leaves it out; a
/// part it hasn't is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    /// Its asset's ID.
    pub id: AssetId,
}

impl Meta {
    /// The extension an ID file adds to its asset's name.
    pub const EXTENSION: &str = "meta";

    /// The ID file of the asset at `asset`: its path with `.meta` after, such as `stone.png.meta`.
    pub fn path(asset: &Path) -> PathBuf {
        asset.with_added_extension(Self::EXTENSION)
    }

    /// Reads an ID file's text, past a byte-order mark if it starts with one, as some Windows
    /// editors write.
    ///
    /// Fails with `asset.bad_meta`, beginning with the line and column where ron stopped for text
    /// that isn't an ID file's, as one with a part it hasn't or without `id`, or naming `id` for
    /// one that isn't an ID.
    pub fn parse(text: &str) -> Result<Self, Failure> {
        /// An ID file's parts as ron reads them, before its ID is checked: named as the type it
        /// reads, which ron's messages name.
        #[derive(Deserialize)]
        #[serde(rename = "Meta", deny_unknown_fields)]
        struct Read {
            id: String,
        }
        // ron's whitespace isn't Unicode's, so a byte-order mark would stop it at 1:1.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let read: Read = ron::from_str(text).map_err(|error| {
            let at = error.span.end;
            let message = format!("{}:{}: {}", at.line, at.col, error.code);
            Failure::new("asset.bad_meta", message)
        })?;
        let id = read.id.parse().map_err(|failure: Failure| {
            Failure::new("asset.bad_meta", format!("`id`: {}", failure.message))
        })?;
        Ok(Self { id })
    }

    /// Reads the ID file at `path`, as [`parse`](Self::parse) does.
    ///
    /// Fails with `asset.bad_meta` if it can't be read, or as `parse` does; the message says why
    /// without naming the file, which its caller names by its path from the asset root.
    pub fn read(path: &Path) -> Result<Self, Failure> {
        let text = fs::read_to_string(path).map_err(|error| {
            Failure::new("asset.bad_meta", format!("it can't be read: {error}"))
        })?;
        Self::parse(&text)
    }

    /// Its canonical text, a part to a line, four spaces to a level, a comma after each part, its
    /// lines ending in `\n`, without a byte-order mark, which reads back as it is.
    pub fn text(&self) -> String {
        format!("(\n    id: \"{}\",\n)\n", self.id)
    }
}

/// When a file last changed and its length, as its folder's listing gives them, which show that it
/// changed.
pub type Stamp = (SystemTime, u64);

/// What [`scan`] found under an asset root: equal to another scan's while nothing it lists changed
/// between them.
#[derive(Debug, Default, PartialEq)]
pub struct Scan {
    /// The textures, models and fonts, in the order of their paths.
    pub assets: Vec<Scanned>,
    /// The ID files whose assets aren't there, in the order of their paths.
    pub orphans: Vec<Orphan>,
    /// What the scan couldn't take, in the order of their paths.
    pub unread: Vec<Unread>,
}

/// A texture or a model [`scan`] found: its path from the root, with `/` between its parts, such as
/// `assets/stone.png`; the file, as the scan reached it; its stamp, `None` if its listing gave
/// none; its kind; and its ID file: `None` if there's none, or what it holds, or why it doesn't:
/// `asset.bad_meta` for one that doesn't read or isn't a regular file, `asset.meta_case` for one
/// whose name matches only when case is ignored, whose path from the root `misnamed` gives.
#[derive(Debug, PartialEq)]
pub struct Scanned {
    pub path: String,
    pub file: PathBuf,
    pub stamp: Option<Stamp>,
    pub kind: AssetKind,
    pub meta: Option<Result<Meta, Failure>>,
    pub misnamed: Option<String>,
}

/// An ID file whose asset isn't there, as a regular file of its name, though a link or a folder,
/// which the scan doesn't take, may be: its own path from the root, such as
/// `assets/gone.png.meta`; the file, as the scan reached it; the kind of the asset it names; and
/// what it holds, or why it doesn't read.
#[derive(Debug, PartialEq)]
pub struct Orphan {
    pub path: String,
    pub file: PathBuf,
    pub kind: AssetKind,
    pub meta: Result<Meta, Failure>,
}

/// What the scan couldn't take: a folder it couldn't list, `.` for the root itself, or an entry
/// whose name isn't Unicode, or whose type it couldn't tell, which it skips; its path from the
/// root, and why.
#[derive(Debug, PartialEq)]
pub struct Unread {
    pub path: String,
    pub error: String,
}

/// Scans the asset root `root` for textures, models and fonts and their ID files, as
/// ARCHITECTURE.md's Project state describes: every folder under it but those whose names start
/// with `.`, those named `target`, and at its top level [`UNBUILT`]'s, which a package's root holds;
/// never through a link, to a folder or to a file; each asset's ID file by its exact name.
///
/// Matching by exact name, rather than asking the file system for the file, gives the same answer
/// on Windows' and macOS's file systems, which ignore case, as on Linux's.
pub fn scan(root: &Path) -> Scan {
    let mut scan = Scan::default();
    // Each folder still to scan, and its path from the root, `""` for the root itself.
    let mut folders = vec![(root.to_owned(), String::new())];
    while let Some((dir, at)) = folders.pop() {
        let Some(entries) = entries(&dir, &at, &mut scan.unread) else {
            continue;
        };
        // Each ID file, its extension in any case, by its name in lowercase, but those named exactly
        // for an entry beside them: an asset without its own finds one named for it but for case.
        let mut misnamed = HashMap::new();
        for name in entries.keys() {
            let exact = (name.strip_suffix(".meta")).is_some_and(|of| entries.contains_key(of));
            if stem(name).is_some() && !exact {
                misnamed.entry(name.to_lowercase()).or_insert(name.as_str());
            }
        }
        let mut claimed = HashSet::new();
        for (name, &entry) in &entries {
            let Entry::File(stamp) = entry else {
                continue;
            };
            let Some(kind) = AssetKind::of(Path::new(name)) else {
                continue;
            };
            let id_file = format!("{name}.{}", Meta::EXTENSION);
            let mut found_as = None;
            let meta = match entries.get(&id_file) {
                Some(Entry::File(_)) => Some(Meta::read(&dir.join(&id_file))),
                Some(_) => Some(Err(Failure::new(
                    "asset.bad_meta",
                    "it isn't a regular file",
                ))),
                None => misnamed.get(&id_file.to_lowercase()).map(|&found| {
                    claimed.insert(found);
                    found_as = Some(join(&at, found));
                    let message = format!(
                        "its ID file is `{found}`, whose name differs from `{id_file}` in case \
                         alone"
                    );
                    Err(Failure::new("asset.meta_case", message))
                }),
            };
            scan.assets.push(Scanned {
                path: join(&at, name),
                file: dir.join(name),
                stamp,
                kind,
                meta,
                misnamed: found_as,
            });
        }
        for (name, &entry) in &entries {
            // As an asset is a regular file alone, an ID file beside a link or a folder of its
            // asset's name is left without one.
            let orphan = stem(name).filter(|&of| {
                matches!(entry, Entry::File(_))
                    && !matches!(entries.get(of), Some(Entry::File(_)))
                    && !claimed.contains(&**name)
            });
            if let Some(kind) = orphan.and_then(|of| AssetKind::of(Path::new(of))) {
                let file = dir.join(name);
                let meta = Meta::read(&file);
                let path = join(&at, name);
                scan.orphans.push(Orphan {
                    path,
                    file,
                    kind,
                    meta,
                });
            }
        }
        for (name, &entry) in &entries {
            if entry == Entry::Folder && !unbuilt(&at, name) {
                folders.push((dir.join(name), join(&at, name)));
            }
        }
    }
    scan.assets.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    scan.orphans.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    scan.unread.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    scan
}

/// A regular file [`files`] found: its path from the root, with `/` between its parts, such as
/// `sounds/dash.wav`, and the file, as the walk reached it.
#[derive(Debug, PartialEq)]
pub struct Found {
    pub path: String,
    pub file: PathBuf,
}

/// Every regular file under the asset root `root`, through the folders [`scan`] goes through, in
/// the order of their paths, and what the walk couldn't take, as `scan` gives it: what a game's
/// package is cooked from, as `tools/cook` picks from them.
pub fn files(root: &Path) -> (Vec<Found>, Vec<Unread>) {
    let (mut found, mut unread) = (Vec::new(), Vec::new());
    let mut folders = vec![(root.to_owned(), String::new())];
    while let Some((dir, at)) = folders.pop() {
        let Some(entries) = entries(&dir, &at, &mut unread) else {
            continue;
        };
        for (name, &entry) in &entries {
            match entry {
                Entry::File(_) => found.push(Found {
                    path: join(&at, name),
                    file: dir.join(name),
                }),
                Entry::Folder if !unbuilt(&at, name) => {
                    folders.push((dir.join(name), join(&at, name)));
                }
                _ => {}
            }
        }
    }
    found.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    unread.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    (found, unread)
}

/// Whether the folder `name`, of the folder at `at` from the root, `""` for the root itself, is
/// one a scan skips, as built rather than an asset's: every `target`, and at the top [`UNBUILT`]'s.
fn unbuilt(at: &str, name: &str) -> bool {
    name == "target" || (at.is_empty() && UNBUILT.contains(&name))
}

/// What an entry of a folder is, as its listing tells without following links.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// A regular file, with its stamp, `None` if the listing gave none.
    File(Option<Stamp>),
    Folder,
    /// A link, to a folder or to a file, which the scan never goes through: on Windows, a junction
    /// too.
    Link,
    /// Anything else, such as a FIFO or a device, which the scan never opens.
    Other,
}

/// The entries of folder `dir`, at `at` from the root, `""` for the root itself, by their names, but
/// those whose names start with `.`; or `None` if it can't be listed. Each entry the scan can't
/// take, and the folder if it can't be listed, is added to `unread`.
fn entries(dir: &Path, at: &str, unread: &mut Vec<Unread>) -> Option<BTreeMap<String, Entry>> {
    // A listing cut short may leave out an asset's ID file, so a folder is taken whole or not at
    // all.
    let listed = fs::read_dir(dir).and_then(|listing| listing.collect::<io::Result<Vec<_>>>());
    let listed = match listed {
        Ok(listed) => listed,
        Err(error) => {
            let path = if at.is_empty() { "." } else { at };
            unread.push(Unread {
                path: path.to_owned(),
                error: error.to_string(),
            });
            return None;
        }
    };
    let mut entries = BTreeMap::new();
    for entry in listed {
        let name = entry.file_name();
        if name.as_encoded_bytes().starts_with(b".") {
            continue;
        }
        let name = match name.into_string() {
            Ok(name) => name,
            // Names that don't round-trip can't be kept apart once made lossy, and a scene
            // couldn't name such a file anyway.
            Err(name) => {
                let path = join(at, &name.to_string_lossy());
                let error = "its name isn't Unicode".to_owned();
                unread.push(Unread { path, error });
                continue;
            }
        };
        let kind = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => Entry::Link,
            Ok(kind) if kind.is_dir() => Entry::Folder,
            // Windows' listing gives it with the type, and Unix's a stat that follows no link.
            Ok(kind) if kind.is_file() => {
                let metadata = entry.metadata().ok();
                Entry::File(metadata.and_then(|file| Some((file.modified().ok()?, file.len()))))
            }
            Ok(_) => Entry::Other,
            Err(error) => {
                let (path, error) = (join(at, &name), error.to_string());
                unread.push(Unread { path, error });
                continue;
            }
        };
        entries.insert(name, kind);
    }
    Some(entries)
}

/// The name of the file whose ID file `name` would be, by its extension, `.meta` whatever its
/// case: `name` without it; `None` for a name that doesn't end so.
fn stem(name: &str) -> Option<&str> {
    let path = Path::new(name);
    let meta =
        (path.extension()).is_some_and(|extension| extension.eq_ignore_ascii_case(Meta::EXTENSION));
    path.file_stem().and_then(OsStr::to_str).filter(|_| meta)
}

/// The path from the root of entry `name` of the folder at `at` from it, `""` for the root itself:
/// `/` between its parts.
fn join(at: &str, name: &str) -> String {
    if at.is_empty() {
        name.to_owned()
    } else {
        format!("{at}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    const STONE: &str = "(\n    id: \"5f3c9a1e07b2d846\",\n)\n";

    fn id(text: &str) -> AssetId {
        text.parse().expect("an ID")
    }

    #[test]
    fn an_asset_id_is_sixteen_lowercase_hex_digits_never_zero() {
        for text in ["5f3c9a1e07b2d846", "0000000000000001", "1234567890123456"] {
            assert_eq!(id(text).to_string(), text);
            assert_eq!(format!("{:?}", id(text)), text);
        }
        assert_eq!(id("0000000000000001").get(), 1);
        for text in [
            "",
            "5f3c9a1e07b2d84",
            "5f3c9a1e07b2d8461",
            "5F3C9A1E07B2D846",
            "0000000000000000",
            "5f3c9a1e07b2d84g",
            " f3c9a1e07b2d846",
            "+f3c9a1e07b2d846",
        ] {
            let failure = text.parse::<AssetId>().expect_err(text);
            assert_eq!(failure.code, "asset.bad_id", "{text:?}");
        }
    }

    /// A drawn ID's text has a letter but `e`, so that a JSON reader never takes it for a number,
    /// as `gg set` would read a value.
    #[test]
    fn a_drawn_id_holds_a_letter_and_never_reads_as_a_json_number() {
        let mut drawn = HashSet::new();
        for _ in 0..10_000 {
            let id = AssetId::draw();
            let text = id.to_string();
            assert!(text.contains(['a', 'b', 'c', 'd', 'f']), "{text}");
            let json = serde_json::from_str::<Value>(&text);
            assert!(!matches!(json, Ok(Value::Number(_))), "{text}");
            assert_eq!(text.parse(), Ok(id));
            drawn.insert(id);
        }
        assert_eq!(drawn.len(), 10_000, "each draw another");
    }

    /// An ID file reads back from the text it writes, and from what an editor may make of it: on
    /// a line, with Windows' line breaks, or after a byte-order mark.
    #[test]
    fn an_id_file_reads_and_writes_back_to_its_bytes() {
        let meta = Meta {
            id: id("5f3c9a1e07b2d846"),
        };
        assert_eq!(meta.text(), STONE);
        assert_eq!(Meta::parse(STONE), Ok(meta.clone()));
        for text in [
            "(id:\"5f3c9a1e07b2d846\")".to_owned(),
            STONE.replace('\n', "\r\n"),
            format!("\u{feff}{STONE}"),
        ] {
            assert_eq!(Meta::parse(&text), Ok(meta.clone()), "{text:?}");
        }
        assert_eq!(
            Meta::path(Path::new("assets/stone.png")),
            Path::new("assets/stone.png.meta")
        );
    }

    #[test]
    fn an_id_file_refuses_what_it_cant_be() {
        let refused = |text: &str| Meta::parse(text).expect_err(text);
        let unknown = STONE.replace("    id:", "    mipmaps: true,\n    id:");
        let upper = STONE.replace("5f3c9a1e07b2d846", "5F3C9A1E07B2D846");
        for (text, field) in [(&*unknown, "mipmaps"), ("(\n)\n", "id"), (&*upper, "id")] {
            let failure = refused(text);
            assert_eq!(failure.code, "asset.bad_meta");
            let named = format!("`{field}`");
            assert!(failure.message.contains(&named), "{failure:?}");
        }
        let syntax = refused("(id = \"5f3c9a1e07b2d846\")\n");
        assert_eq!(syntax.code, "asset.bad_meta");
        assert!(syntax.message.starts_with("1:"), "{syntax:?}");
        let unread = Meta::read(Path::new("no such folder/stone.png.meta")).unwrap_err();
        assert_eq!(unread.code, "asset.bad_meta");
        assert!(unread.message.starts_with("it can't be read"), "{unread:?}");
    }

    #[test]
    fn kinds_go_by_extension_whatever_its_case() {
        let kind = |name: &str| AssetKind::of(Path::new(name));
        for (name, expected) in [
            ("a.png", AssetKind::Texture),
            ("a.PNG", AssetKind::Texture),
            ("assets/b.jpg", AssetKind::Texture),
            ("b.JPEG", AssetKind::Texture),
            ("c.gltf", AssetKind::Model),
            ("c.GLB", AssetKind::Model),
            ("fonts/d.ttf", AssetKind::Font),
            ("d.OTF", AssetKind::Font),
        ] {
            assert_eq!(kind(name), Some(expected), "{name}");
        }
        let others = [
            "a.slang",
            "arena.ron",
            "notes.meta",
            "x.png.meta",
            "png",
            "d.woff2",
        ];
        for name in others {
            assert_eq!(kind(name), None, "{name}");
        }
        for name in KINDS {
            let kind: AssetKind = serde_json::from_value(Value::from(name)).expect(name);
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
    }

    /// A scan lists each texture, model and font with what its ID file holds, or why it doesn't,
    /// and the ID files left without their assets; but nothing in hidden folders, in `target`s, or
    /// in a package's own tests, benches and examples, and never through a link.
    #[test]
    fn a_scan_lists_assets_with_their_id_files_and_skips_what_it_should() {
        let root = std::env::temp_dir().join(format!("gg-api-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        let (case, gone, dir) = (
            id("0a1b2c3d4e5f6071"),
            id("1d2c3b4a59687f0e"),
            id("2b3c4d5e6f708192"),
        );
        write("assets/stone.png", "");
        write("assets/stone.png.meta", STONE);
        write("assets/new.jpg", "");
        write("assets/bad.png", "");
        write("assets/bad.png.meta", "(id: \"5f3c");
        write("assets/Case.png", "");
        write("assets/case.png.meta", &Meta { id: case }.text());
        write("assets/gone.png.meta", &Meta { id: gone }.text());
        // An ID file beside a folder of its asset's name, which isn't its asset.
        fs::create_dir_all(root.join("assets/dir.png")).unwrap();
        write("assets/dir.png.meta", &Meta { id: dir }.text());
        write("models/truck.glb", "");
        write("models/tree.gltf", "");
        fs::create_dir_all(root.join("models/tree.gltf.meta")).unwrap();
        write("ui/title.otf", "");
        write("notes.meta", "notes");
        write("shaders/a.slang", "");
        for skipped in ["target", "sub/target", "tests", ".hidden"] {
            write(&format!("{skipped}/x.png"), "");
        }
        write("sub/tests/y.png", "");

        /// Each asset's path, kind, and the ID its ID file holds or the code of why it doesn't.
        type Listed<'a> = (&'a str, AssetKind, Option<Result<AssetId, String>>);
        fn listed(found: &Scan) -> Vec<Listed<'_>> {
            let held = |meta: &Result<Meta, Failure>| {
                (meta.as_ref())
                    .map(|meta| meta.id)
                    .map_err(|failure| failure.code.clone())
            };
            (found.assets.iter())
                .map(|asset| (&*asset.path, asset.kind, asset.meta.as_ref().map(held)))
                .collect()
        }
        use AssetKind::{Font, Model, Texture};
        let code = |code: &str| Some(Err(code.to_owned()));
        let stone = Some(Ok(id("5f3c9a1e07b2d846")));
        let expected: [Listed; 8] = [
            ("assets/Case.png", Texture, code("asset.meta_case")),
            ("assets/bad.png", Texture, code("asset.bad_meta")),
            ("assets/new.jpg", Texture, None),
            ("assets/stone.png", Texture, stone),
            ("models/tree.gltf", Model, code("asset.bad_meta")),
            ("models/truck.glb", Model, None),
            ("sub/tests/y.png", Texture, None),
            ("ui/title.otf", Font, None),
        ];
        let found = scan(&root);
        assert_eq!(listed(&found), expected);
        let file = root.join("assets").join("stone.png");
        assert_eq!(found.assets[3].file, file);
        let length = |at: usize| found.assets[at].stamp.map(|(_, length)| length);
        assert_eq!((length(3), length(5)), (Some(0), Some(0)));
        assert_eq!(scan(&root), found, "nothing changed");
        // Every regular file, through the folders the scan goes through, in the order of its path.
        let (every, unread) = files(&root);
        let paths: Vec<&str> = every.iter().map(|found| &*found.path).collect();
        let expected = [
            "assets/Case.png",
            "assets/bad.png",
            "assets/bad.png.meta",
            "assets/case.png.meta",
            "assets/dir.png.meta",
            "assets/gone.png.meta",
            "assets/new.jpg",
            "assets/stone.png",
            "assets/stone.png.meta",
            "models/tree.gltf",
            "models/truck.glb",
            "notes.meta",
            "shaders/a.slang",
            "sub/tests/y.png",
            "ui/title.otf",
        ];
        assert_eq!(paths, expected);
        assert_eq!(every[7].file, file);
        assert_eq!(unread, []);
        let Some(Err(misnamed)) = &found.assets[0].meta else {
            panic!("a misnamed ID file: {:?}", found.assets[0]);
        };
        assert!(misnamed.message.contains("`case.png.meta`"), "{misnamed:?}");
        let misnamed = found.assets.iter().map(|asset| asset.misnamed.as_deref());
        let named = [
            Some("assets/case.png.meta"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ];
        assert_eq!(misnamed.collect::<Vec<_>>(), named);
        let Some(Err(folder)) = &found.assets[4].meta else {
            panic!("an ID file that's a folder: {:?}", found.assets[4]);
        };
        assert_eq!(folder.message, "it isn't a regular file");
        let orphans: Vec<_> = (found.orphans.iter())
            .map(|orphan| (&*orphan.path, orphan.kind, orphan.meta.clone()))
            .collect();
        let beside_folder = ("assets/dir.png.meta", Texture, Ok(Meta { id: dir }));
        let orphan = ("assets/gone.png.meta", Texture, Ok(Meta { id: gone }));
        assert_eq!(orphans, [beside_folder, orphan]);
        assert!(found.unread.is_empty(), "{:?}", found.unread);

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(root.join("assets"), root.join("linked")).unwrap();
            symlink(root.join("assets/stone.png"), root.join("alias.png")).unwrap();
            write("alias.png.meta", STONE);
            let found = scan(&root);
            assert_eq!(listed(&found), expected, "links skipped");
            let beside_link = found
                .orphans
                .iter()
                .any(|orphan| orphan.path == "alias.png.meta");
            assert!(beside_link, "an ID file beside a link: {:?}", found.orphans);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            fs::write(root.join(OsStr::from_bytes(b"bad\xff.png")), "").unwrap();
            let found = scan(&root);
            assert_eq!(listed(&found), expected);
            let unread: Vec<_> = (found.unread.iter())
                .map(|unread| (&*unread.path, &*unread.error))
                .collect();
            assert_eq!(unread, [("bad\u{fffd}.png", "its name isn't Unicode")]);
        }

        let unlisted = scan(&root.join("none"));
        assert!(unlisted.assets.is_empty());
        let unread: Vec<_> = unlisted.unread.iter().map(|unread| &*unread.path).collect();
        assert_eq!(unread, ["."], "the root itself");
        fs::remove_dir_all(&root).unwrap();
    }
}
