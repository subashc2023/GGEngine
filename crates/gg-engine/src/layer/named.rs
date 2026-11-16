//! Loading the assets text names, standing in for those that can't be, and naming them.

#[cfg(feature = "dev")]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "dev")]
use std::time::SystemTime;

#[cfg(feature = "dev")]
use gg_api::asset::AssetId;
use gg_core::{Error, Result};
use gg_renderer::{Mipmaps, Texels, Texture};
use tracing::{debug, warn};

use super::Context;
use super::adds::model_types;
#[cfg(doc)]
use crate::World;
use crate::assets::{self, Asset};
#[cfg(doc)]
use crate::assets::{Assets, Loaded};
use crate::font::Font;
use crate::model::{Made, Nodes};
use crate::recording::text::Text;
#[cfg(doc)]
use crate::snapshot::Snapshot;
use crate::snapshot::{self, Again};
use crate::{Model, Prefab};

impl Context {
    /// Loads, as [`add_texture`](Self::add_texture), [`add_model`](Self::add_model),
    /// [`add_font`](Self::add_font) and [`add_prefab`](Self::add_prefab) load a file, each texture,
    /// model, font and prefab that `names` names,
    /// as text names one, that none loaded has, or none but one removed, which loads anew, as
    /// [`remove_texture`](Self::remove_texture) says: where a scene, the tool API, or a recorded or
    /// a lockstep peer's command or edit names it, before it's read, as ARCHITECTURE.md's Project
    /// state says; a model's image with its model. A name finds its file as
    /// [`asset_path`](Self::asset_path) says, inside the asset root. In a run that follows another,
    /// the asset takes the name its text gave it, that run's, and its file is found by the names it
    /// was given first.
    ///
    /// One that can't be loaded stands in, if `stand_in`, as for a run that follows another, so
    /// that its texts read as that run's did: a texture of magenta and black squares, a model of no
    /// nodes, or a font that draws in the engine's, named as the name, warning `asset.stand_in`
    /// with why. Otherwise it stays unloaded, and the refusal of what names it says why, as
    /// [`Assets::not_found`] does.
    pub(crate) fn load_named<S: AsRef<str>>(
        &mut self,
        names: impl IntoIterator<Item = Asset<S>>,
        stand_in: bool,
    ) {
        let names: Vec<Asset<S>> = names.into_iter().collect();
        let mut tried: Vec<Asset<&str>> = Vec::new();
        let mut failed = Vec::new();
        for asset in &names {
            let named = asset.as_deref();
            if self.assets.has(&named) || tried.contains(&named) {
                continue;
            }
            tried.push(named);
            let (Asset::Texture(name)
            | Asset::Model(name)
            | Asset::Font(name)
            | Asset::Prefab(name)) = named;
            let (path, loaded) = match named {
                Asset::Texture(_) => self.load_texture_named(name, stand_in),
                Asset::Model(_) => self.load_model_named(name, stand_in),
                Asset::Font(_) => self.load_font_named(name, stand_in),
                Asset::Prefab(_) => self.load_prefab_named(name, stand_in),
            };
            match (loaded, named) {
                (Ok(()), _) => {}
                (Err(error), Asset::Texture(_)) if stand_in => {
                    self.stand_in(name, path.as_deref(), &error);
                }
                (Err(error), Asset::Model(_)) if stand_in => {
                    self.stand_in_model(name, path.as_deref(), &error);
                }
                (Err(error), Asset::Font(_)) if stand_in => {
                    self.stand_in_font(name, path.as_deref(), &error);
                }
                (Err(error), Asset::Prefab(_)) if stand_in => {
                    self.stand_in_prefab(name, path.as_deref(), &error);
                }
                (Err(error), _) => failed.push((name, error)),
            }
        }
        if !tried.is_empty() {
            self.add_assets(|assets| assets.failed(&failed));
        }
    }

