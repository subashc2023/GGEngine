//! Hot reload of games' assets, shaders, textures, models and fonts, in development runs that serve
//! the tool API. A thread of its own, `gg-watch`, watches the files each one read as it was loaded,
//! a shader's own and those it includes, a texture's image, a model's glTF file and those it names,
//! a font's file, and reads it again as they change, off the main thread: compiles the shader,
//! decodes the image, reads the model, bakes the font. The main thread puts what it read in place
//! of the asset, under the same handle, which the next frame it hands the renderer draws. A change
//! that doesn't compile, decode, read or bake, or a shader's that breaks the contract that
//! `Shaders::add` describes, warns, and the asset keeps drawing as it did; until one reads, a
//! change to any file beside its files reads it again too, such as an include it couldn't find,
//! since a compile that fails lists none of the files it read. The thread follows a texture, a
//! model or a font whose file is renamed or moved, as the asset database has it, which the main
//! thread tells it of.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use gg_api::asset::AssetId;
use gg_core::{Error, Result};
use gg_renderer::{Compiled, ShaderSource, Shaders, Texture};
use tracing::{debug, warn};

use crate::Font;
use crate::assets::Catalog;
use crate::font::{self, Bake};
use crate::gltf::{self, Imported};
use crate::image::{self, Image};

/// How long the thread waits between looks at the files. A change reads on the look after the one
/// that sees it, in about a tenth of a second more.
const LOOK: Duration = Duration::from_millis(100);

/// How long before the read that loaded an asset its files must have last changed for it to have
/// read them as they are: files' times can trail the clock that the read's start is read from, as
/// Windows and Linux stamp them from a clock that moves a tick at a time, and FAT keeps them to
/// 2 s. A file changed within it reads the asset again, at worst for nothing.
const MARGIN: Duration = Duration::from_secs(2);

/// How many looks a change to a failing asset's files waits for the directories beside them to hold
/// too: something may write beside them all the time, such as a log, which would otherwise keep
/// the change from reading.
const SETTLE: u32 = 10;

/// How long the run's end waits for a read under way, as a shader's compile, which slangc ends in
/// about a tenth of a second, so that it removes what slangc wrote; one that hangs holds the end up
/// no longer.
const STOP: Duration = Duration::from_secs(1);

/// An asset as it was loaded, whose files the thread watches.
pub(crate) enum Source {
    /// A game's shader.
    Shader(ShaderSource),
    /// `texture`, loaded as `name` from the image file at `path`, by a read begun at `read_at`.
    Texture {
        texture: Texture,
        name: String,
        path: PathBuf,
        read_at: SystemTime,
    },
    /// The model loaded as `name` from the file at `path`, which resolved to `file`, by a read
    /// begun at `read_at` that read `files`, its own first.
    Model {
        file: PathBuf,
        name: String,
        path: PathBuf,
        files: Vec<PathBuf>,
        read_at: SystemTime,
    },
    /// `font`, loaded as `name` from the font file at `path`, by a read begun at `read_at`.
    Font {
        font: Font,
        name: String,
        path: PathBuf,
        read_at: SystemTime,
    },
}

impl Source {
    fn name(&self) -> &str {
        match self {
            Self::Shader(shader) => shader.name(),
            Self::Texture { name, .. } | Self::Model { name, .. } | Self::Font { name, .. } => name,
        }
    }

    /// The file it's read from.
    fn path(&self) -> &Path {
        match self {
            Self::Shader(shader) => shader.path(),
            Self::Texture { path, .. } | Self::Model { path, .. } | Self::Font { path, .. } => path,
        }
    }

    /// The files its read read, its own first.
    fn files(&self) -> &[PathBuf] {
        match self {
            Self::Shader(shader) => shader.files(),
            Self::Texture { path, .. } | Self::Font { path, .. } => slice::from_ref(path),
            Self::Model { files, .. } => files,
        }
    }

    /// When the read that loaded it began: what its files hold from then on may not be in it.
    fn read_at(&self) -> SystemTime {
        match self {
            Self::Shader(shader) => shader.compiled_at(),
            Self::Texture { read_at, .. }
            | Self::Model { read_at, .. }
            | Self::Font { read_at, .. } => *read_at,
        }
    }

