//! Assets by their names: what scenes, snapshots, recordings, lockstep inputs, the world's hash and
//! the tool API name a texture by, where its handle is the renderer's own, so that a restart in
//! another process, or another machine, finds it again: its asset ID, where it has one, and else
//! its path; what the layers loaded, and what was loaded where text named it, by their files as the
//! OS resolves them, so that a file loads once however its path is written; and, in development
//! builds, the asset database, `Catalog`: the textures, models, fonts and prefabs under the root
//! and the IDs their ID files hold, whose text and scan are gg-api's, an ID going with its asset as
//! it's renamed or moved, paired by what its file holds where the rename left its ID file behind.

use std::collections::HashMap;
#[cfg(feature = "dev")]
use std::collections::{BTreeMap, HashSet};
use std::fs;
#[cfg(feature = "dev")]
use std::fs::File;
#[cfg(feature = "dev")]
use std::io::{self, Read};
#[cfg(feature = "dev")]
use std::mem;
use std::path::{self, Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "dev")]
use std::time::{Duration, SystemTime};

#[cfg(feature = "dev")]
use gg_api::Failure;
#[cfg(feature = "dev")]
use gg_api::asset::{self as asset_files, AssetId, AssetKind, Meta, Orphan, Scan, Scanned, Stamp};
use gg_renderer::Texture;
#[cfg(feature = "dev")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "dev")]
use tracing::{info, warn};

#[cfg(feature = "dev")]
use crate::dirs::{self, Kind};
#[cfg(feature = "dev")]
use crate::ecs::mix64;
use crate::model::{Made, Nodes};
use crate::{Font, Model, Music, Prefab, Sound};

/// The textures a context added, as its layers added them or where text named them, each by its
/// index, with the path of the file it was loaded from: relative to the app's
/// [root](crate::App::root), with `/` between its parts, or as it was given for a file outside it;
/// a model's images by the model's path and their order in it, such as `assets/truck.glb#0`. Each
/// is named, as text names it, by its asset ID, 16 hex digits, or a model's image by its model's and
/// its order, such as `5f3c9a1e07b2d846#0`, where the context gave it one as it was added, or since,
/// as a lockstep joiner takes its host's names, and otherwise by its path; a stand-in for one that
/// couldn't be loaded, by the name it stands in for. The engine's white texture has none. A
/// context's worlds and their copies share it, and what a world loads from text finds its textures
/// here, by a name or a path, as an entity is found by its ID: text no texture has names none, and
/// says why it couldn't be loaded, if it was tried. A texture or a model removed keeps its name and
/// path, which its names go on finding, so that what the simulation reads doesn't change with when
/// it was removed; but what loads by a name takes it for none, loading its file anew, and names
/// the new one from then on. In development builds it holds the asset database too, as the last
/// scan left it.
#[derive(Clone, Debug)]
pub(crate) struct Assets {
    /// Each texture's name and path, by its index.
    textures: Vec<Option<Named>>,
    /// Each texture loaded from a file, by its name and by each path the layers gave its file: the
    /// first added, if two have one.
    by_name: HashMap<Arc<str>, Texture>,
    /// Each model's name and path, as a texture's, and what its root spawns, by its index.
    models: Vec<NamedModel>,
    /// Each model, by its name and by each path the layers gave its file, as a texture by
    /// `by_name`.
    model_names: HashMap<Arc<str>, Model>,
    /// Each font's name and path, as a texture's, by its index.
    fonts: Vec<(Arc<str>, Arc<str>)>,
    /// Each font, by its name and by each path the layers gave its file, as a texture by `by_name`.
    font_names: HashMap<Arc<str>, Font>,
    /// Each prefab's name and path, as a texture's, and its text, by its index.
    prefabs: Vec<NamedPrefab>,
    /// Each prefab, by its name and by each path the layers gave its file, as a texture by
    /// `by_name`.
    prefab_names: HashMap<Arc<str>, Prefab>,
    /// What the layers added, and the textures loaded where text named them, and stand-ins, in
    /// order, each with how many meshes, shaders and textures there were after it, as
    /// `texture 1 1 2 assets/logo.png`: what names the meshes, shaders and textures by their
    /// indices, so that what's opaque in a snapshot, which holds indices, comes back only into a
    /// run whose layers added the same, those named loaded again as it's restored. A mesh's, a
    /// shader's and a model's give the types their draws hold the bytes of too, by their names and
    /// sizes, and a shader's its blend, as
    /// `shader 1 2 1 shaders/colored.slang game::Colored 28 () 0 Opaque`, so that a draw comes back
    /// only where its bytes mean what they did. A file a layer adds again counts again; one that
    /// text names, loaded already, doesn't, as a run that names it otherwise, as a lockstep joiner
    /// by its path where its host has its ID, loads it so where that run doesn't. A removal is
    /// noted too, by the index and the path of what it removed, as `removed texture 2
    /// assets/logo.png`, `removed model 0 models/truck.glb` and `removed mesh 3`, so that a run
    /// restored from a snapshot removes it again, where its own assets line up.
    added: Vec<Arc<str>>,
    /// Why each texture that text named by a name no texture had couldn't be loaded, as the last
    /// try at loading such textures found it: its error's code and message, by that name, which a
    /// refusal of text naming it gives. Only the last try's, as what names them is refused just
    /// after it, so that names that never load don't pile up.
    failed: HashMap<Arc<str>, (&'static str, Arc<str>)>,
    /// The asset database, as [`Context::scan_assets`](crate::Context::scan_assets) last left it.
    #[cfg(feature = "dev")]
    catalog: Catalog,
}

/// A texture loaded from a file, as [`Assets`] holds it.
#[derive(Clone, Debug)]
struct Named {
    /// Which it is.
    texture: Texture,
    /// What text names it by: its ID, or else its path.
    name: Arc<str>,
    /// The path of its file, as the first layer to add it wrote it.
    path: Arc<str>,
    /// Whether it was removed, as [`Assets::remove_texture`] says.
    removed: bool,
}

/// A model, as [`Assets`] holds it: named as a texture is, and what its root spawns.
#[derive(Clone, Debug)]
struct NamedModel {
    name: Arc<str>,
    path: Arc<str>,
    nodes: Nodes,
    /// Whether it was removed, as [`Assets::remove_model`] says.
    removed: bool,
}

/// A prefab, as [`Assets`] holds it: named as a texture is, and its text, as its file held it as
/// last read, `None` for a stand-in, whose instances are their roots alone.
#[derive(Clone, Debug)]
struct NamedPrefab {
    name: Arc<str>,
    path: Arc<str>,
    text: Option<Arc<str>>,
}

/// How many textures, models, fonts and prefabs there are, the engine's white texture among them:
/// the indices the next added take, from which [`Assets::named_from`] goes on.
pub(crate) type Counts = (usize, usize, usize, usize);

/// An asset that text names, by its name or a path to its file: a texture, a model, a font or a
/// prefab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asset<S> {
    Texture(S),
    Model(S),
    Font(S),
    Prefab(S),
}

