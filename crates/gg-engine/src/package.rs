//! A game's package: the files under its project's asset root that its runs read, in one file,
//! cooked where the engine would otherwise make something of them as they load, as a font's field
//! or a shader's SPIR-V, which `tools/cook` writes and a build without `dev` runs from, as
//! ARCHITECTURE.md's Project state says; and [`Files`], where a run reads them, its package or the
//! disk.
//!
//! The format, every number little-endian:
//!
//! ```text
//! header   32 bytes: "GGPK", the version (1), the number of entries, the string table's length,
//!          then the asset root's path and the start scene's, each from the project's directory,
//!          as its manifest gives them, and each its start and length in the string table, the
//!          start's 0 long for none
//! entries  32 bytes each, in the order of their paths' bytes: an ID (0 for none), the offset and
//!          length of its data, its path's start in the string table, its path's length (16 bits),
//!          its kind and its codec (8 bits each, the codec 0, stored as it is)
//! strings  the paths, UTF-8, `/` between their parts
//! data     each entry's, at an offset that's a multiple of 16
//! ```
//!
//! Offsets are from the file's start and point nowhere else, so that the file can be mapped into
//! memory and read in place; a run reads it at those offsets instead, as mapping a file is unsafe
//! where another process may change it, and an error reading a mapped page ends the process.

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use gg_core::{Error, Result};

use crate::dirs;

const MAGIC: &[u8; 4] = b"GGPK";
const VERSION: u32 = 1;
const HEADER: usize = 32;
const ENTRY: usize = 32;
/// What each entry's data starts at a multiple of, so that data mapped into memory is aligned for
/// any of the GPU's types.
const ALIGN: u64 = 16;

/// What an entry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The file as it is under the asset root, as a texture's PNG or a scene's text.
    File,
    /// A font's file, baked as [`Context::add_font`](crate::Context::add_font) bakes it.
    Font,
    /// A game's shader, compiled to SPIR-V as
    /// [`Context::add_shader`](crate::Context::add_shader) compiles it.
    Shader,
}

impl Kind {
    fn byte(self) -> u8 {
        match self {
            Self::File => 0,
            Self::Font => 1,
            Self::Shader => 2,
        }
    }

    fn of(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::File,
            1 => Self::Font,
            2 => Self::Shader,
            _ => return None,
        })
    }

    /// What it is, in words, as errors say.
    fn words(self) -> &'static str {
        match self {
            Self::File => "a file",
            Self::Font => "a baked font",
            Self::Shader => "a compiled shader",
        }
    }
}

/// An entry of a package, as its table has it.
#[derive(Clone, Debug)]
struct Entry {
    path: Box<str>,
    id: u64,
    offset: u64,
    len: u64,
    kind: Kind,
}

/// A game's package, opened: its table, read whole as it opens, and its file, whose entries' data
/// it reads as they're asked for.
#[derive(Debug)]
pub(crate) struct Package {
    file: File,
    /// Where it was opened from, for errors.
    path: PathBuf,
    /// In the order of their paths.
    entries: Vec<Entry>,
    /// Each entry's index, by its ID, for those with one.
    by_id: BTreeMap<u64, usize>,
    /// The asset root's path from the project's directory, `/` between its parts.
    assets: Box<str>,
    /// The start scene's path from the project's directory.
    start: Option<Box<str>>,
}

