//! A development build's reloads of assets as their files change: what's watched, what's read.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use gg_api::asset::AssetId;
use gg_core::math::Vec2;
use gg_renderer::{Mipmaps, Texels, Texture};
use tracing::{info, warn};

use super::Context;
#[cfg(doc)]
use crate::World;
use crate::assets;
use crate::audio::{Music, Sound};
use crate::font::{self, Font};
use crate::game_ui;
use crate::hot_reload::{Asset as HotAsset, HotReload, Reloaded, Source};
use crate::image::Image;
use crate::model;
use crate::{Prefab, gltf};

impl Context {
    /// Sends the thread that reloads assets the shaders the layers added since, and puts each
    /// shader, texture, model, font, sound and music it read again since in place of the one
    /// loaded, under the same handle, as [`reload_texture`](Self::reload_texture),
    /// [`reload_model`](Self::reload_model) and [`reload_font`](Self::reload_font) say, which the
    /// next frame handed off draws, a sound's next play plays, logging `Sound reloaded`, and
    /// music's play crossfades to, logging `Music reloaded`: as a frame starts, `recorded` saying
    /// whether a recording follows the run. Once it has sent the shaders, it allocates nothing
    /// unless something came back.
    #[cfg(feature = "dev")]
    pub(crate) fn take_reloaded(&mut self, recorded: bool) {
        let Some(hot_reload) = &mut self.dev.hot_reload else {
            return;
        };
        hot_reload.watch_shaders(&self.shaders);
        while let Some(reloaded) = self.dev.hot_reload.as_ref().and_then(HotReload::take) {
            match reloaded {
                Reloaded::Shader(compiled) => {
                    let source = compiled.source();
                    let (shader, path) = (source.name(), source.path().display());
                    info!(shader, path = %path, "Shader reloaded");
                    self.shaders.replace(compiled);
                }
                Reloaded::Texture {
                    texture,
                    path,
                    image,
                } => self.reload_texture(texture, &path, image),
                Reloaded::Model {
                    file,
                    path,
                    imported,
                } => self.reload_model(&file, &path, imported, recorded),
                Reloaded::Font { font, path, bake } => self.reload_font(font, &path, *bake),
                Reloaded::Prefab { prefab, path, text } => {
                    self.reload_prefab(prefab, &path, text, recorded);
                }
                Reloaded::Ui { ui, path, document } => self.reload_ui(ui, &path, document),
                Reloaded::Sound { sound, path, data } => {
                    let named = assets::name_of(self.root.as_deref(), &path);
                    let frames = data.num_frames();
                    self.audio.replace(sound, *data);
                    info!(sound = &*named, frames, "Sound reloaded");
                }
                Reloaded::Music {
                    music,
                    path,
                    encoded,
                } => {
                    let named = assets::name_of(self.root.as_deref(), &path);
                    let frames = encoded.frames;
                    self.audio.replace_music(music, encoded);
                    info!(music = &*named, frames, "Music reloaded");
                }
            }
        }
    }

    /// Puts what `font` baked again from `path`, `bake`, in place of what it drew, its field taking
    /// the new one's texels at whatever size they are, logging `Font reloaded`, and has a later add
    /// of its file, by whatever path, give the font, as for a stand-in whose file came; but a
    /// stand-in keeps its place rather than take a file that came outside the asset root, as a
    /// texture's does, warning `asset.outside_root`. Text in it is laid out by what it baked from
    /// the next frame.
    #[cfg(feature = "dev")]
    pub(super) fn reload_font(&mut self, font: Font, path: &Path, bake: font::Bake) {
        let named = assets::name_of(self.root.as_deref(), path);
        let was_loaded = self.loaded.fonts.values().any(|&loaded| loaded == font);
        if !was_loaded && let Err(error) = self.within_root(&named, path) {
            let consequence = "the stand-in keeps its place";
            warn!(
                code = error.code(),
                font = &*named,
                "{error:#}; {consequence}"
            );
            return;
        }
        let font::Bake {
            size,
            pixels,
            baked,
        } = bake;
        let field = self.fonts.field(font);
        let replaced = (self.textures).replace(&named, field, size, pixels, Texels::Data);
        if let Err(error) = replaced {
            let consequence = "the font keeps drawing as it did";
            warn!(
                code = error.code(),
                font = &*named,
                "{error:#}; {consequence}"
            );
            return;
        }
        self.fonts.set(font, baked);
        self.uis.font_changed(font);
        if let Some(file) = assets::file_of(path) {
            self.loaded.fonts.entry(file).or_insert(font);
        }
        info!(font = &*named, ?size, "Font reloaded");
    }

