//! The world: the layers' access to it, its scene files and changes, and edits from outside.

use std::path::Path;
#[cfg(feature = "dev")]
use std::path::PathBuf;
use std::time::Instant;

use gg_core::{Error, Result};
#[cfg(doc)]
use gg_renderer::{Circle, Sprite};
use tracing::{debug, error, info, warn};

use super::Context;
#[cfg(doc)]
use super::{Layer, Stack};
use crate::assets::{self, Asset};
#[cfg(doc)]
use crate::edits::Authors;
use crate::edits::{Done, Edit};
use crate::package::{self, Files};
use crate::perf;
use crate::scene_files::{self, SceneFile};
#[cfg(doc)]
use crate::{App, MeshDraw, Transform};
use crate::{Entity, World};

impl Context {
    /// The world, as the frame's ticks left it. After the layers' updates each frame, the engine
    /// draws what has a [`Transform`] and a [`Sprite`], [`Circle`], [`Text`](crate::Text) or
    /// [`MeshDraw`], after what the layers drew. Opaque-shader meshes come first, whatever their
    /// z's, by shader, then mesh. The rest go back to front by z; at one z, sprites, circles,
    /// then text, under meshes, each in ID order, children where their parents carry them. Each
    /// is drawn on its way from where the latest tick started it to where the tick left it, so up
    /// to a tick behind the world here.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The world, to change as a layer attaches or detaches. Otherwise it changes only at ticks
    /// ([`Layer::tick`] is given it) and this panics: a layer queues a command for its next tick.
    pub fn world_mut(&mut self) -> &mut World {
        assert!(
            !self.attached,
            "a layer changes the world only as it attaches or detaches, and at its ticks: queue a \
             command for its next tick"
        );
        &mut self.world
    }

    /// Loads the scene file at `path` into the world as the layer attaches
    /// ([`World::load_scene`] of what [`read_scene`](crate::read_scene) reads), and returns its
    /// roots. It first loads each texture, model, font and prefab the scene names, by ID or path
    /// from the asset root, that no layer has added ([`add_texture`](Self::add_texture),
    /// [`add_model`](Self::add_model), [`add_font`](Self::add_font),
    /// [`add_prefab`](Self::add_prefab)).
    ///
    /// A development build watches the file. A save while the game runs applies between ticks as
    /// operations taking the world's entities from what the file held to what it holds, keeping
    /// what the file doesn't change. The edit's author is the file's path from the app's root,
    /// such as `scenes/level.ron`; the tool API's `undo` takes it back. A run restored from a
    /// snapshot watches the files its run had loaded.
    ///
    /// Logs `scene.loaded`, with the roots made and the time taken.
    ///
    /// Fails as `read_scene` and `load_scene` do, the path beginning the message. An asset that
    /// can't be loaded gives `asset.no_texture`, `asset.no_model`, `asset.no_font` or
    /// `asset.no_prefab`, with why. In a replay or a lockstep joiner, which goes as another run
    /// went, such an asset stands in, warning `asset.stand_in`.
    ///
    /// # Panics
    ///
    /// Outside the layers' attach and detach, as [`world_mut`](Self::world_mut) does.
    pub fn load_scene_file(&mut self, path: impl AsRef<Path>) -> Result<Vec<Entity>> {
        let started = Instant::now();
        let path = path.as_ref();
        let text = crate::ecs::read_scene_from(path, &self.files)?;
        let mut loaded = self.world_mut().load_scene(&text);
        // Seldom: a scene that names what's loaded is read once.
        if loaded.as_ref().is_err_and(|error| {
            matches!(
                error.code(),
                "asset.no_texture" | "asset.no_model" | "asset.no_font" | "asset.no_prefab"
            )
        }) {
            loaded =
                (self.load_scene_assets(&text)).and_then(|_| self.world_mut().load_scene(&text));
        }
        let roots = loaded.map_err(|error| {
            let message = format!("{}:{error}", path.display());
            error.with_message(message)
        })?;
        let author = assets::name_of(self.root.as_deref(), path).to_string();
        let start = self.start.as_deref();
        if start.is_some_and(|start| scene_files::same_file(start, path)) {
            self.scenes.open.author = Some(author.clone());
        }
        let file = SceneFile::new(path.to_owned(), author, text);
        // For the scene changes to come, which may name it.
        self.scenes.cache(file.clone());
        self.scenes.loaded(file);
        info!(
            code = "scene.loaded",
            path = %path.display(),
            roots = roots.len(),
            ms = perf::millis(started.elapsed()),
            "Loaded the scene file"
        );
        Ok(roots)
    }

