//! Hot reload of games' shaders, textures, models, fonts, prefabs, UIs, sounds and music, in
//! development runs that serve the tool API.
//!
//! The `gg-watch` thread watches the files each asset read as it loaded (a shader's includes, a
//! model's glTF file and those it names) and rereads the asset off the main thread as they change.
//! The main thread puts the result in place of the asset, under the same handle. The renderer
//! draws it once its frames have streamed it, the old asset until then; a shader draws from the
//! next frame, a sound's next play plays it, and music's play crossfades to it.
//!
//! A change that doesn't compile, decode, read or bake, or a shader's that breaks the contract
//! `Shaders::add` describes, warns, and the asset keeps drawing or playing as it did. Until one
//! reads, a change to any file beside its files rereads it too: a failed compile lists no files
//! it read, so an include it couldn't find is otherwise missed.
//!
//! The thread follows a texture, model, font, prefab or UI whose file is renamed or moved, as the
//! asset database has it; the main thread tells it. A sound or music has no asset ID, so it's
//! watched where loaded from. A removed texture or model is watched no more.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use gg_api::asset::AssetId;
use gg_core::{Error, Result};
use gg_renderer::{Compiled, ShaderSource, Shaders, Texture};
use kira::sound::static_sound::StaticSoundData;
use tracing::{debug, warn};

use crate::assets::Catalog;
use crate::audio::{self, Encoded};
use crate::font::{self, Bake};
use crate::game_ui::{self, Document};
use crate::gltf::{self, Imported};
use crate::image::{self, Image};
use crate::package;
use crate::prefab;
use crate::{Font, Music, Prefab, Sound};

/// How long the thread waits between looks. A change reads on the look after the one that sees it.
const LOOK: Duration = Duration::from_millis(100);

/// How long before an asset's loading read its files must have last changed for it to have read
/// them as they are. File times can trail the read's clock (Windows and Linux stamp from a coarse
/// clock; FAT keeps 2 s). A file changed within it rereads the asset, at worst for nothing.
const MARGIN: Duration = Duration::from_secs(2);

/// How many looks a failing asset's change waits for the directories beside its files to hold too.
/// Something may write there all the time (a log) and would block the reread.
const SETTLE: u32 = 10;

/// How long the run's end waits for a read under way, so a shader's compile (about 0.1 s) removes
/// what slangc wrote. A read that hangs holds the end up no longer.
const STOP: Duration = Duration::from_secs(1);

/// An asset as it was loaded. Each file-backed kind has its handle, `name`, file `path` and
/// `read_at`, when its loading read began.
pub(crate) enum Source {
    Shader(ShaderSource),
    Texture {
        texture: Texture,
        name: String,
        path: PathBuf,
        read_at: SystemTime,
    },
    /// `path` resolved to `file`; `files` is what the read read, its own first.
    Model {
        file: PathBuf,
        name: String,
        path: PathBuf,
        files: Vec<PathBuf>,
        read_at: SystemTime,
    },
    Font {
        font: Font,
        name: String,
        path: PathBuf,
        read_at: SystemTime,
    },
    Prefab {
        prefab: Prefab,
        name: String,
        path: PathBuf,
        read_at: SystemTime,
    },
    Ui {
        ui: usize,
        name: String,
        path: PathBuf,
        read_at: SystemTime,
    },
    Sound {
        sound: Sound,
        name: String,
        path: PathBuf,
        read_at: SystemTime,
    },
    Music {
        music: Music,
        name: String,
        path: PathBuf,
        read_at: SystemTime,
    },
}

impl Source {
    fn name(&self) -> &str {
        match self {
            Self::Shader(shader) => shader.name(),
            Self::Texture { name, .. }
            | Self::Model { name, .. }
            | Self::Font { name, .. }
            | Self::Prefab { name, .. }
            | Self::Ui { name, .. }
            | Self::Sound { name, .. }
            | Self::Music { name, .. } => name,
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Shader(shader) => shader.path(),
            Self::Texture { path, .. }
            | Self::Model { path, .. }
            | Self::Font { path, .. }
            | Self::Prefab { path, .. }
            | Self::Ui { path, .. }
            | Self::Sound { path, .. }
            | Self::Music { path, .. } => path,
        }
    }

