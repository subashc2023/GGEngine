//! Models: glTF files' scenes, whose meshes and textures a context adds once, and whose nodes a
//! world spawns under each entity whose [`ModelRoot`] names the model, as many times as it likes;
//! and, in development builds, read again as their files change, their meshes and textures replaced
//! under the same handles.

use std::path::Path;
use std::sync::Arc;

use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
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
/// engine loaded: its index among its context's, which a [`ModelRoot`] holds, and which scenes,
/// recordings, snapshots and the tool API name as they name a texture, by its asset ID, or else its
/// path. [`spawn`](Self::spawn) places it in a world.
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

    /// Spawns an entity at `at` with a [`ModelRoot`] of the model, and under it the model's nodes,
    /// as `ModelRoot` says, and returns it. Its ID is drawn from the world's stream, and its nodes'
    /// are mixed from it, so that a world seeded alike spawns the model alike.
    ///
    /// # Panics
    ///
    /// If `world` doesn't share the assets of the context that added the model, as a world a layer
    /// makes doesn't until it's the context's.
    pub fn spawn(self, world: &mut World, at: Transform) -> Entity {
        world.spawn_model(self, at)
    }
}

/// The entity a model's nodes are spawned under: the world spawns them as the entity comes with it,
/// as [`Model::spawn`], a scene loaded or a tool's create makes it, and again as it's given another
/// model; and takes them away with it. Each node of the model's default scene is an entity under it,
/// with its transform in its parent's space, and a [`ModelNode`] saying which part of the model it
/// is; one with a mesh draws it, as a child for each primitive of a mesh with several, since an
/// entity draws one. The nodes are the model's, which a scene names the root's alone of, so that
/// scenes don't save them, tools don't change them, and a pick in the view gives the root. Code may
/// change them as the game runs, as snapshots keep them, until the model's nodes change: in a
/// development run that serves the tool API, neither recorded nor in a lockstep session, as the
/// model's file changes, each node still the model's keeps what code gave it, taking what changed of
/// the model's, and the rest come and go.
///
/// The world keeps it, as it keeps [`Parent`](crate::Parent): a scene, the tool API's operations
/// and [`Model::spawn`] give it, and code reads it but can't change it, since the nodes would then
/// be another model's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct ModelRoot(pub Model);

// And not `Writable`, so that only the world changes it, spawning its nodes as it does.
impl Component for ModelRoot {}

/// A node of the model that its root's [`ModelRoot`] names, or a primitive of a node's mesh: which
/// part of the model it is, in the order of the model's nodes, each after its parent. The world
/// keeps it, as it keeps the root's, and tools change no entity that has it, as `ModelRoot` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct ModelNode(pub u32);

// And not `Writable`, so that only the world changes it.
impl Component for ModelNode {}

/// What a model's root spawns under it, as [`Assets`](crate::assets::Assets) keeps it for the
/// worlds: the scene's nodes, and a part for each primitive of a mesh with more than one, in
/// pre-order, each after its parent; each primitive's draw, with its material; each
/// primitive's surface, which physics collides with as a
/// [`MeshCollider3D`](crate::MeshCollider3D) says, shared by every copy, so that physics tells the
/// model read again, as its file changes, from the one it made a collider of; the skins that move
/// skinned parts' vertices; and the clips an [`Animator`](crate::Animator) plays on its parts,
/// shared too, so that each tick reaches them without copying them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Nodes {
    pub(crate) parts: Vec<Part>,
    pub(crate) draws: Vec<MeshDraw>,
    pub(crate) surfaces: Arc<Vec<Surface>>,
    pub(crate) skins: Arc<Vec<Skin>>,
    pub(crate) clips: Arc<Vec<Clip>>,
}