    /// Loads the texture that text names `name`, as [`load_named`](Self::load_named) does: a
    /// model's image by loading its model, if it isn't. Gives the path from the asset root its name
    /// led to, if it led to one.
    ///
    /// Fails as [`asset_path`](Self::asset_path) and [`add_texture`](Self::add_texture) do; for a
    /// model's image, as [`load_model_named`](Self::load_model_named) does, and with
    /// `asset.model_image` for an image the model hasn't.
    fn load_texture_named(&mut self, name: &str, stand_in: bool) -> (Option<Arc<str>>, Result<()>) {
        if let Some((model, image)) = name.rsplit_once('#')
            && !image.is_empty()
            && image.bytes().all(|b| b.is_ascii_digit())
        {
            let loaded = match self.assets.live_model(model) {
                Some(_) => Ok(()),
                None => self.load_model_named(model, stand_in).1,
            };
            // By the model's own name, as another path to its file may name it.
            let found = loaded.and_then(|()| {
                let loaded = self.assets.live_model(model).expect("loaded");
                let named = format!("{}#{image}", self.assets.model_name(loaded));
                match self.assets.live_texture(&named) {
                    Some(texture) => {
                        self.alias(texture, &name.into());
                        Ok(())
                    }
                    None => {
                        let message = format!(
                            "`{name}` names image {image} of model `{model}`, which hasn't it"
                        );
                        Err(Error::new("asset.model_image", message))
                    }
                }
            });
            return (None, found);
        }
        match self.asset_path(name, "texture", stand_in) {
            Ok(path) => {
                let loaded = self.load_path(name, &path);
                (Some(path), loaded)
            }
            Err(error) => (None, Err(error)),
        }
    }

    /// Loads the model that text names `name`, as [`load_named`](Self::load_named) does, a file
    /// loaded already taking the names alone, as [`load_path`](Self::load_path) says. Gives the
    /// path from the asset root its name led to, if it led to one.
    ///
    /// Fails as [`asset_path`](Self::asset_path) and [`add_model`](Self::add_model) do, and with
    /// `asset.outside_root` for a file outside the root, as through `..` or a link.
    fn load_model_named(&mut self, name: &str, stand_in: bool) -> (Option<Arc<str>>, Result<()>) {
        let path = match self.asset_path(name, "model", stand_in) {
            Ok(path) => path,
            Err(error) => return (None, Err(error)),
        };
        let loaded = self.named_file(name, &path).and_then(|file| {
            let resolved = self.files.file_of(&file);
            let model = match self.loaded_model(&path, &file, resolved.as_deref()) {
                Some(model) => model,
                None => self.add_model(&path, &file)?,
            };
            for named in [&*path, name] {
                if self.assets.live_model(named).is_none() {
                    self.add_assets(|assets| assets.alias_model(model, named.into()));
                }
            }
            Ok(())
        });
        (Some(path), loaded)
    }

    /// Loads the font that text names `name`, as [`load_named`](Self::load_named) does, a file
    /// loaded already taking the names alone, as [`load_path`](Self::load_path) says. Gives the
    /// path from the asset root its name led to, if it led to one.
    ///
    /// Fails as [`asset_path`](Self::asset_path) and [`add_font`](Self::add_font) do, and with
    /// `asset.outside_root` for a file outside the root, as through `..` or a link.
    fn load_font_named(&mut self, name: &str, stand_in: bool) -> (Option<Arc<str>>, Result<()>) {
        let path = match self.asset_path(name, "font", stand_in) {
            Ok(path) => path,
            Err(error) => return (None, Err(error)),
        };
        let loaded = self.named_file(name, &path).and_then(|file| {
            let resolved = self.files.file_of(&file);
            let font = match self.loaded_font(&path, &file, resolved.as_deref()) {
                Some(font) => font,
                None => self.add_font(&path, &file)?,
            };
            for named in [&*path, name] {
                if self.assets.font(named).is_none() {
                    self.add_assets(|assets| assets.alias_font(font, named.into()));
                }
            }
            Ok(())
        });
        (Some(path), loaded)
    }

    /// Loads the prefab that text names `name`, as [`load_named`](Self::load_named) does, a file
    /// loaded already taking the names alone, as [`load_path`](Self::load_path) says. Gives the
    /// path from the asset root its name led to, if it led to one.
    ///
    /// Fails as [`asset_path`](Self::asset_path) and [`add_prefab`](Self::add_prefab) do, and with
    /// `asset.outside_root` for a file outside the root, as through `..` or a link.
    fn load_prefab_named(&mut self, name: &str, stand_in: bool) -> (Option<Arc<str>>, Result<()>) {
        let path = match self.asset_path(name, "prefab", stand_in) {
            Ok(path) => path,
            Err(error) => return (None, Err(error)),
        };
        let loaded = self.named_file(name, &path).and_then(|file| {
            let resolved = self.files.file_of(&file);
            let prefab = match self.loaded_prefab(&path, &file, resolved.as_deref()) {
                Some(prefab) => prefab,
                None => self.add_prefab(&path, &file)?,
            };
            for named in [&*path, name] {
                if self.assets.prefab(named).is_none() {
                    self.add_assets(|assets| assets.alias_prefab(prefab, named.into()));
                }
            }
            Ok(())
        });
        (Some(path), loaded)
    }