impl<S: AsRef<str>> Asset<S> {
    /// The name or path that names it.
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Texture(name) | Self::Model(name) | Self::Font(name) | Self::Prefab(name) => {
                name.as_ref()
            }
        }
    }

    /// It, named by a borrowed name.
    pub(crate) fn as_deref(&self) -> Asset<&str> {
        match self {
            Self::Texture(name) => Asset::Texture(name.as_ref()),
            Self::Model(name) => Asset::Model(name.as_ref()),
            Self::Font(name) => Asset::Font(name.as_ref()),
            Self::Prefab(name) => Asset::Prefab(name.as_ref()),
        }
    }

    /// It, named by a name of its own.
    pub(crate) fn owned(&self) -> Asset<String> {
        match self {
            Self::Texture(name) => Asset::Texture(name.as_ref().to_owned()),
            Self::Model(name) => Asset::Model(name.as_ref().to_owned()),
            Self::Font(name) => Asset::Font(name.as_ref().to_owned()),
            Self::Prefab(name) => Asset::Prefab(name.as_ref().to_owned()),
        }
    }

    /// What it is, in words, as refusals of what names it say.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Texture(_) => "texture",
            Self::Model(_) => "model",
            Self::Font(_) => "font",
            Self::Prefab(_) => "prefab",
        }
    }
}

