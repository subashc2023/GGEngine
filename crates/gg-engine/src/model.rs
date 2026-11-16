//! Models: glTF files' scenes. A context adds their meshes and textures once; a world spawns their
//! nodes under each entity whose [`ModelRoot`] names the model, as often as it likes. Development
//! builds read them again as their files change, replacing meshes and textures under the same
//! handles.

use std::path::Path;
use std::sync::Arc;

use gg_core::math::{DVec3, Mat4, Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{
    AlphaMode, Material, MaterialId, Materials, Mesh, MeshDraw, Meshes, Mipmaps, ModelMesh,
    ModelVertex, PackedVertex, Sampled, Shaders, SkinVertex, Texels, Texture, Textures,
};

use crate::animation::{Clip, Skin};
use crate::gltf::{self, Imported, Part, Primitive, Slot};
use crate::loader::Loader;
use crate::{Component, Entity, Reflect, Transform, World};

/// A model that [`Context::add_model`](crate::Context::add_model) added, or that text named and the
/// engine loaded: its index among its context's, held by a [`ModelRoot`]. Scenes, recordings,
/// snapshots and the tool API name it as they name a texture: by asset ID, else path.
/// [`spawn`](Self::spawn) places it in a world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Model(u32);

impl Model {
    /// The model that's `index`th among its context's.
    pub(crate) fn new(index: usize) -> Self {
        Self(u32::try_from(index).expect("fewer than 2^32 models"))
    }

    /// Its index among its context's models.
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    /// Spawns an entity at `at` with a [`ModelRoot`] of the model and the model's nodes under it
    /// (see `ModelRoot`), and returns it. Its ID is drawn from the world's stream and its nodes'
    /// are mixed from it, so a world seeded alike spawns the model alike.
    ///
    /// # Panics
    ///
    /// If `world` doesn't share the assets of the context that added the model (a world a layer
    /// makes doesn't until it's the context's).
    pub fn spawn(self, world: &mut World, at: Transform) -> Entity {
        world.spawn_model(self, at)
    }
}

/// The entity a model's nodes are spawned under. The world spawns them with the entity
/// ([`Model::spawn`], a loaded scene, a tool's create) and again when it's given another model,
/// and removes them with it.
///
/// Each node of the default scene is an entity under it, with its transform in its parent's space,
/// a [`ModelNode`] and its glTF node's name ([`World::name`]). A node with a mesh draws it, as a
/// child per primitive for several (an entity draws one).
///
/// The nodes are the model's: scenes don't save them, tools don't change them, and a pick in the
/// view gives the root or a node within it (`pick`'s `within`). Code may change them as the game
/// runs, and snapshots keep them, until the model's nodes change. In a development run serving
/// the tool API (neither recorded nor lockstep), when the file changes, nodes still the model's
/// keep what code gave them and take what changed of the model's; the rest come and go.
///
/// The world keeps it, like [`Parent`](crate::Parent). Code reads it but can't change it: the
/// nodes would be another model's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct ModelRoot(pub Model);

// Not `Writable`: only the world changes it, spawning its nodes as it does.
impl Component for ModelRoot {}

/// Which part of the model its root's [`ModelRoot`] names this entity is: a node or a primitive of
/// a node's mesh, by index in the model's node order (each after its parent). Kept by the world;
/// tools change no entity that has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct ModelNode(pub u32);

// Not `Writable`: only the world changes it.
impl Component for ModelNode {}

/// What a model's root spawns under it, as [`Assets`](crate::assets::Assets) keeps it for the
/// worlds:
///
/// - the scene's nodes, plus a part per primitive of a mesh with several, in pre-order;
/// - each primitive's draw, with its material;
/// - each primitive's surface, which physics collides with
///   ([`MeshCollider3D`](crate::MeshCollider3D)), shared by every copy so physics tells a model
///   read again from the one it made a collider of;
/// - the skins that move skinned parts' vertices;
/// - the clips an [`Animator`](crate::Animator) plays, shared so ticks needn't copy them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Nodes {
    pub(crate) parts: Vec<Part>,
    pub(crate) draws: Vec<MeshDraw>,
    pub(crate) surfaces: Arc<Vec<Surface>>,
    pub(crate) skins: Arc<Vec<Skin>>,
    pub(crate) clips: Arc<Vec<Clip>>,
}

/// A primitive's triangles as physics collides with them: level 0's (its whole mesh), with
/// positions as packed and unpacked as the renderer does, so a run from glTF and one from its
/// package (optimized by the cooker) give physics the same surface.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Surface {
    offset: Vec3,
    step: f32,
    positions: Vec<[u16; 3]>,
    /// Three for each triangle, of `positions`.
    indices: Vec<u32>,
}

impl Surface {
    /// The surface of `mesh`, which holds together as the renderer checks.
    fn of(mesh: &ModelMesh) -> Self {
        let whole = mesh.lods.levels.first().map_or(0..0, |level| {
            level.start as usize..(level.start + level.count) as usize
        });
        Self {
            offset: mesh.packing.offset,
            step: mesh.packing.step,
            positions: mesh.vertices.iter().map(|vertex| vertex.position).collect(),
            indices: mesh.indices[whole].to_vec(),
        }
    }

    /// Where vertex `index` is, as [`ModelMesh::positions`] unpacks it.
    fn position(&self, index: u32) -> Vec3 {
        let packed = self.positions[index as usize];
        self.offset + Vec3::from_array(packed.map(f32::from)) * self.step
    }
}