impl Package {
    /// Opens the package at `path`, reading its table.
    ///
    /// Fails with `package.unreadable` if the file can't be opened or read, and with `package.bad`
    /// if it isn't a package of this version, or its table doesn't hold together: an entry past
    /// the file's end, a path that isn't UTF-8, paths out of order or twice, an ID twice, or a
    /// kind or a codec this engine doesn't know.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let unreadable = |error: io::Error| {
            let message = format!("can't read the package {}", path.display());
            Error::new("package.unreadable", message).with_source(error)
        };
        let bad = |problem: String| {
            let message = format!(
                "{} isn't a package this engine reads: {problem}",
                path.display()
            );
            Error::new("package.bad", message)
        };
        // Before it's opened, since opening a FIFO waits for a writer.
        let metadata = std::fs::metadata(path).map_err(unreadable)?;
        if !metadata.is_file() {
            return Err(bad("it isn't a regular file".to_owned()));
        }
        let file = File::open(path).map_err(unreadable)?;
        let size = metadata.len();
        let mut header = [0; HEADER];
        if size < HEADER as u64 {
            return Err(bad(format!("it's {size} bytes, shorter than a header")));
        }
        read_at(&file, 0, &mut header).map_err(unreadable)?;
        if &header[..4] != MAGIC {
            return Err(bad("it doesn't begin with GGPK".to_owned()));
        }
        let version = u32_at(&header, 4);
        if version != VERSION {
            return Err(bad(format!("it's of version {version}, not {VERSION}")));
        }
        let count = u64::from(u32_at(&header, 8));
        let strings_len = u64::from(u32_at(&header, 12));
        let table_len = (count * ENTRY as u64) + strings_len;
        if HEADER as u64 + table_len > size {
            return Err(bad("its table runs past its end".to_owned()));
        }
        let mut table = Vec::new();
        table
            .try_reserve_exact(table_len as usize)
            .map_err(|error| unreadable(io::Error::other(error)))?;
        table.resize(table_len as usize, 0);
        read_at(&file, HEADER as u64, &mut table).map_err(unreadable)?;
        let (rows, strings) = table.split_at(count as usize * ENTRY);
        let string = |start: u32, len: u64| -> std::result::Result<Box<str>, String> {
            let start = start as usize;
            let bytes = (strings.get(start..start + len as usize))
                .ok_or("a path runs past the string table")?;
            let text = std::str::from_utf8(bytes).map_err(|_| "a path isn't UTF-8")?;
            Ok(text.into())
        };
        let assets = string(u32_at(&header, 16), u64::from(u32_at(&header, 20))).map_err(bad)?;
        let start_len = u64::from(u32_at(&header, 28));
        let start = match start_len {
            0 => None,
            len => Some(string(u32_at(&header, 24), len).map_err(bad)?),
        };
        // As a manifest's check keeps them, inside the project's directory.
        let inside = |path: &str| {
            !(path.split('/'))
                .any(|part| matches!(part, "" | "." | "..") || part.contains([':', '\\']))
        };
        if !(&*assets == "." || inside(&assets))
            || start.as_deref().is_some_and(|start| !inside(start))
        {
            return Err(bad(
                "its asset root or its start scene isn't a path inside the project's directory"
                    .to_owned(),
            ));
        }
        let mut entries: Vec<Entry> = Vec::new();
        let mut by_id = BTreeMap::new();
        for row in rows.as_chunks::<ENTRY>().0 {
            let id = u64_at(row, 0);
            let offset = u64_at(row, 8);
            let len = u64_at(row, 16);
            let path = string(u32_at(row, 24), u64::from(u16_at(row, 28))).map_err(bad)?;
            let kind = Kind::of(row[30])
                .ok_or_else(|| bad(format!("`{path}` is of a kind it doesn't know")))?;
            if row[31] != 0 {
                return Err(bad(format!(
                    "`{path}` is compressed in a way it doesn't know"
                )));
            }
            if !offset.is_multiple_of(ALIGN) || offset.checked_add(len).is_none_or(|end| end > size)
            {
                return Err(bad(format!("`{path}`'s data isn't within it, aligned")));
            }
            if entries.last().is_some_and(|last| *last.path >= *path) {
                return Err(bad(format!("`{path}` is out of order, or there twice")));
            }
            if id != 0 && by_id.insert(id, entries.len()).is_some() {
                return Err(bad(format!("`{path}`'s ID, {id:016x}, is another's too")));
            }
            entries.push(Entry {
                path,
                id,
                offset,
                len,
                kind,
            });
        }
        Ok(Self {
            file,
            path: path.to_owned(),
            entries,
            by_id,
            assets,
            start,
        })
    }

    /// The asset root's path from the project's directory, `/` between its parts, as the project's
    /// manifest gives it.
    pub(crate) fn assets(&self) -> &str {
        &self.assets
    }

    /// The start scene's path from the project's directory, as its manifest gives it, if the
    /// project has one.
    pub(crate) fn start(&self) -> Option<&str> {
        self.start.as_deref()
    }

    fn entry(&self, path: &str) -> Option<&Entry> {
        let found = self
            .entries
            .binary_search_by(|entry| (*entry.path).cmp(path));
        found.ok().map(|at| &self.entries[at])
    }

    /// The ID of the entry at `path` from the asset root, if it's there and has one.
    pub(crate) fn id(&self, path: &str) -> Option<u64> {
        self.entry(path).map(|entry| entry.id).filter(|&id| id != 0)
    }

    /// The path from the asset root of the entry whose ID is `id`, if one has it.
    pub(crate) fn path_of(&self, id: u64) -> Option<&str> {
        self.by_id.get(&id).map(|&at| &*self.entries[at].path)
    }

    /// Reads the entry at `path` from the asset root, of `kind`, as far as its first `most` bytes,
    /// as [`dirs::read_file`] reads a file.
    ///
    /// Fails with `NotFound` for a path the package hasn't, and with `InvalidData` for an entry of
    /// another kind.
    pub(crate) fn read(&self, path: &str, kind: Kind, most: u64) -> io::Result<Vec<u8>> {
        let Some(entry) = self.entry(path) else {
            let message = format!("the package {} hasn't `{path}`", self.path.display());
            return Err(io::Error::new(io::ErrorKind::NotFound, message));
        };
        if entry.kind != kind {
            let message = format!(
                "the package {} has `{path}` as {}, not {}",
                self.path.display(),
                entry.kind.words(),
                kind.words()
            );
            return Err(io::Error::new(io::ErrorKind::InvalidData, message));
        }
        let len = entry.len.min(most);
        let mut bytes = Vec::new();
        // An error rather than an abort if there's no memory for it, as `dirs::read_file` gives.
        bytes.try_reserve_exact(usize::try_from(len).map_err(io::Error::other)?)?;
        bytes.resize(len as usize, 0);
        read_at(&self.file, entry.offset, &mut bytes)?;
        Ok(bytes)
    }
}

