//! Models: glTF files' scenes, whose meshes and textures a context adds once, and whose nodes a
//! world spawns under each entity whose [`ModelRoot`] names the model, as many times as it likes;
//! and, in development builds, read again as their files change, their meshes and textures replaced
//! under the same handles.

use std::path::Path;

use gg_core::math::{Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{
    AlphaMode, Lods, Material, Mesh, MeshDraw, Meshes, Mipmaps, ModelVertex, Shaders, Texture,
    Textures,
};

use crate::gltf::{self, Imported, Part, Primitive};
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
/// pre-order, each after its parent; and each primitive's draw, with its material.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Nodes {
    pub(crate) parts: Vec<Part>,
    pub(crate) draws: Vec<MeshDraw>,
}

/// What a model is made of on the renderer, which a reload replaces under the same handles, and a
/// removal removes: each primitive's mesh, each image's texture and how it got its mip levels, and,
/// in development builds, each primitive's material as it was read, which a reload compares its
/// own with, and how many primitives and images the model has now, as a reload that drops some
/// keeps their meshes and textures for their places, which a later one gives back.
#[derive(Clone, Debug, Default)]
pub(crate) struct Made {
    pub(crate) meshes: Vec<Mesh<ModelVertex>>,
    pub(crate) images: Vec<(Texture, Mipmaps)>,
    #[cfg(feature = "dev")]
    materials: Vec<gltf::Material>,
    #[cfg(feature = "dev")]
    counts: (usize, usize),
}

/// Adds what model `name`, as `read` gave it from `path`, draws: its images to `textures`, through
/// `loader`, which decodes them, each with mip levels, weighted by alpha where a masked or blended
/// material samples it, then its primitives to `meshes`, each drawn with its material through
/// `shaders`' model shader, which it adds for every alpha mode, so that a material changed later
/// adds none. Gives what its root spawns, and what it's made of, its images' textures in the
/// model's order.
///
/// Fails, adding nothing, with `renderer.too_many_textures` if `textures` hasn't room for its
/// images; an image whose pixels don't decode warns as the loader decodes it, and its texture keeps
/// its stand-in.
pub(crate) fn add(
    name: &str,
    path: &Path,
    imported: Imported,
    meshes: &mut Meshes,
    shaders: &mut Shaders,
    textures: &mut Textures,
    loader: &mut Loader,
) -> Result<(Nodes, Made)> {
    let Imported {
        parts,
        primitives,
        images,
        ..
    } = imported;
    // Checked first, so that a model adds all of itself or nothing: what `read` gave can't fail to
    // add otherwise.
    room(name, images.len(), textures)?;
    shaders.add_model_shaders();
    let images: Vec<_> = (images.into_iter().enumerate())
        .map(|(i, image)| {
            let mipmaps = mipmaps(&primitives, i);
            let name = format!("{name} image {i}");
            let added = loader.load(textures, &name, path, image, mipmaps);
            let texture = added.expect("an image read fits a texture, and there's room for it");
            (texture, mipmaps)
        })
        .collect();
    let meshes: Vec<_> = (primitives.iter())
        .map(|primitive| {
            let name = format!("{name} {}", primitive.name);
            let vertices = vertices(primitive);
            let mesh = match lods(primitive) {
                Some(lods) => meshes.add_lods(&name, &vertices, &primitive.indices, lods),
                None => meshes.add(&name, &vertices, finest(primitive)),
            };
            mesh.expect("a primitive is whole triangles of its own vertices, in its levels")
        })
        .collect();
    let made = Made {
        #[cfg(feature = "dev")]
        counts: (meshes.len(), images.len()),
        meshes,
        images,
        #[cfg(feature = "dev")]
        materials: primitives
            .iter()
            .map(|primitive| primitive.material)
            .collect(),
    };
    let draws = draws(&primitives, &made, shaders);
    Ok((Nodes { parts, draws }, made))
}