/// A primitive's triangles as physics collides with them: level 0's, its whole mesh's, and its
/// vertices' positions as packed, which unpack as the renderer's do, so that a run from glTF and
/// one from its package, whose meshes the cooker optimized, give physics the same surface.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Surface {
    offset: Vec3,
    step: f32,
    positions: Vec<[u16; 3]>,
    /// Three for each triangle, of `positions`.
    indices: Vec<u32>,
}

impl Surface {
    /// The surface of `mesh`, which holds together, as the renderer checks it.
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
    /// The model's triangles, in its own space, as physics collides with them: each primitive's
    /// surface where its part puts it, a triangle whose corners aren't three apart, which covers
    /// nothing, or that another is too, left out; each turned to start at its least corner by
    /// their coordinates' bits, keeping the way round they go, and in the order of their corners'
    /// bits, of vertices in the order of their bits, none twice. So that the triangles depend on
    /// where they are alone, not on the order their mesh holds them in, which the cooker changes
    /// as it optimizes the mesh.
    pub(crate) fn triangles(&self) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut placed: Vec<Mat4> = Vec::with_capacity(self.parts.len());
        let mut corners: Vec<[Vec3; 3]> = Vec::new();
        for part in &self.parts {
            let own = part.transform.matrix();
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

/// `corners` as [`Nodes::triangles`] gives them: canonical vertices, and triangles of them. Each
/// corner goes by its coordinates' bits, made once and packed into one number in their order, so
/// that it's compared and found at a word's cost: ordering Sponza's 262,267 triangles by bits made
/// again at each comparison took 9.6 s in a debug build, at each Play's first tick, against 1.1 s
/// so. The vertices are found first, so that a triangle orders by its vertices' indices, which
/// are in the order of their bits.
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

/// What a model is made of on the renderer, which a reload replaces under the same handles, and a
/// removal removes: each primitive's mesh and material, each image's texture and the texels its
/// levels are made from, which say what it holds; how many primitives and images the model has
/// now, as a reload that drops some keeps their meshes, materials and textures for their places,
/// which a later one gives back; each skinned primitive's mesh, which each of its skinned parts'
/// entities draws a copy of, posed apart; and, in development builds, each primitive's material
/// as it was read, which a reload compares its own with.
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

/// The renderer's stores that a model adds to, and replaces in as it's read again.
pub(crate) struct Stores<'a> {
    pub(crate) meshes: &'a mut Meshes,
    pub(crate) shaders: &'a mut Shaders,
    pub(crate) textures: &'a mut Textures,
    pub(crate) materials: &'a mut Materials,
}

/// Adds what model `name`, as `read` gave it from `path`, draws, to `stores`: its images as
/// textures, through `loader`, which decodes them, each with mip levels as [`texels`] says, then
/// each primitive's mesh, and its material, which it's drawn with through the model shader, which
/// it adds for every alpha mode, so that a material changed later adds none. Gives what its root
/// spawns, and what it's made of, its images' textures in the model's order.
///
/// Fails, adding nothing, with `renderer.too_many_textures` if the textures haven't room for its
/// images, and as [`model_meshes`] does for a primitive whose mesh doesn't pack or hold together;
/// an image whose pixels don't decode warns as the loader decodes it, and its texture keeps its
/// stand-in.
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
    // Checked first, so that a model adds all of itself or nothing: what `read` gave can't fail to
    // add otherwise.
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

/// Each of `packed`'s meshes that's skinned, by its primitive, shared.
fn skinned(packed: &[(String, ModelMesh)]) -> Vec<Option<Arc<ModelMesh>>> {
    (packed.iter())
        .map(|(_, mesh)| (!mesh.skin.is_empty()).then(|| Arc::new(mesh.clone())))
        .collect()
}

/// What [`reload`] gives: what the model's root spawns, what changed of the model, and the textures
/// of the images added, with their places in the model.
#[cfg(feature = "dev")]
pub(crate) type Reread = (Nodes, Vec<&'static str>, Vec<(usize, Texture)>);

/// Puts what `imported`, model `name` read again, draws in place of what `made` drew, under the
/// same handles, so that every entity drawing it draws it once frames have streamed it: each
/// primitive's vertices and indices, and each image, with its mip levels made as its materials
/// have them. If `whole`, the model takes all of it: primitives and images past those it had are
/// added, or take back the meshes, materials and textures of their places, which those it has no
/// more keep, unused, for when they come back; its materials take what they're read as; and it
/// gives what its root spawns now, its nodes and its draws, with their materials, which adds no
/// shader, as [`add`] added them all. Otherwise it takes only those, and gives back `was`, what its
/// root spawned, which its nodes keep, their materials as they were. Gives too what changed of the
/// model, as "nodes" and "materials", the materials compared as they were read, and the textures
/// of the images added, with their places in the model, which no image of the model had before.
///
/// Fails, replacing nothing, with `asset.model_changed` unless `whole` if it has other numbers of
/// primitives or images, which a model's handles don't hold, and either way if an image holds
/// other than it held, as colors where it held a normal map, which its texture keeps; with
/// `renderer.too_many_textures` if the textures haven't room for the images added; and as
/// [`model_meshes`] does for a primitive whose mesh doesn't pack or hold together.
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
    // A texture keeps what it holds, so an image that holds something else now, as colors where it
    // held a normal map, takes one of its own, as the model's files are read afresh.
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
    // Read again from its files, as a development run does, each has one level.
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

/// Each primitive's draw, of its mesh with its material, as `made` holds them, as many as the
/// model's primitives now are, as `made`'s counts say.
fn draws(made: &Made, materials: &Materials, shaders: &mut Shaders) -> Vec<MeshDraw> {
    (made.meshes.iter().zip(&made.materials))
        .take(made.counts.0)
        .map(|(&mesh, &material)| MeshDraw::with_material(mesh, material, materials, shaders))
        .collect()
}

/// Each of `primitives`' meshes, model `name`'s, as [`model_mesh`] gives it, with the name the
/// renderer knows it by, checked as the renderer checks one it adds: so that a model adds, or
/// replaces, all of itself or nothing.
///
/// Fails with `engine.bad_model`, naming the model and the mesh, as `model_mesh` and
/// [`check_mesh`] do.
fn model_meshes(name: &str, primitives: &[Primitive]) -> Result<Vec<(String, ModelMesh)>> {
    (primitives.iter())
        .map(|primitive| {
            let mesh = model_mesh(name, primitive)?;
            check_mesh(name, primitive, &mesh)?;
            Ok((mesh_name(name, primitive), mesh))
        })
        .collect()
}

/// `primitive`'s mesh, model `name`'s, as the cooker made it, or else packed as glTF has it, of
/// one level, all its indices, which a development run draws whole.
///
/// Fails as [`pack`] does.
pub(crate) fn model_mesh(name: &str, primitive: &Primitive) -> Result<ModelMesh> {
    match &primitive.mesh {
        Some(mesh) => Ok(mesh.clone()),
        None => pack(name, primitive, Vec::new()),
    }
}

/// `primitive`'s vertices, model `name`'s, as glTF has them, packed, with its indices, of `levels`
/// among them, or of one level, all of them, for none: those its triangles use alone, in their
/// order, as the cooker keeps them, so that a run from glTF packs them on the grid a run from its
/// package does, which physics' surfaces need, rather than on one that vertices no triangle uses
/// stretch, as those of an accessor that several primitives share do.
///
/// Fails with `engine.bad_model`, naming the model and the mesh, for vertices that don't pack, as
/// [`ModelMesh::pack`] says: those that span more than packed vertices reach with what an f32
/// holds.
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

/// Which of `primitive`'s vertices its triangles use, in their order, and its indices of them; all
/// of them where they use each, or where it has no triangles, as a mesh of none packs as it did.
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

/// Checks `mesh`, `primitive`'s of model `name`, as the renderer checks one it adds, as
/// [`ModelMesh::check`] says.
///
/// Fails with `engine.bad_model`, naming the model and the mesh, for one that doesn't hold
/// together.
pub(crate) fn check_mesh(name: &str, primitive: &Primitive, mesh: &ModelMesh) -> Result<()> {
    let checked = mesh.check(&mesh_name(name, primitive));
    checked.map_err(|error| refused(name, &error))
}

/// The name the renderer knows `primitive`'s mesh, of model `name`, by.
fn mesh_name(name: &str, primitive: &Primitive) -> String {
    format!("{name} {}", primitive.name)
}

/// The `engine.bad_model` of model `name`, whose mesh the renderer refuses, as its `error` says,
/// which names the mesh.
fn refused(name: &str, error: &Error) -> Error {
    let message = format!("model {name:?} has a mesh the renderer can't draw: {error}");
    Error::new("engine.bad_model", message)
}

/// The vertices of `primitive`, as glTF has them, to pack.
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

/// The material `read` gives, sampling its images among `images`, the model's textures.
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

/// What image `image` of `primitives`' model holds, and so the texels its levels are made from: a
/// map of data, as the first texture to sample it says, its levels averaging its values; or
/// colors, weighted by alpha where a masked or blended material samples them as its base color,
/// since their alpha is then how much it covers, and plainly averaged where opaque ones alone do,
/// as glTF has them ignore its alpha, or where they're emitted. Reading the model left no image
/// sampled as two of those.
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
            translation: Vec3::new(2.0, 0.0, 0.0),
            scale: Vec3::new(-1.0, 1.0, 1.0),
            ..Transform::IDENTITY
        }
    }

    /// A material that samples `image` as its base color.
    fn colored(image: usize) -> gltf::Material {
        let mut material = gltf::Material::default();
        material.textures[Slot::Color as usize] = Some(gltf::Sampling {
            image,
            nearest: false,
            wrap: gg_renderer::Wrap::Repeat,
        });
        material
    }

    /// A model of a node turned, drawing a mesh of two primitives, the first masked; and a node
    /// moved and mirrored, drawing the second primitive again. Each primitive samples an image of
    /// its own.
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

    /// The renderer's stores, which a model adds to.
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

        /// How many meshes, textures, shaders and materials there are.
        fn counts(&self) -> (u32, u32, u32, u32) {
            (
                self.meshes.count(),
                self.textures.count(),
                self.shaders.count(),
                self.materials.count(),
            )
        }
    }

    /// A model of one node, of no texture, its primitive of `positions` and `indices`, at `at`.
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

    /// A level's floor: a square 20 units across in the XZ plane, at y 0, facing up, of two
    /// triangles.
    pub(crate) fn level() -> Imported {
        let corners = vec![
            [-10.0, 0.0, -10.0],
            [-10.0, 0.0, 10.0],
            [10.0, 0.0, 10.0],
            [10.0, 0.0, -10.0],
        ];
        surface(corners, vec![0, 1, 2, 0, 2, 3], Transform::IDENTITY)
    }

    /// A model's triangles are where its nodes put its primitives' surfaces, whatever order its
    /// mesh holds them in: a mesh as the cooker leaves it, its vertices merged and reordered, its
    /// triangles reordered and turned, a coarser level after them, and without a vertex no triangle
    /// used, which the run from glTF packs without too, gives the same triangles; a triangle
    /// covering nothing, or another's twin, is left out.
    #[test]
    fn a_models_triangles_are_where_its_surfaces_are_whatever_their_order() {
        let at = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(0.5),
            scale: Vec3::new(2.0, 1.0, 1.0),
        };
        // Off the grid that the vertex no triangle uses would stretch, so that they'd pack apart.
        let corners = [
            [0.0, 0.0, 0.0],
            [1.1, 0.0, 0.0],
            [1.1, 0.0, 0.9],
            [0.0, 0.3, 0.9],
        ];
        // A vertex no triangle uses, far off, and a corner twice; a triangle covering nothing,
        // and one twice.
        let mut read = corners.to_vec();
        read.extend([[50.0, -50.0, 50.0], corners[2]]);
        let indices = vec![0, 1, 2, 0, 5, 3, 0, 0, 1, 2, 0, 1];
        let mut cx = Context::seeded(1);
        let (from_gltf, _) = added(&mut cx, "read", surface(read, indices, at));
        // As the cooker leaves it: the corners in another order, the triangles in another, each
        // turned, and a level of one triangle after them.
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
        // A package's primitive holds its mesh alone.
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
            let placed = matrix.transform_point3(Vec3::from(corner));
            let near = vertices.iter().any(|vertex| vertex.distance(placed) < 1e-4);
            assert!(near, "{placed} among {vertices:?}");
        }
    }

    /// `imported`, model `name`, added to `cx`'s world's assets, as a context adds one, and what
    /// it's made of.
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
            translation: Vec3::new(0.0, 0.0, -5.0),
            ..Transform::IDENTITY
        };
        let root = model.spawn(cx.world_mut(), at);
        model.spawn(again.world_mut(), at);
        // A world seeded alike spawns it alike.
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
        // A child at the node for each primitive of its mesh, since an entity draws one.
        let parts = <[Entity; 2]>::try_from(children(node)).expect("a part for each primitive");
        assert_eq!(parts.map(part), [Some(1), Some(2)]);
        for part in parts {
            assert_eq!(transform(part), Some(Transform::IDENTITY));
            assert_eq!(children(part), []);
        }
        let [first, second] = parts.map(|part| draw(part).expect("a draw"));
        assert_ne!(first, second);
        // The second primitive again, the same mesh, so that its draws batch.
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

    /// A model read again replaces its meshes and images under their handles, and gives its nodes
    /// and draws now, adding no shader, not even the engine's for a material's alpha mode that none
    /// had, and says which of its nodes and materials changed. Unless it's to take all of it, it
    /// gives the nodes it had, and refuses other numbers of primitives or images; otherwise it
    /// adds the primitives and images past those it had.
    #[cfg(feature = "dev")]
    #[test]
    fn a_model_read_again_replaces_its_meshes_and_images_and_says_what_else_changed() {
        let mut owned = Owned::default();
        let (path, loader) = (Path::new("m.glb"), &mut Loader::inline());
        let added = add("m", path, imported(), owned.stores(), loader);
        let (was, mut made) = added.expect("a model");
        let counts = owned.counts();
        // Each read's, with how many meshes, textures, shaders and materials there are after it.
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

    /// An image's texture holds what the first texture to sample it samples: colors, whose levels
    /// are weighted by alpha where a masked or blended material samples them as its base color,
    /// or a map, as its slot says.
    #[test]
    fn images_hold_what_their_materials_sample_them_as() {
        let Imported { mut primitives, .. } = imported();
        let colors = |mipmaps| Texels::Color(mipmaps);
        assert_eq!(texels(&primitives, 0), colors(Mipmaps::AlphaWeighted));
        assert_eq!(texels(&primitives, 1), colors(Mipmaps::Averaged));
        primitives[1].material.alpha = AlphaMode::Blend;
        assert_eq!(texels(&primitives, 1), colors(Mipmaps::AlphaWeighted));
        // Emitted, its alpha covers nothing.
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

    /// A primitive's material goes into the table as it's read: its factors, and each texture as
    /// the model's image's, or white without one, but its normal map, which it hasn't.
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

    /// A model whose second primitive's vertices span more than packed vertices reach with what an
    /// f32 holds, -f32::MAX beside 0, is refused, naming it and its mesh, rather than panicking,
    /// adding nothing, not even its first primitive's mesh or its images; and, in development
    /// builds, read again so, it replaces nothing.
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
