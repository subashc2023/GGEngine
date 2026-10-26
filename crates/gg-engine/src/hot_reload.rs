//! Hot reload of games' shaders, in development builds' windowed runs. A thread of its own,
//! `gg-watch`, watches the files each shader read, its own and those it includes, and compiles it
//! again as they change; the main thread puts what compiles in place of the shader, which the
//! next frame it hands the renderer draws with. A change that doesn't compile, or breaks the
//! contract that `Shaders::add` describes, warns, and the shader keeps drawing as it did.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use gg_core::{Error, Result};
use gg_renderer::{Compiled, ShaderSource, Shaders};
use tracing::{info, warn};

/// How long the thread waits between looks at the files. A change compiles on the look after the
/// one that sees it, in about a tenth of a second more.
const LOOK: Duration = Duration::from_millis(100);

/// The thread, from the main thread's side. Dropping it ends the thread once a compile under way
/// ends, without waiting for it: a compile can't be cut short, and a slangc that hangs would hang
/// the run's end.
pub(crate) struct HotReload {
    /// How many of the shaders' sources it's been sent.
    sent: usize,
    sources: Sender<ShaderSource>,
    compiled: Receiver<Compiled>,
}

impl HotReload {
    /// Starts the thread. Fails with `engine.hot_reload` if it can't start.
    pub(crate) fn start() -> Result<Self> {
        let (sources, to_watch) = mpsc::channel();
        let (back, compiled) = mpsc::channel();
        thread::Builder::new()
            // Within the 15 bytes Linux keeps of a thread's name.
            .name("gg-watch".to_owned())
            .spawn(move || watch(&to_watch, &back))
            .map_err(|error| {
                let message = "can't start the thread that watches shaders' files";
                Error::new("engine.hot_reload", message).with_source(error)
            })?;
        Ok(Self {
            sent: 0,
            sources,
            compiled,
        })
    }

    /// Sends the thread the games' shaders added since it last did, and puts each that came back
    /// compiled in place of the one it was compiled from. Once it's sent them all, it allocates
    /// nothing unless one came back.
    pub(crate) fn update(&mut self, shaders: &mut Shaders) {
        for source in &shaders.sources()[self.sent..] {
            // The thread runs until this side drops, or until it panics, which the log has.
            let _ = self.sources.send(source.clone());
        }
        self.sent = shaders.sources().len();
        while let Ok(compiled) = self.compiled.try_recv() {
            shaders.replace(compiled);
        }
    }
}