    /// Takes again what the run a snapshot was taken from noted past the notes this run's layers
    /// made alike as they loaded, of `lines`, the snapshot's `asset` lines, in order, as
    /// [`Snapshot::added`] gives them: as a run restored from it, or a lockstep joiner given its
    /// host's world, does before it reads the world, so that its values find every asset they
    /// name, and what's opaque in it comes back where the run's assets line up with the
    /// snapshot's. It loads, as [`load_named`](Self::load_named) does with `stand_in`, each
    /// texture, model, font and prefab a line gives the path of that none loaded has, in the order
    /// that run
    /// loaded them, so that a texture's index, which what's opaque holds, is what it was where the
    /// layers added the same before; a path outside the root, as only a layer gives one, stands
    /// in, as one text names does. And it removes again what that run removed, a texture, a model
    /// or a mesh by the index its line gives, while this run's notes are the snapshot's up to that
    /// line, so that the index is the asset's that run removed; a texture or a model only where the
    /// one at its index is loaded from the path the line gives, and isn't removed already.
    pub(crate) fn load_restored(&mut self, lines: &[String]) {
        let ours = self.assets.added();
        let common = (lines.iter().zip(ours))
            .take_while(|&(theirs, ours)| **theirs == **ours)
            .count();
        // Whether this run's notes are the snapshot's lines up to the one taken.
        let mut lined_up = ours.len() == common;
        for (at, line) in lines.iter().enumerate().skip(common) {
            match snapshot::again(line) {
                Some(Again::Load(asset)) => self.load_named([asset], true),
                // The index a removal gives may be another asset's in a run whose assets part
                // from the snapshot's before it, as one whose newer layers add another first.
                Some(_) if !lined_up => debug!(
                    line = &**line,
                    "Not removed again, as the run's assets part from the snapshot's before it"
                ),
                Some(Again::RemoveTexture(index, path)) => {
                    let texture = self.assets.live_texture_at(index, path);
                    let texture = texture.filter(|&texture| self.unremovable(texture).is_none());
                    if let Some(texture) = texture {
                        self.drop_texture(texture, true);
                    }
                }
                Some(Again::RemoveModel(index, path)) => {
                    if let Some(model) = self.assets.live_model_at(index, path) {
                        self.remove_model(model);
                    }
                }
                Some(Again::RemoveMesh(index)) => {
                    let known = index < self.meshes.count();
                    if known && self.unremovable_mesh(index).is_none() {
                        self.drop_mesh(index);
                    }
                }
                None => {}
            }
            let added = self.assets.added();
            lined_up &= added.len() == at + 1 && *added[at] == **line;
        }
    }

    /// Loads each texture, model, font and prefab scene `text` names that none loaded has, and
    /// those the prefabs it names name, in turn, as [`load_named`](Self::load_named) does, standing
    /// in for what can't be in a run that follows another; and returns the names of those still
    /// missing.
    ///
    /// Fails, loading nothing, as [`World::load_scene`] does but for those.
    pub(crate) fn load_scene_assets(&mut self, text: &str) -> Result<Vec<String>> {
        self.load_scene_assets_as(text, self.follows)
    }

    /// As [`load_scene_assets`](Self::load_scene_assets), standing in for what can't be loaded if
    /// `stand_in`, whatever run this is.
    pub(super) fn load_scene_assets_as(
        &mut self,
        text: &str,
        stand_in: bool,
    ) -> Result<Vec<String>> {
        let mut missing = Vec::new();
        // Again while a prefab loads, whose text may name more: each time one more at least.
        loop {
            let names = self.world.scene_assets(text)?;
            self.load_named(names.iter().map(Asset::as_deref), stand_in);
            let prefab = |asset: &Asset<String>| matches!(asset, Asset::Prefab(_));
            let more = names
                .iter()
                .any(|asset| prefab(asset) && self.assets.has(asset));
            for asset in names.into_iter().filter(|asset| !self.assets.has(asset)) {
                let name = asset.name().to_owned();
                if !missing.contains(&name) {
                    missing.push(name);
                }
            }
            if !more {
                return Ok(missing);
            }
        }
    }