/// What [`reload`] gives: what the model's root spawns, what changed of the model, and the textures
/// of the images added, with their places in the model.
#[cfg(feature = "dev")]
pub(crate) type Reread = (Nodes, Vec<&'static str>, Vec<(usize, Texture)>);

/// Puts what `imported`, model `name` read again, draws in place of what `made` drew, under the
/// same handles, so that every entity drawing it draws it once frames have streamed it: each
/// primitive's vertices and indices, and each image, with its mip levels made as its materials
/// have them. If `whole`, the model takes all of it: primitives and images past those it had are
/// added, or take back the meshes and textures of their places, which those it has no more keep,
/// unused, for when they come back; and it gives what its root spawns now, its nodes and
/// its draws, with their materials, which adds no shader, as [`add`] added them all. Otherwise it
/// takes only those, and gives back `was`, what its root spawned, which its nodes keep. Gives too
/// what changed of the model, as "nodes" and "materials", the materials compared as they were read,
/// and the textures of the images added, with their places in the model, which no image of the
/// model had before.
///
/// Fails, replacing nothing, with `asset.model_changed` unless `whole` if it has other numbers of
/// primitives or images, which a model's handles don't hold; and with `renderer.too_many_textures`
/// if `textures` hasn't room for the images added.
#[cfg(feature = "dev")]
#[expect(
    clippy::too_many_arguments,
    reason = "the renderer's three stores apart"
)]
pub(crate) fn reload(
    name: &str,
    imported: Imported,
    made: &mut Made,
    was: &Nodes,
    whole: bool,
    meshes: &mut Meshes,
    shaders: &mut Shaders,
    textures: &mut Textures,
) -> Result<Reread> {
    let Imported {
        parts,
        primitives,
        images,
        ..
    } = imported;
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
    room(name, counts.1.saturating_sub(made.images.len()), textures)?;
    let materials = primitives.iter().map(|primitive| primitive.material);
    let changed = [
        (parts != was.parts, "nodes"),
        (!materials.eq(made.materials.iter().copied()), "materials"),
    ];
    let changed = changed.into_iter().filter(|&(changed, _)| changed);
    let changed = changed.map(|(_, what)| what).collect();
    let mut added = Vec::new();
    for (i, image) in images.into_iter().enumerate() {
        let name = format!("{name} image {i}");
        let mipmaps = mipmaps(&primitives, i);
        let texels = image.texels(mipmaps);
        match made.images.get_mut(i) {
            Some((texture, was)) => {
                let replaced = textures.replace(&name, *texture, image.size, image.pixels, texels);
                replaced.expect("a decoded image fits a texture");
                *was = mipmaps;
            }
            None => {
                let texture = textures.add(&name, image.size, image.pixels, texels);
                let texture = texture.expect("a decoded image fits a texture, and there's room");
                made.images.push((texture, mipmaps));
                added.push((i, texture));
            }
        }
    }
    for (i, primitive) in primitives.iter().enumerate() {
        let name = format!("{name} {}", primitive.name);
        let vertices = vertices(primitive);
        // Read again from its files, as a development run does, it has one level.
        match made.meshes.get(i) {
            Some(&mesh) => {
                let replaced = meshes.replace(&name, mesh, &vertices, finest(primitive));
                replaced.expect("a primitive is whole triangles of its own vertices");
            }
            None => {
                let mesh = meshes.add(&name, &vertices, finest(primitive));
                made.meshes
                    .push(mesh.expect("whole triangles of its own vertices"));
            }
        }
    }
    if !whole {
        return Ok((was.clone(), changed, added));
    }
    made.counts = counts;
    made.materials = primitives
        .iter()
        .map(|primitive| primitive.material)
        .collect();
    let draws = draws(&primitives, made, shaders);
    Ok((Nodes { parts, draws }, changed, added))
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

/// Each of `primitives`' draws, of its mesh among `made`'s, with its material.
fn draws(primitives: &[Primitive], made: &Made, shaders: &mut Shaders) -> Vec<MeshDraw> {
    (primitives.iter().zip(&made.meshes))
        .map(|(primitive, &mesh)| {
            let material = material(&primitive.material, &made.images);
            MeshDraw::with_material(mesh, &material, shaders)
        })
        .collect()
}

/// The levels of detail of `primitive`, as the cooker made them, with the sphere round its vertices
/// that a draw picks one by; `None` for a primitive of one level, and for one whose vertices aren't
/// all finite, which no sphere holds.
fn lods(primitive: &Primitive) -> Option<Lods> {
    if primitive.lods.len() < 2 {
        return None;
    }
    let positions = primitive
        .positions
        .iter()
        .map(|&position| Vec3::from(position));
    let (low, high) = (positions.clone())
        .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(low, high), p| {
            (low.min(p), high.max(p))
        });
    let center = (low + high) / 2.0;
    let radius = positions.map(|p| p.distance(center)).fold(0.0, f32::max);
    (center.is_finite() && radius.is_finite()).then(|| Lods {
        levels: primitive.lods.clone(),
        center,
        radius,
    })
}

/// The indices of `primitive`'s finest level, all of them for one read from glTF.
fn finest(primitive: &Primitive) -> &[u32] {
    match primitive.lods.first() {
        Some(lod) => {
            let start = lod.start as usize;
            &primitive.indices[start..start + lod.count as usize]
        }
        None => &primitive.indices,
    }
}

/// The vertices of `primitive`, as the model shader reads them.
fn vertices(primitive: &Primitive) -> Vec<ModelVertex> {
    (primitive.positions.iter())
        .zip(&primitive.uvs)
        .zip(&primitive.colors)
        .map(|((&position, &uv), &color)| ModelVertex {
            position: Vec3::from(position),
            uv: Vec2::from(uv),
            color: Vec4::from(color),
        })
        .collect()
}

