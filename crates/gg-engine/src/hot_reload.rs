//! Hot reload of games' shaders, in development builds' windowed runs. A thread of its own,
//! `gg-watch`, watches the files each shader read, its own and those it includes, and compiles it
//! again as they change; the main thread puts what compiles in place of the shader, which the
//! next frame it hands the renderer draws with. A change that doesn't compile, or breaks the
//! contract that `Shaders::add` describes, warns, and the shader keeps drawing as it did; until
//! one compiles, a change to any file beside its files compiles it again too, such as an include
//! it couldn't find, since a compile that fails lists none of the files it read.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use gg_core::{Error, Result};
use gg_renderer::{Compiled, ShaderSource, Shaders};
use tracing::{debug, info, warn};

/// How long the thread waits between looks at the files. A change compiles on the look after the
/// one that sees it, in about a tenth of a second more.
const LOOK: Duration = Duration::from_millis(100);

/// How long before the compile that added a shader its files must have last changed for it to
/// have read them as they are: files' times can trail the clock that compile's start is read
/// from, as Windows and Linux stamp them from a clock that moves a tick at a time, and FAT keeps
/// them to 2 s. A file changed within it compiles the shader again, at worst for nothing.
const MARGIN: Duration = Duration::from_secs(2);

/// How many looks a change to a failing shader's files waits for the directories beside them to
/// hold too: something may write beside them all the time, such as a log, which would otherwise
/// keep the change from compiling.
const SETTLE: u32 = 10;

/// How long the run's end waits for a compile under way, which slangc ends in about a tenth of a
/// second, so that it removes what slangc wrote; one that hangs holds the end up no longer.
const STOP: Duration = Duration::from_secs(1);

/// The thread, from the main thread's side, which [`HotReload::stop`] ends. Dropping it ends the
/// thread too, once a compile under way ends, without waiting for it: a compile can't be cut
/// short.
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
    /// compiled in place of the one it was compiled from, logging `Shader reloaded`. Once it's
    /// sent them all, it allocates nothing unless one came back.
    pub(crate) fn update(&mut self, shaders: &mut Shaders) {
        for source in &shaders.sources()[self.sent..] {
            // The thread runs until this side drops, or until it panics, which the log has.
            let _ = self.sources.send(source.clone());
        }
        self.sent = shaders.sources().len();
        while let Ok(compiled) = self.compiled.try_recv() {
            let source = compiled.source();
            info!(shader = source.name(), path = %source.path().display(), "Shader reloaded");
            shaders.replace(compiled);
        }
    }

    /// Ends the thread, which starts no compile once this side lets go, waiting up to [`STOP`]
    /// for one under way, so that it removes what slangc wrote: slangc outlives a process that
    /// ends first, and writes it after. What compiles meanwhile isn't put in place.
    pub(crate) fn stop(self) {
        let Self {
            sources, compiled, ..
        } = self;
        drop(sources);
        let until = Instant::now() + STOP;
        // Until the thread ends, letting go of its side.
        while (compiled.recv_timeout(until.saturating_duration_since(Instant::now()))).is_ok() {}
    }
}