    /// Which texture, model or font it is; `None` for a shader.
    fn asset(&self) -> Option<Asset> {
        match self {
            Self::Shader(_) => None,
            Self::Texture { texture, .. } => Some(Asset::Texture(*texture)),
            Self::Model { file, .. } => Some(Asset::Model(file.clone())),
            Self::Font { font, .. } => Some(Asset::Font(*font)),
        }
    }

    /// Reads it again from its files, as it was loaded: compiles a shader and checks it against
    /// the contract for the types it was added for, decodes a texture's image, reads a model, and
    /// bakes a font. Returns what it read, and the files it read, its own first.
    ///
    /// Fails as loading it does.
    fn read(&self) -> Result<(Reloaded, Vec<PathBuf>)> {
        match self {
            Self::Shader(shader) => {
                let compiled = shader.compile()?;
                let files = compiled.files().to_vec();
                Ok((Reloaded::Shader(compiled), files))
            }
            Self::Texture {
                texture,
                name,
                path,
                ..
            } => {
                let image = image::load_image(name, path)?;
                let reloaded = Reloaded::Texture {
                    texture: *texture,
                    path: path.clone(),
                    image,
                };
                Ok((reloaded, vec![path.clone()]))
            }
            Self::Model {
                file, name, path, ..
            } => {
                let (imported, files) = gltf::read_listing(name, path)?;
                let reloaded = Reloaded::Model {
                    file: file.clone(),
                    path: path.clone(),
                    imported,
                };
                Ok((reloaded, files))
            }
            Self::Font {
                font, name, path, ..
            } => {
                let bake = Box::new(font::bake(name, path)?);
                let reloaded = Reloaded::Font {
                    font: *font,
                    path: path.clone(),
                    bake,
                };
                Ok((reloaded, vec![path.clone()]))
            }
        }
    }
}

/// A texture, a model or a font that the thread watches, as the main thread knows it: a texture
/// and a font by its handle, and a model by the file it resolved to as it was loaded, which the
/// context keeps it by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Asset {
    Texture(Texture),
    Model(PathBuf),
    Font(Font),
}

/// An asset that the thread read again, which the main thread puts in place of the one loaded.
pub(crate) enum Reloaded {
    Shader(Compiled),
    /// `texture`'s image, read from `path`.
    Texture {
        texture: Texture,
        path: PathBuf,
        image: Image,
    },
    /// What the model loaded from `file` draws, read from `path`.
    Model {
        file: PathBuf,
        path: PathBuf,
        imported: Imported,
    },
    /// What `font` baked again from `path`, boxed, as a bake is over 2 KiB and the rest are small.
    Font {
        font: Font,
        path: PathBuf,
        bake: Box<Bake>,
    },
}

/// What the main thread sends the thread.
enum Message {
    /// An asset to watch.
    Watch(Source),
    /// A texture, a model or a font whose file is at a path anew, renamed or moved, as what it
    /// holds.
    Moved(Asset, PathBuf),
}

/// The thread, from the main thread's side, which ends it as it drops.
pub(crate) struct HotReload {
    /// How many of the shaders' sources it's been sent.
    sent: usize,
    /// `None` once it's dropping.
    messages: Option<Sender<Message>>,
    reloaded: Receiver<Reloaded>,
    /// The textures, models and fonts it watches whose files are under the asset root, or will be,
    /// which it follows by their asset IDs as the files are renamed or moved.
    followed: Vec<Followed>,
}

/// A texture, a model or a font whose file is under the asset root, which [`HotReload::follow`]
/// follows.
struct Followed {
    asset: Asset,
    /// The name it was loaded as.
    name: String,
    /// Its asset ID, once the asset database has one for its file: a file new to the run, as a
    /// stand-in's that came, has none until the database's next scan gives it one.
    id: Option<AssetId>,
    /// Its file's path from the asset root, as the asset database last had it, or else as it was
    /// loaded from; `None` while the thread doesn't watch it, as for a stand-in for a texture whose
    /// ID no asset has.
    at: Option<Arc<str>>,
}