impl Default for Assets {
    /// The engine's white texture alone, which every renderer's textures start with.
    fn default() -> Self {
        Self {
            textures: vec![None],
            by_name: HashMap::new(),
            models: Vec::new(),
            model_names: HashMap::new(),
            fonts: Vec::new(),
            font_names: HashMap::new(),
            prefabs: Vec::new(),
            prefab_names: HashMap::new(),
            added: Vec::new(),
            failed: HashMap::new(),
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

    /// The name of `texture`, which text names it by, if it has one.
    pub(crate) fn name(&self, texture: Texture) -> Option<&str> {
        let named = self.textures.get(texture.index() as usize)?.as_ref()?;
        Some(&named.name)
    }

    /// The path of `texture`'s file, if it has one: for people, whom a path tells more than an ID.
    pub(crate) fn path(&self, texture: Texture) -> Option<&str> {
        let named = self.textures.get(texture.index() as usize)?.as_ref()?;
        Some(&named.path)
    }

    /// The texture that `name` names, by its name or a path to its file, if one does: one removed
    /// too, which its names find until another loads from its file, as the simulation finds it.
    pub(crate) fn texture(&self, name: &str) -> Option<Texture> {
        self.by_name.get(name).copied()
    }

    /// The texture that `name` names, as [`texture`](Self::texture) finds it, unless it was
    /// removed: what a load finds loaded, rather than loading it anew.
    pub(crate) fn live_texture(&self, name: &str) -> Option<Texture> {
        self.texture(name).filter(|&texture| !self.removed(texture))
    }

    /// The texture of index `index`, if its file's path is `path` and it isn't removed: as a run
    /// restored from a snapshot finds what the snapshot's run removed.
    pub(crate) fn live_texture_at(&self, index: u32, path: &str) -> Option<Texture> {
        let named = self.textures.get(index as usize)?.as_ref()?;
        (&*named.path == path && !named.removed).then_some(named.texture)
    }

    /// Whether `texture` was removed.
    pub(crate) fn removed(&self, texture: Texture) -> bool {
        let named = self.textures.get(texture.index() as usize);
        named.is_some_and(|named| named.as_ref().is_some_and(|named| named.removed))
    }

    /// Has `texture` count as removed from now on, keeping its name and path, as the struct says.
    pub(crate) fn remove_texture(&mut self, texture: Texture) {
        let named = self.textures.get_mut(texture.index() as usize);
        let named = named.and_then(Option::as_mut);
        named.expect("a texture of a file is named").removed = true;
    }

    /// How many textures, models, fonts and prefabs there are, as [`Counts`] says.
    pub(crate) fn counts(&self) -> Counts {
        let (textures, models) = (self.textures.len(), self.models.len());
        (textures, models, self.fonts.len(), self.prefabs.len())
    }

    /// The name and path of each texture, then each model, then each font, then each prefab from
    /// `from` on, as [`counts`] gave it, that's named otherwise than by its path, as by its ID, in
    /// the order they were added: what a run that names its assets by IDs tells those that follow
    /// it, as a recording does its replay and a lockstep host its joiner.
    ///
    /// [`counts`]: Self::counts
    pub(crate) fn named_from(&self, from: Counts) -> impl Iterator<Item = (&str, &str)> {
        let textures = self.textures.iter().skip(from.0).flatten();
        let textures = textures.map(|named| (&named.name, &named.path));
        let models = self.models.iter().skip(from.1);
        let models = models.map(|model| (&model.name, &model.path));
        let fonts = self
            .fonts
            .iter()
            .skip(from.2)
            .map(|(name, path)| (name, path));
        let prefabs = self.prefabs.iter().skip(from.3);
        let prefabs = prefabs.map(|prefab| (&prefab.name, &prefab.path));
        let named = textures.chain(models).chain(fonts).chain(prefabs);
        let named = named.filter(|(name, path)| name != path);
        named.map(|(name, path)| (&**name, &**path))
    }

    /// The name of `prefab`, which text names it by.
    pub(crate) fn prefab_name(&self, prefab: Prefab) -> &str {
        &self.prefabs[prefab.index()].name
    }

    /// The path of `prefab`'s file: for people, whom a path tells more than an ID.
    pub(crate) fn prefab_path(&self, prefab: Prefab) -> &str {
        &self.prefabs[prefab.index()].path
    }

    /// Whether these assets have `prefab`, as they haven't one another context added.
    pub(crate) fn has_prefab(&self, prefab: Prefab) -> bool {
        prefab.index() < self.prefabs.len()
    }

    /// The prefab that `name` names, by its name or a path to its file, if one does.
    pub(crate) fn prefab(&self, name: &str) -> Option<Prefab> {
        self.prefab_names.get(name).copied()
    }

    /// The text of `prefab`, as its file held it as last read; `None` for a stand-in, or a prefab
    /// these assets don't have, as one another context added.
    pub(crate) fn prefab_text(&self, prefab: Prefab) -> Option<&Arc<str>> {
        self.prefabs.get(prefab.index())?.text.as_ref()
    }

    /// Adds the next prefab, loaded from the file `path` names, named `name`, of `text`, or a
    /// stand-in without, and returns it: each named by them from now on, unless another names it
    /// already.
    pub(crate) fn add_prefab(
        &mut self,
        path: Arc<str>,
        name: Arc<str>,
        text: Option<Arc<str>>,
    ) -> Prefab {
        let prefab = Prefab::new(self.prefabs.len());
        self.prefabs.push(NamedPrefab {
            name: name.clone(),
            path: path.clone(),
            text,
        });
        self.alias_prefab(prefab, name);
        self.alias_prefab(prefab, path);
        prefab
    }

    /// Has `path` name `prefab` too, added before from another path to its file, unless it names
    /// one already.
    pub(crate) fn alias_prefab(&mut self, prefab: Prefab, path: Arc<str>) {
        self.prefab_names.entry(path).or_insert(prefab);
    }

    /// Adds the next prefab, as [`add_prefab`](Self::add_prefab) does, of `text`, which the run has
    /// just written in a new file at `path`: which `path` names from now on, in place of a prefab
    /// loaded from a file there before, as one deleted since.
    #[cfg(feature = "dev")]
    pub(crate) fn add_saved_prefab(
        &mut self,
        path: Arc<str>,
        name: Arc<str>,
        text: Arc<str>,
    ) -> Prefab {
        self.prefab_names.remove(&path);
        self.add_prefab(path, name, Some(text))
    }

    /// Has `prefab` hold `text` from now on, as its file read again gives it.
    #[cfg(feature = "dev")]
    pub(crate) fn set_prefab_text(&mut self, prefab: Prefab, text: Arc<str>) {
        self.prefabs[prefab.index()].text = Some(text);
    }

    /// The name of `font`, which text names it by.
    pub(crate) fn font_name(&self, font: Font) -> &str {
        &self.fonts[font.index()].0
    }

    /// The path of `font`'s file: for people, whom a path tells more than an ID, as the Inspector
    /// shows it in development builds.
    #[cfg(feature = "dev")]
    pub(crate) fn font_path(&self, font: Font) -> &str {
        &self.fonts[font.index()].1
    }

    /// The font that `name` names, by its name or a path to its file, if one does.
    pub(crate) fn font(&self, name: &str) -> Option<Font> {
        self.font_names.get(name).copied()
    }

    /// Adds `font`, the next font added, loaded from the file `path` names, named `name`.
    pub(crate) fn add_font(&mut self, font: Font, path: Arc<str>, name: Arc<str>) {
        assert_eq!(
            font.index(),
            self.fonts.len(),
            "fonts are named in the order they're added"
        );
        self.font_names.entry(name.clone()).or_insert(font);
        self.font_names.entry(path.clone()).or_insert(font);
        self.fonts.push((name, path));
    }

    /// Has `path` name `font` too, added before from another path to its file, unless it names one
    /// already.
    pub(crate) fn alias_font(&mut self, font: Font, path: Arc<str>) {
        self.font_names.entry(path).or_insert(font);
    }

    /// The name of `model`, which text names it by.
    pub(crate) fn model_name(&self, model: Model) -> &str {
        &self.models[model.index()].name
    }

    /// The path of `model`'s file: for people, whom a path tells more than an ID.
    pub(crate) fn model_path(&self, model: Model) -> &str {
        &self.models[model.index()].path
    }

    /// The model that `name` names, by its name or a path to its file, if one does: one removed
    /// too, as [`texture`](Self::texture) finds a texture.
    pub(crate) fn model(&self, name: &str) -> Option<Model> {
        self.model_names.get(name).copied()
    }

    /// The model that `name` names, unless it was removed, as
    /// [`live_texture`](Self::live_texture) finds a texture.
    pub(crate) fn live_model(&self, name: &str) -> Option<Model> {
        self.model(name).filter(|&model| !self.model_removed(model))
    }

    /// The model of index `index`, if its file's path is `path` and it isn't removed, as
    /// [`live_texture_at`](Self::live_texture_at) finds a texture.
    pub(crate) fn live_model_at(&self, index: usize, path: &str) -> Option<Model> {
        let model = self.models.get(index)?;
        (&*model.path == path && !model.removed).then(|| Model::new(index))
    }

    /// Whether `model` was removed.
    pub(crate) fn model_removed(&self, model: Model) -> bool {
        self.models
            .get(model.index())
            .is_some_and(|model| model.removed)
    }

    /// Has `model` count as removed from now on, keeping its name, its path and what its root
    /// spawns, as the struct says.
    pub(crate) fn remove_model(&mut self, model: Model) {
        self.models[model.index()].removed = true;
    }

    /// What `model`'s root spawns; `None` for a model these assets don't have, as one another
    /// context added.
    pub(crate) fn nodes(&self, model: Model) -> Option<&Nodes> {
        self.models.get(model.index()).map(|model| &model.nodes)
    }

    /// Adds the next model, loaded from the file `path` names, named `name`, whose root spawns
    /// `nodes`, and returns it: each named by them from now on, unless another names it already
    /// that isn't removed.
    pub(crate) fn add_model(&mut self, path: Arc<str>, name: Arc<str>, nodes: Nodes) -> Model {
        let model = Model::new(self.models.len());
        self.models.push(NamedModel {
            name: name.clone(),
            path: path.clone(),
            nodes,
            removed: false,
        });
        self.alias_model(model, name);
        self.alias_model(model, path);
        model
    }

    /// Has `path` name `model` too, added before from another path to its file, unless it names
    /// one already that isn't removed.
    pub(crate) fn alias_model(&mut self, model: Model, path: Arc<str>) {
        let models = &self.models;
        let entry = self.model_names.entry(path).or_insert(model);
        if models[entry.index()].removed {
            *entry = model;
        }
    }

    /// Has `model`'s root spawn `nodes` from now on, as its file read again gives them.
    #[cfg(feature = "dev")]
    pub(crate) fn set_nodes(&mut self, model: Model, nodes: Nodes) {
        self.models[model.index()].nodes = nodes;
    }

    /// Adds `texture`, the next texture added, loaded from the file `path` names, named `name`:
    /// each naming it from now on, unless it names another already that isn't removed.
    pub(crate) fn add_texture(&mut self, texture: Texture, path: Arc<str>, name: Arc<str>) {
        assert_eq!(
            texture.index() as usize,
            self.textures.len(),
            "textures are named in the order they're added"
        );
        self.textures.push(Some(Named {
            texture,
            name: name.clone(),
            path: path.clone(),
            removed: false,
        }));
        self.alias(texture, name);
        self.alias(texture, path);
    }

    /// Adds `texture`, the next texture added, which no text names, as a font's field.
    pub(crate) fn add_unnamed(&mut self, texture: Texture) {
        assert_eq!(
            texture.index() as usize,
            self.textures.len(),
            "textures are named in the order they're added"
        );
        self.textures.push(None);
    }

    /// Has `path` name `texture` too, added before from another path to its file, unless it names
    /// one already that isn't removed; `texture` keeps its first path.
    pub(crate) fn alias(&mut self, texture: Texture, path: Arc<str>) {
        let textures = &self.textures;
        let entry = self.by_name.entry(path).or_insert(texture);
        let named = textures[entry.index() as usize].as_ref();
        if named.is_some_and(|named| named.removed) {
            *entry = texture;
        }
    }

    /// What a refusal of text that names a texture, a model, a font or a prefab, as `kind` says,
    /// `name`,
    /// which none has, says of it: that none is loaded, and why it couldn't be, if it was tried.
    pub(crate) fn not_found(&self, kind: &str, name: &str) -> String {
        match self.failed.get(name) {
            Some((code, why)) => {
                format!(
                    "no {kind} has the ID or path `{name}`, and it can't be loaded ({code}): {why}"
                )
            }
            None => format!("no {kind} loaded has the ID or path `{name}`"),
        }
    }

    /// Keeps why the last try at loading textures couldn't load each that text names as `failed`
    /// does, in place of what the tries before kept, for refusals of text naming one to say, as
    /// [`not_found`](Self::not_found) does.
    pub(crate) fn failed(&mut self, failed: &[(&str, gg_core::Error)]) {
        let why = |error: &gg_core::Error| (error.code(), format!("{error:#}").into());
        let failed = failed
            .iter()
            .map(|(name, error)| ((*name).into(), why(error)));
        self.failed = failed.collect();
    }

    /// Whether the last try at loading textures couldn't load the one text names `name`.
    pub(crate) fn has_failed(&self, name: &str) -> bool {
        self.failed.contains_key(name)
    }

    /// Whether `asset` names one loaded, and not removed, as a load takes it.
    pub(crate) fn has<S: AsRef<str>>(&self, asset: &Asset<S>) -> bool {
        match asset {
            Asset::Texture(name) => self.live_texture(name.as_ref()).is_some(),
            Asset::Model(name) => self.live_model(name.as_ref()).is_some(),
            Asset::Font(name) => self.font(name.as_ref()).is_some(),
            Asset::Prefab(name) => self.prefab(name.as_ref()).is_some(),
        }
    }

    /// Names each texture, model, font and prefab as `names` gives its path a name, and every other
    /// by its
    /// path, in place of what named it before: as a run takes the names its recording's run, or
    /// its lockstep host, gave its assets.
    #[cfg(feature = "dev")]
    pub(crate) fn rename(&mut self, names: &HashMap<Arc<str>, Arc<str>>) {
        let Self {
            textures,
            by_name,
            models,
            model_names,
            fonts,
            font_names,
            prefabs,
            prefab_names,
            ..
        } = self;
        for (index, prefab) in prefabs.iter_mut().enumerate() {
            let at = Prefab::new(index);
            if prefab.name != prefab.path && prefab_names.get(&prefab.name) == Some(&at) {
                prefab_names.remove(&prefab.name);
            }
            prefab.name = names.get(&prefab.path).unwrap_or(&prefab.path).clone();
        }
        for (index, prefab) in prefabs.iter().enumerate() {
            let at = Prefab::new(index);
            prefab_names.entry(prefab.name.clone()).or_insert(at);
        }
        for (index, (name, path)) in fonts.iter_mut().enumerate() {
            let at = Font::new(index);
            if name != path && font_names.get(name) == Some(&at) {
                font_names.remove(name);
            }
            *name = names.get(path).unwrap_or(path).clone();
        }
        for (index, (name, _)) in fonts.iter().enumerate() {
            font_names.entry(name.clone()).or_insert(Font::new(index));
        }
        for (index, model) in models.iter_mut().enumerate() {
            let at = Model::new(index);
            if model.name != model.path && model_names.get(&model.name) == Some(&at) {
                model_names.remove(&model.name);
            }
            model.name = names.get(&model.path).unwrap_or(&model.path).clone();
        }
        // Those not removed first, as a name finds one loaded since in place of one removed.
        for removed in [false, true] {
            let kept = models.iter().enumerate();
            for (index, model) in kept.filter(|(_, model)| model.removed == removed) {
                model_names
                    .entry(model.name.clone())
                    .or_insert(Model::new(index));
            }
        }
        // Every old name goes before any new one comes, as one texture may take another's.
        for named in textures.iter().flatten() {
            if named.name != named.path && by_name.get(&named.name) == Some(&named.texture) {
                by_name.remove(&named.name);
            }
        }
        for named in textures.iter_mut().flatten() {
            named.name = names.get(&named.path).unwrap_or(&named.path).clone();
        }
        for removed in [false, true] {
            for named in (textures.iter().flatten()).filter(|named| named.removed == removed) {
                by_name.entry(named.name.clone()).or_insert(named.texture);
            }
        }
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

    /// Takes an asset the run has just written into the database, as [`Catalog::bind`] does.
    #[cfg(feature = "dev")]
    pub(crate) fn bind(&mut self, path: Arc<str>, kind: AssetKind, id: AssetId) {
        self.catalog.bind(path, kind, id);
    }
}

/// What the layers loaded from files, or was loaded where text named it, each by its file, as
/// [`file_of`] resolves it, so that a file added again, by whatever form of its path, isn't loaded
/// again; but for what's removed, whose file loads anew.
#[derive(Default)]
pub(crate) struct Loaded {
    /// Each texture's or sprite sheet's, which share it: its texture, and its size in pixels, which
    /// a sheet is cut by.
    pub(crate) textures: HashMap<PathBuf, (Texture, [u32; 2])>,
    pub(crate) models: HashMap<PathBuf, Model>,
    pub(crate) fonts: HashMap<PathBuf, Font>,
    pub(crate) prefabs: HashMap<PathBuf, Prefab>,
    pub(crate) sounds: HashMap<PathBuf, Sound>,
    pub(crate) music: HashMap<PathBuf, Music>,
    /// What each model is made of on the renderer, by its index, which a reload replaces and a
    /// removal removes: a stand-in's of nothing.
    pub(crate) made: Vec<Made>,
    /// Each path a shader's file was added by, which the renderer's shaders know it by, with the
    /// file it resolves to.
    pub(crate) shaders: HashMap<PathBuf, PathBuf>,
    /// The textures cut into sprite sheets, whose cells a reload at another size would move.
    #[cfg(feature = "dev")]
    pub(crate) sheets: Vec<Texture>,
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

/// Whether text names a texture `name` by an asset ID, 16 lowercase hex digits, rather than by a
/// path, as ARCHITECTURE.md's Project state says.
pub(crate) fn is_id(name: &str) -> bool {
    name.len() == 16 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The textures, models, fonts and prefabs under the app's root, as the scans found them, and their
/// IDs,
/// from their ID files, which gg-api reads: the asset database. An asset the run has found it
/// keeps, with its ID, even once its file is gone, so that its ID stays its for the run, until it
/// turns up at another path, renamed or moved, which takes it, as [`scanned`] says.
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

/// A problem with an asset's ID file: its code, as [`scanned`] lists them, and its warning's
/// message.
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

    /// Whether the last scan found an asset at `path` from the root, with `/` between its parts.
    pub(crate) fn present(&self, path: &str) -> bool {
        self.assets.get(path).is_some_and(|asset| asset.present)
    }

    /// The ID of the asset at `path` from the root, with `/` between its parts, if it has one: the
    /// ID it keeps for the run.
    pub(crate) fn id(&self, path: &str) -> Option<AssetId> {
        self.assets.get(path)?.id
    }

    /// The path from the root, with `/` between its parts, of the asset whose ID is `id` this run,
    /// if one's is.
    pub(crate) fn path(&self, id: AssetId) -> Option<&str> {
        let path = self.ids.get(&id)?;
        (self.id(path) == Some(id)).then_some(&**path)
    }

    /// The ID files whose assets the last scan didn't find, in the order of their paths.
    pub(crate) fn orphans(&self) -> &[(Arc<str>, AssetKind, Option<AssetId>)] {
        &self.orphans
    }

    /// Why the root couldn't be listed at the last scan, if it couldn't.
    pub(crate) fn unlisted(&self) -> Option<&str> {
        self.unlisted.as_deref()
    }

    /// Takes in the asset of `kind` the run has just written at `path` from the root, with `/`
    /// between its parts, and its ID file, holding `id`, which no asset has this run: as the next
    /// scan would, so that what names it by its ID finds it at once.
    pub(crate) fn bind(&mut self, path: Arc<str>, kind: AssetKind, id: AssetId) {
        let cataloged = Cataloged {
            kind,
            id: Some(id),
            present: true,
            problem: None,
        };
        self.assets.insert(Arc::clone(&path), cataloged);
        self.ids.insert(id, path);
    }
}

/// Scans the assets under `root`, as gg-api's `scan` does, and gives what `catalog` becomes, as
/// [`scanned`] says.
#[cfg(feature = "dev")]
pub(crate) fn scan(
    catalog: &Catalog,
    root: &Path,
    prints: &mut Prints,
    writes: bool,
) -> Option<Catalog> {
    scanned(catalog, &asset_files::scan(root), prints, writes)
}

/// What `catalog` becomes of `found`, what a scan found under the asset root, or `None` if nothing
/// changed, warning of each problem once a run and writing the ID files missing if `writes`.
///
/// The problems: `asset.no_id`, none where the run writes none; `asset.bad_meta`, one that doesn't
/// read, its asset going without an ID; `asset.meta_case`, one named so in case alone;
/// `asset.duplicate_id`; `asset.orphaned`, one without its asset; `asset.id_changed`;
/// `asset.unwritten`; and `asset.unlisted`, what the scan couldn't take. The run rewrites no ID
/// file there, as one that doesn't read may be a merge's conflict or someone's edit.
///
/// An asset found gets the ID its ID file holds, unless another asset has it this run, and keeps
/// it for the run, whatever its ID file holds later: at a run's first scan, the asset first by
/// path keeps an ID two hold. An asset whose ID file holds the ID of one the run had that's gone
/// takes it, as after a rename that took its ID file along, logging `asset.renamed`. Where the run
/// `writes`, an asset new to the run without an ID file takes one that a rename left behind, moved
/// beside it, if its file holds what that ID's asset last held, as [`renamed`] says, `prints`
/// telling; and any other without one gets one written whole beside it, holding the ID it has
/// this run, or else one drawn, unless another ID file holds that ID now.
#[cfg(feature = "dev")]
pub(crate) fn scanned<'f>(
    catalog: &Catalog,
    found: &'f Scan,
    prints: &mut Prints,
    writes: bool,
) -> Option<Catalog> {
    let mut next = catalog.clone();
    for cataloged in next.assets.values_mut() {
        cataloged.present = false;
        cataloged.problem = None;
    }
    let root_unread = found.unread.iter().find(|unread| unread.path == ".");
    next.unlisted = root_unread.map(|unread| unread.error.as_str().into());
    // Every asset found is present before any takes an ID, so that a duplicate's holder, and a
    // renamed asset's former self, are told present or gone whatever their paths' order.
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
    let renamed = if writes {
        renamed(&next, found, prints)
    } else {
        HashMap::new()
    };
    // Each asset's ID file, as found, or as moved beside it.
    let moved: HashMap<&str, Result<Meta, Failure>> = (renamed.iter())
        .map(|(&path, &(_, id))| (path, Ok(Meta { id })))
        .collect();
    let meta_of = |scanned: &'f Scanned| moved.get(scanned.path.as_str()).or(scanned.meta.as_ref());
    // Each ID an ID file holds, by the path of the first in order of those that hold it.
    let ids = (found.assets.iter()).filter_map(|scanned| match meta_of(scanned) {
        Some(Ok(meta)) => Some((format!("{}.{}", scanned.path, Meta::EXTENSION), meta.id)),
        _ => None,
    });
    let left = |orphan: &&Orphan| !renamed.values().any(|&(from, _)| from == orphan.path);
    let orphaned = (found.orphans.iter().filter(left))
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
        if let Some(Ok(meta)) = meta_of(scanned) {
            first.entry(meta.id).or_insert(scanned.path.as_str());
        }
    }
    for scanned in &found.assets {
        let paired = renamed.contains_key(scanned.path.as_str());
        let meta = meta_of(scanned);
        let (id, problem) = identify(&mut next, &held, &first, scanned, meta, writes, paired);
        let cataloged = (next.assets.get_mut(scanned.path.as_str())).expect("found");
        (cataloged.id, cataloged.problem) = (id, problem);
    }
    let mut orphans = Vec::with_capacity(found.orphans.len());
    for orphan in found.orphans.iter().filter(left) {
        let path: Arc<str> = orphan.path.as_str().into();
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
        let id = orphan.meta.as_ref().ok().map(|meta| meta.id);
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

/// What asset `scanned` gets of its ID file, `meta`, as the scan found it beside it or a rename's
/// was moved there, `paired`, given the ID it had this run, if any; `held`, each ID an ID file
/// holds, by the path of the first that holds it; and `first`, each ID an asset's ID file holds, by
/// the first such asset's path: its ID, and what's wrong, if anything, warned of once a run;
/// writing its ID file if it's missing and the run `writes`, and binding its ID in `next` if it's
/// new to it, as [`scanned`] says.
#[cfg(feature = "dev")]
fn identify(
    next: &mut Catalog,
    held: &HashMap<AssetId, String>,
    first: &HashMap<AssetId, &str>,
    scanned: &Scanned,
    meta: Option<&Result<Meta, Failure>>,
    writes: bool,
    paired: bool,
) -> (Option<AssetId>, Option<Problem>) {
    let path = scanned.path.as_str();
    let had = next.assets[path].id;
    let written;
    let meta = match meta {
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
                // A run's first scan gives an ID to the first by path whose ID file holds it.
                let takes = first.get(&id) == Some(&path);
                if takes && !present {
                    // Renamed or moved, its ID file with it or moved after it: its ID goes with it,
                    // and the asset it was is no more.
                    next.ids.insert(id, path.into());
                    next.assets.remove(&holder);
                    if !paired {
                        info!(
                            code = "asset.renamed",
                            path,
                            from = &*holder,
                            id = %id,
                            "{holder} is now {path}, its ID file with it: it keeps the ID {id}"
                        );
                    }
                    return (Some(id), None);
                }
                let code = "asset.duplicate_id";
                let message = if takes {
                    format!(
                        "{path}.meta holds {id}, {holder}'s ID this run: from the next run, {path} \
                         takes it, as the first by path whose ID file holds it, and {holder} goes \
                         without, unless {path}.meta is removed before then, for a run that writes \
                         ID files to write {path} a fresh one"
                    )
                } else {
                    format!(
                        "{path}.meta holds {id}, {holder}'s ID: {path} has no ID until its ID file \
                         is removed, after which a run that writes ID files writes it a fresh one"
                    )
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

/// The assets `found` new to the run without ID files, as after a rename that left theirs behind,
/// whose files hold what the asset of an ID file left behind last held, as `prints` last found it,
/// and no other's: each by its path, with the path of that ID file, which is moved beside it, and
/// the ID it holds, logging `asset.renamed`. An ID file is left behind where the scan finds it
/// without its asset, holding an ID that no asset found holds in its ID file, nor has this run. Two
/// new assets alike, or two ID files left behind whose assets were alike, pair with none, as a copy
/// may be either. The move links the ID file beside the asset, which fails rather than write over
/// one there, then removes it, so that a move that fails leaves it where it was.
///
/// Reads each new asset's file of a length that such an asset had, as [`Prints`] says.
#[cfg(feature = "dev")]
fn renamed<'f>(
    next: &Catalog,
    found: &'f Scan,
    prints: &mut Prints,
) -> HashMap<&'f str, (&'f str, AssetId)> {
    let held: HashSet<AssetId> = (found.assets.iter())
        .filter_map(|scanned| Some(scanned.meta.as_ref()?.as_ref().ok()?.id))
        .collect();
    let taken = |id: &AssetId| {
        let holder = next.ids.get(id).and_then(|holder| next.assets.get(holder));
        held.contains(id) || holder.is_some_and(|holder| holder.present)
    };
    let mut left: HashMap<Print, Vec<(&Orphan, AssetId)>> = HashMap::new();
    for orphan in &found.orphans {
        if let Ok(meta) = &orphan.meta
            && !taken(&meta.id)
            && let Some(&print) = prints.by_id.get(&meta.id)
        {
            left.entry(print).or_default().push((orphan, meta.id));
        }
    }
    if left.is_empty() {
        return HashMap::new();
    }
    let lengths: HashSet<u64> = left.keys().map(|print| print.length).collect();
    let mut new: HashMap<Print, Vec<&Scanned>> = HashMap::new();
    for scanned in &found.assets {
        let fresh = scanned.meta.is_none() && next.id(&scanned.path).is_none();
        let alike = (scanned.stamp).is_none_or(|(_, length)| lengths.contains(&length));
        if fresh
            && alike
            && let Some(print) = prints.of(scanned)
        {
            new.entry(print).or_default().push(scanned);
        }
    }
    let mut renamed = HashMap::new();
    for (print, assets) in &new {
        let ([asset], Some([(orphan, id)])) = (&assets[..], left.get(print).map(Vec::as_slice))
        else {
            continue;
        };
        if fs::hard_link(&orphan.file, Meta::path(&asset.file)).is_err() {
            continue;
        }
        // Should it stay, its ID is in two ID files, and the scans from then on warn of it as left
        // without its asset, saying to delete it.
        let _ = fs::remove_file(&orphan.file);
        let (path, from) = (asset.path.as_str(), orphan.path.as_str());
        let was = from.rsplit_once('.').map_or(from, |(asset, _)| asset);
        info!(
            code = "asset.renamed",
            path,
            from = was,
            id = %id,
            "{was} is now {path}, its file as it was: its ID file is moved beside it, so that it \
             keeps the ID {id}"
        );
        renamed.insert(path, (from, *id));
    }
    renamed
}

/// What a file holds, as far as telling an asset's file renamed from another goes: its length, and
/// a hash of its bytes, each 8 of them mixed in turn by SplitMix64's mix, which files of one length
/// share by chance about once in 2^64 pairs.
#[cfg(feature = "dev")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Print {
    length: u64,
    hash: u64,
}

#[cfg(feature = "dev")]
impl Print {
    /// How much of a file is read at a time: a multiple of 8, so that only the last part read ends
    /// in a part of a word.
    const CHUNK: usize = 1 << 16;

    /// The print of the file at `path`.
    ///
    /// Fails as reading it does.
    fn of(path: &Path) -> io::Result<Self> {
        let _zone = gg_core::zone!();
        let mut file = File::open(path)?;
        let mut chunk = Vec::with_capacity(Self::CHUNK);
        let (mut length, mut hash) = (0, 0);
        loop {
            chunk.clear();
            (&mut file)
                .take(Self::CHUNK as u64)
                .read_to_end(&mut chunk)?;
            length += chunk.len() as u64;
            hash = mixed(hash, &chunk);
            if chunk.len() < Self::CHUNK {
                return Ok(Self { length, hash });
            }
        }
    }
}

/// `hash` with `bytes` mixed into it, as little-endian words of 8, the last padded with zeros.
#[cfg(feature = "dev")]
fn mixed(hash: u64, bytes: &[u8]) -> u64 {
    bytes.chunks(8).fold(hash, |hash, word| {
        let mut padded = [0; 8];
        padded[..word.len()].copy_from_slice(word);
        mix64(hash ^ u64::from_le_bytes(padded))
    })
}

/// The assets' [`Print`]s, which pair a renamed asset with the ID file its rename left behind, as
/// [`scanned`] says: each file's, by its path from the root, with its stamp as it was read, so that
/// it's read again only once it changes; and each ID's, of what its asset last held, of the assets
/// found and those whose ID files are left behind. A run that writes ID files keeps them in its
/// clone's [`cache`] for the next, so that a rename made while no run watched pairs as the
/// next run starts.
#[cfg(feature = "dev")]
#[derive(Debug, Default)]
pub(crate) struct Prints {
    by_path: HashMap<Arc<str>, (Option<Stamp>, Print)>,
    by_id: HashMap<AssetId, Print>,
    /// Whether the cache couldn't be written this run, which warns once.
    unsaved: bool,
}

/// What a clone's cache holds: the prints of the assets under the asset root, which resolves to
/// `root`, the file's name a hash of it: each file's, as its path from the root, when it last
/// changed in nanoseconds since 1970, its length and its hash; and each ID's, as the ID, the length
/// and the hash.
#[cfg(feature = "dev")]
#[derive(Serialize, Deserialize)]
struct Cached {
    root: String,
    files: Vec<(String, u64, u64, u64)>,
    ids: Vec<(String, u64, u64)>,
}

/// The most a cache's file may hold, which is read whole.
#[cfg(feature = "dev")]
const MOST_CACHED: u64 = 1 << 26;

#[cfg(feature = "dev")]
impl Prints {
    /// What the cache at `cache` holds of the asset root `root`; none if it's missing, can't be
    /// read, or is of another root, as a cache may be.
    pub(crate) fn read(cache: &Path, root: &Path) -> Self {
        let _zone = gg_core::zone!();
        let read = dirs::read_file(cache, MOST_CACHED).ok();
        let cached = read.and_then(|bytes| serde_json::from_slice::<Cached>(&bytes).ok());
        let root = file_of(root).map(|root| root.to_string_lossy().into_owned());
        let Some(cached) = cached.filter(|cached| Some(&cached.root) == root.as_ref()) else {
            return Self::default();
        };
        let files = cached.files.into_iter().map(|(path, nanos, length, hash)| {
            let changed = SystemTime::UNIX_EPOCH + Duration::from_nanos(nanos);
            (
                path.into(),
                (Some((changed, length)), Print { length, hash }),
            )
        });
        let ids = (cached.ids.into_iter())
            .filter_map(|(id, length, hash)| Some((id.parse().ok()?, Print { length, hash })));
        Self {
            by_path: files.collect(),
            by_id: ids.collect(),
            unsaved: false,
        }
    }

    /// Writes the prints to the cache at `cache`, of the asset root `root`, whole, as
    /// [`dirs::write`] does, warning `asset.no_cache` once a run if it can't.
    pub(crate) fn save(&mut self, cache: &Path, root: &Path) {
        let _zone = gg_core::zone!();
        let nanos = |stamp: &Option<Stamp>| {
            let (changed, _) = (*stamp)?;
            let since = changed.duration_since(SystemTime::UNIX_EPOCH).ok()?;
            u64::try_from(since.as_nanos()).ok()
        };
        let files = self.by_path.iter().filter_map(|(path, (stamp, print))| {
            Some((path.to_string(), nanos(stamp)?, print.length, print.hash))
        });
        let ids = (self.by_id.iter()).map(|(id, print)| (id.to_string(), print.length, print.hash));
        let cached = Cached {
            root: file_of(root).map_or_else(String::new, |root| root.to_string_lossy().into()),
            files: files.collect(),
            ids: ids.collect(),
        };
        let text = serde_json::to_vec(&cached).expect("prints serialize");
        if let Err(error) = dirs::write(cache, &text)
            && !mem::replace(&mut self.unsaved, true)
        {
            warn!(
                code = "asset.no_cache",
                path = %cache.display(),
                error = %error,
                "Can't keep the assets' prints in {}: {error}; an asset renamed while no run \
                 watches the asset root gets a fresh ID at the next",
                cache.display()
            );
        }
    }

    /// The print of `scanned`'s file, read unless it was since the file last changed; `None` if it
    /// can't be read.
    fn of(&mut self, scanned: &Scanned) -> Option<Print> {
        if let Some(&(stamp, print)) = self.by_path.get(scanned.path.as_str())
            && stamp.is_some()
            && stamp == scanned.stamp
        {
            return Some(print);
        }
        let print = Print::of(&scanned.file).ok()?;
        let path = scanned.path.as_str().into();
        self.by_path.insert(path, (scanned.stamp, print));
        Some(print)
    }

    /// Takes the prints of the assets `found` that have IDs in `catalog`, each its ID's too, reading
    /// those whose files changed since they were last read; keeps those of the IDs that ID files
    /// `found` left behind hold; and forgets the rest. Returns whether any changed.
    pub(crate) fn update(&mut self, catalog: &Catalog, found: &Scan) -> bool {
        let was = mem::take(&mut self.by_path);
        let mut changed = false;
        let mut ids = HashSet::new();
        for scanned in &found.assets {
            let Some(id) = catalog.id(&scanned.path) else {
                continue;
            };
            ids.insert(id);
            let path: Arc<str> = scanned.path.as_str().into();
            let print = match was.get(&path) {
                Some(&(stamp, print)) if stamp.is_some() && stamp == scanned.stamp => print,
                // Being written, or held by its writer: a later look reads it.
                _ => match Print::of(&scanned.file) {
                    Ok(print) => {
                        changed = true;
                        print
                    }
                    Err(_) => continue,
                },
            };
            self.by_path.insert(path, (scanned.stamp, print));
            changed |= self.by_id.insert(id, print) != Some(print);
        }
        let left = found.orphans.iter();
        ids.extend(left.filter_map(|orphan| Some(orphan.meta.as_ref().ok()?.id)));
        let count = self.by_id.len();
        self.by_id.retain(|id, _| ids.contains(id));
        changed || self.by_id.len() != count || self.by_path.len() != was.len()
    }
}

/// Where the [`Prints`] of the assets under `root` are kept between runs of app `app`: a file of the
/// root's own, by a hash of the path it resolves to, in the app's directory for caches, as
/// [`dirs::dir`] says, so that each clone of a game keeps its own, whichever build runs it. `None`
/// without the directory, or for a root that doesn't resolve.
#[cfg(feature = "dev")]
pub(crate) fn cache(app: &str, root: &Path) -> Option<PathBuf> {
    let root = file_of(root)?;
    let dir = dirs::dir(app, Kind::Cache, std::env::consts::OS, |name| {
        std::env::var_os(name)
    })?;
    let key = mixed(0, root.as_os_str().as_encoded_bytes());
    Some(dir.join("assets").join(format!("{key:016x}.json")))
}

/// Whether the problem `code` with what's at `about` is new to `warned`, which then holds it.
#[cfg(feature = "dev")]
fn once(warned: &mut HashSet<(&'static str, Arc<str>)>, code: &'static str, about: &str) -> bool {
    warned.insert((code, about.into()))
}

#[cfg(test)]
mod tests;
