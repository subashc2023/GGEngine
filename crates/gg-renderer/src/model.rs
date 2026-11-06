//! Models' meshes: the engine's vertex type and shader for meshes with materials, as glTF files
//! describe them, shaded by glTF's metallic-roughness model, or
//! KHR_materials_pbrSpecularGlossiness's, under the frame's lights, or drawn unlit in a frame
//! without any; and the materials themselves, in a table the GPU keeps, which each draw's
//! parameters index.

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3, Vec4};
use gg_gpu::{Sampler, Wrap};

use crate::{Mesh, MeshDraw, PackedVertex, Shaders, Texture};

/// The model shaders, which the build compiled from `shaders/model.slang`, for materials that
/// blend, and from the two files that include it, for opaque and masked ones.
pub(crate) static BLENDED_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.spv"));
pub(crate) static OPAQUE_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_opaque.spv"));
pub(crate) static MASKED_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked.spv"));
/// The masked one's vertex shader again, with `shadow_main`, which draws its shadows cut out as it
/// is.
pub(crate) static MASKED_SHADOW_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked_shadow.spv"));

/// The opaque and masked model shaders again, drawing the meshlets a view kept of a mesh through
/// vertex shaders, `meshlet_vertex_main`, and the masked one's shadows; for a device without mesh
/// shaders.
pub(crate) static OPAQUE_MESHLETS_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_opaque_meshlets.spv"));
pub(crate) static MASKED_MESHLETS_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked_meshlets.spv"));
pub(crate) static MASKED_MESHLETS_SHADOW_SPIRV: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/model_masked_meshlets_shadow.spv"
));

/// The same through a mesh shader, `mesh_main`, alone, for a device with them: its pipelines take
/// their fragment shaders from the modules above, as
/// [`GraphicsDesc::mesh`](gg_gpu::GraphicsDesc::mesh) says.
pub(crate) static OPAQUE_MESH_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_opaque_mesh.spv"));
pub(crate) static MASKED_MESH_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked_mesh.spv"));

/// A vertex of a model's mesh: where it is in the mesh's space, where it samples its material's
/// textures, a color the material's is multiplied by, in linear RGBA, the way its surface faces,
/// a unit vector, and its tangent, the way the textures' X runs along it, a unit vector, with the
/// sign that makes the bitangent `cross(normal, tangent) * w` run along their Y, as glTF's
/// `TANGENT` has it. A mesh without a normal map may leave its tangents as anything.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub struct ModelVertex {
    pub position: Vec3,
    pub uv: Vec2,
    pub color: Vec4,
    pub normal: Vec3,
    pub tangent: Vec4,
}

/// How a material covers what's behind it, as glTF's `alphaMode` says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    /// Wholly, whatever its alpha.
    Opaque,
    /// Wholly where its alpha is at least `cutoff`, and not at all elsewhere.
    Mask { cutoff: f32 },
    /// By its alpha, not multiplied into its color, sorted with what else blends by its z in the
    /// world.
    Blend,
}

/// A texture a material samples, and how: at its nearest texel, of its nearest mip level, as
/// glTF's `magFilter` NEAREST asks, or else filtered linearly within and between its levels, with
/// up to 16 samples along the way the surface recedes; wrapping as `wrap` says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sampled {
    pub texture: Texture,
    pub nearest: bool,
    pub wrap: Wrap,
}

impl Sampled {
    /// The engine's white texture, which leaves what it multiplies as it is.
    pub const WHITE: Self = Self::of(Texture::WHITE);

    /// `texture`, filtered linearly, repeating.
    pub const fn of(texture: Texture) -> Self {
        Self {
            texture,
            nearest: false,
            wrap: Wrap::Repeat,
        }
    }

    /// The engine's sampler for it.
    fn sampler(self) -> u32 {
        match (self.nearest, self.wrap) {
            (true, Wrap::Clamp) => Sampler::NearestClamp.index(),
            (true, Wrap::Repeat) => Sampler::NearestRepeat.index(),
            (true, Wrap::Mirror) => Sampler::NearestMirror.index(),
            (false, wrap) => wrap.anisotropic(),
        }
    }
}