    /// Loads each texture, model, font and prefab `text`, a value as a recording writes it, names
    /// that
    /// none loaded has, standing in for what can't be, as [`load_named`](Self::load_named) does: a
    /// value that a recording or a lockstep peer gives, which names assets as the run that wrote it
    /// did.
    pub(crate) fn load_followed(&mut self, text: &Text) {
        let mut names = Vec::new();
        text.assets(&mut names);
        if !names.is_empty() {
            self.load_named(names, true);
        }
    }

    /// The path from the asset root of the file whose texture, model or font, as `kind` says, text
    /// names `name`, as [`load_named`](Self::load_named) loads it: in a run that follows another,
    /// the path given that name, as [`name_as`](Self::name_as) gives them; in a run that reads its
    /// game's package, the asset's whose ID it is there; in a development build, the asset's whose
    /// ID it is, as the asset database holds it; and else `name` itself, as a path. A run that
    /// follows another and isn't to stand in for what it can't load, as a lockstep joiner asked by
    /// the tool API, finds only what it was given, so that it names the asset as its host would.
    ///
    /// Fails with `asset.not_given` for what such a run wasn't given; `asset.unknown_id` for an ID
    /// no asset has this run, naming the file the project's fetch list gives it, if it gives it
    /// one, as [`fetched`](Self::fetched) finds it, or any ID in a build without `dev` that reads
    /// no package, as it reads no ID file; and `asset.outside_root` for an absolute path.
    pub(super) fn asset_path(&self, name: &str, kind: &str, stand_in: bool) -> Result<Arc<str>> {
        #[cfg(feature = "dev")]
        if let Some(given) = &self.given {
            if let Some((path, _)) = given.iter().find(|(_, given)| ***given == *name) {
                return Ok(path.clone());
            }
            if self.follows && !stand_in && !given.contains_key(name) {
                let message = format!(
                    "`{name}` isn't a name or a path that the lockstep host has given a {kind}, \
                     and a joiner loads only those, so that both peers name it alike: name it \
                     through the host"
                );
                return Err(Error::new("asset.not_given", message));
            }
        }
        #[cfg(not(feature = "dev"))]
        let _ = stand_in;
        if assets::is_id(name) {
            if let Some(package) = self.files.packaged() {
                let id = u64::from_str_radix(name, 16).ok();
                if let Some(path) = id.and_then(|id| package.path_of(id)) {
                    return Ok(path.into());
                }
                let message = format!("no asset in the game's package has the ID {name}");
                return Err(Error::new("asset.unknown_id", message));
            }
            #[cfg(feature = "dev")]
            {
                let id = name.parse::<AssetId>().ok();
                if let Some(path) = id.and_then(|id| self.assets.catalog().path(id)) {
                    return Ok(path.into());
                }
                let mut message =
                    format!("no asset under the asset root has the ID {name} this run");
                if let Some(path) = id.and_then(|id| self.fetched(id)) {
                    message += &format!(
                        ", but the project's fetch list gives it to `{path}`: `cargo gg fetch`, \
                         in the project's directory, fetches it with its ID"
                    );
                }
                return Err(Error::new("asset.unknown_id", message));
            }
            #[cfg(not(feature = "dev"))]
            {
                let message = format!(
                    "{name} is an asset ID, but a build without `dev` reads no ID file, so it finds \
                     a {kind} by its path alone, but in its game's package"
                );
                return Err(Error::new("asset.unknown_id", message));
            }
        }
        let path = Path::new(name);
        let prefixed = matches!(
            path.components().next(),
            Some(std::path::Component::Prefix(_))
        );
        if path.has_root() || prefixed {
            let message = format!(
                "`{name}` is an absolute path, which would name a file on this machine alone: a \
                 {kind} is named by its ID or by its path from the asset root"
            );
            return Err(Error::new("asset.outside_root", message));
        }
        Ok(name.into())
    }