    /// Loads the project's start scene as the layer attaches
    /// ([`load_scene_file`](Self::load_scene_file)) and returns its roots. It's the open scene,
    /// which the tool API's `scene` saves, empties and replaces. It owns the entities its file
    /// holds and those clients' edits make into it, never those the game's code makes. Its edits'
    /// author is its path from the asset root, such as `scenes/arena.ron`.
    ///
    /// Fails with `engine.no_project` for a game that isn't a project's
    /// ([`App::project`](crate::App::project)), and as `load_scene_file` does.
    ///
    /// # Panics
    ///
    /// Outside the layers' attach and detach, as [`world_mut`](Self::world_mut) does.
    pub fn load_start_scene(&mut self) -> Result<Vec<Entity>> {
        let Some(start) = self.start.clone() else {
            let message = "the game isn't a project's, which has a start scene: `App::project` \
                           makes it one";
            return Err(Error::new("engine.no_project", message));
        };
        self.load_scene_file(start)
    }

    /// Drops the scene a layer asked for outside a tick ([`World::change_scene`] in its `update`,
    /// `event` or `attach`), warning `scene.not_in_tick`. It runs as each tick starts, before the
    /// layers' ticks, so only what a tick asks for goes in, as a recording and the other lockstep
    /// peer have it.
    pub(crate) fn drop_untimely_scene(&mut self) {
        if let Some(scene) = self.world.take_asked_scene() {
            warn!(
                code = "scene.not_in_tick",
                scene = &*scene,
                "A layer asked for the scene {scene} outside its tick, where the world doesn't \
                 change, so it's dropped: a layer asks in its tick, as for a command it queued"
            );
        }
    }

    /// If the world's last tick asked for a scene ([`World::change_scene`]), puts it in place of
    /// the world's entities ([`change_scene`](Self::change_scene)), as each layer's tick returns,
    /// in a rollback or a SyncTest too. Logs `scene.entered` at the debug level. On an error the
    /// world stays as it was and notes it for [`confirm_scene`](Self::confirm_scene) to log once
    /// the tick is final: a tick that may run again would log it twice, and one that never does
    /// would log a refusal that didn't happen.
    pub(crate) fn change_asked_scene(&mut self) {
        let Some(scene) = self.world.take_asked_scene() else {
            return;
        };
        let started = Instant::now();
        match self.change_scene(&scene) {
            Ok(roots) => debug!(
                code = "scene.entered",
                scene = &*scene,
                roots,
                ms = perf::millis(started.elapsed()),
                "Put the scene's entities in place of the world's"
            ),
            Err(error) => {
                debug!(
                    refused = error.code(),
                    scene = &*scene,
                    "{error:#}; the world stays as it was"
                );
                self.world.refuse_scene(&scene, &error);
            }
        }
    }

    /// Puts scene file `scene` (its path from the asset root, `/` between parts) in place of the
    /// world's entities ([`World::change_scene`]), and returns the roots made. The text is
    /// [`scene_text`](Self::scene_text)'s. Models and prefabs it names that no layer has added
    /// load first, none standing in; then textures and fonts, standing in if they can't load. So
    /// whether the scene goes in depends on its text and its models and prefabs alone. A server's
    /// session, clients too, loads nothing: a client reads its server's states only of the assets
    /// loaded.
    ///
    /// Fails, changing nothing, with:
    ///
    /// - `scene.bad_path` for a name that isn't a path inside the asset root: absolute, empty, or
    ///   holding `.`, `..`, `\`, `:` or a control character.
    /// - `asset.no_model` or `asset.no_prefab`, saying why.
    /// - `net.no_scene_change` in a server's session for a scene that names an asset the layers
    ///   haven't loaded.
    /// - The errors of `scene_text` and [`World::load_scene`] into the world without its
    ///   entities, the name beginning the message.
    pub(crate) fn change_scene(&mut self, scene: &str) -> Result<usize> {
        let _zone = gg_core::zone!();
        if !package::inside(scene) || scene.chars().any(char::is_control) {
            let message = format!(
                "`{scene}` isn't a scene file's path from the asset root, `/` between its parts, \
                 none of them empty, `.` or `..`, nor holding `\\`, `:` or a control character, \
                 which a game names its scenes by on every OS and in its package alike"
            );
            return Err(Error::new("scene.bad_path", message));
        }
        let text = self.scene_text(scene)?;
        self.load_assets_of(scene, &text)?;
        let roots = self.world.change_entities(scene, &text).map_err(|error| {
            let message = format!("{scene}:{error}");
            error.with_message(message)
        })?;
        Ok(roots.len())
    }