/// The thread: watches each shader it's sent, looking at its files every [`LOOK`] and sending
/// `back` what compiles again, until the main thread's side drops.
fn watch(sources: &Receiver<ShaderSource>, back: &Sender<Compiled>) {
    let mut watched: Vec<(ShaderSource, Files)> = Vec::new();
    loop {
        match sources.recv_timeout(LOOK) {
            Ok(source) => {
                let mut files = Files::new(source.path(), stamp);
                // To learn the files it includes: the shader drawing is what was added, so this
                // compile is dropped.
                compile(&source, &mut files, None);
                watched.push((source, files));
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let _zone = gg_core::zone!("look at shaders' files");
        for (source, files) in &mut watched {
            if files.look(stamp) {
                compile(source, files, Some(back));
            }
        }
    }
}

/// Compiles `source` again from its files, noting in `files` those it lists, and sends it `back`,
/// if given, if it compiles and keeps to the contract; or says why it doesn't.
fn compile(source: &ShaderSource, files: &mut Files, back: Option<&Sender<Compiled>>) {
    let path = source.path().display();
    match source.compile() {
        Ok(compiled) => {
            files.compiled(compiled.files(), stamp);
            if let Some(back) = back {
                info!(shader = source.name(), path = %path, "Shader reloaded");
                // Unless the main thread's side has dropped, which ends the thread next.
                let _ = back.send(compiled);
            }
        }
        Err(error) => warn!(
            code = error.code(),
            shader = source.name(),
            path = %path,
            "{error:#}; the shader keeps drawing as it did"
        ),
    }
}

/// When a file was last changed, and its length, which show that it changed; `None` while it
/// can't be read, as while an editor replaces it.
type Stamp = Option<(SystemTime, u64)>;

/// The file at `path`'s stamp, as the file system has it.
fn stamp(path: &Path) -> Stamp {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// The files a shader read, each with its stamp as the shader last compiled from it, which the
/// thread looks at to see them change.
struct Files {
    /// Its own first, then those it includes, as its last compile listed them.
    files: Vec<(PathBuf, Stamp)>,
    /// The files' stamps on the last look, where they differed from those it compiled from: a
    /// change, which compiles once it holds for a look, so that an editor's partial writes and
    /// atomic renames settle first.
    changing: Option<Vec<Stamp>>,
}

impl Files {
    /// The shader's own file at `path`, as `stamp` finds it now, before the compile that lists
    /// the rest.
    fn new(path: &Path, stamp: impl Fn(&Path) -> Stamp) -> Self {
        Self {
            files: vec![(path.to_owned(), stamp(path))],
            changing: None,
        }
    }

    /// Looks at the files, as `stamp` finds them, and returns whether the shader is to compile
    /// again: once they differ from what it last compiled from, and are as the look before found
    /// them. Its stamps are then the files', as the compile will find them, so that a change while
    /// it compiles, or one it read half done, compiles again; and a compile that fails waits for
    /// the next change. While the shader's own file is missing, it waits for it.
    fn look(&mut self, stamp: impl Fn(&Path) -> Stamp) -> bool {
        let now: Vec<Stamp> = self.files.iter().map(|(path, _)| stamp(path)).collect();
        if (self.files.iter().zip(&now)).all(|((_, then), now)| then == now) {
            self.changing = None;
            return false;
        }
        let held = self.changing.as_ref() == Some(&now) && now[0].is_some();
        if held {
            for ((_, then), now) in self.files.iter_mut().zip(now) {
                *then = now;
            }
            self.changing = None;
        } else {
            self.changing = Some(now);
        }
        held
    }

    /// Takes the files a compile listed, `listed`: those watched already keep their stamps, from
    /// before the compile, and the rest take `stamp`'s as it ends. One the shader newly includes
    /// that changes while it compiles waits for the next change.
    fn compiled(&mut self, listed: &[PathBuf], stamp: impl Fn(&Path) -> Stamp) {
        let files = (listed.iter())
            .map(|path| {
                let watched = self.files.iter().find(|(file, _)| file == path);
                let stamped = watched.map_or_else(|| stamp(path), |&(_, stamped)| stamped);
                (path.clone(), stamped)
            })
            .collect();
        self.files = files;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use gg_renderer::Blend;

    use super::*;
    use crate::math::Vec4;

    /// Files' stamps as a test sets them, a number for each version of a file.
    #[derive(Default)]
    struct Disk(RefCell<HashMap<PathBuf, Stamp>>);

    impl Disk {
        fn set(&self, path: &str, version: Option<u64>) {
            let stamp = version.map(|version| (SystemTime::UNIX_EPOCH, version));
            self.0.borrow_mut().insert(PathBuf::from(path), stamp);
        }

        fn stamp(&self) -> impl Fn(&Path) -> Stamp + '_ {
            |path| self.0.borrow().get(path).copied().flatten()
        }
    }

    /// A shader whose own file, `own.slang`, includes `shared.slang`, both as version 1.
    fn watched(disk: &Disk) -> Files {
        disk.set("own.slang", Some(1));
        disk.set("shared.slang", Some(1));
        let mut files = Files::new(Path::new("own.slang"), disk.stamp());
        let listed = ["own.slang", "shared.slang"].map(PathBuf::from);
        files.compiled(&listed, disk.stamp());
        files
    }

    #[test]
    fn a_change_compiles_once_it_holds_for_a_look() {
        let disk = Disk::default();
        let mut files = watched(&disk);
        assert!(!files.look(disk.stamp()), "nothing changed");
        // An edit to the file it includes, written in three goes, then left.
        for version in 2..5 {
            disk.set("shared.slang", Some(version));
            assert!(!files.look(disk.stamp()), "still changing at {version}");
        }
        assert!(files.look(disk.stamp()), "held for a look");
        assert!(!files.look(disk.stamp()), "compiled from as it is");
        assert_eq!(files.files[1].1, Some((SystemTime::UNIX_EPOCH, 4)));
    }

    #[test]
    fn a_change_while_it_compiles_or_after_it_fails_compiles_again() {
        let disk = Disk::default();
        let mut files = watched(&disk);
        disk.set("own.slang", Some(2));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
        // Changed again while it compiled, which lists its files with their stamps from before.
        disk.set("own.slang", Some(3));
        let listed = ["own.slang", "shared.slang"].map(PathBuf::from);
        files.compiled(&listed, disk.stamp());
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()), "the change during the compile");
        // That compile failed, which noted nothing: the same files don't compile again until the
        // next change.
        assert!(!files.look(disk.stamp()));
        assert!(!files.look(disk.stamp()));
        disk.set("own.slang", Some(4));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
    }

    #[test]
    fn a_missing_file_waits_until_it_returns() {
        let disk = Disk::default();
        let mut files = watched(&disk);
        // Replaced by an editor that deletes it first.
        disk.set("own.slang", None);
        for _ in 0..3 {
            assert!(!files.look(disk.stamp()), "its own file is missing");
        }
        disk.set("own.slang", Some(2));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
        // A file it includes that goes missing does compile, which fails, as slangc says why.
        disk.set("shared.slang", None);
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
    }

    #[test]
    fn files_a_compile_lists_anew_are_watched_from_then_on() {
        let disk = Disk::default();
        let mut files = watched(&disk);
        disk.set("other.slang", Some(7));
        let listed = ["own.slang", "other.slang"].map(PathBuf::from);
        files.compiled(&listed, disk.stamp());
        // The file it no longer includes, and the one it includes now.
        disk.set("shared.slang", Some(2));
        assert!(!files.look(disk.stamp()));
        assert!(!files.look(disk.stamp()));
        disk.set("other.slang", Some(8));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
    }

    #[test]
    fn an_edit_to_a_shaders_file_comes_back_compiled() {
        let dir = std::env::temp_dir().join(format!("gg-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a directory");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        let text = fs::read_to_string(fixture).expect("the fixture");
        let path = dir.join("flat_color.slang");
        fs::write(&path, &text).expect("the shader's file");
        let mut shaders = gg_renderer::Shaders::default();
        let flat = shaders.add::<[f32; 3], Vec4>("flat", &path, Blend::Opaque);
        flat.expect("a shader");

        let (sources, to_watch) = mpsc::channel();
        let (back, compiled) = mpsc::channel();
        let thread = thread::spawn(move || watch(&to_watch, &back));
        let source = shaders.sources()[0].clone();
        sources.send(source).expect("the thread");
        // Edited until an edit comes back, since one before the thread first looks at the file
        // goes unseen: it's what the shader compiled from.
        let mut came_back = None;
        for edit in 0..40 {
            fs::write(&path, format!("{text}\n// Edit {edit}.\n")).expect("an edit");
            if let Ok(compiled) = compiled.recv_timeout(Duration::from_millis(500)) {
                came_back = Some(compiled);
                break;
            }
        }
        let came_back = came_back.expect("an edit comes back compiled");
        assert_eq!(came_back.source(), &shaders.sources()[0]);
        assert_eq!(came_back.files().len(), 1, "{came_back:?}");
        shaders.replace(came_back);
        drop(sources);
        thread.join().expect("the thread ends as its side drops");
        let _ = fs::remove_dir_all(&dir);
    }
}