    /// The path, from the project's directory, of the file the project's fetch list, `fetch.ron`,
    /// gives asset ID `id`, as gg-api's `Fetch` reads it, if it gives it one: a file that `gg
    /// fetch` fetches, which the repository doesn't hold, and so a checkout hasn't until it's
    /// fetched. `None` for a context that isn't a project's game, or one without a fetch list, or
    /// whose list doesn't read. It reads the list each time, as only an error asks.
    #[cfg(feature = "dev")]
    fn fetched(&self, id: AssetId) -> Option<String> {
        let list = gg_api::Fetch::read(self.project_dir.as_deref()?).ok()?;
        let mut files = list.files();
        files.find(|file| file.id == Some(id)).map(|file| file.path)
    }

    /// Loads the texture at `path` from the asset root, which text names `name`, as
    /// [`add_texture`](Self::add_texture) loads a file, its name `name` too: a run that follows
    /// another names it so, as that run did. A file loaded already, as by a layer's add, takes the
    /// names alone, unnoted, as [`Assets`] keeps its notes.
    ///
    /// Fails as [`named_file`](Self::named_file) does, and as `add_texture` does.
    fn load_path(&mut self, name: &str, path: &str) -> Result<()> {
        let file = self.named_file(name, path)?;
        let resolved = self.files.file_of(&file);
        let texture = match self.loaded_texture(path, &file, resolved.as_deref()) {
            Some((texture, _)) => texture,
            None => self.add_texture(path, &file)?,
        };
        for named in [path, name] {
            self.alias(texture, &named.into());
        }
        Ok(())
    }

    /// The file at `path` from the asset root, which text names `name`, as a run that follows
    /// another names its asset from then on, as that run did.
    ///
    /// Fails with `asset.outside_root` for a file outside the root, as through `..` or a link.
    pub(super) fn named_file(&mut self, name: &str, path: &str) -> Result<PathBuf> {
        // Before anything can fail, so that a stand-in is named so too.
        #[cfg(feature = "dev")]
        if let Some(given) = &mut self.given
            && name != path
            && !given.contains_key(path)
        {
            given.insert(path.into(), name.into());
        }
        #[cfg(not(feature = "dev"))]
        let _ = name;
        let file = match &self.root {
            Some(root) => root.join(path),
            None => PathBuf::from(path),
        };
        self.within_root(path, &file)?;
        Ok(file)
    }

    /// Fails with `asset.outside_root` if `file`, which `path` names, resolves outside the asset
    /// root, as through `..` or a link, or, in a run that reads its game's package, as written; one
    /// that doesn't resolve, as one that isn't there, doesn't.
    pub(super) fn within_root(&self, path: &str, file: &Path) -> Result<()> {
        let within = self.root.as_deref().unwrap_or(Path::new("."));
        let outside = if self.reads_package() {
            self.files.within(file).is_none()
        } else {
            matches!(
                (assets::file_of(file), assets::file_of(within)),
                (Some(resolved), Some(within)) if !resolved.starts_with(&within)
            )
        };
        if outside {
            let message = format!(
                "`{path}` is outside the asset root, {}: an asset is named by its ID or by its path \
                 from the asset root",
                within.display()
            );
            return Err(Error::new("asset.outside_root", message));
        }
        Ok(())
    }

    /// Has a stand-in, a texture of magenta and black squares, take the place of the texture text
    /// names `name`, of the file at `path` from the asset root, if its name led to one, which
    /// couldn't be loaded, as `why` says, warning `asset.stand_in`: named as the file would have
    /// been, or else as `name`. In a run that reloads assets, the file's image takes the stand-in's
    /// place once it loads, as after a checkout brings it, or that of the asset whose ID `name` is,
    /// once one has it.
    fn stand_in(&mut self, name: &str, path: Option<&str>, why: &Error) {
        // 8 pixels across and down, in squares of 4.
        let pixels: Vec<u8> = (0..64)
            .flat_map(|i| match (i % 8 / 4 + i / 8 / 4) % 2 {
                0 => [255, 0, 255, 255],
                _ => [0, 0, 0, 255],
            })
            .collect();
        let texture = match self.textures.add(name, [8, 8], pixels, Mipmaps::None) {
            Ok(texture) => texture,
            Err(error) => {
                warn!(
                    code = error.code(),
                    name, "Can't stand in for texture `{name}`: {error:#}"
                );
                return;
            }
        };
        let (path, named, file): (Arc<str>, Arc<str>, _) = match path {
            Some(path) => {
                let file = self
                    .root
                    .as_deref()
                    .map_or(PathBuf::from(path), |root| root.join(path));
                let named = self.texture_name(path, &file, None, None);
                (path.into(), named, Some(file))
            }
            None => (name.into(), name.into(), None),
        };
        // So that it gives way to its texture once its file comes, or once an asset has its ID; but
        // never to a file outside the asset root.
        #[cfg(feature = "dev")]
        match &file {
            Some(_) if why.code() == "asset.outside_root" => {}
            Some(file) => self.watch_texture(texture, name, file, SystemTime::now(), &named),
            None => self.watch_for(texture, name),
        }
        #[cfg(not(feature = "dev"))]
        let _ = file;
        self.add_assets(|assets| assets.add_texture(texture, path.clone(), named));
        self.alias(texture, &name.into());
        self.added("texture", &path);
        warn!(
            code = "asset.stand_in",
            name,
            path = &*path,
            cause = why.code(),
            "Texture `{name}` can't be loaded, so a stand-in of magenta and black squares takes \
             its place, as the run follows another, which names it: {why:#}"
        );
    }

