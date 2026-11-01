//! Models: glTF files' scenes, whose meshes and textures a layer adds once, and whose nodes it
//! spawns into a world as entities, as many times as it likes; and, in development builds, read
//! again as their files change, their meshes and textures replaced under the same handles.

use gg_core::math::{Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{
    AlphaMode, Material, Mesh, MeshDraw, Meshes, Mipmaps, ModelVertex, Shaders, Texture, Textures,
};

use crate::gltf::{self, Imported, Part, Primitive};
use crate::{Entity, Transform, World};

/// A glTF file's scene, its meshes and textures added: what
/// [`Context::add_model`](crate::Context::add_model) gives, which [`spawn`](Self::spawn) places in
/// a world as entities.
#[derive(Clone, Debug)]
pub struct Model {
    /// The scene's nodes, and a part for each primitive of a mesh with more than one, in pre-order:
    /// each after its parent.
    parts: Vec<Part>,
    /// Each primitive's mesh, and its draw, with its material.
    primitives: Vec<(Mesh<ModelVertex>, MeshDraw)>,
    /// Each image's texture, and how it got its mip levels, which a reload makes them by again.
    #[cfg(feature = "dev")]
    images: Vec<(Texture, Mipmaps)>,
    /// Each primitive's material as it was read, which a reload compares its own with.
    #[cfg(feature = "dev")]
    materials: Vec<gltf::Material>,
}

impl Model {
    /// Adds what model `name`, as `read` gave it, draws: its images to `textures`, each with mip
    /// levels, weighted by alpha where a masked or blended material samples it, then its primitives
    /// to `meshes`, each drawn with its material through `shaders`' model shader. Gives the model
    /// and its images' textures, in the model's order.
    ///
    /// Fails, adding nothing, with `renderer.too_many_textures` if `textures` hasn't room for its
    /// images.
    pub(crate) fn add(
        name: &str,
        imported: Imported,
        meshes: &mut Meshes,
        shaders: &mut Shaders,
        textures: &mut Textures,
    ) -> Result<(Self, Vec<Texture>)> {
        let Imported {
            parts,
            primitives,
            images,
            ..
        } = imported;
        // Checked first, so that a model adds all of itself or nothing: what `read` gave can't fail
        // to add otherwise.
        if images.len() > textures.room() as usize {
            return Err(Error::new(
                "renderer.too_many_textures",
                format!(
                    "model {name:?} has {} images, but the renderer holds {} more textures",
                    images.len(),
                    textures.room()
                ),
            ));
        }
        let images: Vec<(Texture, Mipmaps)> = images
            .into_iter()
            .enumerate()
            .map(|(i, image)| {
                let name = format!("{name} image {i}");
                let mipmaps = mipmaps(&primitives, i);
                let added = textures.add(&name, image.size, image.pixels, mipmaps);
                let texture =
                    added.expect("a decoded image fits a texture, and there's room for it");
                (texture, mipmaps)
            })
            .collect();
        #[cfg(feature = "dev")]
        let materials = primitives
            .iter()
            .map(|primitive| primitive.material)
            .collect();
        let primitives = primitives
            .iter()
            .map(|primitive| {
                let name = format!("{name} {}", primitive.name);
                let mesh = meshes.add(&name, &vertices(primitive), &primitive.indices);
                let mesh = mesh.expect("a primitive is whole triangles of its own vertices");
                let material = material(&primitive.material, &images);
                (mesh, MeshDraw::with_material(mesh, &material, shaders))
            })
            .collect();
        let textures = images.iter().map(|&(texture, _)| texture).collect();
        let model = Self {
            parts,
            primitives,
            #[cfg(feature = "dev")]
            images,
            #[cfg(feature = "dev")]
            materials,
        };
        Ok((model, textures))
    }

    /// Puts what `imported`, model `name` read again, draws in place of what the model drew, under
    /// the same handles, so that every entity spawned from it draws it from the frame that takes
    /// it: each primitive's vertices and indices, and each image, with its mip levels made as they
    /// were. What changed of the rest stays as it was, its parts' nodes and transforms, which only
    /// [`spawn`](Self::spawn) reads, and its materials, which the entities' draws hold, and is
    /// returned, as "nodes" and "materials", for the reload to warn of: materials compared as they
    /// were read, so that one changed adds no shader, as a draw made of it might, of the engine's
    /// own for its alpha mode, whose index what's added after it would then follow.
    ///
    /// Fails with `asset.model_changed`, replacing nothing, if it has other numbers of primitives
    /// or images, which the model's handles don't hold.
    #[cfg(feature = "dev")]
    pub(crate) fn reload(
        &self,
        name: &str,
        imported: Imported,
        meshes: &mut Meshes,
        textures: &mut Textures,
    ) -> Result<Vec<&'static str>> {
        let Imported {
            parts,
            primitives,
            images,
            ..
        } = imported;
        if (primitives.len(), images.len()) != (self.primitives.len(), self.images.len()) {
            let count = |n: usize, what: &str| match n {
                1 => format!("1 {what}"),
                _ => format!("{n} {what}s"),
            };
            let message = format!(
                "model {name:?} has {} and {}, where it had {} and {}, which a run takes only as \
                 it starts",
                count(primitives.len(), "primitive"),
                count(images.len(), "image"),
                count(self.primitives.len(), "primitive"),
                count(self.images.len(), "image"),
            );
            return Err(Error::new("asset.model_changed", message));
        }
        for (i, (image, &(texture, mipmaps))) in images.into_iter().zip(&self.images).enumerate() {
            let name = format!("{name} image {i}");
            let replaced = textures.replace(&name, texture, image.size, image.pixels, mipmaps);
            replaced.expect("a decoded image fits a texture");
        }
        let mut materials = false;
        let held = self.primitives.iter().zip(&self.materials);
        for (primitive, (&(mesh, _), material)) in primitives.iter().zip(held) {
            let name = format!("{name} {}", primitive.name);
            let replaced = meshes.replace(&name, mesh, &vertices(primitive), &primitive.indices);
            replaced.expect("a primitive is whole triangles of its own vertices");
            materials |= primitive.material != *material;
        }
        let kept = [(parts != self.parts, "nodes"), (materials, "materials")];
        Ok(kept
            .into_iter()
            .filter(|&(changed, _)| changed)
            .map(|(_, what)| what)
            .collect())
    }

    /// Spawns the model into `world` as entities, and returns its root, at `at`: under it, the
    /// scene's nodes, each with its transform in its parent's space, those with a mesh drawing it,
    /// one entity for each primitive of a mesh with several, since an entity draws one. Every
    /// entity is new, its ID drawn from the world's stream, the root's first, then the rest in the
    /// file's order, so that a world seeded alike spawns the model alike.
    pub fn spawn(&self, world: &mut World, at: Transform) -> Entity {
        let root = world.spawn(at);
        let mut spawned = world.take_list();
        for part in &self.parts {
            let entity = match part.primitive {
                Some(i) => world.spawn((part.transform, self.primitives[i].1)),
                None => world.spawn(part.transform),
            };
            let parent = part.parent.map_or(root, |i| spawned[i]);
            let set = world.set_parent(entity, Some(parent));
            set.expect("a new entity takes an earlier one as its parent");
            spawned.push(entity);
        }
        world.keep_list(spawned);
        root
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
fn mipmaps(primitives: &[Primitive], image: usize) -> Mipmaps {
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
mod tests {
    use gg_core::math::Quat;

    use super::*;
    use crate::image::Image;
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
    fn imported() -> Imported {
        let triangle = |name: &str, material| gltf::Primitive {
            name: name.to_owned(),
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0; 2]; 3],
            colors: vec![[1.0; 4]; 3],
            indices: vec![0, 1, 2],
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

    fn add(imported: Imported, textures: &mut Textures) -> Result<Model> {
        let (mut meshes, mut shaders) = (Meshes::default(), Shaders::default());
        let added = Model::add("m", imported, &mut meshes, &mut shaders, textures);
        added.map(|(model, _)| model)
    }

    #[test]
    fn a_model_spawns_under_a_root_each_part_under_its_parent() {
        let model = add(imported(), &mut Textures::default()).expect("a model");
        let at = Transform {
            translation: Vec3::new(0.0, 0.0, -5.0),
            ..Transform::IDENTITY
        };
        let (mut cx, mut again) = (Context::seeded(1), Context::seeded(1));
        let root = model.spawn(cx.world_mut(), at);
        model.spawn(again.world_mut(), at);
        // A world seeded alike spawns it alike.
        assert_eq!(cx.world().hash(), again.world().hash());

        let world = cx.world();
        let children = |entity| -> Vec<Entity> { world.children(entity).expect("alive").collect() };
        let transform = |entity| world.get::<Transform>(entity).copied();
        let draw = |entity| world.get::<MeshDraw>(entity).copied();
        assert_eq!((transform(root), draw(root)), (Some(at), None));
        let [node, mirror] = <[Entity; 2]>::try_from(children(root)).expect("two nodes");
        assert_eq!((transform(node), draw(node)), (Some(turned()), None));
        // A child at the node for each primitive of its mesh, since an entity draws one.
        let parts = <[Entity; 2]>::try_from(children(node)).expect("a part for each primitive");
        for part in parts {
            assert_eq!(transform(part), Some(Transform::IDENTITY));
            assert_eq!(children(part), []);
        }
        let [first, second] = parts.map(|part| draw(part).expect("a draw"));
        assert_ne!(first, second);
        // The second primitive again, the same mesh, so that its draws batch.
        assert_eq!(
            (transform(mirror), draw(mirror)),
            (Some(mirrored()), Some(second))
        );
        assert_eq!(children(mirror), []);
    }

    /// A model read again replaces its meshes and images under their handles, adding none, not even
    /// the engine's shader for a material's alpha mode that none had, and says which of its nodes
    /// and materials changed, which stay as they were; one of other numbers of primitives or images
    /// replaces nothing.
    #[cfg(feature = "dev")]
    #[test]
    fn a_model_read_again_replaces_its_meshes_and_images_and_says_what_else_changed() {
        let (mut meshes, mut shaders, mut textures) =
            (Meshes::default(), Shaders::default(), Textures::default());
        let added = Model::add("m", imported(), &mut meshes, &mut shaders, &mut textures);
        let (model, _) = added.expect("a model");
        let counts = (meshes.count(), shaders.count(), textures.count());
        let mut reload = |imported| model.reload("m", imported, &mut meshes, &mut textures);
        assert_eq!(reload(imported()).expect("read again"), [""; 0]);
        let mut moved = imported();
        moved.parts[0].transform = mirrored();
        assert_eq!(reload(moved).expect("read again"), ["nodes"]);
        let mut recolored = imported();
        recolored.primitives[1].material.color = [0.5; 4];
        // Masked and opaque before: the engine's blended model shader isn't added.
        recolored.primitives[1].material.alpha = AlphaMode::Blend;
        recolored.parts.pop();
        let changed = reload(recolored).expect("read again");
        assert_eq!(changed, ["nodes", "materials"]);
        let mut fewer = imported();
        fewer.images.pop();
        fewer.primitives[1].material.image = None;
        let error = reload(fewer).expect_err("an image fewer");
        assert_eq!(error.code(), "asset.model_changed");
        assert!(
            error.to_string().contains("2 primitives and 1 image,"),
            "{error}"
        );
        let after = (meshes.count(), shaders.count(), textures.count());
        assert_eq!(after, counts, "no handle added");
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
        add(imported(), &mut textures).expect("room for both images");
        assert_eq!(textures.room(), 0);
        let error = add(imported(), &mut textures).expect_err("no room");
        assert_eq!(error.code(), "renderer.too_many_textures");
        assert!(error.to_string().contains("\"m\" has 2 images"), "{error}");
    }
}
