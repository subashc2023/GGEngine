//! Assets named by their paths: what scenes, snapshots, recordings and the tool API name a texture
//! by, where its handle is the renderer's own, so that a restart in another process, or another
//! machine, finds it again; what the layers loaded, by their files as the OS resolves them, so that
//! a file loads once however a layer writes its path; and, in development builds, the asset
//! database, `Catalog`: the textures and models under the root and the IDs their ID files hold,
//! whose text and scan are gg-api's.

use std::collections::HashMap;
#[cfg(feature = "dev")]
use std::collections::{BTreeMap, HashSet};
use std::fs;
#[cfg(feature = "dev")]
use std::io;
use std::path::{self, Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "dev")]
use gg_api::asset::{self as asset_files, AssetId, AssetKind, Meta, Scanned};
use gg_renderer::Texture;
#[cfg(feature = "dev")]
use tracing::{info, warn};

use crate::Model;

/// The textures a context added, each by its index, named by the file it was loaded from: its
/// path relative to the app's [root](crate::App::root), with `/` between its parts, or as it was
/// given for a file outside it; a model's images by the model's path and their order in it, such
/// as `assets/truck.glb#0`. The engine's white texture has no name. A context's worlds and their
/// copies share it, and what a world loads from text finds its textures here, as an entity is found
/// by its ID: a path no texture has names none. In development builds it holds the asset database
/// too, as the last scan left it.
#[derive(Clone, Debug)]
pub(crate) struct Assets {
    /// Each texture's name, by its index.
    textures: Vec<Option<Arc<str>>>,
    /// Each texture added from a file of its own, by each name the layers gave its file: the first
    /// added, if two files have one name.
    by_path: HashMap<Arc<str>, Texture>,
    /// What the layers added, in order, each with how many meshes, shaders and textures there
    /// were after it, as `texture 1 1 2 assets/logo.png`: what names the meshes, shaders and
    /// textures by their indices, so that what's opaque in a snapshot, which holds indices, comes
    /// back only into a run whose layers added the same. A mesh's, a shader's and a model's give
    /// the types their draws hold the bytes of too, by their names and sizes, and a shader's its
    /// blend, as `shader 1 2 1 shaders/colored.slang game::Colored 28 () 0 Opaque`, so that a draw
    /// comes back only where its bytes mean what they did. A file added again counts again.
    added: Vec<Arc<str>>,
    /// The asset database, as [`Context::scan_assets`](crate::Context::scan_assets) last left it.
    #[cfg(feature = "dev")]
    catalog: Catalog,
}

impl Default for Assets {
    /// The engine's white texture alone, which every renderer's textures start with.
    fn default() -> Self {
        Self {
            textures: vec![None],
            by_path: HashMap::new(),
            added: Vec::new(),
            #[cfg(feature = "dev")]
            catalog: Catalog::default(),
        }
    }
}

impl Assets {
    /// What the layers added, in order, as the struct says.
    pub(crate) fn added(&self) -> &[Arc<str>] {
        &self.added
    }

    /// Records that the layers added `what`, as the struct says.
    pub(crate) fn note(&mut self, what: String) {
        self.added.push(what.into());
    }

    /// The name of `texture`, if it has one.
    pub(crate) fn path(&self, texture: Texture) -> Option<&str> {
        let name = self.textures.get(texture.index() as usize)?;
        name.as_deref()
    }

    /// The texture loaded from the file `path` names, if one was.
    pub(crate) fn texture(&self, path: &str) -> Option<Texture> {
        self.by_path.get(path).copied()
    }

    /// Names `texture`, the next texture added, loaded from the file `path` names.
    pub(crate) fn add_texture(&mut self, texture: Texture, path: Arc<str>) {
        assert_eq!(
            texture.index() as usize,
            self.textures.len(),
            "textures are named in the order they're added"
        );
        self.by_path.entry(path.clone()).or_insert(texture);
        self.textures.push(Some(path));
    }

    /// Has `path` name `texture` too, added before from another path to its file, unless it names
    /// one already; `texture` keeps its first name.
    pub(crate) fn alias(&mut self, texture: Texture, path: Arc<str>) {
        self.by_path.entry(path).or_insert(texture);
    }