    /// Has a stand-in, a model of no nodes, take the place of the model text names `name`, as
    /// [`stand_in`](Self::stand_in) has one take a texture's: so that what names it reads, though
    /// its roots spawn nothing, and a hash of the world parts from its run's where their nodes
    /// were. It never gives way to its file, since a run that follows another takes no nodes the
    /// model's file would give later.
    fn stand_in_model(&mut self, name: &str, path: Option<&str>, why: &Error) {
        let (path, named): (Arc<str>, Arc<str>) = match path {
            Some(path) => {
                let root = self.root.as_deref();
                let file = root.map_or(PathBuf::from(path), |root| root.join(path));
                (path.into(), self.texture_name(path, &file, None, None))
            }
            None => (name.into(), name.into()),
        };
        self.add_assets(|assets| {
            let model = assets.add_model(path.clone(), named, Nodes::default());
            assets.alias_model(model, name.into());
        });
        self.loaded.made.push(Made::default());
        self.added("model", &format!("{path} {}", model_types()));
        warn!(
            code = "asset.stand_in",
            name,
            path = &*path,
            cause = why.code(),
            "Model `{name}` can't be loaded, so a stand-in of no nodes takes its place, as the run \
             follows another, which names it: {why:#}"
        );
    }

    /// Has a stand-in, a font that draws in the engine's, take the place of the font text names
    /// `name`, as [`stand_in`](Self::stand_in) has one take a texture's: its field a texture of a
    /// texel, which its file's takes once it bakes, as after a checkout brings it, or that of the
    /// asset whose ID `name` is, once one has it, in a run that reloads assets.
    fn stand_in_font(&mut self, name: &str, path: Option<&str>, why: &Error) {
        let field = (self.textures).add(
            &format!("{name} field"),
            [1, 1],
            [0; 4].as_slice(),
            Texels::Data,
        );
        let field = match field {
            Ok(field) => field,
            Err(error) => {
                warn!(
                    code = error.code(),
                    name, "Can't stand in for font `{name}`: {error:#}"
                );
                return;
            }
        };
        let font = self.fonts.push(field, None);
        let (path, named, file): (Arc<str>, Arc<str>, _) = match path {
            Some(path) => {
                let root = self.root.as_deref();
                let file = root.map_or(PathBuf::from(path), |root| root.join(path));
                let named = self.texture_name(path, &file, None, None);
                (path.into(), named, Some(file))
            }
            None => (name.into(), name.into(), None),
        };
        #[cfg(feature = "dev")]
        match &file {
            Some(_) if why.code() == "asset.outside_root" => {}
            Some(file) => self.watch_font(font, name, file, SystemTime::now(), &named),
            None => self.watch_font_for(font, name),
        }
        #[cfg(not(feature = "dev"))]
        let _ = file;
        self.add_assets(|assets| {
            assets.add_unnamed(field);
            assets.add_font(font, path.clone(), named);
            assets.alias_font(font, name.into());
        });
        self.added("font", &path);
        warn!(
            code = "asset.stand_in",
            name,
            path = &*path,
            cause = why.code(),
            "Font `{name}` can't be loaded, so text in it draws in the engine's own font, Inter, \
             as the run follows another, which names it: {why:#}"
        );
    }