impl Nodes {
    /// The model's triangles in its own space, as physics collides with them: each primitive's
    /// surface where its part puts it. Triangles covering nothing (corners not three apart) or
    /// repeating another are left out. Each starts at its least corner by coordinate bits, keeping
    /// its winding; triangles and vertices are ordered by bits, none twice. So they depend on
    /// where they are alone, not on their mesh's order (the cooker changes it).
    pub(crate) fn triangles(&self) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut placed: Vec<Mat4> = Vec::with_capacity(self.parts.len());
        let mut corners: Vec<[Vec3; 3]> = Vec::new();
        for part in &self.parts {
            // A model's parts are near its root, where `f32` holds.
            let own = part.transform.matrix_from(DVec3::ZERO);
            let matrix = part.parent.map_or(own, |parent| placed[parent] * own);
            placed.push(matrix);
            let surface = part
                .primitive
                .and_then(|primitive| self.surfaces.get(primitive));
            let Some(surface) = surface else {
                continue;
            };
            let at = |index: u32| matrix.transform_point3(surface.position(index));
            for &triangle in surface.indices.as_chunks::<3>().0 {
                corners.push(triangle.map(at));
            }
        }
        canonical(corners)
    }
}

/// `corners` as [`Nodes::triangles`] gives them. Each corner goes by its coordinates' bits, packed
/// once into one number so comparing costs a word: Sponza's 262,267 triangles took 9.6 s in a
/// debug build at Play's first tick with bits remade per comparison, 1.1 s so. Vertices are found
/// first, so triangles order by vertex index, which follows bits.
fn canonical(corners: Vec<[Vec3; 3]>) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let key = |point: Vec3| {
        let [x, y, z] = point.to_array().map(|at| u128::from(at.to_bits()));
        x << 64 | y << 32 | z
    };
    let mut keyed: Vec<[u128; 3]> = corners.iter().map(|triangle| triangle.map(key)).collect();
    keyed.retain(|&[a, b, c]| a != b && b != c && c != a);
    let mut keys: Vec<u128> = keyed.iter().flatten().copied().collect();
    keys.sort_unstable();
    keys.dedup();
    let index = |key: u128| {
        let found = keys.binary_search(&key);
        u32::try_from(found.expect("each corner among the vertices")).expect("fewer than 2^32")
    };
    let mut indices: Vec<[u32; 3]> = keyed
        .iter()
        .map(|triangle| {
            let mut triangle = triangle.map(index);
            let least = (0..3).min_by_key(|&corner| triangle[corner]);
            triangle.rotate_left(least.expect("three corners"));
            triangle
        })
        .collect();
    indices.sort_unstable();
    indices.dedup();
    let unpack = |key: u128| {
        let [x, y, z] = [64, 32, 0].map(|shift| f32::from_bits((key >> shift) as u32));
        Vec3::new(x, y, z)
    };
    (keys.into_iter().map(unpack).collect(), indices)
}

/// What a model is made of on the renderer; a reload replaces it under the same handles, a removal
/// removes it: each primitive's mesh and material; each image's texture and the texels its levels
/// are made from (what it holds); the counts of primitives and images now (a reload that drops
/// some keeps their meshes, materials and textures for a later one to give back); each skinned
/// primitive's mesh, which skinned parts draw copies of; in development builds, each material as
/// read, for a reload to compare with.
#[derive(Clone, Debug, Default)]
pub(crate) struct Made {
    pub(crate) meshes: Vec<Mesh<PackedVertex>>,
    /// By primitive: its mesh, if it's skinned.
    pub(crate) skinned: Vec<Option<Arc<ModelMesh>>>,
    pub(crate) materials: Vec<MaterialId>,
    pub(crate) images: Vec<(Texture, Texels)>,
    counts: (usize, usize),
    #[cfg(feature = "dev")]
    read: Vec<gltf::Material>,
}

/// The renderer's stores a model adds to, and replaces in when read again.
pub(crate) struct Stores<'a> {
    pub(crate) meshes: &'a mut Meshes,
    pub(crate) shaders: &'a mut Shaders,
    pub(crate) textures: &'a mut Textures,
    pub(crate) materials: &'a mut Materials,
}

/// Adds what model `name` (`imported`, from `path`) draws to `stores`: images as textures through
/// `loader`, with mip levels per [`texels`], then each primitive's mesh and material, drawn
/// through the model shader (added for every alpha mode, so a later material change adds none).
/// Gives what its root spawns and what it's made of.
///
/// Fails, adding nothing, with `renderer.too_many_textures` if the textures lack room for its
/// images, and as [`model_meshes`] does. An image whose pixels don't decode warns as the loader
/// decodes it; its texture keeps its stand-in.
pub(crate) fn add(
    name: &str,
    path: &Path,
    imported: Imported,
    stores: Stores<'_>,
    loader: &mut Loader,
) -> Result<(Nodes, Made)> {
    let Imported {
        parts,
        primitives,
        images,
        skins,
        clips,
        ..
    } = imported;
    let Stores {
        meshes,
        shaders,
        textures,
        materials,
    } = stores;
    // Checked first, so a model adds all of itself or nothing; nothing else can fail.
    room(name, images.len(), textures)?;
    let packed = model_meshes(name, &primitives)?;
    let surfaces = Arc::new(packed.iter().map(|(_, mesh)| Surface::of(mesh)).collect());
    let skinned = skinned(&packed);
    shaders.add_model_shaders();
    let images: Vec<_> = (images.into_iter().enumerate())
        .map(|(i, image)| {
            let texels = texels(&primitives, i);
            let name = format!("{name} image {i}");
            let added = loader.load(textures, &name, path, image, texels);
            let texture = added.expect("an image read fits a texture, and there's room for it");
            (texture, texels)
        })
        .collect();
    let meshes: Vec<_> = (packed.into_iter())
        .map(|(name, mesh)| {
            let mesh = meshes.add_model(&name, &mesh);
            mesh.expect("a mesh checked as the renderer checks it")
        })
        .collect();
    let added = (primitives.iter())
        .map(|primitive| materials.add(&material(&primitive.material, &images)))
        .collect();
    let made = Made {
        counts: (meshes.len(), images.len()),
        meshes,
        skinned,
        materials: added,
        images,
        #[cfg(feature = "dev")]
        read: primitives
            .iter()
            .map(|primitive| primitive.material)
            .collect(),
    };
    let draws = draws(&made, materials, shaders);
    let nodes = Nodes {
        parts,
        draws,
        surfaces,
        skins: Arc::new(skins),
        clips: Arc::new(clips),
    };
    Ok((nodes, made))
}

