//! A development build's asset database: scanning the asset root, and the thread keeping it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use gg_api::asset::{AssetId, AssetKind};
use tracing::warn;

use super::Context;
use crate::Prefab;
use crate::asset_watch::AssetWatch;
use crate::assets::{self, Catalog, Prints};
use crate::hot_reload::HotReload;

impl Context {
    /// Scans the assets under the app's root, as [`assets::scan`] says, writing the ID files
    /// missing if `writes`, and warning of each missing otherwise, and has the worlds share what it
    /// found: as a test's context is given its root, and as the tool API lists them in a run that
    /// no thread watches them for. A context without a root has none, nor one that reads a package,
    /// whose assets have the IDs the package gives them. A texture is named by the ID
    /// the database holds for it as it's added, so that a scan after names none afresh.
    ///
    /// Each scan walks the whole root and reads every ID file on the main thread, O(files), and
    /// clones the catalog, O(assets): about 70 ms for 1,000 assets on Windows, but a few
    /// milliseconds apiece for ID files just copied in, the first time they're read; and each ID
    /// file it writes is flushed to the disk, about 6 ms apiece.
    #[cfg(feature = "dev")]
    pub(crate) fn scan_assets(&mut self, writes: bool) {
        self.scan_with(&mut Prints::default(), writes);
    }

    /// Scans the assets under the app's root, as [`scan_assets`](Self::scan_assets) does, pairing
    /// renames by `prints`.
    #[cfg(feature = "dev")]
    fn scan_with(&mut self, prints: &mut Prints, writes: bool) {
        // A run from a package reads its files there alone, and its assets' IDs.
        let Some(root) = self.root.as_deref().filter(|_| !self.reads_package()) else {
            return;
        };
        let _zone = gg_core::zone!();
        if let Some(catalog) = assets::scan(self.assets.catalog(), root, prints, writes) {
            self.set_catalog(catalog);
        }
    }

    /// Has the worlds share `catalog`, the asset database as a scan left it, and the thread that
    /// reloads assets follow each texture, model, font and prefab whose file it has elsewhere
    /// since, as [`HotReload::follow`] says.
    #[cfg(feature = "dev")]
    fn set_catalog(&mut self, catalog: Catalog) {
        self.add_assets(|assets| assets.set_catalog(catalog));
        let catalog = self.assets.catalog();
        self.uis.follow(|asset| {
            let id = asset
                .parse::<AssetId>()
                .ok()
                .filter(|_| assets::is_id(asset))?;
            catalog.path(id).map(Arc::from)
        });
        if let (Some(hot_reload), Some(root)) = (&mut self.dev.hot_reload, &self.root) {
            hot_reload.follow(self.assets.catalog(), root);
        }
    }