    /// Has a stand-in, a prefab whose instances are their roots alone, take the place of the prefab
    /// text names `name`, as [`stand_in_model`](Self::stand_in_model) has one take a model's: so
    /// that what names it reads, though its instances spawn nothing. It never gives way to its
    /// file, since a run that follows another takes no entities the prefab's file would give later.
    fn stand_in_prefab(&mut self, name: &str, path: Option<&str>, why: &Error) {
        let (path, named): (Arc<str>, Arc<str>) = match path {
            Some(path) => {
                let root = self.root.as_deref();
                let file = root.map_or(PathBuf::from(path), |root| root.join(path));
                (path.into(), self.texture_name(path, &file, None, None))
            }
            None => (name.into(), name.into()),
        };
        self.add_assets(|assets| {
            let prefab = assets.add_prefab(path.clone(), named, None);
            assets.alias_prefab(prefab, name.into());
        });
        self.added("prefab", &path);
        warn!(
            code = "asset.stand_in",
            name,
            path = &*path,
            cause = why.code(),
            "Prefab `{name}` can't be loaded, so a stand-in whose instances are their roots alone \
             takes its place, as the run follows another, which names it: {why:#}"
        );
    }

    /// What names the texture or the model loaded from `file`, which a layer gave as `path`, whose
    /// path among assets is `named`, as [`Assets`] says, or with `image`, its image of that place
    /// in the model: in a development build, the name given its path, if the run is given its
    /// assets' names, as [`name_as`](Self::name_as) says; and else its ID, as [`id_of`] finds it,
    /// with an image's place after the model's; and otherwise its path, as a build without `dev`
    /// that reads no package, as it reads no ID file, names every one.
    ///
    /// [`id_of`]: Self::id_of
    pub(super) fn texture_name(
        &self,
        named: &str,
        path: &Path,
        file: Option<&Path>,
        image: Option<usize>,
    ) -> Arc<str> {
        let at: Arc<str> = match image {
            Some(i) => format!("{named}#{i}").into(),
            None => named.into(),
        };
        #[cfg(feature = "dev")]
        if let Some(given) = &self.given {
            return given.get(&at).cloned().unwrap_or(at);
        }
        match (self.id_of(path, file), image) {
            (Some(id), Some(i)) => format!("{id:016x}#{i}").into(),
            (Some(id), None) => format!("{id:016x}").into(),
            (None, _) => at,
        }
    }

    /// The ID of the asset at `path`, whose file is `file`: in a run that reads its game's package,
    /// the package's; else, in a development build, the asset database's, as
    /// [`asset_id`](Self::asset_id) finds it; and else none, as a build without `dev` reads no ID
    /// file.
    fn id_of(&self, path: &Path, file: Option<&Path>) -> Option<u64> {
        if let Some(package) = self.files.packaged() {
            return package.id(&self.files.within(path)?);
        }
        #[cfg(feature = "dev")]
        return self.asset_id(path, file).map(AssetId::get);
        #[cfg(not(feature = "dev"))]
        {
            let _ = file;
            None
        }
    }

    /// The texture loaded from `file`, which a layer gave as `path`, whose path among assets is
    /// `named`, with its size, if one was and isn't removed: by the file, as [`Loaded`] keeps it;
    /// or else, in a development build, the one loaded from the asset's file before it was renamed
    /// or moved, which its ID names, kept for `file` too from then on, so that an asset loads once
    /// whatever its file's been called.
    pub(super) fn loaded_texture(
        &mut self,
        named: &str,
        path: &Path,
        file: Option<&Path>,
    ) -> Option<(Texture, [u32; 2])> {
        if let Some(&found) = file.and_then(|file| self.loaded.textures.get(file)) {
            return Some(found);
        }
        let id = self.texture_name(named, path, file, None);
        let texture = self
            .assets
            .live_texture(&id)
            .filter(|_| assets::is_id(&id))?;
        let loaded = self.loaded.textures.values();
        let found = loaded.copied().find(|&(loaded, _)| loaded == texture)?;
        if let Some(file) = file {
            self.loaded.textures.insert(file.to_owned(), found);
        }
        Some(found)
    }

    /// The model loaded from `file`, which a layer gave as `path`, whose path among assets is
    /// `named`, if one was, as [`loaded_texture`](Self::loaded_texture) finds a texture.
    pub(super) fn loaded_model(
        &mut self,
        named: &str,
        path: &Path,
        file: Option<&Path>,
    ) -> Option<Model> {
        if let Some(&found) = file.and_then(|file| self.loaded.models.get(file)) {
            return Some(found);
        }
        let id = self.texture_name(named, path, file, None);
        let model = self.assets.live_model(&id).filter(|_| assets::is_id(&id))?;
        // Only one loaded from a file, which a stand-in wasn't.
        if !self.loaded.models.values().any(|&loaded| loaded == model) {
            return None;
        }
        if let Some(file) = file {
            self.loaded.models.insert(file.to_owned(), model);
        }
        Some(model)
    }