/// The thread: watches each shader it's sent, looking at its files every [`LOOK`] and sending
/// `back` what compiles again, until the main thread's side drops.
fn watch(sources: &Receiver<ShaderSource>, back: &Sender<Compiled>) {
    let mut watched: Vec<(ShaderSource, Files)> = Vec::new();
    loop {
        match sources.recv_timeout(LOOK) {
            Ok(source) => {
                watched.push(watching(source));
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let _zone = gg_core::zone!("look at shaders' files");
        for at in 0..watched.len() {
            if !watched[at].1.look(stamp) {
                continue;
            }
            // No compile starts once the main thread's side has let go, which waits for the one
            // under way alone. Shaders sent meanwhile are looked at from the next look.
            if !take(sources, &mut watched) {
                return;
            }
            let (source, files) = &mut watched[at];
            compile(source, files, back);
        }
    }
}

/// A shader the thread has been sent, with its files as it was added.
fn watching(source: ShaderSource) -> (ShaderSource, Files) {
    let _zone = gg_core::zone!();
    let files = Files::new(source.files(), source.compiled_at(), stamp);
    (source, files)
}

/// Takes the shaders the thread has been sent since it last looked, if any: false once the main
/// thread's side has let go.
fn take(sources: &Receiver<ShaderSource>, watched: &mut Vec<(ShaderSource, Files)>) -> bool {
    loop {
        match sources.try_recv() {
            Ok(source) => watched.push(watching(source)),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

/// Compiles `source` again from its files, noting in `files` those it lists, and sends it `back`
/// if it compiles and keeps to the contract; or says why it doesn't, unless it said so last time,
/// from the same files.
fn compile(source: &ShaderSource, files: &mut Files, back: &Sender<Compiled>) {
    let path = source.path().display();
    // As the compile begins, so that a change while it fails compiles it again.
    let dirs = files.dirs(stamp);
    match source.compile() {
        Ok(compiled) => {
            files.compiled(compiled.files(), stamp);
            // Unless the main thread's side has let go, which ends the thread next.
            let _ = back.send(compiled);
        }
        Err(error) => {
            let why = format!("{error:#}");
            if files.failed(dirs, &why) {
                warn!(
                    code = error.code(),
                    shader = source.name(),
                    path = %path,
                    "{why}; the shader keeps drawing as it did"
                );
            } else {
                let (code, shader) = (error.code(), source.name());
                debug!(code, shader, path = %path, "Shader fails as it did");
            }
        }
    }
}

/// When a file was last changed, and its length, which show that it changed; `None` while it
/// can't be read, as while an editor replaces it.
type Stamp = Option<(SystemTime, u64)>;

/// The file at `path`'s stamp, as the file system has it. A directory's is when the latest of its
/// entries changed, and a digest of their names and stamps for its length, which changes as a
/// file in it is made, removed or changed, but not as one in a directory within it is changed.
fn stamp(path: &Path) -> Stamp {
    let metadata = fs::metadata(path).ok()?;
    if metadata.is_dir() {
        return entries(path);
    }
    Some((metadata.modified().ok()?, metadata.len()))
}

/// The stamp of the directory at `dir`, from its entries, which it reads each time: a ceiling in a
/// directory of many files, which the OS's notifications of changes would lift.
fn entries(dir: &Path) -> Stamp {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).ok()? {
        // One removed as it's listed is gone.
        let Ok(entry) = entry else { continue };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        entries.push((entry.file_name(), modified, metadata.len()));
    }
    // Directories list their entries in no particular order.
    entries.sort_unstable();
    let mut digest = DefaultHasher::new();
    entries.hash(&mut digest);
    let latest = entries.iter().map(|&(_, modified, _)| modified).max();
    Some((latest.unwrap_or(SystemTime::UNIX_EPOCH), digest.finish()))
}

/// The files a shader read, each with its stamp as the shader last compiled from it, which the
/// thread looks at to see them change.
struct Files {
    /// Its own first, then those it includes, as its add or its last compile that succeeded listed
    /// them.
    files: Vec<(PathBuf, Stamp)>,
    /// While its last compile failed, the directories those files are in, with their stamps as it
    /// began: a compile that fails lists none of the files it read, so a change to any file in
    /// them compiles the shader again, such as one it includes that it couldn't find, or found
    /// broken.
    dirs: Vec<(PathBuf, Stamp)>,
    /// Why its last compile failed, and its files' stamps as it began, so that one that fails the
    /// same way from the same files isn't news.
    failure: Option<(String, Vec<Stamp>)>,
    /// The stamps on the last look, its files' and then its directories', where they differed
    /// from those it compiled from: a change, which compiles once it holds for a look, so that an
    /// editor's partial writes and atomic renames settle first.
    changing: Option<Vec<Stamp>>,
    /// How many looks in a row its files, changed from those it compiled from, have held while
    /// its directories haven't.
    unsettled: u32,
}

impl Files {
    /// The files `listed`, its own first, that a shader read as it was added, in a compile begun
    /// at `compiled_at`, each with its stamp as `stamp` finds it now. That's what the shader
    /// compiled from if each file last changed more than [`MARGIN`] before; if one changed since,
    /// or about then, or is missing, the shader may be older than its files, and it starts with no
    /// stamps, as if it had read no files, so that it compiles again once they hold for a look.
    fn new(listed: &[PathBuf], compiled_at: SystemTime, stamp: impl Fn(&Path) -> Stamp) -> Self {
        let mut files: Vec<(PathBuf, Stamp)> = (listed.iter())
            .map(|path| (path.clone(), stamp(path)))
            .collect();
        let as_compiled = files.iter().all(|(_, stamped)| {
            stamped.is_some_and(|(modified, _)| {
                (compiled_at.duration_since(modified)).is_ok_and(|age| age > MARGIN)
            })
        });
        if !as_compiled {
            for (_, stamped) in &mut files {
                *stamped = None;
            }
        }
        Self {
            files,
            dirs: Vec::new(),
            failure: None,
            changing: None,
            unsettled: 0,
        }
    }

    /// Looks at the files, and the directories while its last compile failed, as `stamp` finds
    /// them, and returns whether the shader is to compile again: once they differ from what it
    /// last compiled from, and are as the look before found them. Its stamps are then theirs, as
    /// the compile will find them, so that a change while it compiles, or one it read half done,
    /// compiles again; and a compile that fails waits for the next change. While the shader's own
    /// file is missing, it waits for it. A change to its files that holds waits for the directories
    /// to hold too, but no more than [`SETTLE`] looks.
    fn look(&mut self, stamp: impl Fn(&Path) -> Stamp) -> bool {
        let watched = || self.files.iter().chain(&self.dirs);
        let now: Vec<Stamp> = watched().map(|(path, _)| stamp(path)).collect();
        if watched().zip(&now).all(|((_, then), now)| then == now) {
            self.changing = None;
            self.unsettled = 0;
            return false;
        }
        let files = self.files.len();
        let (files_held, dirs_held) = (self.changing.as_ref()).map_or((false, false), |was| {
            (was[..files] == now[..files], was[files..] == now[files..])
        });
        let changed = (self.files.iter().zip(&now)).any(|((_, then), now)| then != now);
        self.unsettled = if files_held && changed && !dirs_held {
            self.unsettled + 1
        } else {
            0
        };
        let held = files_held && now[0].is_some() && (dirs_held || self.unsettled >= SETTLE);
        if held {
            let stamped = self.files.iter_mut().chain(&mut self.dirs);
            for ((_, then), now) in stamped.zip(now) {
                *then = now;
            }
            self.changing = None;
            self.unsettled = 0;
        } else {
            self.changing = Some(now);
        }
        held
    }

    /// Takes the files a compile that succeeded listed, `listed`: those watched already keep their
    /// stamps, from before the compile, and the rest take `stamp`'s as it ends. One the shader
    /// newly includes that changes while it compiles waits for the next change. The directories
    /// are watched no longer.
    fn compiled(&mut self, listed: &[PathBuf], stamp: impl Fn(&Path) -> Stamp) {
        let files = (listed.iter())
            .map(|path| {
                let watched = self.files.iter().find(|(file, _)| file == path);
                let stamped = watched.map_or_else(|| stamp(path), |&(_, stamped)| stamped);
                (path.clone(), stamped)
            })
            .collect();
        self.files = files;
        self.dirs.clear();
        self.failure = None;
    }

    /// The directories its files are in, each once, as `stamp` finds them now.
    fn dirs(&self, stamp: impl Fn(&Path) -> Stamp) -> Vec<(PathBuf, Stamp)> {
        let mut dirs: Vec<(PathBuf, Stamp)> = Vec::new();
        for dir in self.files.iter().filter_map(|(file, _)| file.parent()) {
            if dirs.iter().all(|(seen, _)| seen != dir) {
                dirs.push((dir.to_owned(), stamp(dir)));
            }
        }
        dirs
    }

    /// Notes that a compile failed, as `why` says, its files' directories having been `dirs` as
    /// it began, which it watches until one succeeds. Returns whether that's news: it isn't if
    /// the last compile failed the same way from the same files, as when a change beside them
    /// alone set this one off.
    fn failed(&mut self, dirs: Vec<(PathBuf, Stamp)>, why: &str) -> bool {
        self.dirs = dirs;
        let read: Vec<Stamp> = self.files.iter().map(|&(_, stamped)| stamped).collect();
        let news = (self.failure.as_ref()).is_none_or(|(was, then)| was != why || *then != read);
        self.failure = Some((why.to_owned(), read));
        news
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::thread::JoinHandle;

    use gg_renderer::Blend;
    use tracing::Level;
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::Targets;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::math::Vec4;
    use crate::testing::Codes;

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

    /// A shader whose own file, `own.slang`, includes `shared.slang`, both as version 1, which it
    /// was added from, compiling well after they last changed.
    fn watched(disk: &Disk) -> Files {
        disk.set("own.slang", Some(1));
        disk.set("shared.slang", Some(1));
        let listed = ["own.slang", "shared.slang"].map(PathBuf::from);
        Files::new(&listed, SystemTime::UNIX_EPOCH + MARGIN * 2, disk.stamp())
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
    fn a_shader_whose_files_changed_as_it_was_added_compiles_once_they_hold() {
        let disk = Disk::default();
        disk.set("own.slang", Some(1));
        disk.set("shared.slang", Some(1));
        let listed = ["own.slang", "shared.slang"].map(PathBuf::from);
        // They last changed a moment before its compile began, by the file system's clock, which
        // can trail the one the compile's start was read from: it may have read what was before.
        let mut files = Files::new(&listed, SystemTime::UNIX_EPOCH + MARGIN / 2, disk.stamp());
        assert!(!files.look(disk.stamp()), "a look for them to hold");
        assert!(files.look(disk.stamp()), "held for a look");
        assert!(!files.look(disk.stamp()), "compiled from as they are");

        // Its own file gone by the time the thread looks, as while an editor replaces it: it waits
        // for it.
        disk.set("own.slang", None);
        let mut files = Files::new(&listed, SystemTime::UNIX_EPOCH + MARGIN * 2, disk.stamp());
        for _ in 0..3 {
            assert!(!files.look(disk.stamp()), "its own file is missing");
        }
        disk.set("own.slang", Some(2));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
    }

    #[test]
    fn a_failed_compile_watches_its_files_directories_until_one_succeeds() {
        let disk = Disk::default();
        disk.set("shaders/own.slang", Some(1));
        disk.set("shaders/shared.slang", Some(1));
        disk.set("shaders", Some(1));
        let listed = ["shaders/own.slang", "shaders/shared.slang"].map(PathBuf::from);
        let mut files = Files::new(&listed, SystemTime::UNIX_EPOCH + MARGIN * 2, disk.stamp());
        // While it compiles, a change beside its files isn't one.
        disk.set("shaders", Some(2));
        assert!(!files.look(disk.stamp()));
        assert!(!files.look(disk.stamp()));

        // An edit naming an include that isn't there, which fails.
        disk.set("shaders/own.slang", Some(2));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
        let dirs = files.dirs(disk.stamp());
        assert!(files.failed(dirs, "no extra.slang"), "the first failure");
        assert!(!files.look(disk.stamp()), "nothing changed since");

        // The include made beside them, which the directory shows: it compiles again.
        disk.set("shaders", Some(3));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
        // Failing as it did, from the same files, isn't news; from a file changed since, it is.
        let dirs = files.dirs(disk.stamp());
        assert!(!files.failed(dirs, "no extra.slang"), "the same failure");
        disk.set("shaders/own.slang", Some(3));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
        let dirs = files.dirs(disk.stamp());
        assert!(files.failed(dirs, "no extra.slang"), "from an edited file");

        // Once one compiles, the directory is watched no longer.
        disk.set("shaders", Some(4));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
        disk.set("shaders/extra.slang", Some(1));
        let mut listed = listed.to_vec();
        listed.push(PathBuf::from("shaders/extra.slang"));
        files.compiled(&listed, disk.stamp());
        disk.set("shaders", Some(5));
        assert!(!files.look(disk.stamp()));
        assert!(!files.look(disk.stamp()));
    }

    #[test]
    fn a_failing_shaders_fix_compiles_while_something_writes_beside_it() {
        let disk = Disk::default();
        disk.set("shaders/own.slang", Some(1));
        disk.set("shaders", Some(1));
        let listed = [PathBuf::from("shaders/own.slang")];
        let mut files = Files::new(&listed, SystemTime::UNIX_EPOCH + MARGIN * 2, disk.stamp());
        // An edit that doesn't compile, so that its directory is watched.
        disk.set("shaders/own.slang", Some(2));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()));
        let dirs = files.dirs(disk.stamp());
        assert!(files.failed(dirs, "a syntax error"));
        // Something writing beside it at every look, as a log does, which alone compiles nothing.
        let mut written = 1;
        let mut write = || {
            written += 1;
            disk.set("shaders", Some(written));
        };
        for _ in 0..SETTLE * 2 {
            write();
            assert!(!files.look(disk.stamp()), "the directory never holds");
        }
        // The fix, which waits for the directory to hold, but not for long.
        disk.set("shaders/own.slang", Some(3));
        for look in 0..SETTLE {
            write();
            assert!(!files.look(disk.stamp()), "waiting, at look {look}");
        }
        write();
        assert!(files.look(disk.stamp()), "waiting no longer");
    }

    #[test]
    fn stopping_ends_the_thread_at_once_between_compiles() {
        let hot_reload = HotReload::start().expect("the thread");
        let stopping = Instant::now();
        hot_reload.stop();
        assert!(stopping.elapsed() < STOP, "{:?}", stopping.elapsed());
    }

    /// The text of a shader that draws meshes in one color.
    fn flat_color() -> String {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        fs::read_to_string(fixture).expect("the fixture")
    }

    /// Writes `text` to the shader file at `path`, dated an hour back, so that a shader is added
    /// from its file as it is.
    fn written_long_ago(path: &Path, text: &str) {
        fs::write(path, text).expect("a shader's file");
        let an_hour_ago = SystemTime::now() - Duration::from_secs(60 * 60);
        let file = fs::File::options().write(true).open(path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    }

    /// The thread, watching `shaders`' sources: its side, which ends it as it drops; what it sends
    /// back; the codes of the warnings it logs, as for a compile that fails, which sends nothing
    /// back; and the thread.
    fn started(
        shaders: &Shaders,
    ) -> (
        Sender<ShaderSource>,
        Receiver<Compiled>,
        Receiver<String>,
        JoinHandle<()>,
    ) {
        let (sources, to_watch) = mpsc::channel();
        let (back, compiled) = mpsc::channel();
        let (codes, warned) = mpsc::channel();
        let warnings = Targets::new().with_target("gg_engine::hot_reload", Level::WARN);
        let subscriber = tracing_subscriber::registry().with(Codes(codes).with_filter(warnings));
        let thread = thread::spawn(move || {
            tracing::subscriber::with_default(subscriber, || watch(&to_watch, &back));
        });
        for source in shaders.sources() {
            sources.send(source.clone()).expect("the thread");
        }
        (sources, compiled, warned, thread)
    }

    #[test]
    fn edits_before_the_thread_looks_or_after_come_back_compiled() {
        let scratch = Scratch::new("watch");
        let dir = &scratch.0;
        fs::create_dir_all(dir).expect("a directory");
        let text = flat_color();
        let paths = ["a.slang", "b.slang"].map(|file| dir.join(file));
        let mut shaders = Shaders::default();
        for (name, path) in ["a", "b"].into_iter().zip(&paths) {
            written_long_ago(path, &text);
            let added = shaders.add::<[f32; 3], Vec4>(name, path, Blend::Opaque);
            added.expect("a shader");
        }
        let edit = |path: &Path, edit: u32| {
            fs::write(path, format!("{text}\n// Edit {edit}.\n")).expect("an edit");
        };
        // b, edited once it's added but before the thread first looks at its file.
        edit(&paths[1], 1);

        let (sources, compiled, _, thread) = started(&shaders);
        let a_while = Duration::from_secs(30);
        let came_back = |what: &str| compiled.recv_timeout(a_while).expect(what);
        let b = came_back("b comes back compiled");
        assert_eq!(b.source(), &shaders.sources()[1]);
        // a, as it was added, compiles only once it's edited.
        let more = compiled.recv_timeout(LOOK * 5);
        assert!(more.is_err(), "nothing else comes back");
        edit(&paths[0], 2);
        let a = came_back("a comes back compiled");
        assert_eq!(a.source(), &shaders.sources()[0]);
        assert_eq!(a.files().len(), 1, "{a:?}");
        shaders.replace(a);
        shaders.replace(b);
        drop(sources);
        thread.join().expect("the thread ends as its side drops");
    }

    /// The names of the files `compiled` read.
    fn names(compiled: &Compiled) -> Vec<&str> {
        (compiled.files().iter())
            .filter_map(|file| file.file_name()?.to_str())
            .collect()
    }

    #[test]
    fn an_include_made_or_fixed_after_a_compile_failed_on_it_comes_back_compiled() {
        let scratch = Scratch::new("watch-includes");
        let dir = &scratch.0;
        fs::create_dir_all(dir).expect("a directory");
        let text = flat_color();
        let path = dir.join("shader.slang");
        written_long_ago(&path, &text);
        let mut shaders = Shaders::default();
        let added = shaders.add::<[f32; 3], Vec4>("shader", &path, Blend::Opaque);
        added.expect("a shader");

        let (sources, compiled, warned, thread) = started(&shaders);
        let a_while = Duration::from_secs(30);
        let came_back = |what: &str| compiled.recv_timeout(a_while).expect(what);
        // A compile that fails warns, and sends nothing back.
        let failed = |what: &str| {
            let warning = warned.recv_timeout(a_while);
            assert_eq!(warning.as_deref(), Ok("renderer.bad_shader"), "{what}");
            assert!(compiled.try_recv().is_err(), "{what}");
        };

        // An include it names that isn't there yet, made once the compile has failed.
        fs::write(&path, format!("#include \"extra.slang\"\n{text}")).expect("an edit");
        failed("it doesn't compile without its include");
        fs::write(dir.join("extra.slang"), "static const float EXTRA = 1.0;\n").expect("one");
        let extra = came_back("it compiles with its include made");
        assert_eq!(names(&extra), ["shader.slang", "extra.slang"]);

        // An include that's there but broken, fixed where it is once the compile has failed.
        fs::write(dir.join("fix.slang"), "static const float FIX = ;\n").expect("one");
        fs::write(&path, format!("#include \"fix.slang\"\n{text}")).expect("an edit");
        failed("it doesn't compile with its include broken");
        fs::write(dir.join("fix.slang"), "static const float FIX = 1.0;\n").expect("the fix");
        let fixed = came_back("it compiles with its include fixed");
        assert_eq!(names(&fixed), ["shader.slang", "fix.slang"]);

        shaders.replace(extra);
        shaders.replace(fixed);
        drop(sources);
        thread.join().expect("the thread ends as its side drops");
    }
}