    /// Loads what `text`, of scene `scene`, names that no layer has added
    /// ([`change_scene`](Self::change_scene)). In a server's session it loads none and refuses a
    /// scene that names what no layer loaded.
    ///
    /// Fails as `change_scene` does for what the scene names, the name beginning the message.
    pub(crate) fn load_assets_of(&mut self, scene: &str, text: &str) -> Result<()> {
        // A world put in the world's place, as a SyncTest's copy, may share fewer assets than the
        // layers have added since, which the scene may name.
        self.share_assets();
        let named = |error: Error| {
            let message = format!("{scene}:{error}");
            error.with_message(message)
        };
        if !self.served {
            return self.load_change_assets(text).map_err(named);
        }
        let missing = self.world.scene_assets(text).map_err(named)?;
        if let Some(asset) = missing.first() {
            let message = format!(
                "{scene} names {}, which no layer has loaded, and a server's session loads none as \
                 it plays, so that its clients read its states: load what the game's scenes name as \
                 its layers load",
                asset.name()
            );
            return Err(Error::new("net.no_scene_change", message));
        }
        Ok(())
    }

    /// Loads what scene `text` names that no layer has added, for
    /// [`change_scene`](Self::change_scene): models and prefabs first, and those the prefabs name
    /// in turn, none standing in, so a refusal leaves no stand-in behind; then textures and
    /// fonts, standing in if they can't be loaded.
    ///
    /// Fails as [`World::load_scene`] does into the world without its entities, but for what it
    /// names, and with `asset.no_model` or `asset.no_prefab`, saying why.
    fn load_change_assets(&mut self, text: &str) -> Result<()> {
        loop {
            let names = self.world.scene_assets(text)?;
            let heavy =
                |asset: &&Asset<String>| matches!(asset, Asset::Model(_) | Asset::Prefab(_));
            let heavy: Vec<&Asset<String>> = names.iter().filter(heavy).collect();
            if heavy.is_empty() {
                break;
            }
            self.load_named(heavy.iter().map(|asset| asset.as_deref()), false);
            if let Some(asset) = heavy.into_iter().find(|asset| !self.assets.has(asset)) {
                let (code, kind) = match asset {
                    Asset::Model(_) => ("asset.no_model", "model"),
                    _ => ("asset.no_prefab", "prefab"),
                };
                let message = format!(
                    "{}; a scene change stands in for no model or prefab, which would leave the \
                     scene without what they hold",
                    self.assets.not_found(kind, asset.name())
                );
                return Err(Error::new(code, message));
            }
        }
        self.load_scene_assets_as(text, true).map(drop)
    }

    /// The text of scene file `scene` (its path from the asset root) for the run's scene changes:
    /// as the run read it once, as it started or as a change first named it, or as a development
    /// build's edit took a save of it since, so a tick run again, a replay and each lockstep peer
    /// read the same. Otherwise it's read now, from the package if the run reads one, else from
    /// disk, and kept.
    ///
    /// Fails with:
    ///
    /// - `scene.no_root` in a run without an asset root ([`App::root`]).
    /// - `scene.not_found` for a file that isn't there.
    /// - `scene.case` for a name that differs from the file's in case.
    /// - `asset.outside_root` for a link that leads outside the root.
    /// - `scene.file` for a file that can't be read, is over 16 MiB or isn't UTF-8.
    ///
    /// [`App::root`]: crate::App::root
    pub(crate) fn scene_text(&mut self, scene: &str) -> Result<String> {
        if let Some(file) = self.scenes.cached.iter().find(|file| file.author == scene) {
            return Ok(file.text.clone());
        }
        let Some(root) = self.root.clone() else {
            return Err(no_root(scene));
        };
        let path = package::under(&root, scene);
        let text = Self::read_scene_text(&self.files, &root, scene, &path, true)?;
        self.scenes
            .cache(SceneFile::new(path, scene.to_owned(), text.clone()));
        Ok(text)
    }