fn skinned(packed: &[(String, ModelMesh)]) -> Vec<Option<Arc<ModelMesh>>> {
    (packed.iter())
        .map(|(_, mesh)| (!mesh.skin.is_empty()).then(|| Arc::new(mesh.clone())))
        .collect()
}

/// What [`reload`] gives: what the root spawns, what changed, and the images' textures added, with
/// their places.
#[cfg(feature = "dev")]
pub(crate) type Reread = (Nodes, Vec<&'static str>, Vec<(usize, Texture)>);

/// Puts what `imported` (model `name` read again) draws in place of what `made` drew, under the
/// same handles, so entities drawing it draw the new once frames have streamed it: each
/// primitive's vertices and indices, and each image with its mip levels.
///
/// If `whole`, the model takes all of it:
/// - primitives and images past those it had are added, or take back the meshes, materials and
///   textures of their places (dropped ones keep theirs, unused, for a return);
/// - its materials take what they're read as;
/// - it gives what its root spawns now, nodes and draws with materials, adding no shader ([`add`]
///   added them all).
///
/// Otherwise it takes only the meshes and images and gives back `was`, what its root spawned,
/// which its nodes keep, materials as they were.
///
/// Gives too what changed, as "nodes" and "materials" (compared as read), and the textures of
/// images added, with their places in the model.
///
/// Fails, replacing nothing, with:
/// - `asset.model_changed`, unless `whole`, for other numbers of primitives or images (a model's
///   handles don't hold them);
/// - `asset.model_changed`, either way, if an image holds other than it held, as colors where it
///   held a normal map (its texture keeps what it held);
/// - `renderer.too_many_textures` if the textures lack room for the images added;
/// - what [`model_meshes`] does.
#[cfg(feature = "dev")]
pub(crate) fn reload(
    name: &str,
    imported: Imported,
    made: &mut Made,
    was: &Nodes,
    whole: bool,
    stores: Stores<'_>,
) -> Result<Reread> {
    let Imported {
        parts,
        primitives,
        images,
        skins,
        clips,
        ..
    } = imported;
    let Stores {
        meshes,
        shaders,
        textures,
        materials,
    } = stores;
    let counts = (primitives.len(), images.len());
    let had = made.counts;
    if counts != had && !whole {
        let count = |n: usize, what: &str| match n {
            1 => format!("1 {what}"),
            _ => format!("{n} {what}s"),
        };
        let message = format!(
            "model {name:?} has {} and {}, where it had {} and {}, which a run that's recorded or \
             in a lockstep session takes only as it starts",
            count(counts.0, "primitive"),
            count(counts.1, "image"),
            count(had.0, "primitive"),
            count(had.1, "image"),
        );
        return Err(Error::new("asset.model_changed", message));
    }
    // A texture keeps what it holds, so an image holding something else now (colors where it
    // held a normal map) takes a texture of its own when the file is read afresh.
    let held = |texels| match texels {
        Texels::Map(map) => Some(map),
        _ => None,
    };
    let unlike = (made.images.iter().enumerate())
        .take(images.len())
        .find(|&(i, &(_, was))| held(was) != held(texels(&primitives, i)));
    if let Some((i, &(_, was))) = unlike {
        let now = held(texels(&primitives, i));
        let message = format!(
            "model {name:?}'s image {i} holds {} where it held {}, which its texture keeps: the \
             model takes it as the run starts again",
            gltf::holds(now),
            gltf::holds(held(was))
        );
        return Err(Error::new("asset.model_changed", message));
    }
    room(name, counts.1.saturating_sub(made.images.len()), textures)?;
    // Read again from files, as a development run does, each has one level.
    let packed = model_meshes(name, &primitives)?;
    let surfaces = Arc::new(packed.iter().map(|(_, mesh)| Surface::of(mesh)).collect());
    let skinned = skinned(&packed);
    let read = primitives.iter().map(|primitive| primitive.material);
    let changed = [
        (parts != was.parts, "nodes"),
        (!read.eq(made.read.iter().copied()), "materials"),
    ];
    let changed = changed.into_iter().filter(|&(changed, _)| changed);
    let changed = changed.map(|(_, what)| what).collect();
    let mut added = Vec::new();
    for (i, image) in images.into_iter().enumerate() {
        let name = format!("{name} image {i}");
        let made_from = texels(&primitives, i);
        let texels = image.texels(made_from);
        match made.images.get_mut(i) {
            Some((texture, was)) => {
                let replaced = textures.replace(&name, *texture, image.size, image.pixels, texels);
                replaced.expect("a decoded image fits a texture of what it held");
                *was = made_from;
            }
            None => {
                let texture = textures.add(&name, image.size, image.pixels, texels);
                let texture = texture.expect("a decoded image fits a texture, and there's room");
                made.images.push((texture, made_from));
                added.push((i, texture));
            }
        }
    }
    for (i, mesh) in skinned.into_iter().enumerate() {
        match made.skinned.get_mut(i) {
            Some(was) => *was = mesh,
            None => made.skinned.push(mesh),
        }
    }
    for (i, (name, mesh)) in packed.into_iter().enumerate() {
        match made.meshes.get(i) {
            Some(&made_mesh) => {
                let replaced = meshes.replace_model(&name, made_mesh, &mesh);
                replaced.expect("a mesh checked as the renderer checks it");
            }
            None => {
                let added = meshes.add_model(&name, &mesh);
                made.meshes
                    .push(added.expect("a mesh checked as the renderer checks it"));
            }
        }
    }
    if !whole {
        return Ok((was.clone(), changed, added));
    }
    for (i, primitive) in primitives.iter().enumerate() {
        let material = material(&primitive.material, &made.images);
        match made.materials.get(i) {
            Some(&id) => materials.replace(id, &material),
            None => made.materials.push(materials.add(&material)),
        }
    }
    made.counts = counts;
    made.read = primitives
        .iter()
        .map(|primitive| primitive.material)
        .collect();
    let draws = draws(made, materials, shaders);
    let nodes = Nodes {
        parts,
        draws,
        surfaces,
        skins: Arc::new(skins),
        clips: Arc::new(clips),
    };
    Ok((nodes, changed, added))
}

/// Fails with `renderer.too_many_textures` if `textures` hasn't room for `images` more of model
/// `name`'s images.
fn room(name: &str, images: usize, textures: &Textures) -> Result<()> {
    if images > textures.room() as usize {
        return Err(Error::new(
            "renderer.too_many_textures",
            format!(
                "model {name:?} has {images} images, but the renderer holds {} more textures",
                textures.room()
            ),
        ));
    }
    Ok(())
}

fn draws(made: &Made, materials: &Materials, shaders: &mut Shaders) -> Vec<MeshDraw> {
    (made.meshes.iter().zip(&made.materials))
        .take(made.counts.0)
        .map(|(&mesh, &material)| MeshDraw::with_material(mesh, material, materials, shaders))
        .collect()
}

/// Each of `primitives`' meshes ([`model_mesh`]) with the renderer's name for it, checked as it
/// checks one it adds, so a model adds or replaces all of itself or nothing. Fails with
/// `engine.bad_model` as `model_mesh` and [`check_mesh`] do.
fn model_meshes(name: &str, primitives: &[Primitive]) -> Result<Vec<(String, ModelMesh)>> {
    (primitives.iter())
        .map(|primitive| {
            let mesh = model_mesh(name, primitive)?;
            check_mesh(name, primitive, &mesh)?;
            Ok((mesh_name(name, primitive), mesh))
        })
        .collect()
}

/// `primitive`'s mesh as the cooker made it, else packed from glTF as one level of all its
/// indices (a development run draws it whole).
///
/// Fails as [`pack`] does.
pub(crate) fn model_mesh(name: &str, primitive: &Primitive) -> Result<ModelMesh> {
    match &primitive.mesh {
        Some(mesh) => Ok(mesh.clone()),
        None => pack(name, primitive, Vec::new()),
    }
}

/// `primitive`'s vertices, model `name`'s, packed, with its indices: `levels` among them, or one
/// level of all for none. Only vertices its triangles use are kept, in their order, as the cooker
/// keeps them, so a run from glTF packs on the grid a run from its package does (physics'
/// surfaces need it), not one stretched by unused vertices (as an accessor several primitives
/// share has).
///
/// Fails with `engine.bad_model`, naming the model and the mesh, for vertices that don't pack
/// ([`ModelMesh::pack`]): those spanning more than packed vertices reach with an f32.
pub(crate) fn pack(
    name: &str,
    primitive: &Primitive,
    levels: Vec<gg_renderer::Lod>,
) -> Result<ModelMesh> {
    let mesh = mesh_name(name, primitive);
    let (kept, indices) = used(primitive);
    let all = vertices(primitive);
    let vertices: Vec<ModelVertex> = kept.iter().map(|&i| all[i]).collect();
    let packed = match primitive.reach {
        Some(reach) if !primitive.joints.is_empty() => {
            let skin = (kept.iter())
                .map(|&i| SkinVertex::new(primitive.joints[i], primitive.weights[i]))
                .collect();
            ModelMesh::pack_skinned(&mesh, &vertices, indices, levels, skin, reach)
        }
        _ => ModelMesh::pack(&mesh, &vertices, indices, levels),
    };
    packed.map_err(|error| refused(name, &error))
}

/// Which of `primitive`'s vertices its triangles use, in their order, and its indices of them. All
/// of them if the triangles use each or there are none (a mesh of none packs as it did).
fn used(primitive: &Primitive) -> (Vec<usize>, Vec<u32>) {
    let count = primitive.positions.len();
    let mut uses = vec![false; count];
    for &index in &primitive.indices {
        uses[index as usize] = true;
    }
    if primitive.indices.is_empty() || uses.iter().all(|&used| used) {
        return ((0..count).collect(), primitive.indices.clone());
    }
    let mut moved = vec![u32::MAX; count];
    let mut kept = Vec::new();
    for (i, (used, to)) in uses.into_iter().zip(&mut moved).enumerate() {
        if used {
            *to = u32::try_from(kept.len()).expect("fewer than 2^32 vertices");
            kept.push(i);
        }
    }
    let indices = (primitive.indices.iter())
        .map(|&index| moved[index as usize])
        .collect();
    (kept, indices)
}

/// Checks `mesh` as the renderer does ([`ModelMesh::check`]). Fails with `engine.bad_model`,
/// naming the model and the mesh, if it doesn't hold together.
pub(crate) fn check_mesh(name: &str, primitive: &Primitive, mesh: &ModelMesh) -> Result<()> {
    let checked = mesh.check(&mesh_name(name, primitive));
    checked.map_err(|error| refused(name, &error))
}

fn mesh_name(name: &str, primitive: &Primitive) -> String {
    format!("{name} {}", primitive.name)
}

/// `engine.bad_model` for model `name`; `error` names the mesh the renderer refuses.
fn refused(name: &str, error: &Error) -> Error {
    let message = format!("model {name:?} has a mesh the renderer can't draw: {error}");
    Error::new("engine.bad_model", message)
}

fn vertices(primitive: &Primitive) -> Vec<ModelVertex> {
    (primitive.positions.iter())
        .zip(&primitive.uvs)
        .zip(&primitive.colors)
        .zip(&primitive.normals)
        .zip(&primitive.tangents)
        .map(
            |((((&position, &uv), &color), &normal), &tangent)| ModelVertex {
                position: Vec3::from(position),
                uv: Vec2::from(uv),
                color: Vec4::from(color),
                normal: Vec3::from(normal),
                tangent: Vec4::from(tangent),
            },
        )
        .collect()
}

fn material(read: &gltf::Material, images: &[(Texture, Texels)]) -> Material {
    let sampled = |slot: Slot| {
        read.texture(slot).map(|sampling| Sampled {
            texture: images[sampling.image].0,
            nearest: sampling.nearest,
            wrap: sampling.wrap,
        })
    };
    let white = |slot| sampled(slot).unwrap_or(Sampled::WHITE);
    Material {
        color: Vec4::from(read.color),
        color_texture: white(Slot::Color),
        workflow: read.workflow,
        surface_texture: white(Slot::Surface),
        normal_texture: sampled(Slot::Normal),
        normal_scale: read.normal_scale,
        occlusion_texture: white(Slot::Occlusion),
        occlusion_strength: read.occlusion_strength,
        emissive: Vec3::from(read.emissive),
        emissive_texture: white(Slot::Emissive),
        alpha: read.alpha,
        double_sided: read.double_sided,
        unlit: read.unlit,
    }
}

/// What image `image` of `primitives`' model holds, so the texels its levels are made from: a map
/// (by the first slot sampling it), levels averaging its values; or colors, alpha-weighted where
/// a masked or blended material samples them as base color (alpha is then coverage), else plainly
/// averaged (opaque materials ignore alpha in glTF; emitted too). Reading the model left no image
/// sampled as two of these.
pub(crate) fn texels(primitives: &[Primitive], image: usize) -> Texels {
    let mut uses = (primitives.iter()).flat_map(|primitive| {
        let material = &primitive.material;
        let sampled = Slot::ALL.into_iter().filter(move |&slot| {
            material
                .texture(slot)
                .is_some_and(|texture| texture.image == image)
        });
        sampled.map(move |slot| (slot, material))
    });
    let covers = |(slot, material): (Slot, &gltf::Material)| {
        let blends = matches!(material.alpha, AlphaMode::Mask { .. } | AlphaMode::Blend);
        slot == Slot::Color && blends
    };
    let Some(first) = uses.next() else {
        return Texels::Color(Mipmaps::Averaged);
    };
    if let Some(map) = first.1.map(first.0) {
        return Texels::Map(map);
    }
    if covers(first) || uses.any(covers) {
        Texels::Color(Mipmaps::AlphaWeighted)
    } else {
        Texels::Color(Mipmaps::Averaged)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use gg_core::math::Quat;
    use gg_renderer::Workflow;

    use super::*;
    use crate::image::{Format, Image};
    use crate::{Context, gltf};

    fn turned() -> Transform {
        Transform {
            rotation: Quat::from_rotation_y(1.0),
            ..Transform::IDENTITY
        }
    }

    fn mirrored() -> Transform {
        Transform {
            translation: DVec3::new(2.0, 0.0, 0.0),
            scale: Vec3::new(-1.0, 1.0, 1.0),
            ..Transform::IDENTITY
        }
    }

    fn colored(image: usize) -> gltf::Material {
        let mut material = gltf::Material::default();
        material.textures[Slot::Color as usize] = Some(gltf::Sampling {
            image,
            nearest: false,
            wrap: gg_renderer::Wrap::Repeat,
        });
        material
    }

    /// A turned node drawing a mesh of two primitives (the first masked), and a moved, mirrored
    /// node drawing the second again. Each primitive samples its own image.
    pub(crate) fn imported() -> Imported {
        let triangle = |name: &str, material| gltf::Primitive {
            name: name.to_owned(),
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0; 2]; 3],
            colors: vec![[1.0; 4]; 3],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
            indices: vec![0, 1, 2],
            mesh: None,
            material,
            joints: Vec::new(),
            weights: Vec::new(),
            reach: None,
        };
        let masked = gltf::Material {
            alpha: AlphaMode::Mask { cutoff: 0.5 },
            ..colored(0)
        };
        let opaque = colored(1);
        let part = |transform, parent, primitive| gltf::Part {
            name: None,
            transform,
            parent,
            primitive,
            light: None,
            skin: None,
        };
        let image = Image {
            size: [1, 1],
            pixels: vec![255; 4],
            format: Format::Rgba8,
        };
        Imported {
            parts: vec![
                part(turned(), None, None),
                part(Transform::IDENTITY, Some(0), Some(0)),
                part(Transform::IDENTITY, Some(0), Some(1)),
                part(mirrored(), None, Some(1)),
            ],
            primitives: vec![triangle("pair/0", masked), triangle("pair/1", opaque)],
            images: vec![image.clone(), image],
            ignored: Vec::new(),
            skins: Vec::new(),
            clips: Vec::new(),
        }
    }

    #[derive(Default)]
    struct Owned {
        meshes: Meshes,
        shaders: Shaders,
        textures: Textures,
        materials: Materials,
    }

    impl Owned {
        fn stores(&mut self) -> Stores<'_> {
            Stores {
                meshes: &mut self.meshes,
                shaders: &mut self.shaders,
                textures: &mut self.textures,
                materials: &mut self.materials,
            }
        }

        fn counts(&self) -> (u32, u32, u32, u32) {
            (
                self.meshes.count(),
                self.textures.count(),
                self.shaders.count(),
                self.materials.count(),
            )
        }
    }

    /// A textureless model of one node at `at`, its primitive of `positions` and `indices`.
    pub(crate) fn surface(positions: Vec<[f32; 3]>, indices: Vec<u32>, at: Transform) -> Imported {
        let count = positions.len();
        let primitive = gltf::Primitive {
            name: "surface/0".to_owned(),
            positions,
            uvs: vec![[0.0; 2]; count],
            colors: vec![[1.0; 4]; count],
            normals: vec![[0.0, 1.0, 0.0]; count],
            tangents: vec![[1.0, 0.0, 0.0, 1.0]; count],
            indices,
            mesh: None,
            material: gltf::Material::default(),
            joints: Vec::new(),
            weights: Vec::new(),
            reach: None,
        };
        let part = gltf::Part {
            name: None,
            transform: at,
            parent: None,
            primitive: Some(0),
            light: None,
            skin: None,
        };
        Imported {
            parts: vec![part],
            primitives: vec![primitive],
            images: Vec::new(),
            ignored: Vec::new(),
            skins: Vec::new(),
            clips: Vec::new(),
        }
    }

    /// A level's floor: two triangles, a square 20 units across in the XZ plane at y 0, facing up.
    pub(crate) fn level() -> Imported {
        let corners = vec![
            [-10.0, 0.0, -10.0],
            [-10.0, 0.0, 10.0],
            [10.0, 0.0, 10.0],
            [10.0, 0.0, -10.0],
        ];
        surface(corners, vec![0, 1, 2, 0, 2, 3], Transform::IDENTITY)
    }

    /// A model's triangles are where its nodes put its surfaces, whatever order the mesh holds
    /// them in: a mesh as the cooker leaves it (vertices merged and reordered, triangles
    /// reordered and turned, a coarser level after, no unused vertex) gives the glTF run's
    /// triangles. Triangles covering nothing, or twins, are left out.
    #[test]
    fn a_models_triangles_are_where_its_surfaces_are_whatever_their_order() {
        let at = Transform {
            translation: DVec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(0.5),
            scale: Vec3::new(2.0, 1.0, 1.0),
        };
        // Off the grid the unused vertex would stretch, so they'd pack apart.
        let corners = [
            [0.0, 0.0, 0.0],
            [1.1, 0.0, 0.0],
            [1.1, 0.0, 0.9],
            [0.0, 0.3, 0.9],
        ];
        // An unused vertex far off, a corner twice, a triangle covering nothing, one twice.
        let mut read = corners.to_vec();
        read.extend([[50.0, -50.0, 50.0], corners[2]]);
        let indices = vec![0, 1, 2, 0, 5, 3, 0, 0, 1, 2, 0, 1];
        let mut cx = Context::seeded(1);
        let (from_gltf, _) = added(&mut cx, "read", surface(read, indices, at));
        // As the cooker leaves it: corners and triangles reordered, each turned, a one-triangle
        // level after them.
        let moved = vec![corners[3], corners[2], corners[1], corners[0]];
        let mut cooked = surface(moved, vec![2, 1, 3, 1, 0, 3, 3, 2, 1], at);
        let level = |start, count, error| gg_renderer::Lod {
            start,
            count,
            error,
        };
        let levels = vec![level(0, 6, 0.0), level(6, 3, 0.5)];
        let mesh = pack("cooked", &cooked.primitives[0], levels).expect("packs");
        let primitive = &mut cooked.primitives[0];
        primitive.mesh = Some(mesh);
        primitive.positions.clear();
        primitive.uvs.clear();
        primitive.colors.clear();
        primitive.normals.clear();
        primitive.tangents.clear();
        primitive.indices.clear();
        let (from_package, _) = added(&mut cx, "cooked", cooked);
        let assets = cx.world().assets();
        let triangles = |model| assets.nodes(model).expect("added").triangles();
        let (vertices, indices) = triangles(from_gltf);
        assert_eq!((vertices.len(), indices.len()), (4, 2));
        assert_eq!(triangles(from_package), (vertices.clone(), indices));
        // Each where its node puts it, within half a step of the grid it packs on.
        let matrix = at.matrix();
        for corner in corners {
            let placed = matrix.transform_point3(Vec3::from(corner).as_dvec3());
            let near = vertices
                .iter()
                .any(|vertex| vertex.distance(placed.as_vec3()) < 1e-4);
            assert!(near, "{placed} among {vertices:?}");
        }
    }

    /// `imported` added to `cx`'s assets as a context does, and what it's made of.
    pub(crate) fn added(cx: &mut Context, name: &str, imported: Imported) -> (Model, Made) {
        let mut owned = Owned::default();
        let (path, loader) = (Path::new("m.glb"), &mut Loader::inline());
        let added = add(name, path, imported, owned.stores(), loader);
        let (nodes, made) = added.expect("a model");
        let assets = cx.world_mut().assets_mut();
        (assets.add_model(name.into(), name.into(), nodes), made)
    }

    #[test]
    fn a_model_spawns_under_a_root_each_part_under_its_parent() {
        let (mut cx, mut again) = (Context::seeded(1), Context::seeded(1));
        let (model, _) = added(&mut cx, "m", imported());
        added(&mut again, "m", imported());
        let at = Transform {
            translation: DVec3::new(0.0, 0.0, -5.0),
            ..Transform::IDENTITY
        };
        let root = model.spawn(cx.world_mut(), at);
        model.spawn(again.world_mut(), at);
        assert_eq!(cx.world().hash(), again.world().hash());

        let world = cx.world();
        let children = |entity| -> Vec<Entity> { world.children(entity).expect("alive").collect() };
        let transform = |entity| world.get::<Transform>(entity).copied();
        let draw = |entity| world.get::<MeshDraw>(entity).copied();
        let part = |entity| world.get::<ModelNode>(entity).map(|&ModelNode(part)| part);
        assert_eq!((transform(root), draw(root)), (Some(at), None));
        assert_eq!(world.get::<ModelRoot>(root), Some(&ModelRoot(model)));
        let [node, mirror] = <[Entity; 2]>::try_from(children(root)).expect("two nodes");
        assert_eq!(
            (transform(node), draw(node), part(node)),
            (Some(turned()), None, Some(0))
        );
        // A child per primitive of its mesh: an entity draws one.
        let parts = <[Entity; 2]>::try_from(children(node)).expect("a part for each primitive");
        assert_eq!(parts.map(part), [Some(1), Some(2)]);
        for part in parts {
            assert_eq!(transform(part), Some(Transform::IDENTITY));
            assert_eq!(children(part), []);
        }
        let [first, second] = parts.map(|part| draw(part).expect("a draw"));
        assert_ne!(first, second);
        // The second primitive again, the same mesh, so its draws batch.
        assert_eq!(
            (transform(mirror), draw(mirror), part(mirror)),
            (Some(mirrored()), Some(second), Some(3))
        );
        assert_eq!(children(mirror), []);
        // Each node's ID mixed from the root's, as the world finds them by.
        let root_id = world.id(root).expect("alive");
        for (entity, i) in [(node, 0), (parts[0], 1), (parts[1], 2), (mirror, 3)] {
            let id = crate::ecs::node_id(root_id, i).expect("an ID");
            assert_eq!(world.id(entity), Some(id));
        }
    }

    /// A model read again replaces its meshes and images under their handles, gives its nodes and
    /// draws now, adds no shader (not even for an alpha mode none had), and says which nodes and
    /// materials changed. Unless it takes all, it gives the nodes it had and refuses other counts
    /// of primitives or images; else it adds those past what it had.
    #[cfg(feature = "dev")]
    #[test]
    fn a_model_read_again_replaces_its_meshes_and_images_and_says_what_else_changed() {
        let mut owned = Owned::default();
        let (path, loader) = (Path::new("m.glb"), &mut Loader::inline());
        let added = add("m", path, imported(), owned.stores(), loader);
        let (was, mut made) = added.expect("a model");
        let counts = owned.counts();
        let mut again = |imported, whole| {
            let reloaded = reload("m", imported, &mut made, &was, whole, owned.stores());
            (reloaded, owned.counts())
        };
        let (nodes, changed, added) = again(imported(), false).0.expect("read again");
        assert_eq!(
            (nodes, changed, added),
            (was.clone(), Vec::new(), Vec::new())
        );
        let mut moved = imported();
        moved.parts[0].transform = mirrored();
        let (nodes, changed, _) = again(moved.clone(), false).0.expect("read again");
        assert_eq!((&nodes, changed), (&was, vec!["nodes"]), "kept");
        let (nodes, changed, _) = again(moved, true).0.expect("read again");
        assert_eq!(changed, ["nodes"]);
        assert_eq!(nodes.parts[0].transform, mirrored(), "taken");
        let mut recolored = imported();
        recolored.primitives[1].material.color = [0.5; 4];
        // Masked and opaque before: the engine's blended model shader was added with them.
        recolored.primitives[1].material.alpha = AlphaMode::Blend;
        recolored.parts.pop();
        let (nodes, changed, _) = again(recolored, true).0.expect("read again");
        assert_eq!(changed, ["nodes", "materials"]);
        assert_ne!(nodes.draws[1], was.draws[1], "the draw of its material now");
        assert_eq!(nodes.parts.len(), 3);

        let mut fewer = imported();
        fewer.images.pop();
        fewer.primitives[1].material = gltf::Material::default();
        let error = again(fewer.clone(), false).0.expect_err("an image fewer");
        assert_eq!(error.code(), "asset.model_changed");
        assert!(
            error.to_string().contains("2 primitives and 1 image,"),
            "{error}"
        );
        let (taken, fewer_counted) = again(fewer, true);
        taken.expect("an image fewer, taken");
        assert_eq!(fewer_counted, counts, "nothing added, not even a shader");
        let (taken, counted) = again(imported(), true);
        let (nodes, _, added) = taken.expect("the image again");
        assert_eq!(added, [], "its texture taken back, under the names it had");
        assert_eq!(
            nodes.draws, was.draws,
            "drawing as it did, with the same textures and materials"
        );
        assert_eq!(counted, counts, "nothing added");

        // An image that's a normal map now, whose texture holds colors.
        let mut remapped = imported();
        let colors = remapped.primitives[1].material.textures[Slot::Color as usize].take();
        remapped.primitives[1].material.textures[Slot::Normal as usize] = colors;
        for whole in [false, true] {
            let error = again(remapped.clone(), whole).0.expect_err("a map now");
            assert_eq!(error.code(), "asset.model_changed");
            assert!(
                error
                    .to_string()
                    .contains("holds a normal map where it held colors"),
                "{error}"
            );
        }
    }

    /// An image's texture holds what the first slot sampling it says ([`texels`]).
    #[test]
    fn images_hold_what_their_materials_sample_them_as() {
        let Imported { mut primitives, .. } = imported();
        let colors = |mipmaps| Texels::Color(mipmaps);
        assert_eq!(texels(&primitives, 0), colors(Mipmaps::AlphaWeighted));
        assert_eq!(texels(&primitives, 1), colors(Mipmaps::Averaged));
        primitives[1].material.alpha = AlphaMode::Blend;
        assert_eq!(texels(&primitives, 1), colors(Mipmaps::AlphaWeighted));
        let textures = &mut primitives[1].material.textures;
        textures.swap(Slot::Color as usize, Slot::Emissive as usize);
        assert_eq!(texels(&primitives, 1), colors(Mipmaps::Averaged));
        for (slot, map) in [
            (Slot::Surface, gg_renderer::Map::Surface),
            (Slot::Occlusion, gg_renderer::Map::Surface),
            (Slot::Normal, gg_renderer::Map::Normal),
        ] {
            let textures = &mut primitives[1].material.textures;
            let sampled = textures.iter_mut().find_map(Option::take);
            textures[slot as usize] = sampled;
            assert_eq!(texels(&primitives, 1), Texels::Map(map), "{slot:?}");
        }
        // A specular-glossiness material's surface map holds colors.
        let material = &mut primitives[1].material;
        material.workflow = Workflow::SpecularGlossiness {
            specular: Vec3::ONE,
            glossiness: 1.0,
        };
        let sampled = material.textures.iter_mut().find_map(Option::take);
        material.textures[Slot::Surface as usize] = sampled;
        assert_eq!(texels(&primitives, 1), colors(Mipmaps::Averaged));
    }

    /// A primitive's material goes into the table as read: its factors, and each texture as the
    /// model's image, or white without one (but no normal map without one).
    #[test]
    fn a_material_takes_its_images_textures() {
        let workflow = Workflow::SpecularGlossiness {
            specular: Vec3::splat(0.25),
            glossiness: 0.5,
        };
        let read = gltf::Material {
            workflow,
            emissive: [1.0, 2.0, 3.0],
            unlit: true,
            ..colored(1)
        };
        let images = [
            (Texture::WHITE, Texels::Color(Mipmaps::Averaged)),
            (Texture::WHITE, Texels::Color(Mipmaps::Averaged)),
        ];
        let made = material(&read, &images);
        assert_eq!(made.color_texture, Sampled::of(images[1].0));
        assert_eq!(made.surface_texture, Sampled::WHITE);
        assert_eq!(made.normal_texture, None);
        assert_eq!(
            (made.workflow, made.emissive, made.unlit),
            (workflow, Vec3::new(1.0, 2.0, 3.0), true)
        );
    }

    #[test]
    fn a_model_adds_only_if_the_renderer_has_room_for_all_its_images() {
        let mut textures = Textures::default();
        while textures.room() > 2 {
            let added = textures.add("filler", [1, 1], &[0; 4], Mipmaps::None);
            added.expect("room for it");
        }
        let add = |textures: &mut Textures| {
            let (mut meshes, mut shaders) = (Meshes::default(), Shaders::default());
            let mut materials = Materials::default();
            let (path, loader) = (Path::new("m.glb"), &mut Loader::inline());
            let stores = Stores {
                meshes: &mut meshes,
                shaders: &mut shaders,
                textures,
                materials: &mut materials,
            };
            add("m", path, imported(), stores, loader)
        };
        add(&mut textures).expect("room for both images");
        assert_eq!(textures.room(), 0);
        let error = add(&mut textures).expect_err("no room");
        assert_eq!(error.code(), "renderer.too_many_textures");
        assert!(error.to_string().contains("\"m\" has 2 images"), "{error}");
    }

    /// A model whose second primitive spans more than packed vertices reach with an f32
    /// (-f32::MAX beside 0) is refused, naming it and its mesh, not panicking, and adds nothing,
    /// not even its first mesh or images. In development builds, read again so, it replaces
    /// nothing.
    #[test]
    fn a_model_whose_mesh_doesnt_pack_is_refused() {
        let mut far = imported();
        far.primitives[1].positions[0] = [-f32::MAX, 0.0, 0.0];
        let refused = |error: Error| {
            assert_eq!(error.code(), "engine.bad_model", "{error}");
            let message = error.to_string();
            assert!(message.contains("model \"m\""), "{message}");
            assert!(message.contains("mesh \"m pair/1\""), "{message}");
        };
        let mut owned = Owned::default();
        let counts = owned.counts();
        let (path, loader) = (Path::new("m.glb"), &mut Loader::inline());
        refused(add("m", path, far.clone(), owned.stores(), loader).expect_err("refused"));
        assert_eq!(owned.counts(), counts, "nothing added");
        #[cfg(feature = "dev")]
        {
            let added = add("m", path, imported(), owned.stores(), loader);
            let (was, mut made) = added.expect("a model");
            let counts = owned.counts();
            for whole in [false, true] {
                let reloaded = reload("m", far.clone(), &mut made, &was, whole, owned.stores());
                refused(reloaded.expect_err("refused"));
                assert_eq!(owned.counts(), counts, "nothing added");
            }
        }
    }
}