/// The material `read` gives, sampling its image among `images`, the model's textures.
fn material(read: &gltf::Material, images: &[(Texture, Mipmaps)]) -> Material {
    Material {
        color: Vec4::from(read.color),
        texture: (read.image).map_or(Material::default().texture, |i| images[i].0),
        nearest: read.nearest,
        wrap: read.wrap,
        alpha: read.alpha,
        double_sided: read.double_sided,
    }
}

/// How image `image` of `primitives`' model gets its mip levels: weighted by alpha where a masked
/// or blended material samples it, since its alpha is then how much it covers, and plainly averaged
/// where opaque ones alone do, as glTF has them ignore its alpha.
pub(crate) fn mipmaps(primitives: &[Primitive], image: usize) -> Mipmaps {
    let covers = primitives.iter().any(|primitive| {
        let material = &primitive.material;
        let blends = matches!(material.alpha, AlphaMode::Mask { .. } | AlphaMode::Blend);
        material.image == Some(image) && blends
    });
    if covers {
        Mipmaps::AlphaWeighted
    } else {
        Mipmaps::Averaged
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use gg_core::math::Quat;

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

    /// A model of a node turned, drawing a mesh of two primitives, the first masked; and a node
    /// moved and mirrored, drawing the second primitive again. Each primitive samples an image of
    /// its own.
    pub(crate) fn imported() -> Imported {
        let triangle = |name: &str, material| gltf::Primitive {
            name: name.to_owned(),
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0; 2]; 3],
            colors: vec![[1.0; 4]; 3],
            indices: vec![0, 1, 2],
            lods: Vec::new(),
            material,
        };
        let masked = gltf::Material {
            image: Some(0),
            alpha: AlphaMode::Mask { cutoff: 0.5 },
            ..gltf::Material::default()
        };
        let opaque = gltf::Material {
            image: Some(1),
            ..gltf::Material::default()
        };
        let part = |transform, parent, primitive| gltf::Part {
            transform,
            parent,
            primitive,
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
        }
    }

    /// `imported`, model `name`, added to `cx`'s world's assets, as a context adds one, and what
    /// it's made of.
    pub(crate) fn added(cx: &mut Context, name: &str, imported: Imported) -> (Model, Made) {
        let (mut meshes, mut shaders, mut textures) =
            (Meshes::default(), Shaders::default(), Textures::default());
        let (path, loader) = (Path::new("m.glb"), &mut Loader::inline());
        let added = add(
            name,
            path,
            imported,
            &mut meshes,
            &mut shaders,
            &mut textures,
            loader,
        );
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
        let (mut meshes, mut shaders, mut textures) =
            (Meshes::default(), Shaders::default(), Textures::default());
        let (path, loader) = (Path::new("m.glb"), &mut Loader::inline());
        let added = add(
            "m",
            path,
            imported(),
            &mut meshes,
            &mut shaders,
            &mut textures,
            loader,
        );
        let (was, mut made) = added.expect("a model");
        let counts = (meshes.count(), textures.count(), shaders.count());
        // Each read's, with how many meshes, textures and shaders there are after it.
        let mut again = |imported, whole| {
            let (shaders, meshes, textures) = (&mut shaders, &mut meshes, &mut textures);
            let reloaded = reload(
                "m", imported, &mut made, &was, whole, meshes, shaders, textures,
            );
            (
                reloaded,
                (meshes.count(), textures.count(), shaders.count()),
            )
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
        fewer.primitives[1].material.image = None;
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
            "drawing as it did, with the same textures"
        );
        assert_eq!(counted, counts, "nothing added");
    }
    #[test]
    fn images_that_masked_or_blended_materials_sample_have_levels_weighted_by_alpha() {
        let Imported { mut primitives, .. } = imported();
        assert_eq!(mipmaps(&primitives, 0), Mipmaps::AlphaWeighted);
        assert_eq!(mipmaps(&primitives, 1), Mipmaps::Averaged);
        primitives[1].material.alpha = AlphaMode::Blend;
        assert_eq!(mipmaps(&primitives, 1), Mipmaps::AlphaWeighted);
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
            let (path, loader) = (Path::new("m.glb"), &mut Loader::inline());
            add(
                "m",
                path,
                imported(),
                &mut meshes,
                &mut shaders,
                textures,
                loader,
            )
        };
        add(&mut textures).expect("room for both images");
        assert_eq!(textures.room(), 0);
        let error = add(&mut textures).expect_err("no room");
        assert_eq!(error.code(), "renderer.too_many_textures");
        assert!(error.to_string().contains("\"m\" has 2 images"), "{error}");
    }
}