    /// Puts `image`, read again from `path`, in place of `texture`'s pixels, at whatever size it
    /// is, logging `Texture reloaded`, and has a later add of its file, by whatever path, give the
    /// texture, as for a stand-in whose file came. A sprite sheet's texture keeps drawing as it did
    /// rather than take an image of another size, whose cells would be elsewhere in it than its
    /// sprites have them, warning `asset.sheet_resized`; and a stand-in keeps its place rather
    /// than take a file that came outside the asset root, as through `..` or a link, as text's
    /// names are kept inside it, warning `asset.outside_root`. A texture removed since its file
    /// was read takes nothing. The image takes the place of any its loader was still decoding.
    #[cfg(feature = "dev")]
    pub(super) fn reload_texture(&mut self, texture: Texture, path: &Path, image: Image) {
        if self.assets.removed(texture) {
            return;
        }
        let named = assets::name_of(self.root.as_deref(), path);
        let texels = image.texels(Texels::Color(Mipmaps::None));
        let Image { size, pixels, .. } = image;
        let loaded = self.loaded.textures.values();
        let was = loaded.copied().find(|&(loaded, _)| loaded == texture);
        // Only a stand-in was loaded from no file.
        if was.is_none()
            && let Err(error) = self.within_root(&named, path)
        {
            let consequence = "the stand-in keeps its place";
            warn!(
                code = error.code(),
                texture = &*named,
                "{error:#}; {consequence}"
            );
            return;
        }
        if let Some((_, was)) = was
            && was != size
            && self.loaded.sheets.contains(&texture)
        {
            warn!(
                code = "asset.sheet_resized",
                texture = &*named,
                "Texture `{named}` is {size:?} pixels across and down now, where it was {was:?}, \
                 but it's cut into a sprite sheet's cells, which a layer finds by its size as it \
                 adds it: it keeps drawing as it did until the run starts again"
            );
            return;
        }
        if let Err(error) = self.textures.replace(&named, texture, size, pixels, texels) {
            let consequence = "the texture keeps drawing as it did";
            warn!(
                code = error.code(),
                texture = &*named,
                "{error:#}; {consequence}"
            );
            return;
        }
        // In place of its pixels, if they're still decoding, which a replacement fills.
        self.loader.forget(texture);
        for (loaded, was) in self.loaded.textures.values_mut() {
            if *loaded == texture {
                *was = size;
            }
        }
        self.uis
            .texture_resized(texture, Vec2::new(size[0] as f32, size[1] as f32));
        if let Some(file) = assets::file_of(path) {
            self.loaded.textures.entry(file).or_insert((texture, size));
        }
        info!(texture = &*named, ?size, "Texture reloaded");
    }