/// How a model's mesh looks, as glTF's metallic-roughness materials say: lit by the frame's
/// lights, its base color, how it reflects, its normal map, its occlusion and what it emits; or
/// unlit, as KHR_materials_unlit has it, its base color alone, as every material draws in a frame
/// without lights. Each of its textures multiplies its factor, and samples where the vertices say;
/// covering what's behind it as `alpha` says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// The base color, in linear RGBA, alpha not multiplied in, which its texture's colors and the
    /// vertices' multiply: for a [`Workflow::SpecularGlossiness`], its diffuse color.
    pub color: Vec4,
    pub color_texture: Sampled,
    /// How it reflects, which its surface texture's texels multiply.
    pub workflow: Workflow,
    /// Its surface's map: a [`Map::Surface`](crate::Map::Surface), sampled as stored, of a
    /// [`Workflow::MetallicRoughness`], or colors, sRGB-encoded, of a
    /// [`Workflow::SpecularGlossiness`].
    pub surface_texture: Sampled,
    /// Its normal map, a [`Map::Normal`](crate::Map::Normal), if it has one, its X and Y scaled
    /// by `normal_scale`; without one, it faces as its vertices' normals say.
    pub normal_texture: Option<Sampled>,
    pub normal_scale: f32,
    /// Its occlusion map, a [`Map::Surface`](crate::Map::Surface) whose red darkens the light that
    /// reaches it from all around rather than from the frame's lights, by `occlusion_strength`
    /// from 0, none, to 1, all of it.
    pub occlusion_texture: Sampled,
    pub occlusion_strength: f32,
    /// The light it emits, in linear RGB, which its emissive texture's colors multiply: added as
    /// the view shows it, rather than scaled by the exposure, so that an emissive white shows as
    /// white whatever the light.
    pub emissive: Vec3,
    pub emissive_texture: Sampled,
    pub alpha: AlphaMode,
    /// Whether both of its triangles' faces draw; otherwise only their fronts, those whose corners
    /// wind counterclockwise seen from in front, or clockwise under a transform that mirrors them.
    pub double_sided: bool,
    /// Whether it draws its base color alone, unlit, whatever the frame's lights.
    pub unlit: bool,
}

impl Default for Material {
    /// glTF's default material: white, fully metallic and rough, opaque and single-sided.
    fn default() -> Self {
        Self {
            color: Vec4::ONE,
            color_texture: Sampled::WHITE,
            workflow: Workflow::default(),
            surface_texture: Sampled::WHITE,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: Sampled::WHITE,
            occlusion_strength: 1.0,
            emissive: Vec3::ZERO,
            emissive_texture: Sampled::WHITE,
            alpha: AlphaMode::Opaque,
            double_sided: false,
            unlit: false,
        }
    }
}

/// How a material's surface reflects light, its workflow, as PBR calls it, with the factors its
/// surface texture's texels multiply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Workflow {
    /// As glTF's metallic-roughness materials do: how metallic it is, from 0, a dielectric, to 1,
    /// a metal, which its surface texture's blue multiplies, and how rough, from 0, a mirror, to
    /// 1, which its green multiplies.
    MetallicRoughness { metallic: f32, roughness: f32 },
    /// As KHR_materials_pbrSpecularGlossiness's materials do: the color it reflects head-on, in
    /// linear RGB, which its surface texture's colors multiply, and how glossy it is, from 0, rough,
    /// to 1, a mirror, which its alpha multiplies. It diffuses its base color times 1 less its
    /// specular color's strongest channel, and is shaded by the same BRDF, as rough as 1 less its
    /// glossiness.
    SpecularGlossiness { specular: Vec3, glossiness: f32 },
}

