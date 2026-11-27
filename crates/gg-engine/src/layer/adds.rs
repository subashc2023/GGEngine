//! The assets layers add from files and remove, and the assets the worlds share.

use std::any::type_name;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
#[cfg(feature = "dev")]
use std::time::SystemTime;

use bytemuck::NoUninit;
use gg_core::math::Vec2;
use gg_core::{Error, Result};
#[cfg(feature = "dev")]
use gg_renderer::Meshes;
use gg_renderer::{
    Blend, Mesh, Mipmaps, ModelParams, ModelVertex, Shader, SpriteSheet, Texels, Texture, Textures,
};
use tracing::{debug, info};

use super::{Context, Loading};
use crate::assets::{self, Asset, Assets};
use crate::cells::ModelRead;
use crate::font::{self, Font};
use crate::game_ui::{self, GameUi};
#[cfg(feature = "dev")]
use crate::hot_reload::Asset as HotAsset;
use crate::image::{self, Image};
#[cfg(doc)]
use crate::light::Light;
use crate::loader::Loader;
use crate::model;
use crate::package;
use crate::perf;
#[cfg(doc)]
use crate::{Animator, MeshDraw, ModelRoot, PrefabRoot, World};
use crate::{Model, Prefab, Shape, prefab};

impl Context {
    /// Compiles the Slang shader at `path` for layers to draw meshes of vertex type `V` with,
    /// taking parameters of type `P` with each draw, such as a color or texture: `()` for none.
    /// `name` names it in errors and GPU captures.
    ///
    /// `blend` sets how its color goes over what's drawn before it, such as [`Blend::Alpha`] for
    /// straight alpha, as PNGs store it, and so depth writes. Every shader draws only where it's
    /// as near as what's there or nearer. An opaque one writes depth, so what's drawn behind it
    /// later doesn't show over it. One that blends doesn't, so what's behind shows through. The
    /// engine checks the contract in [`gg_renderer::Shaders::add`].
    ///
    /// In a development run that serves the tool API, the shader recompiles as its file or an
    /// include changes, and the next frame draws with it. A change that doesn't compile, or breaks
    /// the contract, warns, and the shader keeps drawing as before.
    ///
    /// A file added again, by any form of its path, with the same vertex and parameter types and
    /// blend gives the first shader, compiled and reloaded once.
    ///
    /// # Errors
    ///
    /// - `renderer.bad_shader`, with slangc's diagnostics, if it doesn't compile.
    /// - `renderer.shader_mismatch`, with each way it breaks the contract.
    /// - `renderer.shader_files` if slangc's directory in the OS's temporary directory can't be
    ///   made or what it wrote there can't be read.
    /// - `renderer.no_slangc` if slangc can't be run.
    pub fn add_shader<V: NoUninit, P: NoUninit>(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
        blend: Blend,
    ) -> Result<Shader<V, P>> {
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let added = &self.loaded.shaders;
        let same = |by: &Path| {
            file.as_ref()
                .is_some_and(|file| added.get(by) == Some(file))
        };
        let shader = match self.shaders.find::<V, P>(blend, same) {
            Some(shader) => shader,
            None if self.reads_package() => {
                let spirv = (self.files)
                    .read_as(path, package::Kind::Shader, u64::MAX)
                    .map_err(|error| {
                        let message = format!(
                            "shader {name:?} at {} can't be read from the game's package",
                            path.display()
                        );
                        Error::new("renderer.bad_shader", message).with_source(error)
                    })?;
                self.shaders.add_compiled(name, path, spirv, blend)?
            }
            None => self.shaders.add(name, path, blend)?,
        };
        if let Some(file) = file {
            self.loaded.shaders.insert(path.to_owned(), file);
        }
        let types = format!("{} {} {blend:?}", layout::<V>(), layout::<P>());
        self.added("shader", &format!("{named} {types}"));
        Ok(shader)
    }