    /// The font loaded from `file`, which a layer gave as `path`, whose path among assets is
    /// `named`, if one was, as [`loaded_texture`](Self::loaded_texture) finds a texture.
    pub(super) fn loaded_font(
        &mut self,
        named: &str,
        path: &Path,
        file: Option<&Path>,
    ) -> Option<Font> {
        if let Some(&found) = file.and_then(|file| self.loaded.fonts.get(file)) {
            return Some(found);
        }
        let id = self.texture_name(named, path, file, None);
        let font = self.assets.font(&id).filter(|_| assets::is_id(&id))?;
        // Only one loaded from a file, which a stand-in wasn't until its file came.
        if !self.loaded.fonts.values().any(|&loaded| loaded == font) {
            return None;
        }
        if let Some(file) = file {
            self.loaded.fonts.insert(file.to_owned(), font);
        }
        Some(font)
    }

    /// The prefab loaded from `file`, which a layer gave as `path`, whose path among assets is
    /// `named`, if one was, as [`loaded_texture`](Self::loaded_texture) finds a texture.
    pub(super) fn loaded_prefab(
        &mut self,
        named: &str,
        path: &Path,
        file: Option<&Path>,
    ) -> Option<Prefab> {
        if let Some(&found) = file.and_then(|file| self.loaded.prefabs.get(file)) {
            return Some(found);
        }
        let id = self.texture_name(named, path, file, None);
        let prefab = self.assets.prefab(&id).filter(|_| assets::is_id(&id))?;
        // Only one loaded from a file, which a stand-in wasn't.
        if !self.loaded.prefabs.values().any(|&loaded| loaded == prefab) {
            return None;
        }
        if let Some(file) = file {
            self.loaded.prefabs.insert(file.to_owned(), prefab);
        }
        Some(prefab)
    }

    /// The ID the asset database holds for `file`, which a layer gave as `path`: by that path
    /// from the root, if it's given inside it, as layers spell a file's path as its disk does, or
    /// else by the path from the root of the file the OS resolves it to, as through `..`, a link
    /// or, where names ignore it, case.
    #[cfg(feature = "dev")]
    pub(super) fn asset_id(&self, path: &Path, file: Option<&Path>) -> Option<AssetId> {
        let root = self.root.as_deref()?;
        let catalog = self.assets.catalog();
        let spelled = path.strip_prefix(root).ok();
        let spelled = spelled.and_then(|within| catalog.id(&assets::name_of(None, within)));
        spelled.or_else(|| {
            let within = file?.strip_prefix(assets::file_of(root)?).ok()?;
            catalog.id(&assets::name_of(None, within))
        })
    }

    /// Has the run name the textures its layers add as `names`, each a name and the path it names,
    /// names their paths, and every other by its path, in place of the IDs the asset database
    /// holds, renaming those added already: as a replay names them as its recording's run did, and
    /// a lockstep session's joiner as its host does, whose ID files its own disk may not hold.
    #[cfg(feature = "dev")]
    pub(crate) fn name_as(&mut self, names: impl IntoIterator<Item = (Arc<str>, Arc<str>)>) {
        let given = names.into_iter().map(|(name, path)| (path, name)).collect();
        self.rename(given);
    }

    /// As [`name_as`](Self::name_as), but with `names` beside the names given already: as a
    /// lockstep host names the textures it adds after its joiner has joined.
    #[cfg(feature = "dev")]
    pub(crate) fn name_also(&mut self, names: impl IntoIterator<Item = (Arc<str>, Arc<str>)>) {
        let mut given = self.given.take().unwrap_or_default();
        given.extend(names.into_iter().map(|(name, path)| (path, name)));
        self.rename(given);
    }

    /// Names the textures as `given` names their paths, and every other by its path, from now on.
    #[cfg(feature = "dev")]
    fn rename(&mut self, given: HashMap<Arc<str>, Arc<str>>) {
        self.add_assets(|assets| assets.rename(&given));
        self.given = Some(given);
    }
}