    /// Puts what the model loaded from `file` draws, `imported`, read again from `path`, in place
    /// of what it drew, as [`model::reload`] says, logging `Model reloaded`: all of it, its roots'
    /// nodes fitted to its nodes as [`World::fit_nodes`] says, in the world and the world as edited,
    /// unless the run is `recorded`, or in a lockstep session or follows another, whose worlds
    /// change only as the run they go with changed them; there it takes its meshes and images
    /// alone, warning `asset.model_changed` of what changed that stays as it was until the run
    /// starts again, its nodes and materials, or of other numbers of primitives or images, which
    /// it doesn't take.
    #[cfg(feature = "dev")]
    fn reload_model(&mut self, file: &Path, path: &Path, imported: gltf::Imported, recorded: bool) {
        let Some(&model) = self.loaded.models.get(file) else {
            return;
        };
        let named = assets::name_of(self.root.as_deref(), path);
        let whole = !recorded && self.player.is_none() && !self.follows;
        let was = self.assets.nodes(model).expect("a model loaded").clone();
        let made = &mut self.loaded.made[model.index()];
        let stores = model::Stores {
            meshes: &mut self.meshes,
            shaders: &mut self.shaders,
            textures: &mut self.textures,
            materials: &mut self.materials,
        };
        let replaced = imported.images.len().min(made.images.len());
        let reloaded = model::reload(&named, imported, made, &was, whole, stores);
        let (nodes, kept, added) = match reloaded {
            Ok(reloaded) => reloaded,
            Err(error) => {
                warn!(
                    code = error.code(),
                    model = &*named,
                    "{error:#}; the model keeps drawing as it did"
                );
                return;
            }
        };
        // In place of their pixels, if they're still decoding, which the images replaced fill.
        for &(image, _) in &self.loaded.made[model.index()].images[..replaced] {
            self.loader.forget(image);
        }
        self.add_assets(|assets| {
            let (name, at) = (assets.model_name(model), assets.model_path(model));
            let images: Vec<_> = (added.iter())
                .map(|&(i, image)| {
                    (
                        image,
                        format!("{at}#{i}").into(),
                        format!("{name}#{i}").into(),
                    )
                })
                .collect();
            for (image, path, name) in images {
                assets.add_texture(image, path, name);
            }
        });
        if !whole && !kept.is_empty() {
            warn!(
                code = "asset.model_changed",
                model = &*named,
                "Model `{named}` reloaded its meshes and images, but its {} changed too, which the \
                 entities spawned from it keep as they were until the run starts again, as the run \
                 is recorded or in a lockstep session",
                kept.join(" and ")
            );
            return;
        }
        if nodes != was {
            self.add_assets(|assets| assets.set_nodes(model, nodes));
            self.world.fit_model(model, &was);
            if let Some(edited) = &mut self.dev.edited {
                edited.world.fit_model(model, &was);
            }
        }
        info!(model = &*named, "Model reloaded");
    }

    /// Puts `text`, prefab `prefab`'s file read again from `path`, in place of what it held, and
    /// has every instance of it, in the world and the world as edited, take what changed, as
    /// [`World::refit_prefab`] says, logging `Prefab reloaded`; but where the run is `recorded`, in
    /// a lockstep session or follows another, whose worlds change only as the run they go with
    /// changed them, it keeps what it held, warning `asset.prefab_changed`. What the text names
    /// that isn't loaded is loaded first; text that doesn't load into the world, as one naming a
    /// type the world hasn't, or a prefab that links it, warns with its error's code, and the
    /// prefab and its instances stay as they were.
    #[cfg(feature = "dev")]
    pub(super) fn reload_prefab(
        &mut self,
        prefab: Prefab,
        path: &Path,
        text: String,
        recorded: bool,
    ) {
        let named = assets::name_of(self.root.as_deref(), path);
        if recorded || self.player.is_some() || self.follows {
            warn!(
                code = "asset.prefab_changed",
                prefab = &*named,
                "Prefab `{named}` changed, which its instances keep as they were until the run \
                 starts again, as the run is recorded or in a lockstep session"
            );
            return;
        }
        if self
            .assets
            .prefab_text(prefab)
            .is_some_and(|was| **was == *text)
        {
            return;
        }
        let _ = self.load_scene_assets(&text);
        let text: Arc<str> = text.into();
        // Each world's found before either takes them, so that both take the text or neither does.
        let refits = self.world.refits(prefab, &text).and_then(|refits| {
            let edited = self.dev.edited.as_ref();
            let edited = edited.map(|edited| edited.world.refits(prefab, &text));
            Ok((refits, edited.transpose()?))
        });
        let (refits, edited_refits) = match refits {
            Ok(refits) => refits,
            Err(error) => {
                warn!(
                    code = error.code(),
                    prefab = &*named,
                    "{error:#}; the prefab and its instances stay as they were"
                );
                return;
            }
        };
        self.world.refit_prefab(refits);
        if let (Some(edited), Some(refits)) = (&mut self.dev.edited, edited_refits) {
            edited.world.refit_prefab(refits);
        }
        self.add_assets(|assets| assets.set_prefab_text(prefab, text));
        info!(prefab = &*named, "Prefab reloaded");
    }