    /// Loads the PNG or JPEG file at `path` as a texture for sprites and layers' shaders to sample
    /// through the handle: a sprite's `texture`, or a field of a shader's parameters. Any PNG
    /// loads, and any JPEG that's Huffman-coded at 8 bits, sequential or progressive. Colors are
    /// taken as sRGB-encoded whatever the file's profile or gamma, as glTF requires, and alpha as
    /// not multiplied in. Shaders sample colors decoded to linear, with coordinates from (0, 0) at
    /// the top left to (1, 1) at the bottom right. `name` names it in errors and GPU captures.
    ///
    /// Adding reads the file and its header, which gives the size, and hands out the handle.
    /// Pixels decode off the main thread and frames stream them to the GPU within their budget;
    /// until then the texture samples white (see [`streaming`](Self::streaming)). A file whose
    /// header reads but whose pixels don't decode loads anyway, warns `engine.bad_texture` as it
    /// decodes, and samples white. A run from a game's package reads the cooker's BC7 compression.
    ///
    /// In a development run that serves the tool API, the texture takes its file's image again at
    /// any size as the file changes, once frames have streamed it (hot_reload.rs). A file that
    /// doesn't decode warns, and the texture keeps drawing as before. A test's context,
    /// [`Context::seeded`], decodes on the calling thread as it adds, warning at once. The add
    /// logs `asset.texture_loaded` and the decode `asset.texture_decoded`, each at debug level
    /// with its duration.
    ///
    /// A file added again, by any form of its path, gives the first texture without loading again
    /// (`assets::file_of` resolves it). So does an asset's file renamed or moved since, by its ID.
    /// `name` is then unused, and the texture keeps its first path. After
    /// [`remove_texture`](Self::remove_texture) the file loads anew. Scenes, snapshots, recordings
    /// and the tool API name it by its asset ID where its file has one (ARCHITECTURE.md's Project
    /// state), else by that path.
    ///
    /// # Errors
    ///
    /// - `engine.bad_texture` if the file can't be read, isn't a regular file, is over 1 GiB, isn't
    ///   a PNG or JPEG as far as its header shows, or has a side over 8,192 pixels, the most every
    ///   Vulkan 1.4 device takes.
    /// - `renderer.too_many_textures` past the 61,440 a renderer holds, the engine's white one
    ///   among them.
    pub fn add_texture(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Texture> {
        // `ping` says so, so `gg run` gives the load longer than a hang.
        #[cfg(feature = "dev")]
        let _loading = crate::api::loading();
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let texture = match self.loaded_texture(&named, path, file.as_deref()) {
            Some((texture, _)) => {
                self.alias(texture, &named);
                texture
            }
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let image = image::load_image(name, path, &self.files)?;
                let size = image.size;
                let (loader, textures) = (&mut self.loader, &mut self.textures);
                let texture = loader.load(textures, name, path, image, Mipmaps::None)?;
                let id = self.texture_name(&named, path, file.as_deref(), None);
                #[cfg(feature = "dev")]
                self.watch_texture(texture, name, path, read_at, &id);
                self.add_assets(|assets| assets.add_texture(texture, named.clone(), id));
                if let Some(file) = file {
                    self.loaded.textures.insert(file, (texture, size));
                }
                texture_loaded(&named, size, started);
                texture
            }
        };
        self.added("texture", &named);
        Ok(texture)
    }