    fn files(&self) -> &[PathBuf] {
        match self {
            Self::Shader(shader) => shader.files(),
            Self::Texture { path, .. }
            | Self::Font { path, .. }
            | Self::Prefab { path, .. }
            | Self::Ui { path, .. }
            | Self::Sound { path, .. }
            | Self::Music { path, .. } => slice::from_ref(path),
            Self::Model { files, .. } => files,
        }
    }

    fn read_at(&self) -> SystemTime {
        match self {
            Self::Shader(shader) => shader.compiled_at(),
            Self::Texture { read_at, .. }
            | Self::Model { read_at, .. }
            | Self::Font { read_at, .. }
            | Self::Prefab { read_at, .. }
            | Self::Ui { read_at, .. }
            | Self::Sound { read_at, .. }
            | Self::Music { read_at, .. } => *read_at,
        }
    }

    /// `None` for a shader, sound or music, which have no asset ID.
    fn asset(&self) -> Option<Asset> {
        match self {
            Self::Shader(_) | Self::Sound { .. } | Self::Music { .. } => None,
            Self::Texture { texture, .. } => Some(Asset::Texture(*texture)),
            Self::Model { file, .. } => Some(Asset::Model(file.clone())),
            Self::Font { font, .. } => Some(Asset::Font(*font)),
            Self::Prefab { prefab, .. } => Some(Asset::Prefab(*prefab)),
            Self::Ui { ui, .. } => Some(Asset::Ui(*ui)),
        }
    }

    /// Rereads it as it was loaded (a shader is also checked against its types' contract).
    /// Returns what it read and the files it read, its own first.
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
                let image = image::load_image(name, path, &package::Files::default())?;
                let image = image::decode_texture(name, path, image)?;
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
                let read = gltf::read_listing(name, path, &package::Files::default());
                let (mut imported, files) = read?;
                // Here, not by the loader: the model takes them at once.
                imported.decode_images(name, path)?;
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
            Self::Prefab {
                prefab, name, path, ..
            } => {
                let reloaded = Reloaded::Prefab {
                    prefab: *prefab,
                    path: path.clone(),
                    text: prefab::read(name, path, &package::Files::default())?,
                };
                Ok((reloaded, vec![path.clone()]))
            }
            Self::Ui { ui, name, path, .. } => {
                let reloaded = Reloaded::Ui {
                    ui: *ui,
                    path: path.clone(),
                    document: game_ui::read(name, path, &package::Files::default())?,
                };
                Ok((reloaded, vec![path.clone()]))
            }
            Self::Sound {
                sound, name, path, ..
            } => {
                let reloaded = Reloaded::Sound {
                    sound: *sound,
                    path: path.clone(),
                    data: Box::new(audio::decode(name, path, &package::Files::default())?),
                };
                Ok((reloaded, vec![path.clone()]))
            }
            Self::Music {
                music, name, path, ..
            } => {
                let reloaded = Reloaded::Music {
                    music: *music,
                    path: path.clone(),
                    encoded: audio::read_music(name, path, &package::Files::default())?,
                };
                Ok((reloaded, vec![path.clone()]))
            }
        }
    }
}

/// An asset by the main thread's key: handle (texture, font, prefab), index (UI) or the file a
/// model resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Asset {
    Texture(Texture),
    Model(PathBuf),
    Font(Font),
    Prefab(Prefab),
    Ui(usize),
}

/// An asset reread from `path`, for the main thread to put in place of the one loaded.
pub(crate) enum Reloaded {
    Shader(Compiled),
    Texture {
        texture: Texture,
        path: PathBuf,
        image: Image,
    },
    Model {
        file: PathBuf,
        path: PathBuf,
        imported: Imported,
    },
    /// Boxed: a bake is over 2 KiB and the rest are small.
    Font {
        font: Font,
        path: PathBuf,
        bake: Box<Bake>,
    },
    Prefab {
        prefab: Prefab,
        path: PathBuf,
        text: String,
    },
    Ui {
        ui: usize,
        path: PathBuf,
        document: Document,
    },
    /// Boxed: kira's data, settings included, is over 400 bytes, twice the next largest.
    Sound {
        sound: Sound,
        path: PathBuf,
        data: Box<StaticSoundData>,
    },
    Music {
        music: Music,
        path: PathBuf,
        encoded: Encoded,
    },
}