    /// Names the textures added since there were `before`, a model's images, from the model's path,
    /// `path`, and their order in it.
    pub(crate) fn add_images(&mut self, path: &str, before: u32, count: u32) {
        assert_eq!(
            before as usize,
            self.textures.len(),
            "textures are named in the order they're added"
        );
        let images = (0..count - before).map(|image| Some(format!("{path}#{image}").into()));
        self.textures.extend(images);
    }

    /// The asset database, as the last scan left it.
    #[cfg(feature = "dev")]
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Puts `catalog`, what a scan made of the database, in its place.
    #[cfg(feature = "dev")]
    pub(crate) fn set_catalog(&mut self, catalog: Catalog) {
        self.catalog = catalog;
    }
}

/// What the layers loaded from files, each by its file, as [`file_of`] resolves it, so that a file
/// added again, by whatever form of its path, isn't loaded again.
#[derive(Default)]
pub(crate) struct Loaded {
    /// Each texture's or sprite sheet's, which share it: its texture, and its size in pixels, which
    /// a sheet is cut by.
    pub(crate) textures: HashMap<PathBuf, (Texture, [u32; 2])>,
    pub(crate) models: HashMap<PathBuf, Model>,
    /// Each path a shader's file was added by, which the renderer's shaders know it by, with the
    /// file it resolves to.
    pub(crate) shaders: HashMap<PathBuf, PathBuf>,
}

/// What names the file at `path` among assets: its path relative to `root`, with `/` between its
/// parts, if it's in `root`, and otherwise as it's given, with `/` too: as the layer wrote it,
/// rather than as the disk spells it, so that every checkout of a game names its files alike, as
/// one whose file system ignores case may keep a file in the case an editor last saved it in.
pub(crate) fn name_of(root: Option<&Path>, path: &Path) -> Arc<str> {
    let relative = root.and_then(|root| path.strip_prefix(root).ok());
    let text = relative.unwrap_or(path).to_string_lossy();
    if path::MAIN_SEPARATOR == '/' {
        text.into()
    } else {
        text.replace(path::MAIN_SEPARATOR, "/").into()
    }
}

/// The file at `path`, as the OS resolves `.`, `..`, links and, where names ignore it, as on
/// Windows, case, so that one file is one file however its path is written; `None` for a path that
/// doesn't resolve, as one of no file.
pub(crate) fn file_of(path: &Path) -> Option<PathBuf> {
    let _zone = gg_core::zone!();
    fs::canonicalize(path).ok()
}

/// The textures and models under the app's root, as the scans found them, and their IDs, from
/// their ID files, which gg-api reads: the asset database. It only grows: an asset the run has
/// found it keeps, with its ID, even once its file is gone, so that its ID stays its for the run.
#[cfg(feature = "dev")]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Catalog {
    /// Each asset found, by its path from the root, with `/` between its parts.
    assets: BTreeMap<Arc<str>, Cataloged>,
    /// The asset each ID is, by its path: each ID bound once a run.
    ids: HashMap<AssetId, Arc<str>>,
    /// The ID files whose assets the last scan didn't find: each its path, its asset's kind, and its
    /// ID if it reads.
    orphans: Vec<(Arc<str>, AssetKind, Option<AssetId>)>,
    /// Why the root couldn't be listed at the last scan, if it couldn't.
    unlisted: Option<Arc<str>>,
    /// The problems warned of, each by its code and the path of what it's about, so that each warns
    /// once a run.
    warned: HashSet<(&'static str, Arc<str>)>,
}

/// An asset the database holds.
#[cfg(feature = "dev")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cataloged {
    pub(crate) kind: AssetKind,
    pub(crate) id: Option<AssetId>,
    /// Whether the last scan found its file.
    pub(crate) present: bool,
    /// What's wrong with its ID file as the last scan found it, if anything.
    pub(crate) problem: Option<Problem>,
}

/// A problem with an asset's ID file: its code, as ARCHITECTURE.md's Project state names them, and
/// its warning's message.
#[cfg(feature = "dev")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) code: &'static str,
    pub(crate) message: Arc<str>,
}

#[cfg(feature = "dev")]
impl Problem {
    fn new(code: &'static str, message: String) -> Self {
        let message = message.into();
        Self { code, message }
    }
}

#[cfg(feature = "dev")]
impl Catalog {
    /// The assets the last scan found, in the order of their paths: each its path and what's known.
    pub(crate) fn listed(&self) -> impl Iterator<Item = (&str, &Cataloged)> {
        let present = self
            .assets
            .iter()
            .filter(|(_, cataloged)| cataloged.present);
        present.map(|(path, cataloged)| (&**path, cataloged))
    }