    /// Reads scene file `scene`, at `path` under `root`, from `files`, as
    /// [`scene_text`](Self::scene_text) does, on any thread. If `named`, checks its name against
    /// the disk's, part by part, for one the disk didn't give.
    pub(crate) fn read_scene_text(
        files: &Files,
        root: &Path,
        scene: &str,
        path: &Path,
        named: bool,
    ) -> Result<String> {
        /// The most a scene file holds, as the most a prefab's does.
        const MOST: u64 = 16 << 20;
        let packaged = files.packaged().is_some();
        let not_found = |within: &str| {
            let message = format!("there's no scene {scene} in {within}");
            Error::new("scene.not_found", message)
        };
        let case = |found: &str| {
            let message = format!(
                "the scene {scene} is {found}, in another case, which a package or a disk that \
                 tells case apart, as Linux's does, wouldn't find: name it as its file is named"
            );
            Error::new("scene.case", message)
        };
        if !packaged {
            let exactly = if named {
                scene_files::named_exactly(root, scene)
            } else {
                Ok(())
            };
            exactly.map_err(|found| {
                found.map_or_else(|| not_found("the asset root"), |found| case(&found))
            })?;
            let canonical = |path: &Path| std::fs::canonicalize(path).ok();
            if let (Some(root), Some(file)) = (canonical(root), canonical(path))
                && !file.starts_with(root)
            {
                let message = format!(
                    "the scene {scene} is a link to {}, outside the asset root, which a package \
                     wouldn't hold",
                    file.display()
                );
                return Err(Error::new("asset.outside_root", message));
            }
        }
        let bytes = match files.read(path, MOST + 1) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && packaged => {
                let paths = files.paths();
                let other = (paths.iter())
                    .filter_map(|other| other.strip_prefix(root).ok())
                    .map(|other| other.to_string_lossy().replace('\\', "/"))
                    .find(|other| other.to_lowercase() == scene.to_lowercase());
                return Err(match other {
                    Some(other) => case(&other),
                    None => not_found("the game's package").with_source(error),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(not_found("the asset root").with_source(error));
            }
            Err(error) => {
                let message = format!("can't read the scene {scene}");
                return Err(Error::new("scene.file", message).with_source(error));
            }
        };
        if bytes.len() as u64 > MOST {
            let message = format!("the scene {scene} is more than 16 MiB, which no change reads");
            return Err(Error::new("scene.file", message));
        }
        String::from_utf8(bytes).map_err(|error| {
            let message = format!("the scene {scene} isn't UTF-8 text");
            Error::new("scene.file", message).with_source(error)
        })
    }

    /// Reads each scene file in the project's scenes folders, in a development build reading the
    /// disk, and keeps its text as [`scene_text`](Self::scene_text) does. It runs as the run
    /// starts, so each lockstep peer and a replay reads them then, not at a tick. One that can't
    /// be read is left for a change to name, which fails as `scene_text` does.
    #[cfg(feature = "dev")]
    pub(crate) fn read_scenes(&mut self) {
        let _zone = gg_core::zone!();
        let Some(root) = self.root.clone() else {
            return;
        };
        if self.reads_package() {
            return;
        }
        let mut found = Vec::new();
        // Each folder as the disk names it, so each directory is listed once.
        let named = |folder: &PathBuf| {
            let name = assets::name_of(Some(&root), folder);
            package::inside(&name) && scene_files::named_exactly(&root, &name).is_ok()
        };
        let mut folders: Vec<PathBuf> = self
            .dev
            .folders
            .iter()
            .filter(|f| named(f))
            .cloned()
            .collect();
        while let Some(folder) = folders.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() && !hidden => folders.push(path),
                    Ok(kind)
                        if kind.is_file()
                            && !hidden
                            && path.extension().is_some_and(|ext| ext == "ron") =>
                    {
                        found.push(path);
                    }
                    _ => {}
                }
            }
        }
        found.sort();
        for path in found {
            let scene = assets::name_of(Some(&root), &path).to_string();
            let cached = self.scenes.cached.iter().any(|file| file.author == scene);
            if cached || !package::inside(&scene) {
                continue;
            }
            if let Ok(text) = Self::read_scene_text(&self.files, &root, &scene, &path, false) {
                self.scenes.cache(SceneFile::new(path, scene, text));
            }
        }
    }

    /// Runs once a tick is final: at once in a run alone, a replay, a lockstep session that
    /// predicts nothing and a server; in a rollback session once it has run on both peers'
    /// inputs; and as such a tick starts, the world before it final too. It takes what changed the
    /// world's scene since it last did ([`World::change_scene`]), logging `scene.replaced`. The
    /// scene's file is then the scene file in the world (the open scene in a project's game),
    /// which a development build applies saves of, in place of the layers' loads, whose entities
    /// went. It also logs the last change refused since, at the error level, with its code and
    /// count. A client's ticks are never final; its server's world is.
    pub(crate) fn confirm_scene(&mut self) {
        self.confirm_scene_from(None);
    }

    /// Takes the world's scene and refused changes as already confirmed, for a world that comes
    /// in from a snapshot or a lockstep host, or held aside while a copy played, with its scene
    /// files.
    pub(crate) fn seen_scene(&mut self) {
        self.scene_seen = self.world.scene_changes();
        self.refused_seen = self.world.scenes_refused();
    }

    /// [`confirm_scene`](Self::confirm_scene) of `world`, a final world, or the context's own for
    /// `None`. It allocates nothing while nothing changed.
    pub(crate) fn confirm_scene_from(&mut self, world: Option<&World>) {
        let world = match world {
            Some(world) => world,
            None => &self.world,
        };
        let (changes, refused) = (world.scene_changes(), world.scenes_refused());
        if (changes, refused) == (self.scene_seen, self.refused_seen) {
            return;
        }
        let changed = changes != self.scene_seen;
        let scene = world.scene().filter(|_| changed).map(str::to_owned);
        let last = (refused != self.refused_seen)
            .then(|| world.resource::<crate::scene_change::Refused>())
            .flatten()
            .and_then(|refused| refused.last.clone());
        let times = refused - self.refused_seen;
        (self.scene_seen, self.refused_seen) = (changes, refused);
        if let Some((code, refused, message)) = last {
            error!(
                code = &*code,
                scene = &*refused,
                times,
                "{message}; the world stays as it was"
            );
        }
        let Some(scene) = scene else {
            return;
        };
        info!(
            code = "scene.replaced",
            scene = &*scene,
            changes,
            "The scene's entities took the place of the world's"
        );
        let file = (self.scenes.cached.iter())
            .find(|file| file.author == scene)
            .cloned();
        self.scenes.files.clear();
        self.scenes.open = scene_files::Open {
            author: (self.start.is_some() && file.is_some()).then(|| scene.clone()),
            ..scene_files::Open::default()
        };
        self.scenes.files.extend(file);
        #[cfg(feature = "dev")]
        self.scenes.changed();
    }

    /// Applies `edit`, an author's from outside the layers, to the world, between ticks,
    /// unrecorded, as a replay does ([`Stack::edit`] records it first). A script's reload, which
    /// no author undoes, goes to the [`Scripts`](crate::Scripts) that loaded it.
    ///
    /// Fails as [`Authors::edit`] does, and as [`Hosts::reload`](crate::luau::Hosts::reload) does.
    pub(crate) fn apply_edit(&mut self, edit: Edit) -> Result<Done> {
        #[cfg(feature = "dev")]
        self.scenes.changed();
        match edit {
            Edit::Script { path, text } => {
                self.hosts.reload(&mut self.world, &path, text)?;
                Ok(Done::Applied(Vec::new()))
            }
            Edit::Scene { path, text } => {
                // A cell that came in from another text no longer goes out as it came.
                if let Some(cell) = self.world.cell_of_file(&path) {
                    self.world.forget_came_as(cell);
                }
                let root = self.root.clone().unwrap_or_default();
                let file = SceneFile::new(package::under(&root, &path), path, text);
                self.scenes.cache(file);
                Ok(Done::Applied(Vec::new()))
            }
            // Every log goes, the authors' and layers' (the Inspector's), which the renewed world
            // refuses, freeing nothing twice.
            Edit::Renew => {
                let dropped = self.authors.entries(&self.world);
                self.authors.clear(&mut self.world);
                self.world.renew();
                if dropped > 0 {
                    warn!(
                        code = "edits.history_cleared",
                        dropped,
                        "Dropped the {dropped} edits the authors could undo or redo, as a lockstep \
                         joiner joined, or the peers' worlds were made the same again: no log of \
                         the joiner's holds them"
                    );
                }
                Ok(Done::Applied(Vec::new()))
            }
            edit => {
                let done = self.authors.edit(&mut self.world, edit)?;
                // Of the world in the context's place: a copy's goes with it at `stop`.
                self.scenes.open.edited = true;
                Ok(done)
            }
        }
    }
}

/// The refusal of `scene` in a run without an asset root ([`App::root`]): `scene.no_root`.
pub(super) fn no_root(scene: &str) -> Error {
    let message = format!(
        "there's no asset root to find the scene {scene} under: `App::root` or `App::project` \
         gives the game one"
    );
    Error::new("scene.no_root", message)
}