    /// Loads the PNG or JPEG file at `path` as a sprite sheet: a texture, loaded like
    /// [`add_texture`](Self::add_texture)'s, cut into cells `cell` pixels across and down, packed
    /// edge to edge from its top left, for [`SpriteSheet::sprite`] to draw. A file added before,
    /// as a texture or sheet, isn't loaded again: its texture is cut into these cells. The
    /// texture reloads like `add_texture`'s, but only at its size, where its sprites have its
    /// cells. [`remove_texture`](Self::remove_texture) removes `sheet.texture`.
    ///
    /// Fails like `add_texture`, and with `engine.bad_sprite_sheet` if a side of `cell` is 0
    /// or more than the image's.
    pub fn add_sprite_sheet(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
        cell: [u32; 2],
    ) -> Result<SpriteSheet> {
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let sheet = match self.loaded_texture(&named, path, file.as_deref()) {
            Some((texture, size)) => {
                fits(name, path, size, cell)?;
                self.alias(texture, &named);
                SpriteSheet {
                    texture,
                    size,
                    cell,
                }
            }
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let image = image::load_image(name, path, &self.files)?;
                let (loader, textures) = (&mut self.loader, &mut self.textures);
                let sheet = add_sheet(loader, textures, name, path, image, cell)?;
                let (texture, size) = (sheet.texture, sheet.size);
                let id = self.texture_name(&named, path, file.as_deref(), None);
                #[cfg(feature = "dev")]
                self.watch_texture(texture, name, path, read_at, &id);
                self.add_assets(|assets| assets.add_texture(texture, named.clone(), id));
                if let Some(file) = file {
                    self.loaded.textures.insert(file, (texture, size));
                }
                texture_loaded(&named, size, started);
                sheet
            }
        };
        #[cfg(feature = "dev")]
        if !self.loaded.sheets.contains(&sheet.texture) {
            self.loaded.sheets.push(sheet.texture);
        }
        self.added("texture", &named);
        Ok(sheet)
    }

    /// What's still on its way to the GPU (see [`Loading`]). A loading screen shows until it's
    /// empty, as a test runs frames until it is.
    pub fn streaming(&self) -> Loading {
        Loading {
            decoding: self.loader.pending(),
            streaming: self.streamed,
        }
    }

    /// When the engine started, the first time it's asked once the layers have attached and
    /// [`streaming`](Self::streaming) is empty. The run then logs `engine.loaded`. `None` before
    /// and after, and in a test's or replay's context.
    pub(crate) fn take_loaded(&mut self) -> Option<Instant> {
        if self.load_started.is_none() || !self.attached || !self.streaming().is_empty() {
            return None;
        }
        self.load_started.take()
    }

    /// Adds a mesh for layers to draw: triangles, three of `indices` each, indexing `vertices`.
    /// A vertex is any `#[repr(C)]` type without padding that derives bytemuck's `NoUninit`.
    /// `name` names the mesh in errors and GPU captures. Frames stream it to the GPU within their
    /// budget, and draws of it draw nothing until it's there.
    ///
    /// Fails with `renderer.bad_mesh` if it has no triangles, indices left over from whole
    /// triangles, or an index past its vertices.
    pub fn add_mesh<V: NoUninit>(
        &mut self,
        name: &str,
        vertices: &[V],
        indices: &[u32],
    ) -> Result<Mesh<V>> {
        let mesh = self.meshes.add(name, vertices, indices)?;
        self.added("mesh", &format!("{name} {}", layout::<V>()));
        Ok(mesh)
    }

    /// Loads the glTF 2.0 model at `path`, a `.gltf` or `.glb` file, and the files it names: what
    /// its default scene draws, `scene` or else the first. [`Model::spawn`] places it in worlds as
    /// entities, as does a [`ModelRoot`] naming it, as scenes do by its name. `name` names it in
    /// errors and GPU captures.
    ///
    /// Its meshes and images are added once, like [`add_mesh`](Self::add_mesh) and
    /// [`add_texture`](Self::add_texture). Each image is read as far as its header, then decoded
    /// off the main thread with its mip levels, so one whose pixels don't decode warns and samples
    /// white. Each draws once frames have streamed it. The add logs `asset.model_loaded` with its
    /// duration and the meshes and images it added.
    ///
    /// A run from a game's package reads the model as cooked (cooked.rs): meshes optimized, with
    /// levels of detail each draw picks by its size on screen, and images compressed with their
    /// levels (BC7 of colors or values, BC5 of normal maps, DDS blocks as they are).
    ///
    /// A file added again gives the first model until [`remove_model`](Self::remove_model). It's
    /// named by its asset ID where its file has one, else by its path, like a texture, and its
    /// images by its name and their places in it, such as `5f3c9a1e07b2d846#0` or
    /// `assets/truck.glb#0`. In a development run that serves the tool API, it takes what its
    /// files hold again as they change (hot_reload.rs): meshes and images, and, unless the run is
    /// recorded, in a lockstep session or follows another, nodes and materials, which its roots'
    /// nodes take.
    ///
    /// # What it reads
    ///
    /// - Each node's transform (a matrix, or translation, rotation and scale) and its
    ///   KHR_lights_punctual light, which becomes a [`Light`] on the node's entity.
    /// - Each primitive's positions, normals, tangents, the texture coordinates its material's
    ///   textures name, COLOR_0 (floats or normalized integers) and 8, 16 or 32-bit indices. No
    ///   normals draws flat; a lit normal map without tangents takes MikkTSpace's.
    /// - Each material's glTF metallic-roughness (ARCHITECTURE.md's Rendering), or
    ///   KHR_materials_pbrSpecularGlossiness in its place; normal map and scale; occlusion map and
    ///   strength; emissive factor times KHR_materials_emissive_strength, and its texture; each
    ///   texture's wrapping and whether it magnifies to the nearest texel; alpha mode and cutoff;
    ///   double-sidedness (else only triangles' fronts draw); KHR_materials_unlit.
    /// - Buffers in files in the model's folder, named by URIs relative to the model's and read no
    ///   further than their byteLength, in base64 data URIs, or in a GLB's BIN chunk. Images there
    ///   or in buffer views: PNGs or JPEGs like `add_texture`'s, or BC7 DDS files with one level
    ///   or their full chain, named by a texture's MSFT_texture_dds in place of its source.
    ///   Formats are told apart by their bytes, not the declared types.
    /// - Skins, moving skinned primitives' vertices by their first four joints (JOINTS_0,
    ///   WEIGHTS_0), joints being nodes in the scene. Animations: clips of linear, stepped or
    ///   cubic-spline translations, rotations and scales, played by an [`Animator`] on a root.
    ///
    /// # What it ignores
    ///
    /// It logs these at info level as `engine.model_ignored`, and the model draws as at rest:
    ///
    /// - cameras, animation channels of morph weights or of nodes outside the scene, JOINTS_1 and
    ///   WEIGHTS_1, and extensions the model uses without requiring but this doesn't read;
    /// - a material's texture that reads other texture coordinates than its others, or whose image
    ///   another texture samples as what it doesn't, as data where it samples colors;
    /// - a primitive's TANGENT with non-finite numbers, which glTF forbids: it draws without
    ///   tangents.
    ///
    /// It ignores without a word: minification filters (it samples trilinearly and anisotropically
    /// unless a texture magnifies to the nearest texel), morph targets while their weights are 0,
    /// the other scenes, and whatever the scene doesn't reach.
    ///
    /// # Errors
    ///
    /// Nothing is added on failure.
    ///
    /// - `engine.unsupported_model` for glTF the engine doesn't draw: glTF 1, no scene, primitives
    ///   that aren't triangle lists, sparse accessors, accessors without a buffer view, morph
    ///   weights other than 0, a sampler that wraps across and down differently, a texture without
    ///   an image, a required extension but KHR_materials_unlit, KHR_lights_punctual,
    ///   KHR_materials_emissive_strength, KHR_materials_pbrSpecularGlossiness and
    ///   MSFT_texture_dds, or over 8 GiB to read (buffers, images as read and as decoded, vertices
    ///   and nodes).
    /// - `engine.bad_model` for a file that can't be read, isn't a regular file or isn't glTF, as
    ///   far as reading checks: an accessor past its buffer view; an index past its vertices; a
    ///   skinned primitive without JOINTS_0 and WEIGHTS_0 or moved by a joint past its skin's; a
    ///   skin's joint outside the scene; animation times that don't go up from 0; a node with two
    ///   parents; a matrix that isn't a translation, rotation and scale; a light the extension
    ///   doesn't allow; a URI that's neither a data URI nor a relative path, or has a `..`; an
    ///   image that isn't a PNG, JPEG or BC7 DDS as far as its header; integers written as floats,
    ///   such as `1.0`, though glTF allows them; positions that span more than the packed vertices
    ///   reach with an f32, as -f32::MAX beside 0 do.
    /// - `renderer.too_many_textures` if the renderer hasn't room for its images.
    pub fn add_model(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Model> {
        let zone = gg_core::zone!();
        // `ping` says so, so `gg run` gives the load longer than a hang.
        #[cfg(feature = "dev")]
        let _loading = crate::api::loading();
        let path = path.as_ref();
        zone.text(path.display());
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let model = match self.loaded_model(&named, path, file.as_deref()) {
            Some(model) => {
                if self.assets.live_model(&named).is_none() {
                    self.add_assets(|assets| assets.alias_model(model, named.clone()));
                }
                model
            }
            None => {
                let started = Instant::now();
                // As a cell's preparing read it on a thread, if it did.
                let read = self.cells.preparing.models.remove(path);
                let read = read.unwrap_or_else(|| ModelRead::now(name, path, &self.files));
                #[cfg(feature = "dev")]
                let read_at = read.at;
                let (imported, files) = read.read?;
                if !imported.ignored.is_empty() {
                    info!(
                        code = "engine.model_ignored",
                        model = name,
                        path = %path.display(),
                        "Model {name:?} draws without what the engine ignores of it: {}",
                        imported.ignored.join(", ")
                    );
                }
                let stores = model::Stores {
                    meshes: &mut self.meshes,
                    shaders: &mut self.shaders,
                    textures: &mut self.textures,
                    materials: &mut self.materials,
                };
                let added = model::add(name, path, imported, stores, &mut self.loader);
                let (nodes, made) = added?;
                let id = self.texture_name(&named, path, file.as_deref(), None);
                // Each named by the model's ID, or path, and its order in the model.
                let images: Vec<_> = (made.images.iter().enumerate())
                    .map(|(i, &(image, _))| {
                        let at = Some(i);
                        let id = self.texture_name(&named, path, file.as_deref(), at);
                        (image, format!("{named}#{i}").into(), id)
                    })
                    .collect();
                let model = self.add_assets(|assets| {
                    for (image, path, id) in images {
                        assets.add_texture(image, path, id);
                    }
                    assets.add_model(named.clone(), id, nodes)
                });
                // Its images are still decoding, each logging as it's done.
                info!(
                    code = "asset.model_loaded",
                    model = &*named,
                    meshes = made.meshes.len(),
                    images = made.images.len(),
                    ms = perf::millis(started.elapsed()),
                    "Model loaded"
                );
                self.loaded.made.push(made);
                if let Some(file) = file {
                    #[cfg(feature = "dev")]
                    self.watch_model(&file, name, path, files, read_at);
                    self.loaded.models.insert(file, model);
                }
                #[cfg(not(feature = "dev"))]
                let _ = files;
                model
            }
        };
        self.added("model", &format!("{named} {}", model_types()));
        Ok(model)
    }

    /// The model of built-in `shape`, added the first time it's asked for and given again after,
    /// as [`add_model`](Self::add_model) gives a file's. Text names it by [`Shape::name`], as
    /// `gg:cube`, which loads it alike. Once [removed](Self::remove_model), it's added anew.
    pub fn shape(&mut self, shape: Shape) -> Model {
        let name = shape.name();
        let model = match self.assets.live_model(name) {
            Some(model) => model,
            None => {
                let _zone = gg_core::zone!();
                let stores = model::Stores {
                    meshes: &mut self.meshes,
                    shaders: &mut self.shaders,
                    textures: &mut self.textures,
                    materials: &mut self.materials,
                };
                let (path, imported) = (Path::new(name), shape.imported());
                let added = model::add(name, path, imported, stores, &mut self.loader);
                // It has no image to find room for, and its mesh packs.
                let (nodes, made) = added.expect("a shape adds");
                // No file, so no ID: its name is its path.
                let model =
                    self.add_assets(|assets| assets.add_model(name.into(), name.into(), nodes));
                self.loaded.made.push(made);
                model
            }
        };
        self.added("model", &format!("{name} {}", model_types()));
        model
    }

    /// Removes `texture`, a texture or a sprite sheet's (`sheet.texture`) that a layer added or
    /// text named, as a game does with what it draws no more, such as a left level's. The GPU lets
    /// go of its image once the frames and copies that may use it have finished, and what of it
    /// was still decoding or streaming is dropped. From the frame that takes the removal on, what
    /// samples it samples white. Its handle is never given out again, so a stale one in the world
    /// or a snapshot never samples another texture. A development run watches its file no more.
    /// Removing one removed already does nothing.
    ///
    /// The simulation reads nothing that changes: the texture keeps its name and path, which the
    /// world's hash, scenes and snapshots read, and its names still find it. The world's sprites
    /// keep it, scene text a tick loads finds it, and a world hashes alike whenever a layer
    /// removed it. But what loads a texture by name or file (a layer's add, a scene the tool API
    /// or a scene file loads, a recorded or lockstep peer's command or edit, a restore) takes it
    /// for none and loads the file anew, as another texture, which its names find from then on.
    ///
    /// # Panics
    ///
    /// If `texture` is the engine's white texture or a font's field, which aren't removed; a
    /// model's image, which goes with its [model](Self::remove_model); or another context's.
    pub fn remove_texture(&mut self, texture: Texture) {
        if let Some(why) = self.unremovable(texture) {
            panic!("texture {} {why}", texture.index());
        }
        self.drop_texture(texture, true);
    }

    /// Removes `mesh`, which a layer added, like [`remove_texture`](Self::remove_texture): the GPU
    /// lets go of its buffers once the frames and copies that may use them have finished, and every
    /// draw of it draws nothing from the frame that takes the removal on. Its handle is never
    /// given out again. The simulation reads nothing of it: the world's hash reads nothing of a
    /// [`MeshDraw`]. Removing one removed already does nothing.
    ///
    /// # Panics
    ///
    /// If `mesh` is the engine's quad, which sprites, circles, lines and text are drawn on; a
    /// model's, which goes with its [model](Self::remove_model); or another context's.
    pub fn remove_mesh<V>(&mut self, mesh: Mesh<V>) {
        if let Some(why) = self.unremovable_mesh(mesh.index()) {
            panic!("mesh {} {why}", mesh.index());
        }
        self.drop_mesh(mesh.index());
    }

    /// Removes `model`, which a layer added or text named, and its meshes and images with it, like
    /// [`remove_mesh`](Self::remove_mesh) and [`remove_texture`](Self::remove_texture): what draws
    /// them draws nothing, or samples white, from the frame that takes the removal on. A
    /// development run watches its files no more. It keeps its name, path and nodes, so a root
    /// naming it spawns what it did, whenever it was removed, of draws that draw nothing. What
    /// loads it by name or file loads it anew, as another model, as for a texture. Removing one
    /// removed already does nothing.
    ///
    /// # Panics
    ///
    /// If `model` is another context's.
    pub fn remove_model(&mut self, model: Model) {
        let Some(made) = self.loaded.made.get(model.index()) else {
            panic!("model {} is another context's", model.index());
        };
        if self.assets.model_removed(model) {
            return;
        }
        let (meshes, images) = (made.meshes.clone(), made.images.clone());
        for mesh in meshes {
            self.meshes.remove(mesh);
        }
        for (image, _) in images {
            self.drop_texture(image, false);
        }
        let files = self.loaded.models.iter();
        let files: Vec<PathBuf> = files
            .filter(|&(_, &loaded)| loaded == model)
            .map(|(file, _)| file.clone())
            .collect();
        for file in files {
            self.loaded.models.remove(&file);
            #[cfg(feature = "dev")]
            if let Some(hot_reload) = &mut self.dev.hot_reload {
                hot_reload.unwatch(HotAsset::Model(file));
            }
        }
        let note = format!(
            "removed model {} {}",
            model.index(),
            self.assets.model_path(model)
        );
        self.add_assets(|assets| assets.remove_model(model));
        self.note(note);
    }

    /// Why `texture` isn't removable (see [`remove_texture`](Self::remove_texture)); `None` if it
    /// is.
    pub(super) fn unremovable(&self, texture: Texture) -> Option<&'static str> {
        if self.assets.name(texture).is_none() {
            return Some(
                "is the engine's white texture, a font's field or another context's texture, none \
                 of which is removed",
            );
        }
        let mut images = self.loaded.made.iter().flat_map(|made| &made.images);
        let of_model = images.any(|&(image, _)| image == texture);
        of_model.then_some("is a model's image, which goes with its model, through `remove_model`")
    }

    /// Why the mesh of index `index` isn't removable (see [`remove_mesh`](Self::remove_mesh)).
    pub(super) fn unremovable_mesh(&self, index: u32) -> Option<&'static str> {
        if index == 0 {
            return Some(
                "is the engine's quad, which sprites, circles, lines and text are drawn on",
            );
        }
        let mut meshes = self.loaded.made.iter().flat_map(|made| &made.meshes);
        let of_model = meshes.any(|mesh| mesh.index() == index);
        of_model.then_some("is a model's, which goes with its model, through `remove_model`")
    }

    /// Removes `texture` unless it's removed already (see
    /// [`remove_texture`](Self::remove_texture)). If `noted`, notes it among what the layers
    /// added, by index and path; a model's image goes with its model's note.
    pub(super) fn drop_texture(&mut self, texture: Texture, noted: bool) {
        if self.assets.removed(texture) {
            return;
        }
        self.textures.remove(texture);
        self.loader.forget(texture);
        (self.loaded.textures).retain(|_, &mut (loaded, _)| loaded != texture);
        #[cfg(feature = "dev")]
        {
            self.loaded.sheets.retain(|&sheet| sheet != texture);
            if let Some(hot_reload) = &mut self.dev.hot_reload {
                hot_reload.unwatch(HotAsset::Texture(texture));
            }
        }
        let path = self
            .assets
            .path(texture)
            .expect("a texture removed is named");
        let note = noted.then(|| format!("removed texture {} {path}", texture.index()));
        self.add_assets(|assets| assets.remove_texture(texture));
        if let Some(note) = note {
            self.note(note);
        }
    }

    /// Removes the mesh of index `index` unless it's removed already (see
    /// [`remove_mesh`](Self::remove_mesh)), noting it by index among what the layers added.
    pub(super) fn drop_mesh(&mut self, index: u32) {
        if self.meshes.remove_index(index) {
            self.note(format!("removed mesh {index}"));
        }
    }

    /// Loads the TrueType or OpenType font at `path`, a `.ttf` or `.otf` file, for
    /// [`draw_text`](Self::draw_text) to draw text in, through the handle. Loading bakes its
    /// field: a data texture of multi-channel signed distance fields, one for each glyph of the
    /// characters Windows-1252 prints that the font has, and for its `.notdef`, which draws for
    /// characters the font or field lacks except whitespace and control characters. The outlines
    /// are the default instance's (a variable font's at its axes' defaults), unhinted, since the
    /// field draws at any size.
    ///
    /// `name` names the font in errors and its field in GPU captures, as `<name> field`. Frames
    /// stream the field to the GPU, and text draws nothing until it's there, as glyphs bake and
    /// the field grows. The add logs `asset.font_loaded` with its duration.
    ///
    /// A file added again, by any form of its path, gives the first font without baking again, as
    /// does an asset's file renamed or moved since, by its ID. Scenes, snapshots, recordings and
    /// the tool API name it by its asset ID where its file has one, else by its path, like a
    /// texture. In a development run that serves the tool API, it bakes again off the main thread
    /// as its file changes.
    ///
    /// # Errors
    ///
    /// - `engine.bad_font` if the file can't be read, isn't a regular file, is over 256 MiB, isn't
    ///   a TrueType or OpenType font with outlines (a bitmap-only font isn't), or has glyphs that
    ///   don't fit a field of 8,192 texels square, alone or together.
    /// - `renderer.too_many_textures` if the renderer hasn't room for its field.
    pub fn add_font(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Font> {
        // `ping` says so, so `gg run` gives the load longer than a hang.
        #[cfg(feature = "dev")]
        let _loading = crate::api::loading();
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let font = match self.loaded_font(&named, path, file.as_deref()) {
            Some(font) => {
                if self.assets.font(&named).is_none() {
                    self.add_assets(|assets| assets.alias_font(font, named.clone()));
                }
                font
            }
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let font::Bake {
                    size,
                    pixels,
                    baked,
                } = font::load(name, path, &self.files)?;
                let field =
                    (self.textures).add(&format!("{name} field"), size, pixels, Texels::Data);
                let field = field?;
                let id = self.texture_name(&named, path, file.as_deref(), None);
                let font = self.fonts.push(field, Some(baked));
                #[cfg(feature = "dev")]
                self.watch_font(font, name, path, read_at, &id);
                self.add_assets(|assets| {
                    assets.add_unnamed(field);
                    assets.add_font(font, named.clone(), id);
                });
                if let Some(file) = file {
                    self.loaded.fonts.insert(file, font);
                }
                info!(
                    code = "asset.font_loaded",
                    font = &*named,
                    ?size,
                    ms = perf::millis(started.elapsed()),
                    "Font loaded"
                );
                font
            }
        };
        self.added("font", &named);
        Ok(font)
    }

    /// Loads the prefab at `path`, a `.prefab` file holding a scene of one subtree, as
    /// [`World::save_scene`] saves one, for [`Prefab::spawn`] and the [`PrefabRoot`]s naming it to
    /// spawn instances of, through the handle. `name` names it in errors. A file added again, by
    /// any form of its path, gives the first prefab, as does an asset's file renamed or moved
    /// since, by its ID. It's named by its asset ID where its file has one, else by its path, like
    /// a texture.
    ///
    /// Its text is read as it loads, and the textures, models, fonts and prefabs it names that none
    /// loaded has load after it, like a scene's. Its components are made as instances spawn, of
    /// the types the world has registered, so a text that doesn't load is refused then. The load
    /// logs `asset.prefab_loaded` with its duration, those loads among it. In a development run
    /// that serves the tool API, it's read again as its file changes and every instance takes
    /// what changed (see [`PrefabRoot`]), unless the run is recorded, in a lockstep session or
    /// follows another.
    ///
    /// Fails with `engine.bad_prefab` if the file can't be read, isn't a regular file, is over
    /// 16 MiB, isn't UTF-8, or isn't a scene's RON as far as its syntax and entity IDs go.
    pub fn add_prefab(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Prefab> {
        // `ping` says so, so `gg run` gives the load longer than a hang.
        #[cfg(feature = "dev")]
        let _loading = crate::api::loading();
        let path = path.as_ref();
        let named = assets::name_of(self.root.as_deref(), path);
        let file = self.files.file_of(path);
        let prefab = match self.loaded_prefab(&named, path, file.as_deref()) {
            Some(prefab) => {
                if self.assets.prefab(&named).is_none() {
                    self.add_assets(|assets| assets.alias_prefab(prefab, named.clone()));
                }
                prefab
            }
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let text: Arc<str> = prefab::read(name, path, &self.files)?.into();
                let id = self.texture_name(&named, path, file.as_deref(), None);
                let held = Some(Arc::clone(&text));
                let prefab =
                    self.add_assets(|assets| assets.add_prefab(named.clone(), id.clone(), held));
                #[cfg(feature = "dev")]
                self.watch_prefab(prefab, name, path, read_at, &id);
                if let Some(file) = file {
                    self.loaded.prefabs.insert(file, prefab);
                }
                self.added("prefab", &named);
                // What its text names, as a scene's load loads it, after it, so that a restored
                // run loads them in the same order; a text that doesn't load is refused as its
                // instances spawn.
                let _ = self.load_scene_assets(&text);
                info!(
                    code = "asset.prefab_loaded",
                    prefab = &*named,
                    ms = perf::millis(started.elapsed()),
                    "Prefab loaded"
                );
                return Ok(prefab);
            }
        };
        self.added("prefab", &named);
        Ok(prefab)
    }

    /// Loads the UI at `path`, a `.ui` file holding a tree of named nodes (ARCHITECTURE.md's
    /// Events and layers), or the UI file under the asset root whose asset ID `path` is, as text
    /// names one. Adds a [`GameUi`] of it for the layer to drive through the handle, with its own
    /// focus and what code sets of its nodes, apart from any other UI of the file, which is read
    /// once, by any form of its path. `name` names it in errors, the log and the tool API's `ui`.
    /// The file is named by its asset ID where it has one, else by its path, like a texture's.
    ///
    /// The fonts and textures its nodes name, by ID or path, load with it, like a scene's. The
    /// load logs `asset.ui_loaded` with its duration, theirs among it. In a development run that
    /// serves the tool API, the file is read again as it changes: every UI of it takes its nodes,
    /// each node keeps what code set of it by name, the focus stays on the button of its name, and
    /// it logs `UI reloaded`. A change that doesn't read warns with its code, the UI staying as it
    /// was.
    ///
    /// # Errors
    ///
    /// - `engine.bad_ui` if the file can't be read, isn't a regular file, is over 16 MiB or isn't
    ///   UTF-8.
    /// - `ui.syntax`, `ui.bad_name`, `ui.duplicate_name`, `ui.bad_field`, `ui.unnamed_button`,
    ///   `ui.bad_value` or `ui.no_node` for a file that isn't a UI's (`game_ui::file::parse`),
    ///   with the line and column where it's refused.
    /// - As finding an ID's file fails, such as `asset.unknown_id`.
    /// - `asset.no_font` or `asset.no_texture` for a font or texture it names that doesn't load,
    ///   saying why.
    /// - `renderer.too_many_textures` if the renderer hasn't room for the texture rounded corners
    ///   are cut from, which the first UI adds.
    pub fn add_ui(&mut self, name: &str, path: impl AsRef<Path>) -> Result<GameUi> {
        // `ping` says so, so `gg run` gives the load longer than a hang.
        #[cfg(feature = "dev")]
        let _loading = crate::api::loading();
        let path = path.as_ref();
        let path = match path.to_str().filter(|given| assets::is_id(given)) {
            Some(id) => {
                // A run that follows another loads a UI of its own, which names no asset of the
                // world's, by its ID as its own database has it.
                let at = self.asset_path(id, "UI", true)?;
                self.named_file(id, &at)?
            }
            None => path.to_owned(),
        };
        if self.uis.disc.is_none() {
            let (size, pixels) = game_ui::disc();
            let disc = self
                .textures
                .add("UI corners", size, pixels, Mipmaps::AlphaWeighted)?;
            self.add_assets(|assets| assets.add_unnamed(disc));
            self.uis.disc = Some(disc);
        }
        let file = self.files.file_of(&path);
        let named = assets::name_of(self.root.as_deref(), &path);
        let asset = self.texture_name(&named, &path, file.as_deref(), None);
        let found = match self.uis.file_of(&asset, file.as_deref()) {
            Some(found) => found,
            None => {
                let started = Instant::now();
                #[cfg(feature = "dev")]
                let read_at = SystemTime::now();
                let mut document = game_ui::read(name, &path, &self.files)?;
                self.load_ui_assets(&mut document)?;
                info!(
                    code = "asset.ui_loaded",
                    ui = &*named,
                    ms = perf::millis(started.elapsed()),
                    "UI loaded"
                );
                let found = self.uis.add_file(asset.clone(), named, document, file);
                #[cfg(feature = "dev")]
                self.watch_ui(found, name, &path, read_at, &asset);
                found
            }
        };
        Ok(self.uis.add(found, name))
    }

    /// Loads each font and texture the nodes of `document`, a UI's, name that none loaded has,
    /// like [`load_named`](Self::load_named) for a scene. Gives each node what it names, and an
    /// image its texture's size, where a layer's add or a scene loaded it from its file.
    ///
    /// Fails with `asset.no_font` or `asset.no_texture` for one that doesn't load, saying why.
    pub(super) fn load_ui_assets(&mut self, document: &mut game_ui::Document) -> Result<()> {
        let mut names = Vec::new();
        for node in &document.nodes {
            names.extend(node.font.iter().map(|font| Asset::Font(font.name.clone())));
            let textures = node.looks.iter().filter_map(|look| look.texture.as_ref());
            names.extend(textures.map(|texture| Asset::Texture(texture.name.clone())));
        }
        if names.is_empty() {
            return Ok(());
        }
        self.load_named(names, self.follows);
        for node in &mut document.nodes {
            if let Some(font) = &mut node.font {
                let found = self.assets.font(&font.name).ok_or_else(|| {
                    Error::new("asset.no_font", self.assets.not_found("font", &font.name))
                })?;
                font.handle = Some(found);
            }
            for texture in node
                .looks
                .iter_mut()
                .filter_map(|look| look.texture.as_mut())
            {
                let found = self.assets.live_texture(&texture.name).ok_or_else(|| {
                    Error::new(
                        "asset.no_texture",
                        self.assets.not_found("texture", &texture.name),
                    )
                })?;
                texture.handle = Some(found);
            }
            let shown = node.looks[0]
                .texture
                .as_ref()
                .and_then(|texture| texture.handle);
            let loaded = self.loaded.textures.values();
            let size = loaded.copied().find(|&(loaded, _)| Some(loaded) == shown);
            node.natural = size.map(|(_, size)| Vec2::new(size[0] as f32, size[1] as f32));
        }
        Ok(())
    }

    /// Records that a layer added an asset of `kind`, described by `what`: its name, and the types
    /// a mesh's, shader's or model's draws hold the bytes of. [`Assets`] keeps it.
    pub(super) fn added(&mut self, kind: &str, what: &str) {
        let (meshes, shaders) = (self.meshes.count(), self.shaders.count());
        let added = format!("{kind} {meshes} {shaders} {} {what}", self.textures.count());
        self.note(added);
    }

    /// Notes `line` among what the layers added and removed, after
    /// [`note_engine`](Self::note_engine)'s notes. Every note goes through here but the engine's
    /// that [`load_restored`](Self::load_restored) takes again.
    fn note(&mut self, line: String) {
        self.note_engine();
        self.add_assets(|assets| assets.note(line));
    }

    /// Notes each shader and mesh the engine added when first asked for that no note names yet, as
    /// `engine shader 5 text`: shaders, then meshes, each in the order added. Each shifted the
    /// indices of what was added after it, which a restored run's frames don't, so that run adds
    /// it again at its note ([`load_restored`](Self::load_restored)).
    fn note_engine(&mut self) {
        let added = self.assets.added();
        // Only this and `load_restored` note the engine's, once each and at its index, so a count
        // tells whether any is new. Scanning the notes for each note is quadratic in them over a
        // run: fine for hundreds; count them in `Assets` if runs note many thousands.
        let noted = added.iter().filter(|line| line.starts_with("engine "));
        if noted.count() == self.shaders.engine().count() + self.meshes.engine().count() {
            return;
        }
        let shaders = self.shaders.engine();
        let shaders = shaders.map(|(name, at)| format!("engine shader {at} {name}"));
        let meshes = self.meshes.engine();
        let meshes = meshes.map(|(name, at)| format!("engine mesh {at} {name}"));
        let new: Vec<String> = (shaders.chain(meshes))
            .filter(|line| !added.iter().any(|noted| **noted == **line))
            .collect();
        self.add_assets(|assets| {
            for line in new {
                assets.note(line);
            }
        });
    }

    /// Has `path` name `texture`, which another path to its file added before (see
    /// [`Assets::alias`]), unless it already names a texture that isn't removed.
    pub(super) fn alias(&mut self, texture: Texture, path: &Arc<str>) {
        if self.assets.live_texture(path).is_none() {
            self.add_assets(|assets| assets.alias(texture, path.clone()));
        }
    }

    /// Has `add` change the assets the layers added, which the world, and the world as edited while
    /// a copy plays, share. Kept in the context so a world that took the world's place doesn't lose
    /// them, and copied only where a copy of the world shares them.
    pub(super) fn add_assets<R>(&mut self, add: impl FnOnce(&mut Assets) -> R) -> R {
        // The worlds let go of their shares for the change.
        self.world.share_assets(&Arc::default());
        #[cfg(feature = "dev")]
        if let Some(edited) = &mut self.dev.edited {
            edited.world.share_assets(&Arc::default());
        }
        let added = add(Arc::make_mut(&mut self.assets));
        self.world.share_assets(&self.assets);
        #[cfg(feature = "dev")]
        if let Some(edited) = &mut self.dev.edited {
            edited.world.share_assets(&self.assets);
        }
        added
    }

    /// Has the world share the assets again, if a layer put another world in its place (see
    /// [`World::share_assets`]).
    pub(crate) fn share_assets(&mut self) {
        self.world.share_assets(&self.assets);
    }

    /// The meshes the layers added, which the tool API reads extents from.
    #[cfg(feature = "dev")]
    pub(crate) fn meshes(&self) -> &Meshes {
        &self.meshes
    }
}