impl HotReload {
    /// Starts the thread. Fails with `engine.hot_reload` if it can't start.
    pub(crate) fn start() -> Result<Self> {
        let (messages, to_watch) = mpsc::channel();
        let (back, reloaded) = mpsc::channel();
        thread::Builder::new()
            // Within the 15 bytes Linux keeps of a thread's name.
            .name("gg-watch".to_owned())
            .spawn(move || watch(&to_watch, &back))
            .map_err(|error| {
                let message = "can't start the thread that watches assets' files";
                Error::new("engine.hot_reload", message).with_source(error)
            })?;
        Ok(Self {
            sent: 0,
            messages: Some(messages),
            reloaded,
            followed: Vec::new(),
        })
    }

    /// Sends the thread the games' shaders that `shaders` added since it last did. Once it's sent
    /// them all, it allocates nothing.
    pub(crate) fn watch_shaders(&mut self, shaders: &Shaders) {
        for source in &shaders.sources()[self.sent..] {
            self.send(Message::Watch(Source::Shader(source.clone())));
        }
        self.sent = shaders.sources().len();
    }

    /// Has the thread watch `source`, a texture's or a model's, following its file as it's renamed
    /// or moved, if it's at `at` from the asset root, as [`follow`](Self::follow) says: by its asset
    /// ID, `id`, or by the one the asset database gives its file once it has one.
    pub(crate) fn watch(&mut self, source: Source, id: Option<AssetId>, at: Option<Arc<str>>) {
        if let (Some(at), Some(asset)) = (at, source.asset()) {
            let name = source.name().to_owned();
            let at = Some(at);
            self.followed.push(Followed {
                asset,
                name,
                id,
                at,
            });
        }
        self.send(Message::Watch(source));
    }

    /// Has the thread watch `asset`, a texture or a font loaded as `name`, a stand-in for the one
    /// whose asset ID is `id`, which no asset has, once one has it, as [`follow`](Self::follow)
    /// finds it: its file is read then, in the stand-in's place.
    pub(crate) fn watch_for(&mut self, asset: Asset, name: &str, id: AssetId) {
        self.followed.push(Followed {
            asset,
            name: name.to_owned(),
            id: Some(id),
            at: None,
        });
    }

    /// Has the thread follow each texture, model and font it watches whose file the asset database
    /// `catalog`, of the asset root `root`, has elsewhere since it last did, by its ID, which one
    /// without takes from the database once it has one for the file: watching it at that path from
    /// then on, as a file renamed or moved holds what it held; or, for a stand-in, whose ID no
    /// asset had, reading the file it's at, as the next look finds it.
    pub(crate) fn follow(&mut self, catalog: &Catalog, root: &Path) {
        let mut messages = Vec::new();
        for followed in &mut self.followed {
            if followed.id.is_none() {
                followed.id = followed.at.as_deref().and_then(|at| catalog.id(at));
            }
            let Some(at) = followed.id.and_then(|id| catalog.path(id)) else {
                continue;
            };
            if followed.at.as_deref() == Some(at) {
                continue;
            }
            let path = root.join(at);
            let name = followed.name.clone();
            // Never read: it's read as it's first looked at.
            let read_at = SystemTime::UNIX_EPOCH;
            messages.push(match (&followed.at, &followed.asset) {
                (None, &Asset::Texture(texture)) => Message::Watch(Source::Texture {
                    texture,
                    name,
                    path,
                    read_at,
                }),
                (None, &Asset::Font(font)) => Message::Watch(Source::Font {
                    font,
                    name,
                    path,
                    read_at,
                }),
                (_, asset) => Message::Moved(asset.clone(), path),
            });
            followed.at = Some(at.into());
        }
        for message in messages {
            self.send(message);
        }
    }

    /// What the thread read again since it was last asked, an asset at a time.
    pub(crate) fn take(&self) -> Option<Reloaded> {
        self.reloaded.try_recv().ok()
    }