impl Default for Workflow {
    /// glTF's default material's: fully metallic and rough.
    fn default() -> Self {
        Self::MetallicRoughness {
            metallic: 1.0,
            roughness: 1.0,
        }
    }
}

/// A material that [`Materials::add`] handed out: its place in the table of materials the GPU
/// keeps, which a model's draw's parameters carry.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, NoUninit)]
pub struct MaterialId {
    index: u32,
}

impl MaterialId {
    /// Its index, the order [`Materials::add`] handed it out in, from 0.
    pub fn index(self) -> u32 {
        self.index
    }
}

/// The materials models' draws index, kept in a table on the GPU, each entry written once as it's
/// added and again as it's replaced, rather than copied every frame with each draw. It hands out
/// each one's handle at once, and keeps what it wrote until [`Uploads::take`](crate::Uploads::take)
/// takes it, for the frame to write into the table before it draws. A renderer draws with the
/// materials of one `Materials` only. A handle is never given out again, so the table holds every
/// material a run has added.
#[derive(Default)]
pub struct Materials {
    /// What's been written since they were last taken, in order: each material's index and its
    /// entry in the table.
    pub(crate) changes: Vec<(u32, Entry)>,
    /// How each material draws, which a draw of it takes its shader and its faces from.
    drawn: Vec<(AlphaMode, bool)>,
}

impl Materials {
    /// Adds `material`, which a draw [`MeshDraw::with_material`] makes takes from the frame that
    /// writes it on.
    pub fn add(&mut self, material: &Material) -> MaterialId {
        let index = u32::try_from(self.drawn.len()).expect("fewer than 2^32 materials");
        self.drawn.push((material.alpha, material.double_sided));
        self.changes.push((index, Entry::new(material)));
        MaterialId { index }
    }

    /// Puts `material` in place of `id`'s, which every draw of it takes from the frame that writes
    /// it on, in place of any of its writes still waiting for a frame. A draw made before keeps
    /// its shader and its faces, which a material of another alpha mode or sidedness needs drawn
    /// again for.
    ///
    /// # Panics
    ///
    /// If `id` isn't one of these materials, as another `Materials`' handles can be past them.
    pub fn replace(&mut self, id: MaterialId, material: &Material) {
        let Some(drawn) = self.drawn.get_mut(id.index as usize) else {
            panic!("material {} is one of another `Materials`", id.index);
        };
        *drawn = (material.alpha, material.double_sided);
        let entry = Entry::new(material);
        match self
            .changes
            .iter_mut()
            .find(|(index, _)| *index == id.index)
        {
            Some((_, waiting)) => *waiting = entry,
            None => self.changes.push((id.index, entry)),
        }
    }

    /// How many it has handed out: the index of the next.
    pub fn count(&self) -> u32 {
        self.drawn.len() as u32
    }
}

/// A material as the table holds it: `Material` in `shaders/mesh.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub(crate) struct Entry {
    color: Vec4,
    emissive: Vec3,
    /// A [`Workflow::MetallicRoughness`]'s, 0 for the other.
    metallic: f32,
    roughness: f32,
    normal_scale: f32,
    occlusion_strength: f32,
    /// The alpha below which the masked shader discards; the others read none.
    cutoff: f32,
    /// [`UNLIT`], [`NORMAL_MAPPED`] and [`SPECULAR_GLOSSINESS`], as they hold.
    flags: u32,
    /// Each texture's index and its sampler's: the base color's, the surface's, the normal map's,
    /// the occlusion's and the emissive's.
    textures: [[u32; 2]; 5],
    /// A [`Workflow::SpecularGlossiness`]'s, 0 for the other.
    specular: Vec3,
    glossiness: f32,
}

/// The flag of a material drawn unlit.
const UNLIT: u32 = 1;
/// The flag of a material with a normal map.
const NORMAL_MAPPED: u32 = 2;
/// The flag of a material whose surface is a [`Workflow::SpecularGlossiness`].
const SPECULAR_GLOSSINESS: u32 = 4;