/// Logs `asset.texture_loaded` at the debug level, as a game may add many: texture `named`, `size`
/// pixels, added from its file in the time since `started`. Its pixels decode on the loader's
/// threads from then on, as [`Loader`] logs; where the loader decodes inline, as a test's or
/// replay's context's does, they decoded within that time.
fn texture_loaded(named: &str, size: [u32; 2], started: Instant) {
    debug!(
        code = "asset.texture_loaded",
        texture = named,
        ?size,
        ms = perf::millis(started.elapsed()),
        "Texture loaded"
    );
}

/// Adds `image`, sprite sheet `name`'s from `path`, read as far as its header, to `textures`
/// through `loader`, as a sheet of cells `cell` pixels across and down, if they fit, as [`fits`]
/// checks; one that doesn't takes no texture.
pub(super) fn add_sheet(
    loader: &mut Loader,
    textures: &mut Textures,
    name: &str,
    path: &Path,
    image: Image,
    cell: [u32; 2],
) -> Result<SpriteSheet> {
    let size = image.size;
    fits(name, path, size, cell)?;
    let texture = loader.load(textures, name, path, image, Mipmaps::None)?;
    Ok(SpriteSheet {
        texture,
        size,
        cell,
    })
}

/// Checks that cells `cell` pixels across and down fit sprite sheet `name`'s image, from `path`, of
/// `size` pixels.
///
/// Fails with `engine.bad_sprite_sheet` if a side of `cell` is 0 or more than the image's.
fn fits(name: &str, path: &Path, size: [u32; 2], cell: [u32; 2]) -> Result<()> {
    let [width, height] = size;
    if !(1..=width).contains(&cell[0]) || !(1..=height).contains(&cell[1]) {
        return Err(Error::new(
            "engine.bad_sprite_sheet",
            format!(
                "sprite sheet {name:?} at {} is {width}x{height} pixels, but its cells are {}x{}, \
                 where each side is 1 pixel up to the image's",
                path.display(),
                cell[0],
                cell[1]
            ),
        ));
    }
    Ok(())
}

/// Type `T`'s name and size, such as `glam::f32::vec3::Vec3 12`: what a snapshot's assets say a
/// draw holds the bytes of, so a draw kept in one comes back only where they're the same.
fn layout<T>() -> String {
    format!("{} {}", type_name::<T>(), size_of::<T>())
}

/// The types a model's draws hold the bytes of, as its `asset` line says them after its path.
pub(crate) fn model_types() -> String {
    format!("{} {}", layout::<ModelVertex>(), layout::<ModelParams>())
}