/// What the main thread sends the thread.
enum Message {
    Watch(Source),
    /// `asset`'s file is at a path anew (renamed or moved) and holds what it did.
    Moved(Asset, PathBuf),
    /// A removed texture or model: watch it no more.
    Unwatch(Asset),
}

/// The thread, from the main thread's side. Dropping it ends the thread.
pub(crate) struct HotReload {
    /// How many shader sources it's been sent.
    sent: usize,
    /// `None` once dropping.
    messages: Option<Sender<Message>>,
    reloaded: Receiver<Reloaded>,
    followed: Vec<Followed>,
    /// The thread's [`Seen`].
    seen: Arc<Seen>,
    /// What [`taken_up_to`](Self::taken_up_to) last noted.
    looked_at: Option<SystemTime>,
}

/// When the look began, in ms since the Unix epoch, before which the thread has sent every file's
/// change: 0 before a look. A change reads on the look after the one that sees it, so it's the
/// look before the one that ended last. Stored after what that look sent, so a reader who then
/// takes all that came back has it.
#[derive(Default)]
struct Seen(AtomicU64);

/// An asset whose file is under the asset root, which [`HotReload::follow`] follows.
struct Followed {
    asset: Asset,
    /// The name it was loaded as.
    name: String,
    /// Its asset ID, once the database has one for its file (a stand-in's new file has none until
    /// the next scan).
    id: Option<AssetId>,
    /// Its path from the asset root as the database last had it, else as loaded. `None` while the
    /// thread doesn't watch it (a stand-in for a texture whose ID no asset has).
    at: Option<Arc<str>>,
}

impl HotReload {
    /// Starts the thread. Fails with `engine.hot_reload` if it can't start.
    pub(crate) fn start() -> Result<Self> {
        let (messages, to_watch) = mpsc::channel();
        let (back, reloaded) = mpsc::channel();
        let seen = Arc::new(Seen::default());
        let sees = Arc::clone(&seen);
        thread::Builder::new()
            // Within the 15 bytes Linux keeps of a thread's name.
            .name("gg-watch".to_owned())
            .spawn(move || watch(&to_watch, &back, &sees))
            .map_err(|error| {
                let message = "can't start the thread that watches assets' files";
                Error::new("engine.hot_reload", message).with_source(error)
            })?;
        Ok(Self {
            sent: 0,
            messages: Some(messages),
            reloaded,
            followed: Vec::new(),
            seen,
            looked_at: None,
        })
    }

    /// When the thread's look began before which it has sent back every watched file's change, as
    /// [`Seen`] says; `None` before its second look. Read before taking what came back
    /// ([`take`](Self::take)), and noted once all is taken ([`taken_up_to`](Self::taken_up_to)).
    pub(crate) fn seen(&self) -> Option<SystemTime> {
        let ms = self.seen.0.load(Ordering::Acquire);
        (ms > 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_millis(ms))
    }

    /// Notes that what came back by `seen` (from [`seen`](Self::seen)) is in place.
    pub(crate) fn taken_up_to(&mut self, seen: Option<SystemTime>) {
        if seen.is_some() {
            self.looked_at = seen;
        }
    }

    /// When the thread began a look before which each watched file's change is in place, or warned
    /// why not, as the tool API's `files_looked` says; `None` until one is.
    pub(crate) fn looked_at(&self) -> Option<SystemTime> {
        self.looked_at
    }

    /// Sends the thread the shaders added since the last call. Allocates nothing once all are sent.
    pub(crate) fn watch_shaders(&mut self, shaders: &Shaders) {
        for source in &shaders.sources()[self.sent..] {
            self.send(Message::Watch(Source::Shader(source.clone())));
        }
        self.sent = shaders.sources().len();
    }

    /// Watches `source`. If it's at `at` from the asset root, its file is followed
    /// ([`follow`](Self::follow)) by `id`, or by the ID the database later gives the file.
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

    /// Watches `asset` (loaded as `name`), a stand-in for the one with asset ID `id`, which no
    /// asset has. Once one does, [`follow`](Self::follow) finds it and reads its file in place.
    pub(crate) fn watch_for(&mut self, asset: Asset, name: &str, id: AssetId) {
        self.followed.push(Followed {
            asset,
            name: name.to_owned(),
            id: Some(id),
            at: None,
        });
    }