/// Where a run reads the files under its asset root: the disk, or its game's package, in which
/// each is found by its path from the root, `root`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Files {
    package: Option<(Arc<Package>, PathBuf)>,
}

impl Files {
    /// The files under `root` as `package` holds them.
    pub(crate) fn package(package: Arc<Package>, root: PathBuf) -> Self {
        Self {
            package: Some((package, root)),
        }
    }

    /// The run's package, if it reads one.
    pub(crate) fn packaged(&self) -> Option<&Package> {
        self.package.as_ref().map(|(package, _)| &**package)
    }

    /// Reads the file at `path`, as far as its first `most` bytes, as [`dirs::read_file`] does: from
    /// the package, if the run reads one, the file as it is under the asset root.
    pub(crate) fn read(&self, path: &Path, most: u64) -> io::Result<Vec<u8>> {
        self.read_as(path, Kind::File, most)
    }

    /// Reads what the package holds of `kind` for the file at `path`, as far as its first `most`
    /// bytes, as [`Package::read`] does; and from the disk, if the run reads no package, the file
    /// as it is, whatever `kind` says.
    pub(crate) fn read_as(&self, path: &Path, kind: Kind, most: u64) -> io::Result<Vec<u8>> {
        match &self.package {
            None => dirs::read_file(path, most),
            Some((package, root)) => {
                let Some(within) = within(root, path) else {
                    let message = format!(
                        "{} is outside the asset root, {}, whose files alone the package holds",
                        path.display(),
                        root.display()
                    );
                    return Err(io::Error::new(io::ErrorKind::NotFound, message));
                };
                package.read(&within, kind, most)
            }
        }
    }

    /// The file at `path`, as one file however its path is written, as [`assets::file_of`]
    /// resolves it on the disk: in the package, its path from the root joined to the root, its
    /// `.` and `..` taken away as written; `None` for a path outside the root, or one the package
    /// hasn't.
    ///
    /// [`assets::file_of`]: crate::assets::file_of
    pub(crate) fn file_of(&self, path: &Path) -> Option<PathBuf> {
        match &self.package {
            None => crate::assets::file_of(path),
            Some((package, root)) => {
                let within = within(root, path)?;
                package.entry(&within)?;
                Some(root.join(within))
            }
        }
    }

    /// The path from the asset root of the file at `path`, as the package names its entries, if
    /// the run reads one and the path is inside the root, as written.
    pub(crate) fn within(&self, path: &Path) -> Option<String> {
        let (_, root) = self.package.as_ref()?;
        within(root, path)
    }
}

/// `path`, relative, `/` between its parts, joined to `dir` a part at a time, so that its separators
/// are the OS's: `dir` itself for `.`, as gg-api joins a manifest's paths.
pub(crate) fn under(dir: &Path, path: &str) -> PathBuf {
    let parts = path.split('/').filter(|&part| part != ".");
    parts.fold(dir.to_owned(), |under, part| under.join(part))
}