    /// The ID files whose assets the last scan didn't find, in the order of their paths.
    pub(crate) fn orphans(&self) -> &[(Arc<str>, AssetKind, Option<AssetId>)] {
        &self.orphans
    }

    /// Why the root couldn't be listed at the last scan, if it couldn't.
    pub(crate) fn unlisted(&self) -> Option<&str> {
        self.unlisted.as_deref()
    }
}

/// Scans the assets under `root`, as gg-api's `scan` does, and gives what `catalog` becomes, or
/// `None` if nothing changed, warning of each problem once a run and writing the ID files missing
/// if `writes`, as ARCHITECTURE.md's Project state says.
///
/// An asset found gets the ID its ID file holds, unless another asset has it this run, and keeps
/// it for the run, whatever its ID file holds later: at a run's first scan, the asset first by
/// path keeps an ID two hold. An asset without an ID file, where the run `writes`, gets one written
/// whole beside it, holding the ID it has this run, or else one drawn, unless another ID file holds
/// that ID now, as after a rename that took its ID file along.
#[cfg(feature = "dev")]
pub(crate) fn scan(catalog: &Catalog, root: &Path, writes: bool) -> Option<Catalog> {
    let found = asset_files::scan(root);
    let mut next = catalog.clone();
    for cataloged in next.assets.values_mut() {
        cataloged.present = false;
        cataloged.problem = None;
    }
    let root_unread = found.unread.iter().find(|unread| unread.path == ".");
    next.unlisted = root_unread.map(|unread| unread.error.as_str().into());
    // Each ID an ID file holds, by the path of the first in order of those that hold it.
    let ids = (found.assets.iter()).filter_map(|scanned| match &scanned.meta {
        Some(Ok(meta)) => Some((format!("{}.{}", scanned.path, Meta::EXTENSION), meta.id)),
        _ => None,
    });
    let orphaned = (found.orphans.iter())
        .filter_map(|orphan| Some((orphan.path.clone(), orphan.meta.as_ref().ok()?.id)));
    let mut holders: Vec<_> = ids.chain(orphaned).collect();
    holders.sort_unstable();
    let mut held = HashMap::new();
    for (path, id) in holders {
        held.entry(id).or_insert(path);
    }
    // Each ID an asset's ID file holds, by the path of the first asset in order whose file holds
    // it: the asset a run's first scan gives it to.
    let mut first = HashMap::new();
    for scanned in &found.assets {
        if let Some(Ok(meta)) = &scanned.meta {
            first.entry(meta.id).or_insert(scanned.path.as_str());
        }
    }
    // Every asset found is present before any takes an ID, so that a duplicate's holder is told
    // present or gone whatever its path's order.
    for scanned in &found.assets {
        let cataloged = (next.assets.entry(scanned.path.as_str().into())).or_insert(Cataloged {
            kind: scanned.kind,
            id: None,
            present: true,
            problem: None,
        });
        cataloged.kind = scanned.kind;
        cataloged.present = true;
    }
    for scanned in &found.assets {
        let had = next.assets[scanned.path.as_str()].id;
        let (id, problem) = identify(&mut next, &held, &first, scanned, had, writes);
        let cataloged = (next.assets.get_mut(scanned.path.as_str())).expect("found");
        (cataloged.id, cataloged.problem) = (id, problem);
    }
    let mut orphans = Vec::with_capacity(found.orphans.len());
    for orphan in found.orphans {
        let path: Arc<str> = orphan.path.into();
        let asset = path.rsplit_once('.').map_or(&*path, |(asset, _)| asset);
        let code = "asset.orphaned";
        if once(&mut next.warned, code, &path) {
            match &orphan.meta {
                Ok(Meta { id }) => warn!(
                    code,
                    path = &*path,
                    id = %id,
                    "{path} holds the ID {id}, but its asset, {asset}, isn't there: if it was \
                     renamed or moved, move this file beside it, named for it, in place of any ID \
                     file a run wrote it, and it takes this ID from the next run; otherwise delete \
                     this file"
                ),
                Err(failure) => warn!(
                    code,
                    path = &*path,
                    "{path} is the ID file of {asset}, which isn't there, and doesn't read: \
                     {failure}; delete it, or fix it and move it beside its asset, named for it, if \
                     that was renamed or moved"
                ),
            }
        }
        let id = orphan.meta.ok().map(|meta| meta.id);
        orphans.push((path, orphan.kind, id));
    }
    next.orphans = orphans;
    for unread in &found.unread {
        let (path, error) = (&*unread.path, &*unread.error);
        let code = "asset.unlisted";
        if once(&mut next.warned, code, path) {
            warn!(
                code,
                path, error, "Can't scan {path}: {error}; no asset in it or of it has an ID"
            );
        }
    }
    (next != *catalog).then_some(next)
}