impl Entry {
    fn new(material: &Material) -> Self {
        let cutoff = match material.alpha {
            AlphaMode::Mask { cutoff } => cutoff,
            AlphaMode::Opaque | AlphaMode::Blend => 0.0,
        };
        let (metallic, roughness, specular, glossiness) = match material.workflow {
            Workflow::MetallicRoughness {
                metallic,
                roughness,
            } => (metallic, roughness, Vec3::ZERO, 0.0),
            Workflow::SpecularGlossiness {
                specular,
                glossiness,
            } => (0.0, 0.0, specular, glossiness),
        };
        let specular_glossiness = matches!(material.workflow, Workflow::SpecularGlossiness { .. });
        let flags = (u32::from(material.unlit) * UNLIT)
            | (u32::from(material.normal_texture.is_some()) * NORMAL_MAPPED)
            | (u32::from(specular_glossiness) * SPECULAR_GLOSSINESS);
        let sampled = |sampled: Sampled| [sampled.texture.index, sampled.sampler()];
        Self {
            color: material.color,
            emissive: material.emissive,
            metallic,
            roughness,
            specular,
            glossiness,
            normal_scale: material.normal_scale,
            occlusion_strength: material.occlusion_strength,
            cutoff,
            flags,
            textures: [
                sampled(material.color_texture),
                sampled(material.surface_texture),
                sampled(material.normal_texture.unwrap_or(Sampled::WHITE)),
                sampled(material.occlusion_texture),
                sampled(material.emissive_texture),
            ],
        }
    }
}

/// The model shaders' parameters: `Params` in model.slang. Public for its name and size alone,
/// which a snapshot's assets give as what a model's draws hold.
#[doc(hidden)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub struct Params {
    material: MaterialId,
}