/// The path from `root` of the file at `path`, as written, `/` between its parts: as a package names
/// its entries. Both are taken as the disk takes them, a relative path from the working directory,
/// but without reading it, their `.` and `..` taken away as written. `None` for a path outside the
/// root, or the root itself.
fn within(root: &Path, path: &Path) -> Option<String> {
    let (root, path) = (lexical(root)?, lexical(path)?);
    let parts = path.strip_prefix(root).ok()?.components();
    let parts: Option<Vec<&str>> = parts.map(|part| part.as_os_str().to_str()).collect();
    let parts = parts?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// `path` made absolute, from the working directory if it's relative, its `.` and `..` taken away as
/// written, without reading the disk; `None` if the working directory can't be had.
fn lexical(path: &Path) -> Option<PathBuf> {
    let mut lexical = PathBuf::new();
    for component in std::path::absolute(path).ok()?.components() {
        match component {
            Component::CurDir => {}
            // At the top, `..` is the top, as it is on the disk.
            Component::ParentDir => {
                lexical.pop();
            }
            component => lexical.push(component),
        }
    }
    Some(lexical)
}

/// Reads `buf.len()` bytes of `file` from `offset`, without moving a cursor another read shares.
fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        let mut done = 0;
        while done < buf.len() {
            let read = std::os::windows::fs::FileExt::seek_read(
                file,
                &mut buf[done..],
                offset + done as u64,
            )?;
            if read == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            done += read;
        }
        Ok(())
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"))
}

/// A package as `tools/cook` makes it, entry by entry, which it writes whole.
#[derive(Debug, Default)]
pub struct Writer {
    assets: String,
    start: Option<String>,
    /// Each entry's ID, 0 for none, its kind and its data, by its path.
    entries: BTreeMap<String, (u64, Kind, Vec<u8>)>,
}

impl Writer {
    /// A package of a project whose asset root is at `assets` from its directory, and whose start
    /// scene, if it has one, is at `start` from it, each with `/` between its parts, as its
    /// manifest gives them.
    pub fn new(assets: &str, start: Option<&str>) -> Self {
        Self {
            assets: assets.to_owned(),
            start: start.map(str::to_owned),
            entries: BTreeMap::new(),
        }
    }

    /// Adds the entry at `path` from the asset root, `/` between its parts, of `kind`, holding
    /// `data`, with the ID `id` names, if it has one.
    ///
    /// Fails with `package.bad` for a path it has, a path longer than 65,535 bytes or of an empty
    /// part, `.` or `..`, or an ID another entry has.
    pub fn add(&mut self, path: &str, id: Option<u64>, kind: Kind, data: Vec<u8>) -> Result<()> {
        let bad = |problem: &str| {
            Error::new(
                "package.bad",
                format!("can't add `{path}` to a package: {problem}"),
            )
        };
        if path.len() > usize::from(u16::MAX) {
            return Err(bad("its path is longer than 65,535 bytes"));
        }
        if path.split('/').any(|part| matches!(part, "" | "." | "..")) {
            return Err(bad("its path has an empty part, `.` or `..`"));
        }
        let id = id.unwrap_or(0);
        if id != 0 && self.entries.values().any(|&(other, ..)| other == id) {
            return Err(bad(&format!("another entry has its ID, {id:016x}")));
        }
        if self.entries.contains_key(path) {
            return Err(bad("it's there already"));
        }
        self.entries.insert(path.to_owned(), (id, kind, data));
        Ok(())
    }