    /// Follows each watched texture, model and font whose file `catalog` (the database of asset
    /// root `root`) has elsewhere since the last call, by ID; one without an ID takes it from the
    /// database once it has one. It's watched at the new path, since a renamed or moved file holds
    /// what it held. A stand-in, whose ID no asset had, reads the file now at its path.
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
            // Never read, so the first look reads it.
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

    /// Stops watching removed `asset`; the main thread drops what was read of it meanwhile.
    pub(crate) fn unwatch(&mut self, asset: Asset) {
        self.followed.retain(|followed| followed.asset != asset);
        self.send(Message::Unwatch(asset));
    }

    pub(crate) fn take(&self) -> Option<Reloaded> {
        self.reloaded.try_recv().ok()
    }

    fn send(&self, message: Message) {
        if let Some(messages) = &self.messages {
            // The thread runs until this side drops or it panics (the log has that).
            let _ = messages.send(message);
        }
    }
}

impl Drop for HotReload {
    /// Ends the thread, which starts no read once this side lets go, waiting up to [`STOP`] for one
    /// under way so it removes what slangc wrote (slangc outlives a process that ends first).
    /// What's read meanwhile isn't put in place.
    fn drop(&mut self) {
        drop(self.messages.take());
        let until = Instant::now() + STOP;
        // Until the thread ends and lets go of its side.
        while (self.reloaded)
            .recv_timeout(until.saturating_duration_since(Instant::now()))
            .is_ok()
        {}
    }
}

