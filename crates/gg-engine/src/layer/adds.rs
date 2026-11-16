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
use crate::{Model, Prefab, prefab};

impl Context {
    /// Compiles the Slang shader at `path` for layers to draw meshes of vertex type `V` with from
    /// then on, which takes parameters of type `P` with each draw, such as a color or a texture:
    /// `()` for none. `blend` says how its color goes over what's drawn before it, such as
    /// [`Blend::Alpha`] for straight alpha, as PNGs store it; and so whether it writes depth: every
    /// shader draws only where it's as near as what's drawn there or nearer, and an opaque one
    /// writes its depth too, so that what's behind it, drawn later, doesn't show over it, while one
    /// that blends doesn't, so that what's behind it shows through it. `name` names it in errors
    /// and GPU captures. The shader keeps to the contract that [`gg_renderer::Shaders::add`]
    /// describes, which the engine checks.
    ///
    /// In a development run that serves the tool API, the shader compiles again as its file or a
    /// file it includes changes, and the next frame draws with it; a change that doesn't compile,
    /// or breaks the contract, warns, and the shader keeps drawing as it did.
    ///
    /// A file added again, by whatever form of its path, with the same vertex and parameter types
    /// and blend, gives the shader it gave first, which compiles and reloads once.
    ///
    /// Fails with `renderer.bad_shader` and slangc's diagnostics if it doesn't compile, with
    /// `renderer.shader_mismatch` and each way it breaks the contract if it breaks it, with
    /// `renderer.shader_files` if slangc's directory in the OS's temporary directory can't be made
    /// or what it wrote there can't be read, and with `renderer.no_slangc` if slangc can't be run.
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
    /// from then on, through the handle: a sprite's `texture`, or a field of a shader's parameters
    /// as it is. Any PNG loads, and any JPEG that's Huffman-coded at 8 bits, sequential or
    /// progressive, as nearly all are: its colors are taken as sRGB-encoded, whatever color profile
    /// or gamma it holds, as glTF requires, and its alpha as not multiplied into them, and shaders
    /// sample its colors decoded to linear, with texture coordinates from (0, 0) at its top left to
    /// (1, 1) at its bottom right. `name` names the texture in errors and GPU captures. The file is
    /// read, and its header, which gives its size, as the texture is added, which hands out its
    /// handle then; its pixels decode off the main thread, and frames stream them to the GPU within
    /// their budget, what samples it sampling a pixel of white until they're there, as
    /// [`streaming`](Self::streaming) tells. One whose header reads but whose pixels don't decode,
    /// as a file cut short past its header, loads all the same, warning `engine.bad_texture` as it
    /// decodes, and samples as white. A run from a game's package reads it as the cooker
    /// compressed it, to BC7. In a development run that serves the tool API, the texture takes its
    /// file's image again as the file changes, at whatever size, once frames have streamed it,
    /// drawing as it did until then, as hot_reload.rs says; one that doesn't decode
    /// warns, and the texture keeps drawing as it did. A test's context, [`Context::seeded`],
    /// decodes it on the calling thread as it's added instead, warning at once. A file's add logs
    /// `asset.texture_loaded`, and its pixels' decode `asset.texture_decoded`, each with how long
    /// it took, at the debug level.
    ///
    /// A file added again, by whatever form of its path, gives the texture it gave first, without
    /// loading it again, as `assets::file_of` resolves it, and so does an asset's file renamed
    /// or moved since, by the ID that names it; `name` is then unused, and the texture keeps the
    /// path it was first added by, which another path names too; but once the texture's removed,
    /// as [`remove_texture`](Self::remove_texture) says, the file loads anew. Scenes,
    /// snapshots, recordings and the tool API name it by its asset ID, where its file has one, as
    /// ARCHITECTURE.md's Project state says, and otherwise by that path.
    ///
    /// Fails with `engine.bad_texture` if the file can't be read, isn't a regular file, is more
    /// than 1 GiB, isn't a PNG or a JPEG as far as its header, or has a side of more than 8,192
    /// pixels, the most every Vulkan 1.4 device takes, and with `renderer.too_many_textures` past
    /// the 61,440 a renderer holds, the engine's white one among them.
    pub fn add_texture(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Texture> {
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

    /// Loads the PNG or JPEG file at `path` as a sprite sheet: a texture, as
    /// [`add_texture`](Self::add_texture) loads one, its pixels decoding off the main thread,
    /// cut into cells `cell` pixels across and down, packed edge to edge from its top left, which
    /// [`SpriteSheet::sprite`] draws, as its header's size places them. A file added before, as a
    /// texture or a sprite sheet, isn't loaded again: its texture is cut into these cells. Its
    /// texture reloads as `add_texture`'s does, but only at the size it has, where its sprites have
    /// its cells. [`remove_texture`](Self::remove_texture) removes its texture, `sheet.texture`.
    ///
    /// Fails as `add_texture` does, and with `engine.bad_sprite_sheet` if a side of `cell` is 0
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

    /// What's still on its way to the GPU, as [`Loading`] says: the textures whose images are
    /// decoding, and what the renderer was streaming once it had rendered the last frame it handed
    /// back. A loading screen shows until it's empty, as a test runs frames until it is.
    pub fn streaming(&self) -> Loading {
        Loading {
            decoding: self.loader.pending(),
            streaming: self.streamed,
        }
    }

    /// When the engine started, once the run has loaded: the first time it's asked once the layers
    /// have attached and nothing is on its way to the GPU, as [`streaming`](Self::streaming) tells,
    /// which the run then logs `engine.loaded` at; `None` before and after, and in a test's or a
    /// replay's context.
    pub(crate) fn take_loaded(&mut self) -> Option<Instant> {
        if self.load_started.is_none() || !self.attached || !self.streaming().is_empty() {
            return None;
        }
        self.load_started.take()
    }

    /// Adds a mesh for layers to draw from then on: triangles, three of `indices` each, which
    /// index `vertices`. A vertex is any `#[repr(C)]` type without padding, which derives
    /// bytemuck's `NoUninit`. `name` names the mesh in errors and GPU captures. Frames stream it
    /// to the GPU within their budget, and draws of it draw nothing until it's there.
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
    /// its default scene draws, `scene` or else the first, which [`Model::spawn`] places in worlds
    /// as entities, as many times as layers like, as a [`ModelRoot`] does wherever it names the
    /// model, as scenes do by its name. Its meshes and images are added once, as
    /// [`add_mesh`](Self::add_mesh) and [`add_texture`](Self::add_texture) add them, each image
    /// read as far as its header as the model is, and decoded off the main thread, with mip levels
    /// made there too, so that one whose pixels don't decode warns and samples as white, as a
    /// texture's does; each draws once frames have streamed it to the GPU. `name` names the model
    /// in errors and GPU captures. Its add logs `asset.model_loaded`, with how long it took and how
    /// many meshes and images it added, whose decodes log as a texture's do. A run from a game's
    /// package reads the model as the cooker cooked it instead, its meshes optimized, with levels
    /// of detail that each draw picks by its size on screen, and its images compressed with their
    /// levels, BC7 of colors or values and BC5 of normal maps, but DDS images' blocks as they are,
    /// as cooked.rs says. A file added again gives the model it gave first, until it's removed, as
    /// [`remove_model`](Self::remove_model) says.
    ///
    /// Of each node it takes the transform, a matrix or a translation, rotation and scale, and its
    /// light, as KHR_lights_punctual gives it, which the node's entity has as a [`Light`]; of each
    /// primitive its mesh draws, the positions, normals and tangents, the texture coordinates its
    /// material's textures name, and its vertex colors, COLOR_0, as floats or normalized integers,
    /// and its indices, of 8, 16 or 32 bits; and of its material, glTF's metallic-roughness, as
    /// ARCHITECTURE.md's Rendering says: the base color factor and texture, the metallic and
    /// roughness factors and their texture, or, in their place, KHR_materials_pbrSpecularGlossiness's
    /// diffuse factor and texture, specular and glossiness factors and their texture, the normal map
    /// and its scale, the occlusion map and its strength, the emissive factor, times
    /// KHR_materials_emissive_strength's strength, and its texture, each texture's wrapping and
    /// whether it magnifies to the nearest texel, the alpha mode and cutoff, whether it's
    /// double-sided, and whether KHR_materials_unlit makes it unlit.
    /// A primitive without normals draws flat, and a lit material's normal map without tangents
    /// takes MikkTSpace's; a material that isn't double-sided draws its triangles' fronts alone.
    /// Buffers
    /// are in files in the model's folder, named by URIs relative to the model's, read no further
    /// than their byteLength, in base64 data URIs or in a GLB's BIN chunk, and images there or in
    /// buffer views: PNGs or JPEGs, as `add_texture` takes them, or DDS files of BC7, with one level
    /// or their full chain, which a texture's MSFT_texture_dds names in place of its source, told
    /// apart by their bytes rather than by the types the model declares. A file added again gives
    /// the model it gave first. It's
    /// named by its asset ID, where its file has one, or else by its path, as a texture is, and its
    /// images by its name and their places in it, such as `5f3c9a1e07b2d846#0` or
    /// `assets/truck.glb#0`. In a development run that serves the tool API, it takes what its files
    /// hold again as they change, as hot_reload.rs says: its meshes and images, and,
    /// unless the run is recorded, in a lockstep session or follows another, its nodes and
    /// materials, which its roots' nodes take.
    ///
    /// Its skins move skinned primitives' vertices by their joints, its nodes in the scene, each
    /// vertex by its first four joints, JOINTS_0 and WEIGHTS_0, as the frame draws them; its
    /// animations are clips, linear, stepped or cubic splines of its nodes' translations, rotations
    /// and scales, which an [`Animator`] on a root plays on its nodes.
    ///
    /// It ignores cameras, animations' channels of morph weights or of nodes outside the scene,
    /// JOINTS_1 and WEIGHTS_1, and the extensions the model uses without requiring them but
    /// those it reads, a material's texture that reads other texture coordinates than its others,
    /// or whose image another texture samples as what it doesn't, as data where it samples colors,
    /// and a primitive's TANGENT that holds numbers that aren't finite, which glTF doesn't allow,
    /// drawing the primitive as one without them, and logs which it has at info level, as
    /// `engine.model_ignored`: the model draws as
    /// it is at rest. It ignores without a word the textures' minification filters, since it
    /// samples them trilinearly and anisotropically unless they magnify to the nearest texel, morph
    /// targets while their weights are 0, the other scenes, and whatever the scene doesn't reach,
    /// which it doesn't read.
    ///
    /// Fails, adding nothing, with `engine.unsupported_model` for glTF the engine doesn't draw:
    /// glTF 1, a model without a scene, primitives that aren't triangle lists, sparse accessors and
    /// accessors without a buffer view, morph weights other than 0, a sampler that
    /// wraps across and down differently, a texture without an image, any required extension but
    /// KHR_materials_unlit, KHR_lights_punctual, KHR_materials_emissive_strength,
    /// KHR_materials_pbrSpecularGlossiness and MSFT_texture_dds, and a model that takes more than 8
    /// GiB to read, of its buffers, images, as read and as decoded, vertices and nodes; with
    /// `engine.bad_model` for a file that can't be read, isn't a regular file or isn't
    /// glTF, as far as reading it checks, such as an accessor past its buffer view, an index past
    /// its vertices, a skinned primitive without JOINTS_0 and WEIGHTS_0 or moved by a joint past
    /// its skin's, a skin's joint outside the scene, an animation's times that don't go up from 0,
    /// a node with two parents, a matrix that isn't a translation, rotation and
    /// scale, a light the extension doesn't allow, a URI that's neither a data URI nor a relative
    /// path, or that has a `..`, or an image that isn't a PNG, a JPEG or a DDS of BC7 as far as its
    /// header, and,
    /// though glTF allows them, integers written as floats, such as `1.0`, and a primitive whose
    /// positions span more than its packed vertices reach with what an f32 holds, as -f32::MAX
    /// beside 0 do; and with
    /// `renderer.too_many_textures` if the renderer hasn't room for its images.
    pub fn add_model(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Model> {
        let zone = gg_core::zone!();
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

    /// Removes `texture`, a texture or a sprite sheet's (`sheet.texture`) that a layer added or
    /// text named, as a game does with what it draws no more, such as a level's once it has left
    /// the level: the GPU lets go of its image once the frames and copies that may use it have
    /// finished, what of it was still decoding or streaming is dropped, and what samples it samples
    /// a pixel of white from the frame that takes the removal on. Its handle is never given out
    /// again, so that a stale one, in the world or a snapshot, never samples another texture; and
    /// a development run watches its file no more. Removing one removed already does nothing.
    ///
    /// Removal changes nothing the simulation reads: the texture keeps its name and its path, which
    /// the world's hash, scenes and snapshots read, and its names go on finding it, so that the
    /// world's sprites keep it, scene text that a tick loads finds it, and a world hashes alike
    /// whenever a layer removed it. But what loads a texture by its name or its file, a layer's
    /// add, a scene that the tool API or a scene file loads, a recorded or a lockstep peer's
    /// command or edit, and a restore, takes it for none, and loads its file anew, as another
    /// texture, which its names find from then on.
    ///
    /// # Panics
    ///
    /// If `texture` is the engine's white texture or a font's field, which aren't removed, or a
    /// model's image, which goes with its model, as [`remove_model`](Self::remove_model) says; or
    /// another context's.
    pub fn remove_texture(&mut self, texture: Texture) {
        if let Some(why) = self.unremovable(texture) {
            panic!("texture {} {why}", texture.index());
        }
        self.drop_texture(texture, true);
    }

    /// Removes `mesh`, which a layer added, as [`remove_texture`](Self::remove_texture) removes a
    /// texture: the GPU lets go of its buffers once the frames and copies that may use them have
    /// finished, and every draw of it draws nothing from the frame that takes the removal on, its
    /// handle never given out again. The simulation reads nothing of it, as the world's hash reads
    /// nothing of a [`MeshDraw`]. Removing one removed already does nothing.
    ///
    /// # Panics
    ///
    /// If `mesh` is the engine's quad, which sprites, circles, lines and text are drawn on, or a
    /// model's, which goes with its model, as [`remove_model`](Self::remove_model) says; or
    /// another context's.
    pub fn remove_mesh<V>(&mut self, mesh: Mesh<V>) {
        if let Some(why) = self.unremovable_mesh(mesh.index()) {
            panic!("mesh {} {why}", mesh.index());
        }
        self.drop_mesh(mesh.index());
    }

    /// Removes `model`, which a layer added or text named, and its meshes and images with it, as
    /// [`remove_mesh`](Self::remove_mesh) and [`remove_texture`](Self::remove_texture) remove them,
    /// so that what draws them draws nothing, or samples white, from the frame that takes the
    /// removal on; a development run watches its files no more. It keeps its name, its path and
    /// its nodes, as a texture removed keeps its name, so that a root that names it spawns what it
    /// did, whenever it was removed, of draws that draw nothing; but what loads it by its name or
    /// its file loads it anew, as another model, as for a texture. Removing one removed already
    /// does nothing.
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
            if let Some(hot_reload) = &mut self.hot_reload {
                hot_reload.unwatch(HotAsset::Model(file));
            }
        }
        let note = format!(
            "removed model {} {}",
            model.index(),
            self.assets.model_path(model)
        );
        self.add_assets(|assets| {
            assets.remove_model(model);
            assets.note(note);
        });
    }

    /// Why `texture` isn't removed, as [`remove_texture`](Self::remove_texture) says, if it isn't:
    /// `None` for one a layer added or text named.
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

    /// Why the mesh of index `index` isn't removed, as [`remove_mesh`](Self::remove_mesh) says, if
    /// it isn't.
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

    /// Removes `texture`, unless it's removed already, as [`remove_texture`](Self::remove_texture)
    /// says, noting it among what the layers added if `noted`, by its index and its path, as a
    /// texture a layer removes is; a model's image goes with its model's note.
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
            if let Some(hot_reload) = &mut self.hot_reload {
                hot_reload.unwatch(HotAsset::Texture(texture));
            }
        }
        let path = self
            .assets
            .path(texture)
            .expect("a texture removed is named");
        let note = noted.then(|| format!("removed texture {} {path}", texture.index()));
        self.add_assets(|assets| {
            assets.remove_texture(texture);
            if let Some(note) = note {
                assets.note(note);
            }
        });
    }