    /// As a run starts, before the layers load, if a tool API `serves` the run, starts the thread
    /// that reloads the layers' shaders, textures, models, fonts, prefabs, sounds and music as
    /// their files change, as `hot_reload` says, warning if it can't; then scans the assets under
    /// the app's root, as [`scan_assets`](Self::scan_assets) does, writing the ID files missing if
    /// `writes`, and moving those that renames made since the last run left behind beside their
    /// assets, by the prints its cache keeps, as [`assets::scanned`] says; then, if the API serves
    /// the run, starts the thread that keeps the database from then on, as `asset_watch` says,
    /// warning if it can't, so that the API's `assets` scans on the main thread instead. A context
    /// without a root has no assets to scan, nor does a run that reads its game's package, which
    /// reloads none.
    #[cfg(feature = "dev")]
    pub(crate) fn start_assets(&mut self, serves: bool, writes: bool) {
        if self.reads_package() {
            return;
        }
        if serves {
            match HotReload::start() {
                Ok(hot_reload) => self.dev.hot_reload = Some(hot_reload),
                Err(error) => warn!(
                    code = error.code(),
                    "{error:#}; assets don't reload as their files change"
                ),
            }
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let cache = self.dev.asset_cache.clone().filter(|_| writes);
        let mut prints = match &cache {
            Some(cache) => Prints::read(cache, &root),
            None => Prints::default(),
        };
        self.scan_with(&mut prints, writes);
        if !serves {
            return;
        }
        let catalog = self.assets.catalog().clone();
        match AssetWatch::start(root, catalog, prints, writes, cache) {
            Ok(watch) => self.dev.asset_watch = Some(watch),
            Err(error) => warn!(
                code = error.code(),
                "{error:#}; the tool API's `assets` scans the asset root itself"
            ),
        }
    }

    /// Takes the asset database the thread that watches the asset root sent since, if it changed,
    /// and has the worlds share it: as a frame starts, before the tool API's requests are answered.
    #[cfg(feature = "dev")]
    pub(crate) fn take_assets(&mut self) {
        let catalog = self.dev.asset_watch.as_mut().and_then(AssetWatch::take);
        if let Some(catalog) = catalog {
            self.set_catalog(catalog);
        }
    }

    /// Whether a thread watches the asset root, keeping the database.
    #[cfg(feature = "dev")]
    pub(crate) fn watches_assets(&self) -> bool {
        self.dev.asset_watch.is_some()
    }

    /// Asks the thread that watches the asset root to look now, as the tool API's `assets` does;
    /// returns the look's number, which [`assets_answered`](Self::assets_answered) reaches once
    /// it's taken; `None` in a run that no thread watches the root for.
    #[cfg(feature = "dev")]
    pub(crate) fn ask_assets(&self) -> Option<u64> {
        self.dev.asset_watch.as_ref().map(AssetWatch::ask)
    }

    /// The looks [`ask_assets`](Self::ask_assets) asked for that have been taken: those up to it.
    #[cfg(feature = "dev")]
    pub(crate) fn assets_answered(&self) -> u64 {
        self.dev
            .asset_watch
            .as_ref()
            .map_or(u64::MAX, AssetWatch::answered)
    }

    /// The asset database, as the last scan left it.
    #[cfg(feature = "dev")]
    pub(crate) fn catalog(&self) -> &Catalog {
        self.assets.catalog()
    }

    /// Takes the asset of `kind` the run has just written at `path`, with its ID file holding `id`,
    /// into the asset database, as [`Catalog::bind`] does, and asks the thread that watches the
    /// root to look, so that its database takes it too.
    #[cfg(feature = "dev")]
    pub(crate) fn bind_asset(&mut self, path: Arc<str>, kind: AssetKind, id: AssetId) {
        self.add_assets(|assets| assets.bind(path, kind, id));
        self.ask_assets();
    }

    /// Adds the prefab of `text`, which the run has just written in a new file at `path` from the
    /// asset root, with `/` between its parts, named `name`, its asset ID or else its path: as
    /// [`add_prefab`](Self::add_prefab) adds one it reads, watched as its file changes, but a new
    /// prefab whatever was loaded from a file there before, as one deleted since, which neither
    /// `path` nor the file names from now on.
    #[cfg(feature = "dev")]
    pub(crate) fn add_saved_prefab(&mut self, path: &str, name: &str, text: String) -> Prefab {
        // The file holds the text from now on, as a read begun now would give it.
        let read_at = SystemTime::now();
        let file = match &self.root {
            Some(root) => root.join(path),
            None => PathBuf::from(path),
        };
        let text: Arc<str> = text.into();
        let (path, name): (Arc<str>, Arc<str>) = (path.into(), name.into());
        let (held, named) = (Arc::clone(&text), name.clone());
        let prefab = self.add_assets(|assets| assets.add_saved_prefab(path.clone(), named, held));
        self.watch_prefab(prefab, &path, &file, read_at, &name);
        if let Some(resolved) = self.files.file_of(&file) {
            self.loaded.prefabs.insert(resolved, prefab);
        }
        self.added("prefab", &path);
        // What its text names, loaded already, as the entities it was saved from have it.
        let _ = self.load_scene_assets(&text);
        prefab
    }

    /// Whether a layer has added the file at `path` from the app's root, by whatever form of its
    /// path, as a texture, a sprite sheet or a model, or a texture its asset's ID names is loaded,
    /// as one loaded from it before it was renamed is, and not removed since: resolving the file,
    /// some 0.1 ms on Windows.
    #[cfg(feature = "dev")]
    pub(crate) fn has_added(&self, path: &str) -> bool {
        let file = (self.root.as_deref()).and_then(|root| assets::file_of(&root.join(path)));
        let by_file = file.is_some_and(|file| {
            self.loaded.textures.contains_key(&file) || self.loaded.models.contains_key(&file)
        });
        let id = self.assets.catalog().id(path);
        by_file || id.is_some_and(|id| self.assets.live_texture(&id.to_string()).is_some())
    }
}