/// What asset `scanned` gets of the ID file the scan found beside it, given the ID it `had` this
/// run, if any; `held`, each ID an ID file holds, by the path of the first that holds it; and
/// `first`, each ID an asset's ID file holds, by the first such asset's path: its ID, and what's
/// wrong, if anything, warned of once a run; writing its ID file if it's missing and the run
/// `writes`, and binding its ID in `next` if it's new, as [`scan`] says.
#[cfg(feature = "dev")]
fn identify(
    next: &mut Catalog,
    held: &HashMap<AssetId, String>,
    first: &HashMap<AssetId, &str>,
    scanned: &Scanned,
    had: Option<AssetId>,
    writes: bool,
) -> (Option<AssetId>, Option<Problem>) {
    let path = scanned.path.as_str();
    let written;
    let meta = match &scanned.meta {
        Some(meta) => meta,
        None if !writes => {
            let code = "asset.no_id";
            let message = format!(
                "{path} has no ID file, and this run writes none, serving no tool API or joining \
                 a lockstep session: a run that serves it writes one, to commit beside {path}"
            );
            if once(&mut next.warned, code, path) {
                warn!(code, path, "{message}");
            }
            return (had, Some(Problem::new(code, message)));
        }
        None => {
            // Written back, its ID would be in two files.
            if let Some(id) = had
                && let Some(holder) = held.get(&id)
            {
                let code = "asset.duplicate_id";
                let message = format!(
                    "{path} has no ID file, and its ID this run, {id}, is in {holder} now: none is \
                     written for {path}, and the next run that writes ID files gives it a fresh one"
                );
                if once(&mut next.warned, code, path) {
                    warn!(code, path, holder = &**holder, id = %id, "{message}");
                }
                return (had, Some(Problem::new(code, message)));
            }
            let id = had.unwrap_or_else(|| {
                loop {
                    let drawn = AssetId::draw();
                    if !next.ids.contains_key(&drawn) && !held.contains_key(&drawn) {
                        break drawn;
                    }
                }
            });
            let file = Meta::path(&scanned.file);
            written = match crate::dirs::create(&file, Meta { id }.text().as_bytes()) {
                Ok(()) => {
                    info!(
                        code = "asset.id_written",
                        path,
                        id = %id,
                        "Gave {path} the ID {id}, in its ID file {path}.meta, to commit with it"
                    );
                    Ok(Meta { id })
                }
                // Another run wrote it first.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Meta::read(&file),
                Err(error) => {
                    let code = "asset.unwritten";
                    let kept = match had {
                        Some(id) => {
                            format!("{path} keeps the ID {id} this run, which no file holds")
                        }
                        None => format!("{path} has no ID"),
                    };
                    let message = format!("Can't write {path}.meta: {error}; {kept}");
                    if once(&mut next.warned, code, path) {
                        warn!(code, path, error = %error, "{message}");
                    }
                    return (had, Some(Problem::new(code, message)));
                }
            };
            &written
        }
    };
    match meta {
        Ok(meta) => match had {
            Some(had) if had == meta.id => (Some(had), None),
            Some(had) => {
                let code = "asset.id_changed";
                let held = meta.id;
                let message = format!(
                    "{path}.meta now holds {held}, but {path} keeps the ID {had} until the next \
                     run, so that nothing this run names changes"
                );
                if once(&mut next.warned, code, path) {
                    warn!(code, path, id = %had, %held, "{message}");
                }
                (Some(had), Some(Problem::new(code, message)))
            }
            None => {
                let id = meta.id;
                let holder = next.ids.entry(id).or_insert_with(|| path.into());
                if **holder == *path {
                    return (Some(id), None);
                }
                let holder = Arc::clone(holder);
                let present = next
                    .assets
                    .get(&holder)
                    .is_some_and(|holder| holder.present);
                let code = "asset.duplicate_id";
                // A run's first scan gives an ID to the first by path whose ID file holds it.
                let takes = first.get(&id) == Some(&path);
                let message = match (takes, present) {
                    (true, true) => format!(
                        "{path}.meta holds {id}, {holder}'s ID this run: from the next run, {path} \
                         takes it, as the first by path whose ID file holds it, and {holder} goes \
                         without, unless {path}.meta is removed before then, for a run that writes \
                         ID files to write {path} a fresh one"
                    ),
                    (true, false) => format!(
                        "{path}.meta holds {id}, {holder}'s ID this run, though its file is gone, \
                         as after a rename: {path} takes it at the next run"
                    ),
                    (false, _) => format!(
                        "{path}.meta holds {id}, {holder}'s ID: {path} has no ID until its ID file \
                         is removed, after which a run that writes ID files writes it a fresh one"
                    ),
                };
                if once(&mut next.warned, code, path) {
                    warn!(code, path, holder = &*holder, id = %id, "{message}");
                }
                (None, Some(Problem::new(code, message)))
            }
        },
        Err(failure) => {
            if failure.code == "asset.meta_case" {
                let code = "asset.meta_case";
                let name = path.rsplit('/').next().unwrap_or(path);
                let kept = match had {
                    Some(id) => format!("{path} keeps the ID {id} until the next run"),
                    None => format!("until then {path} has no ID"),
                };
                let message = format!(
                    "{path}: {failure}, which some file systems tell apart: rename it \
                     `{name}.meta`; {kept}"
                );
                if once(&mut next.warned, code, path) {
                    let file = scanned.misnamed.as_deref();
                    warn!(code, path, file, "{message}");
                }
                (had, Some(Problem::new(code, message)))
            } else {
                let code = "asset.bad_meta";
                let file = format!("{path}.{}", Meta::EXTENSION);
                let kept = match had {
                    Some(id) => format!("its asset keeps the ID {id} until the next run"),
                    None => "its asset has no ID".to_owned(),
                };
                let message = format!(
                    "{file} doesn't read as an ID file: {failure}; {kept}, and the file is left as \
                     it is, as it may be a merge's conflict or someone's edit: fix it, or remove it \
                     for a run that writes ID files to write a fresh one"
                );
                if once(&mut next.warned, code, &file) {
                    let id = had.map(|id| id.to_string());
                    warn!(code, path = &*file, id, "{message}");
                }
                (had, Some(Problem::new(code, message)))
            }
        }
    }
}