    /// The package, as its file holds it: its entries in the order of their paths' bytes, so that
    /// the same entries make the same bytes.
    ///
    /// Fails with `package.bad` past 4 GiB of paths or 2³² entries, which the table can't count.
    // Made whole in memory, as packages are some megabytes: one of gigabytes would want writing as
    // each entry is cooked.
    pub fn bytes(&self) -> Result<Vec<u8>> {
        let too_big = || Error::new("package.bad", "a package's table can't count so many");
        let mut strings = Vec::new();
        let mut string = |text: &str| -> Result<(u32, u32)> {
            let start = u32::try_from(strings.len()).map_err(|_| too_big())?;
            strings.extend_from_slice(text.as_bytes());
            let len = u32::try_from(text.len()).map_err(|_| too_big())?;
            Ok((start, len))
        };
        let assets = string(&self.assets)?;
        let start = match &self.start {
            Some(start) => string(start)?,
            None => (0, 0),
        };
        let mut paths = Vec::with_capacity(self.entries.len());
        for path in self.entries.keys() {
            paths.push(string(path)?);
        }
        let count = u32::try_from(self.entries.len()).map_err(|_| too_big())?;
        let strings_len = u32::try_from(strings.len()).map_err(|_| too_big())?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        for number in [
            VERSION,
            count,
            strings_len,
            assets.0,
            assets.1,
            start.0,
            start.1,
        ] {
            bytes.extend_from_slice(&number.to_le_bytes());
        }
        let table_end = (HEADER + self.entries.len() * ENTRY + strings.len()) as u64;
        let mut offset = table_end.next_multiple_of(ALIGN);
        for ((_, (id, kind, data)), (start, len)) in self.entries.iter().zip(&paths) {
            bytes.extend_from_slice(&id.to_le_bytes());
            bytes.extend_from_slice(&offset.to_le_bytes());
            bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&start.to_le_bytes());
            bytes.extend_from_slice(&(*len as u16).to_le_bytes());
            bytes.extend_from_slice(&[kind.byte(), 0]);
            offset = (offset + data.len() as u64).next_multiple_of(ALIGN);
        }
        bytes.extend_from_slice(&strings);
        for (_, _, data) in self.entries.values() {
            bytes.resize(bytes.len().next_multiple_of(ALIGN as usize), 0);
            bytes.extend_from_slice(data);
        }
        Ok(bytes)
    }

    /// Writes the package to the file at `path`, whole or not at all, making the directories it's
    /// in.
    ///
    /// Fails as [`bytes`](Self::bytes) does, and with `package.unwritten` if the file can't be
    /// written.
    pub fn write(&self, path: &Path) -> Result<()> {
        let bytes = self.bytes()?;
        dirs::write(path, &bytes).map_err(|error| {
            let message = format!("can't write the package {}", path.display());
            Error::new("package.unwritten", message).with_source(error)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package written in a test's own directory.
    fn written(writer: &Writer, name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gg-package-{}-{name}", std::process::id()));
        let path = dir.join("game.ggpk");
        writer.write(&path).expect("written");
        path
    }

    /// What's written reads back: each entry by its path, of its kind, its ID both ways, the asset
    /// root and the start scene; and the same entries, added in any order, make the same bytes.
    #[test]
    fn a_package_reads_back_what_was_written() {
        let mut writer = Writer::new(".", Some("scenes/arena.ron"));
        let id = 0x5f3c_9a1e_07b2_d846;
        writer
            .add("textures/stone.png", Some(id), Kind::File, vec![1, 2, 3])
            .unwrap();
        writer
            .add("shaders/floor.slang", None, Kind::Shader, vec![7; 33])
            .unwrap();
        writer
            .add("scenes/arena.ron", None, Kind::File, b"()".to_vec())
            .unwrap();
        let path = written(&writer, "reads");
        let package = Package::open(&path).unwrap();
        assert_eq!(package.assets(), ".");
        assert_eq!(package.start(), Some("scenes/arena.ron"));
        let stone = package
            .read("textures/stone.png", Kind::File, u64::MAX)
            .unwrap();
        assert_eq!(stone, [1, 2, 3]);
        assert_eq!(
            package.read("textures/stone.png", Kind::File, 2).unwrap(),
            [1, 2]
        );
        let floor = package
            .read("shaders/floor.slang", Kind::Shader, u64::MAX)
            .unwrap();
        assert_eq!(floor, [7; 33]);
        assert_eq!(package.id("textures/stone.png"), Some(id));
        assert_eq!(package.id("scenes/arena.ron"), None);
        assert_eq!(package.path_of(id), Some("textures/stone.png"));
        let missing = package
            .read("textures/gone.png", Kind::File, u64::MAX)
            .unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        let wrong = package
            .read("shaders/floor.slang", Kind::File, u64::MAX)
            .unwrap_err();
        assert_eq!(wrong.kind(), io::ErrorKind::InvalidData);

        let mut reversed = Writer::new(".", Some("scenes/arena.ron"));
        reversed
            .add("scenes/arena.ron", None, Kind::File, b"()".to_vec())
            .unwrap();
        reversed
            .add("shaders/floor.slang", None, Kind::Shader, vec![7; 33])
            .unwrap();
        reversed
            .add("textures/stone.png", Some(id), Kind::File, vec![1, 2, 3])
            .unwrap();
        assert_eq!(reversed.bytes().unwrap(), writer.bytes().unwrap());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A writer refuses a path twice, an ID twice and paths with empty parts, `.` or `..`.
    #[test]
    fn a_writer_refuses_what_wouldnt_read_back() {
        let mut writer = Writer::new(".", None);
        writer
            .add("a.png", Some(1), Kind::File, Vec::new())
            .unwrap();
        for (path, id) in [
            ("a.png", None),
            ("b.png", Some(1)),
            ("c//d.png", None),
            ("../e.png", None),
        ] {
            let error = writer.add(path, id, Kind::File, Vec::new()).unwrap_err();
            assert_eq!(error.code(), "package.bad", "{path}");
        }
    }

    /// A file that isn't a package, or whose table doesn't hold together, or whose asset root or
    /// start scene a manifest couldn't have, is refused with `package.bad`, and one that isn't
    /// there with `package.unreadable`.
    #[test]
    fn a_package_that_doesnt_hold_together_is_refused() {
        let mut writer = Writer::new("assets", None);
        writer
            .add("a.png", Some(1), Kind::File, vec![0; 5])
            .unwrap();
        writer
            .add("b.png", Some(2), Kind::File, vec![0; 5])
            .unwrap();
        let path = written(&writer, "refused");
        let good = std::fs::read(&path).unwrap();
        assert!(Package::open(&path).is_ok());
        let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
        let mut magic = good.clone();
        magic[0] = b'X';
        cases.push(("magic", magic));
        let mut version = good.clone();
        version[4] = 2;
        cases.push(("version", version));
        cases.push(("short", good[..HEADER + ENTRY].to_vec()));
        // The second entry's ID made the first's.
        let mut twice = good.clone();
        twice[HEADER + ENTRY] = 1;
        cases.push(("ID twice", twice));
        // The second entry's offset past the end.
        let mut past = good.clone();
        past[HEADER + ENTRY + 8..HEADER + ENTRY + 16].copy_from_slice(&u64::MAX.to_le_bytes());
        cases.push(("past the end", past));
        let mut kind = good.clone();
        kind[HEADER + 30] = 9;
        cases.push(("kind", kind));
        let mut codec = good.clone();
        codec[HEADER + 31] = 1;
        cases.push(("codec", codec));
        for (case, bytes) in cases {
            std::fs::write(&path, bytes).unwrap();
            let error = Package::open(&path).unwrap_err();
            assert_eq!(error.code(), "package.bad", "{case}: {error:#}");
        }
        // An asset root or a start scene outside the project's directory, which no manifest has.
        let outside = [
            ("..", None),
            ("assets", Some("../main.ron")),
            ("C:/assets", None),
        ];
        for (assets, start) in outside {
            let path = written(&Writer::new(assets, start), "outside");
            let error = Package::open(&path).unwrap_err();
            assert_eq!(error.code(), "package.bad", "{assets}: {error:#}");
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
        let gone = Package::open(&path.with_file_name("gone.ggpk")).unwrap_err();
        assert_eq!(gone.code(), "package.unreadable");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A path is found in the package by its path from the root as written, `.` and `..` taken
    /// away, and a relative path or root from the working directory, as on the disk; none outside
    /// the root is.
    #[test]
    fn paths_are_found_by_their_paths_from_the_root() {
        let root = std::env::temp_dir().join("game");
        assert_eq!(
            within(&root, &root.join("textures/stone.png")).as_deref(),
            Some("textures/stone.png")
        );
        assert_eq!(
            within(&root, &root.join("a/../b/./c.png")).as_deref(),
            Some("b/c.png")
        );
        assert_eq!(
            within(&root, &root.join("../game/x.png")).as_deref(),
            Some("x.png")
        );
        // A relative root, as a project's directory may be, holds the relative paths under it.
        let relative = Path::new("games/arena");
        assert_eq!(
            within(relative, &relative.join("scenes/arena.ron")).as_deref(),
            Some("scenes/arena.ron")
        );
        let working = std::env::current_dir().unwrap();
        assert_eq!(
            within(&working.join(relative), Path::new("games/arena/x.png")).as_deref(),
            Some("x.png")
        );
        assert_eq!(within(relative, Path::new("scenes/arena.ron")), None);
        assert_eq!(within(&root, &root.join("../other/x.png")), None);
        assert_eq!(within(&root, &std::env::temp_dir().join("x.png")), None);
        assert_eq!(within(&root, &root), None);
    }
}