impl MeshDraw {
    /// A draw of `mesh` with `material`, one of `materials`, through the engine's model shader for
    /// its alpha mode, which `shaders` adds the first time one asks for it, and with the faces it
    /// says.
    ///
    /// # Panics
    ///
    /// If `material` isn't one of `materials`.
    pub fn with_material(
        mesh: Mesh<PackedVertex>,
        material: MaterialId,
        materials: &Materials,
        shaders: &mut Shaders,
    ) -> Self {
        let Some(&(alpha, double_sided)) = materials.drawn.get(material.index as usize) else {
            panic!("material {} is one of another `Materials`", material.index);
        };
        let mut draw = Self::new(mesh, shaders.model(alpha), Params { material });
        draw.culls = !double_sided;
        draw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Meshes;

    fn corner() -> ModelVertex {
        ModelVertex {
            position: Vec3::ZERO,
            uv: Vec2::ZERO,
            color: Vec4::ONE,
            normal: Vec3::Z,
            tangent: Vec4::X,
        }
    }

    #[test]
    fn a_material_draws_with_the_model_shader_for_its_alpha_added_once() {
        let mut meshes = Meshes::default();
        let mut shaders = Shaders::default();
        let mut materials = Materials::default();
        let packed = crate::ModelMesh::pack(&[corner(); 3], vec![0, 1, 2], Vec::new());
        let mesh = meshes.add_model("a", &packed).unwrap();
        let mut draw = |alpha, double_sided| {
            let material = Material {
                alpha,
                double_sided,
                ..Material::default()
            };
            let id = materials.add(&material);
            MeshDraw::with_material(mesh, id, &materials, &mut shaders)
        };
        let opaque = draw(AlphaMode::Opaque, false);
        let masked = draw(AlphaMode::Mask { cutoff: 0.25 }, true);
        let blended = draw(AlphaMode::Blend, false);
        let opaque_again = draw(AlphaMode::Opaque, true);
        // After the engine's sprite shader, each the first time it's asked for.
        let shader = |draw: &MeshDraw| draw.shader;
        assert_eq!(
            [&opaque, &masked, &blended, &opaque_again].map(shader),
            [1, 2, 3, 1]
        );
        assert_eq!(
            shaders.changes.len(),
            4,
            "the sprite shader and three model ones"
        );
        // Masked materials write depth where they draw, as opaque ones do.
        assert!(opaque.batch().is_some() && masked.batch().is_some());
        assert_eq!(blended.batch(), None);
        assert_eq!((opaque.culls, masked.culls), (true, false));
        // Each draw's parameters are its material's place in the table.
        assert_eq!(masked.params(), 1u32.to_le_bytes());
        assert_eq!(opaque_again.params(), 3u32.to_le_bytes());
    }

    /// Each material's entry is written as it's added, and again as it's replaced, the last write
    /// waiting for a frame taking the place of those before it; and each texture samples as it
    /// says, a material without a normal map flagged so.
    #[test]
    fn materials_are_written_as_they_are_added_and_replaced() {
        let mut materials = Materials::default();
        let (white, other) = (Texture::WHITE, Texture { index: 7 });
        let first = materials.add(&Material::default());
        let nearest = Sampled {
            texture: other,
            nearest: true,
            wrap: Wrap::Clamp,
        };
        let mapped = Material {
            color_texture: nearest,
            normal_texture: Some(Sampled::of(other)),
            unlit: true,
            alpha: AlphaMode::Mask { cutoff: 0.5 },
            ..Material::default()
        };
        let second = materials.add(&mapped);
        assert_eq!([first.index(), second.index()], [0, 1]);
        materials.replace(first, &mapped);
        let changed: Vec<u32> = materials.changes.iter().map(|&(index, _)| index).collect();
        assert_eq!(changed, [0, 1], "the replacement took the add's place");
        let (_, entry) = materials.changes[0];
        assert_eq!(entry, Entry::new(&mapped));
        assert_eq!(entry.flags, UNLIT | NORMAL_MAPPED);
        assert_eq!(entry.cutoff, 0.5);
        let anisotropic = Wrap::Repeat.anisotropic();
        assert_eq!(entry.textures[0], [7, Sampler::NearestClamp.index()]);
        assert_eq!(entry.textures[2], [7, anisotropic]);
        assert_eq!(entry.textures[1], [white.index, anisotropic]);
        let plain = Entry::new(&Material::default());
        assert_eq!(plain.flags, 0, "lit, without a normal map");
        let surface = (
            plain.metallic,
            plain.roughness,
            plain.specular,
            plain.glossiness,
        );
        assert_eq!(surface, (1.0, 1.0, Vec3::ZERO, 0.0));
        let glossy = Entry::new(&Material {
            workflow: Workflow::SpecularGlossiness {
                specular: Vec3::splat(0.5),
                glossiness: 0.25,
            },
            ..Material::default()
        });
        assert_eq!(glossy.flags, SPECULAR_GLOSSINESS);
        let surface = (
            glossy.metallic,
            glossy.roughness,
            glossy.specular,
            glossy.glossiness,
        );
        assert_eq!(surface, (0.0, 0.0, Vec3::splat(0.5), 0.25));
        materials.changes.clear();
        materials.replace(second, &Material::default());
        assert_eq!(materials.changes, [(1, plain)]);
    }

    #[test]
    fn a_sampled_texture_at_its_nearest_texel_wraps_as_it_says() {
        let nearest = [
            (Wrap::Clamp, Sampler::NearestClamp),
            (Wrap::Repeat, Sampler::NearestRepeat),
            (Wrap::Mirror, Sampler::NearestMirror),
        ];
        for (wrap, sampler) in nearest {
            let sampled = Sampled {
                nearest: true,
                wrap,
                ..Sampled::WHITE
            };
            assert_eq!(sampled.sampler(), sampler.index(), "{wrap:?}");
        }
        let filtered = Sampled {
            wrap: Wrap::Mirror,
            ..Sampled::WHITE
        };
        assert_eq!(filtered.sampler(), Wrap::Mirror.anisotropic());
    }
}