    /// Removes the mesh of index `index`, unless it's removed already, as
    /// [`remove_mesh`](Self::remove_mesh) says, noting it among what the layers added, by its
    /// index.
    pub(super) fn drop_mesh(&mut self, index: u32) {
        if self.meshes.remove_index(index) {
            self.add_assets(|assets| assets.note(format!("removed mesh {index}")));
        }
    }

    /// Loads the TrueType or OpenType font at `path`, a `.ttf` or `.otf` file, for
    /// [`draw_text`](Self::draw_text) to draw text in from then on, through the handle, baking its
    /// field as it loads: a texture of data, a multi-channel signed distance field of each glyph of
    /// the characters Windows-1252 prints that the font has, and of its `.notdef`, which draws in
    /// place of the characters the font or the field lacks but whitespace and control characters.
    /// The outlines are its default instance's, a variable font's at its axes' defaults, unhinted,
    /// since the field draws them at any size. `name` names
    /// the font in errors and its field in GPU captures, as `<name> field`; frames stream the field
    /// to the GPU, and its text draws nothing until it's there, as its glyphs bake and the field
    /// grows. Its add logs `asset.font_loaded`, with how long it took. A file added again, by
    /// whatever form of its path, gives the font it gave first, without baking it again, as does an
    /// asset's file renamed or moved since, by the ID that names it. Scenes, snapshots, recordings
    /// and the tool API name it by its asset ID, where its file has one, and otherwise by its path,
    /// as a texture is. In a development run that serves the tool API, it bakes again as its file
    /// changes, off the main thread.
    ///
    /// Fails with `engine.bad_font` if the file can't be read, isn't a regular file, is more than
    /// 256 MiB, isn't a TrueType or OpenType font with outlines, as a font of bitmaps alone isn't,
    /// or has glyphs that don't fit a field of 8,192 texels square, alone or together; and with
    /// `renderer.too_many_textures` if the renderer hasn't room for its field.
    pub fn add_font(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Font> {
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
    /// [`World::save_scene`] saves one, for [`Prefab::spawn`] and the [`PrefabRoot`]s that name it
    /// to spawn instances of from then on, through the handle. `name` names it in errors. A file
    /// added again, by whatever form of its path, gives the prefab it gave first, as does an
    /// asset's file renamed or moved since, by the ID that names it. It's named by its asset ID,
    /// where its file has one, or else by its path, as a texture is. Its text is read as it loads,
    /// and the textures, models, fonts and prefabs it names that none loaded has are loaded after
    /// it, as a scene's are; its entities' components are made as its instances spawn, of the types
    /// the world has registered, as a scene's are, which refuses a text that doesn't load. Its load
    /// logs `asset.prefab_loaded`, with how long it took, the loads of what it names among it. In a
    /// development run that serves the tool API, it's read again as its file changes, and every
    /// instance takes what changed, as [`PrefabRoot`] says, unless the run is recorded, in a
    /// lockstep session or follows another.
    ///
    /// Fails with `engine.bad_prefab` if the file can't be read, isn't a regular file, is more than
    /// 16 MiB, isn't UTF-8, or isn't a scene's RON as far as its syntax and its entities' IDs go.
    pub fn add_prefab(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Prefab> {
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

    /// Loads the UI at `path`, a `.ui` file holding a tree of named nodes, as ARCHITECTURE.md's
    /// Events and layers describes, or the UI file under the asset root whose asset ID `path` is,
    /// as text names one; and adds a UI of it, which the layer drives through the handle, as
    /// [`GameUi`] says, with its own focus and what code sets of its nodes, apart from any other UI
    /// of the file, which is read once, by whatever form of its path. `name` names it in errors,
    /// the log and the tool API's `ui`. Its file is named by its asset ID, where it has one, or
    /// else by its path, as a texture's is. The fonts and textures its nodes name, by their IDs or
    /// paths, are loaded with it, as a scene's are, and its file's load logs `asset.ui_loaded`, with
    /// how long it took, theirs among it. In a development run that serves the tool API,
    /// its file is read again as it changes, every UI of it taking its nodes, each node keeping
    /// what code set of it by its name, and the focus staying on the button of its name, logging
    /// `UI reloaded`; a change that doesn't read warns with its code, the UI staying as it was.
    ///
    /// Fails with `engine.bad_ui` if the file can't be read, isn't a regular file, is more than
    /// 16 MiB or isn't UTF-8; with `ui.syntax`, `ui.bad_name`, `ui.duplicate_name`,
    /// `ui.bad_field`, `ui.unnamed_button`, `ui.bad_value` or `ui.no_node` for a file that isn't a
    /// UI's, as `game_ui::file::parse` says, saying the line and column where it's refused;
    /// as finding an ID's file fails, as with `asset.unknown_id`; with `asset.no_font` or
    /// `asset.no_texture` for a font or a texture it names that doesn't load, saying why; and with
    /// `renderer.too_many_textures` if the renderer hasn't room for the texture rounded corners
    /// are cut from, which the first UI adds.
    pub fn add_ui(&mut self, name: &str, path: impl AsRef<Path>) -> Result<GameUi> {
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

    /// Loads each font and texture the nodes of `document`, a UI's, name that none loaded has, as
    /// [`load_named`](Self::load_named) loads what a scene names, and gives each node what it
    /// names, and an image its texture's size, where a layer's add or a scene loaded it from its
    /// file.
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

    /// Records that a layer added an asset of `kind`, as `what` says it: its name, and the types
    /// that a mesh's, a shader's or a model's draws hold the bytes of, as [`Assets`] keeps it.
    pub(super) fn added(&mut self, kind: &str, what: &str) {
        let (meshes, shaders) = (self.meshes.count(), self.shaders.count());
        let added = format!("{kind} {meshes} {shaders} {} {what}", self.textures.count());
        self.add_assets(|assets| assets.note(added));
    }

    /// Has `path` name `texture`, which another path to its file added before, as [`Assets::alias`]
    /// does, unless it names a texture already that isn't removed.
    pub(super) fn alias(&mut self, texture: Texture, path: &Arc<str>) {
        if self.assets.live_texture(path).is_none() {
            self.add_assets(|assets| assets.alias(texture, path.clone()));
        }
    }

    /// Has `add` change the assets the layers added, which the world, and the world as edited while
    /// a copy plays, share from then on: the context's, so that a world that took the world's place
    /// doesn't lose them, and copied only where a copy of the world shares them.
    pub(super) fn add_assets<R>(&mut self, add: impl FnOnce(&mut Assets) -> R) -> R {
        // The worlds let go of their shares for the change.
        self.world.share_assets(&Arc::default());
        #[cfg(feature = "dev")]
        if let Some(edited) = &mut self.edited {
            edited.world.share_assets(&Arc::default());
        }
        let added = add(Arc::make_mut(&mut self.assets));
        self.world.share_assets(&self.assets);
        #[cfg(feature = "dev")]
        if let Some(edited) = &mut self.edited {
            edited.world.share_assets(&self.assets);
        }
        added
    }

    /// Has the world share the assets the layers added again, if a layer put another world in its
    /// place, as it loaded, attached or ticked, as [`World::share_assets`] says.
    pub(crate) fn share_assets(&mut self) {
        self.world.share_assets(&self.assets);
    }

    /// The meshes the layers added, which the tool API reads their extents from.
    #[cfg(feature = "dev")]
    pub(crate) fn meshes(&self) -> &Meshes {
        &self.meshes
    }
}

/// Logs `asset.texture_loaded` at the debug level, as a game may add many: texture `named`, `size`
/// pixels across and down, added from its file in the time since `started`. Its pixels decode on
/// the loader's threads from then on, as [`Loader`] logs; where the loader decodes inline, as a
/// test's or a replay's context's does, they decoded within that time.
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
/// through `loader`, which decodes it, as a sheet of cells `cell` pixels across and down, if they
/// fit it, as [`fits`] checks; one that doesn't takes no texture.
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

/// Checks that cells `cell` pixels across and down fit sprite sheet `name`'s image, from `path`,
/// `size` pixels across and down.
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
/// draw holds the bytes of, so that a draw kept in one comes back only where they're the same.
fn layout<T>() -> String {
    format!("{} {}", type_name::<T>(), size_of::<T>())
}

/// The types a model's draws hold the bytes of, as its `asset` line says them after its path.
pub(crate) fn model_types() -> String {
    format!("{} {}", layout::<ModelVertex>(), layout::<ModelParams>())
}
