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
    /// Scans the assets under the app's root, as [`assets::scan`] says, and has the worlds share
    /// the result. Missing ID files are written if `writes`, else warned of.
    ///
    /// Used by a test's context given a root, and by the tool API in a run no thread watches. A
    /// context without a root scans nothing, nor does one that reads a package, whose assets have
    /// the package's IDs. A texture is named by its database ID as it's added, so a later scan
    /// names none afresh.
    ///
    /// Each scan walks the root, reads every ID file and clones the catalog on the main thread:
    /// O(files + assets), about 70 ms for 1,000 assets on Windows. ID files just copied in take a
    /// few milliseconds apiece the first time they're read. Each ID file written is flushed, about
    /// 6 ms apiece.
    #[cfg(feature = "dev")]
    pub(crate) fn scan_assets(&mut self, writes: bool) {
        self.scan_with(&mut Prints::default(), writes);
    }

    /// Like [`scan_assets`](Self::scan_assets), pairing renames by `prints`.
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

    /// Has the worlds share `catalog`, the database a scan left, and the reload thread follow each
    /// texture, model, font and prefab whose file moved since, as [`HotReload::follow`] says.
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

    /// As a run starts, before the layers load:
    ///
    /// 1. If a tool API `serves` the run, starts the thread that reloads the layers' shaders,
    ///    textures, models, fonts, prefabs, sounds and music as their files change, as
    ///    `hot_reload` says. Warns if it can't.
    /// 2. Scans the assets as [`scan_assets`](Self::scan_assets) does. ID files that renames since
    ///    the last run left behind are moved beside their assets, by the prints the cache keeps, as
    ///    [`assets::scanned`] says.
    /// 3. If the API serves the run, starts the thread that keeps the database from then on, as
    ///    `asset_watch` says. If it can't, warns, and the API's `assets` scans on the main thread.
    ///
    /// A context without a root has nothing to scan, nor does a run that reads its package, which
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

    /// As a frame starts, before the tool API's requests are answered: takes the database the
    /// watching thread sent, if it changed, and has the worlds share it.
    #[cfg(feature = "dev")]
    pub(crate) fn take_assets(&mut self) {
        let catalog = self.dev.asset_watch.as_mut().and_then(AssetWatch::take);
        if let Some(catalog) = catalog {
            self.set_catalog(catalog);
        }
    }

    /// Whether a thread watches the asset root.
    #[cfg(feature = "dev")]
    pub(crate) fn watches_assets(&self) -> bool {
        self.dev.asset_watch.is_some()
    }

    /// Asks the watching thread to look now, as the tool API's `assets` does. Returns the look's
    /// number, which [`assets_answered`](Self::assets_answered) reaches once it's taken. `None` if
    /// no thread watches.
    #[cfg(feature = "dev")]
    pub(crate) fn ask_assets(&self) -> Option<u64> {
        self.dev.asset_watch.as_ref().map(AssetWatch::ask)
    }

    /// The number of the last look taken; those up to it are answered.
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

    /// Takes the asset of `kind` just written at `path`, its ID file holding `id`, into the
    /// database as [`Catalog::bind`] does, and asks the watching thread to look so its copy does
    /// too.
    #[cfg(feature = "dev")]
    pub(crate) fn bind_asset(&mut self, path: Arc<str>, kind: AssetKind, id: AssetId) {
        self.add_assets(|assets| assets.bind(path, kind, id));
        self.ask_assets();
    }

    /// Adds the prefab of `text`, just written in a new file at `path` (from the asset root, `/`
    /// separated), named `name`: its asset ID, else its path.
    ///
    /// Like [`add_prefab`](Self::add_prefab), it's watched as its file changes. It is always a new
    /// prefab, whatever was loaded from a file there before: one deleted since is named by neither
    /// `path` nor the file.
    #[cfg(feature = "dev")]
    pub(crate) fn add_saved_prefab(&mut self, path: &str, name: &str, text: String) -> Prefab {
        // The file holds the text now, as a read begun now would give it.
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
        // What its text names is loaded already, by the entities it was saved from.
        let _ = self.load_scene_assets(&text);
        prefab
    }

    /// Whether a layer has added the file at `path` from the root, by any form of its path, as a
    /// texture, sprite sheet or model, and not removed it since. Also true if a texture its asset
    /// ID names is loaded, as one loaded before a rename is. Resolving the file takes some 0.1 ms
    /// on Windows.
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