    fn send(&self, message: Message) {
        if let Some(messages) = &self.messages {
            // The thread runs until this side drops, or until it panics, which the log has.
            let _ = messages.send(message);
        }
    }
}

impl Drop for HotReload {
    /// Ends the thread, which starts no read once this side lets go, waiting up to [`STOP`] for one
    /// under way, so that it removes what slangc wrote: slangc outlives a process that ends first,
    /// and writes it after. What's read meanwhile isn't put in place.
    fn drop(&mut self) {
        drop(self.messages.take());
        let until = Instant::now() + STOP;
        // Until the thread ends, letting go of its side.
        while (self.reloaded)
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .is_ok()
        {}
    }
}

/// The thread: watches each asset it's sent, looking at its files every [`LOOK`] and sending
/// `back` what reads again, until the main thread's side drops.
fn watch(messages: &Receiver<Message>, back: &Sender<Reloaded>) {
    let mut watched: Vec<(Source, Files)> = Vec::new();
    loop {
        match messages.recv_timeout(LOOK) {
            Ok(message) => {
                take(message, &mut watched);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let _zone = gg_core::zone!("look at assets' files");
        for at in 0..watched.len() {
            if !watched[at].1.look(stamp) {
                continue;
            }
            // No read starts once the main thread's side has let go, which waits for the one under
            // way alone. What's sent meanwhile is taken first, the assets from the next look.
            if !take_all(messages, &mut watched) {
                return;
            }
            let (source, files) = &mut watched[at];
            read(source, files, back);
        }
    }
}

/// Takes `message` into what the thread watches, `watched`.
fn take(message: Message, watched: &mut Vec<(Source, Files)>) {
    match message {
        Message::Watch(source) => {
            let _zone = gg_core::zone!("watch an asset");
            let files = Files::new(source.files(), source.read_at(), stamp);
            watched.push((source, files));
        }
        Message::Moved(asset, path) => {
            let found =
                (watched.iter_mut()).find(|(source, _)| source.asset() == Some(asset.clone()));
            if let Some((source, files)) = found {
                files.moved(&path);
                match source {
                    Source::Shader(_) => {}
                    Source::Texture { path: was, .. }
                    | Source::Model { path: was, .. }
                    | Source::Font { path: was, .. } => *was = path,
                }
            }
        }
    }
}

/// Takes what the thread has been sent since it last looked, if anything: false once the main
/// thread's side has let go.
fn take_all(messages: &Receiver<Message>, watched: &mut Vec<(Source, Files)>) -> bool {
    loop {
        match messages.try_recv() {
            Ok(message) => take(message, watched),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

/// Reads `source` again from its files, noting in `files` those it lists, and sends it `back` if
/// it reads, as [`Source::read`] says; or says why it doesn't, unless it said so last time, from
/// the same files.
fn read(source: &Source, files: &mut Files, back: &Sender<Reloaded>) {
    // As the read begins, so that a change while it fails reads it again.
    let dirs = files.dirs(stamp);
    match source.read() {
        Ok((reloaded, listed)) => {
            files.succeeded(&listed, stamp);
            // Unless the main thread's side has let go, which ends the thread next.
            let _ = back.send(reloaded);
        }
        Err(error) => {
            let why = format!("{error:#}");
            let news = files.failed(dirs, &why);
            let (code, path) = (error.code(), source.path().display());
            match source {
                Source::Shader(_) if news => warn!(
                    code,
                    shader = source.name(),
                    path = %path,
                    "{why}; the shader keeps drawing as it did"
                ),
                Source::Texture { name, .. } if news => warn!(
                    code,
                    texture = name,
                    path = %path,
                    "{why}; the texture keeps drawing as it did"
                ),
                Source::Model { name, .. } if news => warn!(
                    code,
                    model = name,
                    path = %path,
                    "{why}; the model keeps drawing as it did"
                ),
                Source::Font { name, .. } if news => warn!(
                    code,
                    font = name,
                    path = %path,
                    "{why}; the font keeps drawing as it did"
                ),
                _ => debug!(code, asset = source.name(), path = %path, "Fails as it did"),
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

/// The files an asset read, each with its stamp as the asset was last read from it, which the
/// thread looks at to see them change.
struct Files {
    /// Its own first, then the rest it read, as its load or its last read that succeeded listed
    /// them: a shader's includes, a model's buffers and images.
    files: Vec<(PathBuf, Stamp)>,
    /// While its last read failed, the directories those files are in, with their stamps as it
    /// began: a read that fails lists none of the files it read, so a change to any file in them
    /// reads the asset again, such as a shader's include that it couldn't find, or found broken.
    dirs: Vec<(PathBuf, Stamp)>,
    /// Why its last read failed, and its files' stamps as it began, so that one that fails the
    /// same way from the same files isn't news.
    failure: Option<(String, Vec<Stamp>)>,
    /// The stamps on the last look, its files' and then its directories', where they differed
    /// from those it was read from: a change, which reads once it holds for a look, so that an
    /// editor's partial writes and atomic renames settle first.
    changing: Option<Vec<Stamp>>,
    /// How many looks in a row its files, changed from those it was read from, have held while
    /// its directories haven't.
    unsettled: u32,
}

impl Files {
    /// The files `listed`, its own first, that an asset read as it was loaded, in a read begun at
    /// `read_at`, each with its stamp as `stamp` finds it now. That's what the asset was read from
    /// if each file last changed more than [`MARGIN`] before; if one changed since, or about then,
    /// or is missing, the asset may be older than its files, and it starts with no stamps, as if it
    /// had read no files, so that it's read again once they hold for a look.
    fn new(listed: &[PathBuf], read_at: SystemTime, stamp: impl Fn(&Path) -> Stamp) -> Self {
        let mut files: Vec<(PathBuf, Stamp)> = (listed.iter())
            .map(|path| (path.clone(), stamp(path)))
            .collect();
        let as_read = files.iter().all(|(_, stamped)| {
            stamped.is_some_and(|(modified, _)| {
                (read_at.duration_since(modified)).is_ok_and(|age| age > MARGIN)
            })
        });
        if !as_read {
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

    /// Looks at the files, and the directories while its last read failed, as `stamp` finds them,
    /// and returns whether the asset is to be read again: once they differ from what it was last
    /// read from, and are as the look before found them. Its stamps are then theirs, as the read
    /// will find them, so that a change while it reads, or one it read half done, reads it again;
    /// and a read that fails waits for the next change. While the asset's own file is missing, it
    /// waits for it. A change to its files that holds waits for the directories to hold too, but
    /// no more than [`SETTLE`] looks.
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

    /// Takes the files a read that succeeded listed, `listed`: those watched already keep their
    /// stamps, from before the read, and the rest take `stamp`'s as it ends. One the asset newly
    /// reads, as a shader's new include, that changes while it's read waits for the next change.
    /// The directories are watched no longer.
    fn succeeded(&mut self, listed: &[PathBuf], stamp: impl Fn(&Path) -> Stamp) {
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

    /// Notes that a read failed, as `why` says, its files' directories having been `dirs` as it
    /// began, which it watches until one succeeds. Returns whether that's news: it isn't if the
    /// last read failed the same way from the same files, as when a change beside them alone set
    /// this one off.
    fn failed(&mut self, dirs: Vec<(PathBuf, Stamp)>, why: &str) -> bool {
        self.dirs = dirs;
        let read: Vec<Stamp> = self.files.iter().map(|&(_, stamped)| stamped).collect();
        let news = (self.failure.as_ref()).is_none_or(|(was, then)| was != why || *then != read);
        self.failure = Some((why.to_owned(), read));
        news
    }

    /// Takes `path` as the asset's own file's from now on, with the stamp it was read with, as a
    /// rename or a move keeps a file's time and length: so an edit made since it was read, at
    /// either path, reads it again, and a rename alone doesn't.
    fn moved(&mut self, path: &Path) {
        if let Some((own, _)) = self.files.first_mut() {
            path.clone_into(own);
        }
        self.changing = None;
        self.unsettled = 0;
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
        files.succeeded(&listed, disk.stamp());
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
        files.succeeded(&listed, disk.stamp());
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
        files.succeeded(&listed, disk.stamp());
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
    fn a_file_moved_is_watched_where_it_is_as_it_was_read() {
        let disk = Disk::default();
        let mut files = watched(&disk);
        // Renamed, keeping its time and length: gone from where it was, and at its new path as it
        // was read.
        disk.set("own.slang", None);
        disk.set("moved.slang", Some(1));
        assert!(!files.look(disk.stamp()), "missing, so waiting");
        files.moved(Path::new("moved.slang"));
        for _ in 0..3 {
            assert!(!files.look(disk.stamp()), "as it was read");
        }
        disk.set("moved.slang", Some(2));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()), "edited where it is");

        // Renamed again, then edited before the move is known: read again once it is.
        disk.set("moved.slang", None);
        disk.set("again.slang", Some(3));
        assert!(!files.look(disk.stamp()), "missing, so waiting");
        files.moved(Path::new("again.slang"));
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()), "edited as it moved");
    }

    #[test]
    fn dropping_ends_the_thread_at_once_between_reads() {
        let hot_reload = HotReload::start().expect("the thread");
        let stopping = Instant::now();
        drop(hot_reload);
        assert!(stopping.elapsed() < STOP, "{:?}", stopping.elapsed());
    }

    /// The text of a shader that draws meshes in one color.
    fn flat_color() -> String {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        fs::read_to_string(fixture).expect("the fixture")
    }

    /// Writes `bytes` to the file at `path`, dated an hour back, so that an asset is loaded from
    /// its file as it is.
    fn written_long_ago(path: &Path, bytes: impl AsRef<[u8]>) {
        fs::write(path, bytes).expect("an asset's file");
        let an_hour_ago = SystemTime::now() - Duration::from_secs(60 * 60);
        let file = fs::File::options().write(true).open(path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    }

    /// The thread, watching `sources`: its side, which ends it as it drops; what it sends back; the
    /// codes of the warnings it logs, as for a read that fails, which sends nothing back; and the
    /// thread.
    fn started(
        sources: impl IntoIterator<Item = Source>,
    ) -> (
        Sender<Message>,
        Receiver<Reloaded>,
        Receiver<String>,
        JoinHandle<()>,
    ) {
        let (messages, to_watch) = mpsc::channel();
        let (back, reloaded) = mpsc::channel();
        let (codes, warned) = mpsc::channel();
        let warnings = Targets::new().with_target("gg_engine::hot_reload", Level::WARN);
        let subscriber = tracing_subscriber::registry().with(Codes(codes).with_filter(warnings));
        let thread = thread::spawn(move || {
            tracing::subscriber::with_default(subscriber, || watch(&to_watch, &back));
        });
        for source in sources {
            messages.send(Message::Watch(source)).expect("the thread");
        }
        (messages, reloaded, warned, thread)
    }

    /// `shaders`' sources, to watch.
    fn shaders_of(shaders: &Shaders) -> Vec<Source> {
        shaders
            .sources()
            .iter()
            .cloned()
            .map(Source::Shader)
            .collect()
    }

    /// The shader that `reloaded` compiled again.
    fn compiled(reloaded: Reloaded) -> Compiled {
        match reloaded {
            Reloaded::Shader(compiled) => compiled,
            _ => panic!("a shader compiled again"),
        }
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

        let (messages, reloaded, _, thread) = started(shaders_of(&shaders));
        let a_while = Duration::from_secs(30);
        let came_back = |what: &str| compiled(reloaded.recv_timeout(a_while).expect(what));
        let b = came_back("b comes back compiled");
        assert_eq!(b.source(), &shaders.sources()[1]);
        // a, as it was added, compiles only once it's edited.
        let more = reloaded.recv_timeout(LOOK * 5);
        assert!(more.is_err(), "nothing else comes back");
        edit(&paths[0], 2);
        let a = came_back("a comes back compiled");
        assert_eq!(a.source(), &shaders.sources()[0]);
        assert_eq!(a.files().len(), 1, "{a:?}");
        shaders.replace(a);
        shaders.replace(b);
        drop(messages);
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

        let (messages, reloaded, warned, thread) = started(shaders_of(&shaders));
        let a_while = Duration::from_secs(30);
        let came_back = |what: &str| compiled(reloaded.recv_timeout(a_while).expect(what));
        // A compile that fails warns, and sends nothing back.
        let failed = |what: &str| {
            let warning = warned.recv_timeout(a_while);
            assert_eq!(warning.as_deref(), Ok("renderer.bad_shader"), "{what}");
            assert!(reloaded.try_recv().is_err(), "{what}");
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
        drop(messages);
        thread.join().expect("the thread ends as its side drops");
    }

    /// A PNG a pixel high, `width` pixels across, every byte of its RGBA8 `byte`.
    fn png(width: u32, byte: u8) -> Vec<u8> {
        let pixels = vec![byte; width as usize * 4];
        let (color, depth) = (::png::ColorType::Rgba, ::png::BitDepth::Eight);
        crate::image::tests::png(color, depth, width, &pixels, None)
    }

    #[test]
    fn a_texture_or_a_model_edited_comes_back_read_and_one_moved_is_followed() {
        let scratch = Scratch::new("watch-assets");
        let dir = &scratch.0;
        fs::create_dir_all(dir).expect("a directory");
        let (image, model) = (dir.join("a.png"), dir.join("m.glb"));
        written_long_ago(&image, png(1, 9));
        written_long_ago(&model, gltf::tests::model());
        let read_at = SystemTime::now();
        let (imported, files) = gltf::read_listing("m", &model).expect("a model");
        let texture = Source::Texture {
            texture: Texture::WHITE,
            name: "a".to_owned(),
            path: image.clone(),
            read_at,
        };
        let model = Source::Model {
            file: model.clone(),
            name: "m".to_owned(),
            path: model,
            files,
            read_at,
        };
        let (messages, reloaded, warned, thread) = started([texture, model]);
        let a_while = Duration::from_secs(30);
        let came_back = |what: &str| reloaded.recv_timeout(a_while).expect(what);
        let more = reloaded.recv_timeout(LOOK * 5);
        assert!(more.is_err(), "nothing comes back as it was loaded");

        // An image edited, at another size.
        fs::write(&image, png(2, 7)).expect("an edit");
        let Reloaded::Texture {
            texture,
            path,
            image: read,
        } = came_back("the image")
        else {
            panic!("the texture comes back");
        };
        assert_eq!(
            (texture, &path, read.size),
            (Texture::WHITE, &image, [2, 1])
        );
        // Written over with what's no image, which warns, then fixed.
        fs::write(&image, b"not an image").expect("an edit");
        let warning = warned.recv_timeout(a_while);
        assert_eq!(warning.as_deref(), Ok("engine.bad_texture"));
        fs::write(&image, png(3, 5)).expect("the fix");
        let Reloaded::Texture { image: read, .. } = came_back("the image fixed") else {
            panic!("the texture comes back");
        };
        assert_eq!(read.size, [3, 1]);

        // Renamed, which the main thread follows: what it holds is what was read, until it's
        // edited where it is.
        let renamed = dir.join("b.png");
        fs::rename(&image, &renamed).expect("renamed");
        let asset = Asset::Texture(Texture::WHITE);
        messages
            .send(Message::Moved(asset, renamed.clone()))
            .expect("sent");
        let more = reloaded.recv_timeout(LOOK * 5);
        assert!(more.is_err(), "nothing comes back as it was");
        fs::write(&renamed, png(4, 3)).expect("an edit");
        let Reloaded::Texture {
            path, image: read, ..
        } = came_back("the image renamed")
        else {
            panic!("the texture comes back");
        };
        assert_eq!((path, read.size), (renamed, [4, 1]));

        // The model written again: read whole.
        let model = dir.join("m.glb");
        fs::write(&model, gltf::tests::model()).expect("an edit");
        let Reloaded::Model {
            file,
            imported: read,
            ..
        } = came_back("the model")
        else {
            panic!("the model comes back");
        };
        assert_eq!((file, read), (model, imported));
        drop(messages);
        thread.join().expect("the thread ends as its side drops");
    }
}