/// The thread: looks at each asset's files every [`LOOK`], sends `back` what rereads, and notes in
/// `seen` when the look before the last began.
fn watch(messages: &Receiver<Message>, back: &Sender<Reloaded>, seen: &Seen) {
    // `None` where an asset is unwatched, until the look ends.
    let mut watched: Vec<Option<(Source, Files)>> = Vec::new();
    let mut before: Option<SystemTime> = None;
    loop {
        match messages.recv_timeout(LOOK) {
            Ok(message) => {
                take(message, &mut watched);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let began = SystemTime::now();
        if !look(messages, &mut watched, back) {
            return;
        }
        let since = before.and_then(|before| before.duration_since(SystemTime::UNIX_EPOCH).ok());
        if let Some(since) = since {
            let ms = u64::try_from(since.as_millis()).unwrap_or(u64::MAX);
            seen.0.store(ms, Ordering::Release);
        }
        before = Some(began);
    }
}

/// Looks at each `watched` asset's files in order, rereading those that changed and held
/// ([`Files::look`]) and sending `back` what reads ([`read`]). Before each read it takes `messages`
/// ([`take`]), so no read starts once the main thread's side has let go. An asset sent meanwhile
/// is looked at next look; one unwatched meanwhile isn't read and keeps its place as `None` until
/// the look ends, so later places don't shift. Returns false once that side has let go.
fn look(
    messages: &Receiver<Message>,
    watched: &mut Vec<Option<(Source, Files)>>,
    back: &Sender<Reloaded>,
) -> bool {
    let _zone = gg_core::zone!("look at assets' files");
    for at in 0..watched.len() {
        let Some((_, files)) = &mut watched[at] else {
            continue;
        };
        if !files.look(stamp) {
            continue;
        }
        if !take_all(messages, watched) {
            return false;
        }
        if let Some((source, files)) = &mut watched[at] {
            read(source, files, back);
        }
    }
    watched.retain(Option::is_some);
    true
}

/// Applies `message` to `watched`; unwatching leaves `None`.
fn take(message: Message, watched: &mut Vec<Option<(Source, Files)>>) {
    match message {
        Message::Watch(source) => {
            let _zone = gg_core::zone!("watch an asset");
            let files = Files::new(source.files(), source.read_at(), stamp);
            watched.push(Some((source, files)));
        }
        Message::Moved(asset, path) => {
            let found = (watched.iter_mut().flatten())
                .find(|(source, _)| source.asset() == Some(asset.clone()));
            if let Some((source, files)) = found {
                files.moved(&path);
                match source {
                    Source::Shader(_) => {}
                    Source::Texture { path: was, .. }
                    | Source::Model { path: was, .. }
                    | Source::Font { path: was, .. }
                    | Source::Prefab { path: was, .. }
                    | Source::Ui { path: was, .. }
                    | Source::Sound { path: was, .. }
                    | Source::Music { path: was, .. } => *was = path,
                }
            }
        }
        Message::Unwatch(asset) => {
            for entry in watched {
                let of = entry.as_ref().and_then(|(source, _)| source.asset());
                if of.is_some_and(|of| of == asset) {
                    *entry = None;
                }
            }
        }
    }
}

/// Takes what was sent since the last look. False once the main thread's side has let go.
fn take_all(messages: &Receiver<Message>, watched: &mut Vec<Option<(Source, Files)>>) -> bool {
    loop {
        match messages.try_recv() {
            Ok(message) => take(message, watched),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

/// Rereads `source` ([`Source::read`]), noting in `files` those it lists, and sends it `back`;
/// on failure, warns why unless it did last time from the same files.
fn read(source: &Source, files: &mut Files, back: &Sender<Reloaded>) {
    // Taken as the read begins, so a change while it fails rereads.
    let dirs = files.dirs(stamp);
    match source.read() {
        Ok((reloaded, listed)) => {
            files.succeeded(&listed, stamp);
            // Fails only if the main thread's side has let go, which ends the thread next.
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
                Source::Prefab { name, .. } if news => warn!(
                    code,
                    prefab = name,
                    path = %path,
                    "{why}; the prefab and its instances stay as they were"
                ),
                Source::Ui { name, .. } if news => warn!(
                    code,
                    ui = name,
                    path = %path,
                    "{why}; the UI stays as it was"
                ),
                Source::Sound { name, .. } if news => warn!(
                    code,
                    sound = name,
                    path = %path,
                    "{why}; the sound plays as it did"
                ),
                Source::Music { name, .. } if news => warn!(
                    code,
                    music = name,
                    path = %path,
                    "{why}; the music plays as it did"
                ),
                _ => debug!(code, asset = source.name(), path = %path, "Fails as it did"),
            }
        }
    }
}

/// A file's change time and length; `None` while unreadable (as while an editor replaces it).
type Stamp = Option<(SystemTime, u64)>;

/// The stamp of `path`. A directory's is its latest entry's time and a digest of the entries'
/// names and stamps, so it changes as a file in it is made, removed or changed, not one in a
/// subdirectory.
fn stamp(path: &Path) -> Stamp {
    let metadata = fs::metadata(path).ok()?;
    if metadata.is_dir() {
        return entries(path);
    }
    Some((metadata.modified().ok()?, metadata.len()))
}

/// The stamp of `dir`, read from its entries each time: a ceiling of a few thousand files, which
/// the OS's change notifications would lift.
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

/// The files an asset read, each with its stamp as the asset was last read from it.
struct Files {
    /// Its own first, then the rest its load or last successful read listed (a shader's includes,
    /// a model's buffers and images).
    files: Vec<(PathBuf, Stamp)>,
    /// While its last read failed: those files' directories and their stamps as it began. A failed
    /// read lists no files, so a change to any file there rereads the asset (such as an include it
    /// couldn't find, or found broken).
    dirs: Vec<(PathBuf, Stamp)>,
    /// Why its last read failed and its files' stamps as it began; failing alike isn't news.
    failure: Option<(String, Vec<Stamp>)>,
    /// The last look's stamps (files, then directories) where they differed from those it was read
    /// from. A change reads once it holds for a look, so an editor's partial writes and atomic
    /// renames settle first.
    changing: Option<Vec<Stamp>>,
    /// Looks in a row its changed files have held while its directories haven't.
    unsettled: u32,
}

impl Files {
    /// The files `listed` (own first) of an asset loaded by a read begun at `read_at`, stamped
    /// through `stamp` now. That's what it was read from if each file last changed more than
    /// [`MARGIN`] before. If one changed since or about then, or is missing, the asset may be older
    /// than its files, so it starts with no stamps and is reread once they hold for a look.
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

    /// Looks at the files (and the directories while its last read failed) through `stamp`.
    /// Returns whether to reread: they differ from what it was last read from and match the
    /// previous look. Its stamps then become theirs, as the read will find them, so a change during
    /// the read rereads it; a failed read waits for the next change. A missing own file is waited
    /// for. A held change to its files waits for the directories to hold too, for at most
    /// [`SETTLE`] looks.
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

    /// Takes the files `listed` by a successful read. Watched files keep their stamps from before
    /// the read; new ones (a shader's new include) take `stamp`'s as it ends, so one that changes
    /// during the read waits for the next change. The directories are watched no longer.
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

    fn dirs(&self, stamp: impl Fn(&Path) -> Stamp) -> Vec<(PathBuf, Stamp)> {
        let mut dirs: Vec<(PathBuf, Stamp)> = Vec::new();
        for dir in self.files.iter().filter_map(|(file, _)| file.parent()) {
            if dirs.iter().all(|(seen, _)| seen != dir) {
                dirs.push((dir.to_owned(), stamp(dir)));
            }
        }
        dirs
    }

    /// Notes a failed read: `why`, and the files' `dirs` as it began, watched until one succeeds.
    /// Returns whether that's news: not if the last read failed the same way from the same files
    /// (as when a change beside them alone set this one off).
    fn failed(&mut self, dirs: Vec<(PathBuf, Stamp)>, why: &str) -> bool {
        self.dirs = dirs;
        let read: Vec<Stamp> = self.files.iter().map(|&(_, stamped)| stamped).collect();
        let news = (self.failure.as_ref()).is_none_or(|(was, then)| was != why || *then != read);
        self.failure = Some((why.to_owned(), read));
        news
    }

    /// Takes `path` as the own file, with the stamp it was read with (a rename or move keeps time
    /// and length). An edit since the read, at either path, rereads it; a rename alone doesn't.
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

    use gg_renderer::{Blend, Mipmaps, Textures};
    use tracing::Level;
    use tracing_subscriber::Layer;
    use tracing_subscriber::filter::Targets;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::math::Vec4;
    use crate::testing::Codes;

    /// Stamps a test sets: a number per file version.
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

    /// A shader of `own.slang`, which includes `shared.slang`, both version 1, compiled long after.
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
        // An include edited in three writes, then left.
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
        // Changed again mid-compile; the compile lists files with their earlier stamps.
        disk.set("own.slang", Some(3));
        let listed = ["own.slang", "shared.slang"].map(PathBuf::from);
        files.succeeded(&listed, disk.stamp());
        assert!(!files.look(disk.stamp()));
        assert!(files.look(disk.stamp()), "the change during the compile");
        // That compile failed, noting nothing: the same files wait for the next change.
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
        // A missing include does compile; slangc fails it and says why.
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
        // Changed just before its compile began, by a file system clock that can trail: it may
        // have read the older text.
        let mut files = Files::new(&listed, SystemTime::UNIX_EPOCH + MARGIN / 2, disk.stamp());
        assert!(!files.look(disk.stamp()), "a look for them to hold");
        assert!(files.look(disk.stamp()), "held for a look");
        assert!(!files.look(disk.stamp()), "compiled from as they are");

        // Its own file gone by the first look (an editor replacing it): it waits.
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
        // Something writing beside it every look, as a log does, compiles nothing alone.
        let mut written = 1;
        let mut write = || {
            written += 1;
            disk.set("shaders", Some(written));
        };
        for _ in 0..SETTLE * 2 {
            write();
            assert!(!files.look(disk.stamp()), "the directory never holds");
        }
        // The fix waits for the directory to hold, but not for long.
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
        // Renamed, keeping time and length: gone from the old path, as read at the new.
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

        // Renamed again and edited before the move is known: reread once it is.
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

    fn flat_color() -> String {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        fs::read_to_string(fixture).expect("the fixture")
    }

    /// Writes `bytes` to `path`, dated an hour back, so an asset is loaded from its file as is.
    fn written_long_ago(path: &Path, bytes: impl AsRef<[u8]>) {
        fs::write(path, bytes).expect("an asset's file");
        let an_hour_ago = SystemTime::now() - Duration::from_secs(60 * 60);
        let file = fs::File::options().write(true).open(path);
        (file.and_then(|file| file.set_modified(an_hour_ago))).expect("its time");
    }

    /// The thread watching `sources`: its side (drop ends it), what it sends back, the codes of
    /// the warnings it logs, and the thread. A failed read sends nothing back.
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
            tracing::subscriber::with_default(subscriber, || {
                watch(&to_watch, &back, &Seen::default());
            });
        });
        for source in sources {
            messages.send(Message::Watch(source)).expect("the thread");
        }
        (messages, reloaded, warned, thread)
    }

    fn shaders_of(shaders: &Shaders) -> Vec<Source> {
        shaders
            .sources()
            .iter()
            .cloned()
            .map(Source::Shader)
            .collect()
    }

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
        // b, edited after adding but before the thread's first look.
        edit(&paths[1], 1);

        let (messages, reloaded, _, thread) = started(shaders_of(&shaders));
        let a_while = Duration::from_secs(30);
        let came_back = |what: &str| compiled(reloaded.recv_timeout(a_while).expect(what));
        let b = came_back("b comes back compiled");
        assert_eq!(b.source(), &shaders.sources()[1]);
        // a compiles only once edited.
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

        // An include not there yet, made after the compile failed.
        fs::write(&path, format!("#include \"extra.slang\"\n{text}")).expect("an edit");
        failed("it doesn't compile without its include");
        fs::write(dir.join("extra.slang"), "static const float EXTRA = 1.0;\n").expect("one");
        let extra = came_back("it compiles with its include made");
        assert_eq!(names(&extra), ["shader.slang", "extra.slang"]);

        // A broken include, fixed in place after the compile failed.
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

    /// A PNG `width` by 1 pixels, every RGBA8 byte `byte`.
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
        let (mut imported, files) =
            gltf::read_listing("m", &model, &package::Files::default()).expect("a model");
        // As a reload reads it: images decoded.
        imported
            .decode_images("m", &model)
            .expect("its images decode");
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

        // Renamed and followed: it holds what was read until edited in place.
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

    /// A texture unwatched mid-look (its message taken before the look reads a later changed one)
    /// isn't read and shifts nothing after it. The changed one is read, and the look ends with the
    /// unwatched one gone from `watched`.
    #[test]
    fn a_texture_unwatched_while_a_look_is_under_way_shifts_none_it_reads() {
        let scratch = Scratch::new("unwatched-mid-look");
        let dir = &scratch.0;
        fs::create_dir_all(dir).expect("a directory");
        let mut textures = Textures::default();
        let read_at = SystemTime::now();
        // The one unwatched first, then the one that changes, then one that doesn't.
        let (handles, mut watched): (Vec<Texture>, Vec<_>) = ["gone", "changed", "kept"]
            .into_iter()
            .map(|name| {
                let path = dir.join(format!("{name}.png"));
                written_long_ago(&path, png(1, 9));
                let added = textures.add(name, [1, 1], &[0; 4], Mipmaps::None);
                let texture = added.expect("a texture");
                let files = Files::new(slice::from_ref(&path), read_at, stamp);
                let source = Source::Texture {
                    texture,
                    name: name.to_owned(),
                    path,
                    read_at,
                };
                (texture, Some((source, files)))
            })
            .unzip();
        let (messages, to_watch) = mpsc::channel();
        let (back, reloaded) = mpsc::channel();
        fs::write(dir.join("changed.png"), png(2, 7)).expect("an edit");
        // A look sees it change, and the next, as it holds, reads it.
        assert!(look(&to_watch, &mut watched, &back));
        assert!(reloaded.try_recv().is_err(), "read once it holds");
        let gone = Message::Unwatch(Asset::Texture(handles[0]));
        messages.send(gone).expect("sent");
        assert!(look(&to_watch, &mut watched, &back), "the look goes on");
        let Ok(Reloaded::Texture { texture, image, .. }) = reloaded.try_recv() else {
            panic!("the changed texture read again");
        };
        assert_eq!((texture, image.size), (handles[1], [2, 1]));
        assert!(reloaded.try_recv().is_err(), "nothing else read");
        let still: Vec<Texture> = (watched.iter().flatten())
            .filter_map(|(source, _)| match source.asset() {
                Some(Asset::Texture(texture)) => Some(texture),
                _ => None,
            })
            .collect();
        assert_eq!(still, handles[1..]);
    }
}