/// Whether the problem `code` with what's at `about` is new to `warned`, which then holds it.
#[cfg(feature = "dev")]
fn once(warned: &mut HashSet<(&'static str, Arc<str>)>, code: &'static str, about: &str) -> bool {
    warned.insert((code, about.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::tests::Scratch;

    /// A path inside the root is named relative to it, with `/` between its parts on every
    /// platform; one outside it as it's given.
    #[test]
    fn files_are_named_by_their_paths_from_the_root() {
        let root = Path::new("game");
        let inside = root.join("assets").join("logo.png");
        assert_eq!(&*name_of(Some(root), &inside), "assets/logo.png");
        let outside = Path::new("elsewhere").join("logo.png");
        assert_eq!(&*name_of(Some(root), &outside), "elsewhere/logo.png");
        assert_eq!(&*name_of(None, &inside), "game/assets/logo.png");
    }

    /// A file's name is its path as the layer wrote it, whatever case the disk keeps it in: a file
    /// system that ignores case keeps the case an editor last saved a file in, which git doesn't
    /// see, so that a name taken from the disk would differ between checkouts of one commit. The
    /// file itself is one however its path is written, through `.` and `..`, or, on Windows, with
    /// `\` or in another case.
    #[test]
    fn a_file_is_named_as_its_path_is_written() {
        let scratch = Scratch::new("named-as-written");
        let root = scratch.0.join("game");
        fs::create_dir_all(root.join("assets")).expect("a directory");
        fs::write(root.join("assets/Logo.png"), "").expect("a file");
        let logo = root.join("assets").join("logo.png");
        assert_eq!(&*name_of(Some(&root), &logo), "assets/logo.png");
        let real = root.join("assets").join("Logo.png");
        let file = file_of(&real).expect("the file");
        let about = root
            .join(".")
            .join("assets")
            .join("..")
            .join("assets/Logo.png");
        assert_eq!(file_of(&about).as_ref(), Some(&file));
        #[cfg(windows)]
        for spelled in [logo, root.join(r"assets\LOGO.PNG")] {
            assert_eq!(file_of(&spelled).as_ref(), Some(&file), "{spelled:?}");
        }
        assert_eq!(file_of(&root.join("assets/none.png")), None);
    }

    /// A scan gives each asset under the root the ID its ID file holds; writes the ID files missing
    /// where the run may, and says why an asset has none where it may not; warns of each problem
    /// once a run; never writes over an ID file that's there; keeps an asset's ID for the run,
    /// whatever its ID file comes to hold; and writes back no ID another ID file holds, as after a
    /// rename that took the ID file along.
    #[cfg(feature = "dev")]
    #[test]
    fn a_scan_writes_missing_id_files_where_the_run_may_and_each_problem_warns_once() {
        let logged = crate::testing::logged();
        let scratch = Scratch::new("scan-catalog");
        let root = scratch.0.as_path();
        fs::create_dir_all(root).expect("a directory");
        let id = |text: &str| -> AssetId { text.parse().expect("an ID") };
        let write = |name: &str, text: &str| fs::write(root.join(name), text).expect("written");
        let meta = |text: &str| Meta { id: id(text) }.text();
        let read = |name: &str| fs::read_to_string(root.join(name)).expect("an ID file");
        let names = || -> Vec<String> {
            let listed = fs::read_dir(root).expect("listed").map(|entry| {
                let name = entry.expect("an entry").file_name();
                name.into_string().expect("Unicode")
            });
            listed.collect()
        };
        let rescan = |catalog: &mut Catalog, writes| {
            if let Some(next) = scan(catalog, root, writes) {
                *catalog = next;
            }
        };
        let codes = || logged.codes.try_iter().collect::<Vec<_>>();
        // An asset's ID this run and the code of what's wrong with its ID file.
        let state = |catalog: &Catalog, path: &str| {
            let found = catalog.listed().find(|&(listed, _)| listed == path);
            let (_, cataloged) = found.unwrap_or_else(|| panic!("{path} listed"));
            (
                cataloged.id,
                cataloged.problem.as_ref().map(|problem| problem.code),
            )
        };
        let mut catalog = Catalog::default();

        write("a.png", "");
        write("b.png", "");
        write("b.png.meta", &meta("5f3c9a1e07b2d846"));
        rescan(&mut catalog, false);
        assert_eq!(state(&catalog, "a.png"), (None, Some("asset.no_id")));
        assert!(!root.join("a.png.meta").exists(), "a run that writes none");
        let b = Some(id("5f3c9a1e07b2d846"));
        assert_eq!(state(&catalog, "b.png"), (b, None));
        assert_eq!(scan(&catalog, root, false), None, "nothing changed");
        rescan(&mut catalog, true);
        let (a, problem) = state(&catalog, "a.png");
        let a = a.expect("an ID written");
        assert_eq!((read("a.png.meta"), problem), (Meta { id: a }.text(), None));
        assert_eq!(codes(), ["asset.no_id", "asset.id_written"]);

        // The ID it had, whatever its ID file holds now.
        write("b.png.meta", &meta("0a1b2c3d4e5f6071"));
        rescan(&mut catalog, true);
        rescan(&mut catalog, true);
        assert_eq!(state(&catalog, "b.png"), (b, Some("asset.id_changed")));
        assert_eq!(codes(), ["asset.id_changed"]);

        // An ID file that doesn't read, and one named for it but for case, which stay as they are.
        let broken = "(id: \"5f3c";
        write("bad.png", "");
        write("bad.png.meta", broken);
        write("Case.png", "");
        write("case.png.meta", &meta("1d2c3b4a59687f0e"));
        rescan(&mut catalog, true);
        rescan(&mut catalog, true);
        assert_eq!(state(&catalog, "bad.png"), (None, Some("asset.bad_meta")));
        assert_eq!(read("bad.png.meta"), broken);
        assert_eq!(state(&catalog, "Case.png"), (None, Some("asset.meta_case")));
        assert!(
            !names().contains(&"Case.png.meta".to_owned()),
            "{:?}",
            names()
        );
        assert_eq!(codes(), ["asset.meta_case", "asset.bad_meta"]);

        // Two that hold one ID, the first by path keeping it; and one whose asset isn't there.
        let shared = "2b3c4d5e6f708192";
        for name in ["c.png", "d.png"] {
            write(name, "");
            write(&format!("{name}.meta"), &meta(shared));
        }
        write("gone.png.meta", &meta("3c4d5e6f708192a3"));
        rescan(&mut catalog, true);
        rescan(&mut catalog, true);
        assert_eq!(state(&catalog, "c.png"), (Some(id(shared)), None));
        assert_eq!(state(&catalog, "d.png"), (None, Some("asset.duplicate_id")));
        let gone = Some(id("3c4d5e6f708192a3"));
        let orphan = (Arc::from("gone.png.meta"), AssetKind::Texture, gone);
        assert_eq!(catalog.orphans(), [orphan]);
        assert_eq!(codes(), ["asset.duplicate_id", "asset.orphaned"]);
        // Its ID file removed, the other gets a fresh one.
        fs::remove_file(root.join("d.png.meta")).expect("removed");
        rescan(&mut catalog, true);
        let (d, problem) = state(&catalog, "d.png");
        let d = d.expect("a fresh ID");
        assert_ne!(d, id(shared));
        assert_eq!((read("d.png.meta"), problem), (Meta { id: d }.text(), None));
        assert_eq!(codes(), ["asset.id_written"]);

        // Renamed with its ID file, then a new file at the old name, whose ID this run another ID
        // file holds now: nothing written back.
        write("e.png", "");
        rescan(&mut catalog, true);
        let e = state(&catalog, "e.png").0.expect("an ID written");
        for (from, to) in [("e.png", "f.png"), ("e.png.meta", "f.png.meta")] {
            fs::rename(root.join(from), root.join(to)).expect("renamed");
        }
        rescan(&mut catalog, true);
        assert_eq!(state(&catalog, "f.png"), (None, Some("asset.duplicate_id")));
        assert!(catalog.listed().all(|(path, _)| path != "e.png"), "gone");
        write("e.png", "");
        rescan(&mut catalog, true);
        assert_eq!(
            state(&catalog, "e.png"),
            (Some(e), Some("asset.duplicate_id"))
        );
        assert!(!names().contains(&"e.png.meta".to_owned()), "{:?}", names());
        let written = [
            "asset.id_written",
            "asset.duplicate_id",
            "asset.duplicate_id",
        ];
        assert_eq!(codes(), written);
        let mut held: Vec<AssetId> = (names().iter())
            .filter(|name| name.ends_with(".meta"))
            .filter_map(|name| Meta::read(&root.join(name)).ok())
            .map(|meta| meta.id)
            .collect();
        let files = held.len();
        held.sort_unstable();
        held.dedup();
        assert_eq!(held.len(), files, "no two ID files hold one ID");
    }

    /// A copy of an asset made with its ID file has no ID while the original keeps it this run,
    /// and is told which of the two keeps it from the next run, whose first scan gives it to the
    /// first by path whose ID file holds it: here the copy.
    #[cfg(feature = "dev")]
    #[test]
    fn a_duplicate_is_told_which_asset_keeps_the_id_from_the_next_run() {
        let scratch = Scratch::new("duplicate-next-run");
        let root = scratch.0.as_path();
        fs::create_dir_all(root).expect("a directory");
        let x: AssetId = "5f3c9a1e07b2d846".parse().expect("an ID");
        fs::write(root.join("m.png"), "").expect("an asset");
        fs::write(root.join("m.png.meta"), Meta { id: x }.text()).expect("its ID file");
        let catalog = scan(&Catalog::default(), root, true).expect("found");
        for name in ["m.png", "m.png.meta"] {
            fs::copy(root.join(name), root.join(name.replacen('m', "a", 1))).expect("copied");
        }
        let catalog = scan(&catalog, root, true).expect("changed");
        let (_, copy) = (catalog.listed().find(|&(path, _)| path == "a.png")).expect("listed");
        let problem = copy.problem.as_ref().expect("a problem");
        assert_eq!((copy.id, problem.code), (None, "asset.duplicate_id"));
        let told = "from the next run, a.png takes it";
        assert!(problem.message.contains(told), "{problem:?}");
        let next = scan(&Catalog::default(), root, true).expect("found");
        let ids: Vec<_> = next
            .listed()
            .map(|(path, asset)| (path, asset.id))
            .collect();
        assert_eq!(ids, [("a.png", Some(x)), ("m.png", None)]);
    }
}
