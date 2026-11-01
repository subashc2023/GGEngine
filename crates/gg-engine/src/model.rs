//! Models: glTF files' scenes, whose meshes and textures a layer adds once, and whose nodes it
//! spawns into a world as entities, as many times as it likes.

use gg_core::math::{Vec2, Vec3, Vec4};
use gg_core::{Error, Result};
use gg_renderer::{
    AlphaMode, Material, MeshDraw, Meshes, Mipmaps, ModelVertex, Shaders, Texture, Textures,
};

use crate::gltf::{Imported, Primitive};
use crate::{Entity, Transform, World};

/// A glTF file's scene, its meshes and textures added: what
/// [`Context::add_model`](crate::Context::add_model) gives, which [`spawn`](Self::spawn) places in
/// a world as entities.
#[derive(Clone, Debug)]
pub struct Model {
    /// The scene's nodes, and a part for each primitive of a mesh with more than one, in pre-order:
    /// each after its parent.
    parts: Vec<Part>,
}

#[derive(Clone, Debug)]
struct Part {
    transform: Transform,
    /// An earlier part, or the model's root.
    parent: Option<usize>,
    draw: Option<MeshDraw>,
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
        let images: Vec<Texture> = images
            .into_iter()
            .enumerate()
            .map(|(i, image)| {
                let name = format!("{name} image {i}");
                let mipmaps = mipmaps(&primitives, i);
                let added = textures.add(&name, image.size, image.pixels, mipmaps);
                added.expect("a decoded image fits a texture, and there's room for it")
            })
            .collect();
        let draws: Vec<MeshDraw> = primitives
            .iter()
            .map(|primitive| {
                let vertices: Vec<ModelVertex> = (primitive.positions.iter())
                    .zip(&primitive.uvs)
                    .zip(&primitive.colors)
                    .map(|((&position, &uv), &color)| ModelVertex {
                        position: Vec3::from(position),
                        uv: Vec2::from(uv),
                        color: Vec4::from(color),
                    })
                    .collect();
                let name = format!("{name} {}", primitive.name);
                let mesh = meshes.add(&name, &vertices, &primitive.indices);
                let mesh = mesh.expect("a primitive is whole triangles of its own vertices");
                let read = &primitive.material;
                let material = Material {
                    color: Vec4::from(read.color),
                    texture: read
                        .image
                        .map_or(Material::default().texture, |i| images[i]),
                    nearest: read.nearest,
                    wrap: read.wrap,
                    alpha: read.alpha,
                    double_sided: read.double_sided,
                };
                MeshDraw::with_material(mesh, &material, shaders)
            })
            .collect();
        let parts = parts
            .into_iter()
            .map(|part| Part {
                transform: part.transform,
                parent: part.parent,
                draw: part.primitive.map(|i| draws[i]),
            })
            .collect();
        Ok((Self { parts }, images))
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
            let entity = match part.draw {
                Some(draw) => world.spawn((part.transform, draw)),
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