    /// Puts `document`, UI file `file` read again from `path`, in place of what it held, every UI
    /// of it taking its nodes, as [`add_ui`](Self::add_ui) says, logging `UI reloaded`, unless it
    /// holds the same; what it names that isn't loaded is loaded first, and a font or a texture
    /// that doesn't load warns with its code, the UI staying as it was. A UI's state is the run's
    /// alone, so a recorded run or a lockstep peer takes it too.
    #[cfg(feature = "dev")]
    fn reload_ui(&mut self, file: usize, path: &Path, mut document: game_ui::Document) {
        let named = assets::name_of(self.root.as_deref(), path);
        if let Err(error) = self.load_ui_assets(&mut document) {
            warn!(
                code = error.code(),
                ui = &*named,
                "{error:#}; the UI stays as it was"
            );
            return;
        }
        if self.uis.holds(file, &document) {
            return;
        }
        self.uis.reload(file, document);
        info!(ui = self.uis.path(file), "UI reloaded");
    }

    /// Has the thread that reloads assets, if one does, watch UI file `file`, loaded as `name` from
    /// the file at `path` by a read begun at `read_at`, as [`watch_texture`](Self::watch_texture)
    /// has it watch a texture.
    #[cfg(feature = "dev")]
    pub(super) fn watch_ui(
        &mut self,
        file: usize,
        name: &str,
        path: &Path,
        read_at: SystemTime,
        named: &str,
    ) {
        let id = named
            .parse::<AssetId>()
            .ok()
            .filter(|_| assets::is_id(named));
        let (id, at) = self.followed(id, path);
        if let Some(hot_reload) = &mut self.dev.hot_reload {
            let source = Source::Ui {
                ui: file,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `prefab`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, as [`watch_texture`](Self::watch_texture) has
    /// it watch a texture.
    #[cfg(feature = "dev")]
    pub(super) fn watch_prefab(
        &mut self,
        prefab: Prefab,
        name: &str,
        path: &Path,
        read_at: SystemTime,
        named: &str,
    ) {
        let id = named
            .parse::<AssetId>()
            .ok()
            .filter(|_| assets::is_id(named));
        let (id, at) = self.followed(id, path);
        if let Some(hot_reload) = &mut self.dev.hot_reload {
            let source = Source::Prefab {
                prefab,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `texture`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, following its file as it's renamed or moved,
    /// as [`followed`](Self::followed) says, by its asset ID if it's named by one, as `named` is.
    #[cfg(feature = "dev")]
    pub(super) fn watch_texture(
        &mut self,
        texture: Texture,
        name: &str,
        path: &Path,
        read_at: SystemTime,
        named: &str,
    ) {
        let id = named
            .parse::<AssetId>()
            .ok()
            .filter(|_| assets::is_id(named));
        let (id, at) = self.followed(id, path);
        if let Some(hot_reload) = &mut self.dev.hot_reload {
            let source = Source::Texture {
                texture,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, put the file of the asset whose ID `name`
    /// is, if it's one, in place of `texture`, which stands in for it, once one has it.
    #[cfg(feature = "dev")]
    pub(super) fn watch_for(&mut self, texture: Texture, name: &str) {
        let id = name.parse::<AssetId>().ok().filter(|_| assets::is_id(name));
        if let (Some(hot_reload), Some(id)) = (&mut self.dev.hot_reload, id) {
            hot_reload.watch_for(HotAsset::Texture(texture), name, id);
        }
    }

    /// Has the thread that reloads assets, if one does, watch the model loaded as `name` from the
    /// file at `path`, which resolved to `file`, by a read begun at `read_at` that read `files`,
    /// following its file as it's renamed or moved, as [`followed`](Self::followed) says.
    #[cfg(feature = "dev")]
    pub(super) fn watch_model(
        &mut self,
        file: &Path,
        name: &str,
        path: &Path,
        files: Vec<PathBuf>,
        read_at: SystemTime,
    ) {
        let (id, at) = self.followed(self.asset_id(path, Some(file)), path);
        if let Some(hot_reload) = &mut self.dev.hot_reload {
            let source = Source::Model {
                file: file.to_owned(),
                name: name.to_owned(),
                path: path.to_owned(),
                files,
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `font`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, as [`watch_texture`](Self::watch_texture) has
    /// it watch a texture.
    #[cfg(feature = "dev")]
    pub(super) fn watch_font(
        &mut self,
        font: Font,
        name: &str,
        path: &Path,
        read_at: SystemTime,
        named: &str,
    ) {
        let id = named
            .parse::<AssetId>()
            .ok()
            .filter(|_| assets::is_id(named));
        let (id, at) = self.followed(id, path);
        if let Some(hot_reload) = &mut self.dev.hot_reload {
            let source = Source::Font {
                font,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, id, at);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `sound`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, at that path, as no asset ID names a sound.
    #[cfg(feature = "dev")]
    pub(super) fn watch_sound(
        &mut self,
        sound: Sound,
        name: &str,
        path: &Path,
        read_at: SystemTime,
    ) {
        if let Some(hot_reload) = &mut self.dev.hot_reload {
            let source = Source::Sound {
                sound,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, None, None);
        }
    }

    /// Has the thread that reloads assets, if one does, watch `music`, loaded as `name` from the
    /// file at `path` by a read begun at `read_at`, at that path, as no asset ID names music.
    #[cfg(feature = "dev")]
    pub(super) fn watch_music(
        &mut self,
        music: Music,
        name: &str,
        path: &Path,
        read_at: SystemTime,
    ) {
        if let Some(hot_reload) = &mut self.dev.hot_reload {
            let source = Source::Music {
                music,
                name: name.to_owned(),
                path: path.to_owned(),
                read_at,
            };
            hot_reload.watch(source, None, None);
        }
    }

    /// Has the thread that reloads assets, if one does, bake the file of the asset whose ID `name`
    /// is, if it's one, in place of `font`, which stands in for it, once one has it.
    #[cfg(feature = "dev")]
    pub(super) fn watch_font_for(&mut self, font: Font, name: &str) {
        let id = name.parse::<AssetId>().ok().filter(|_| assets::is_id(name));
        if let (Some(hot_reload), Some(id)) = (&mut self.dev.hot_reload, id) {
            hot_reload.watch_for(HotAsset::Font(font), name, id);
        }
    }

    /// What the thread that reloads assets follows the file at `path` by as it's renamed or moved,
    /// as [`HotReload::watch`] takes it: its asset ID, `id`, if it has one, and its path from the
    /// asset root, as the asset database has it by that ID, or else as `path` gives it, or as the
    /// OS resolves it, if it's under the root, by which the database gives it an ID once it has
    /// one, as for a file new to the run; none outside the root.
    #[cfg(feature = "dev")]
    fn followed(&self, id: Option<AssetId>, path: &Path) -> (Option<AssetId>, Option<Arc<str>>) {
        let by_id = id
            .and_then(|id| self.assets.catalog().path(id))
            .map(Arc::from);
        let at = by_id.or_else(|| {
            let root = self.root.as_deref()?;
            let spelled = path
                .strip_prefix(root)
                .ok()
                .map(|within| assets::name_of(None, within));
            spelled.or_else(|| {
                let within = assets::file_of(path)?;
                Some(assets::name_of(
                    None,
                    within.strip_prefix(assets::file_of(root)?).ok()?,
                ))
            })
        });
        (id, at)
    }
}
