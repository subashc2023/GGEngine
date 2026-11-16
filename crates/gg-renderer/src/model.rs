//! Models' meshes: the engine's vertex type and shaders for glTF materials, shaded by glTF's
//! metallic-roughness model or KHR_materials_pbrSpecularGlossiness's under the frame's lights, or
//! unlit without any; and the materials, in a GPU table each draw's parameters index.

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3, Vec4};
use gg_gpu::{Sampler, Wrap};

use crate::{Mesh, MeshDraw, PackedVertex, Shaders, Texture};

/// The model shaders, compiled by the build: `shaders/model.slang` for blending materials, and the
/// two files including it for opaque and masked ones.
pub(crate) static BLENDED_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.spv"));
pub(crate) static OPAQUE_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_opaque.spv"));
pub(crate) static MASKED_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked.spv"));
/// The masked vertex shader again, with `shadow_main`, drawing its shadows cut out.
pub(crate) static MASKED_SHADOW_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked_shadow.spv"));

/// The opaque and masked shaders again, drawing a view's kept meshlets through vertex shaders
/// (`meshlet_vertex_main`), and the masked one's shadows; for devices without mesh shaders.
pub(crate) static OPAQUE_MESHLETS_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_opaque_meshlets.spv"));
pub(crate) static MASKED_MESHLETS_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked_meshlets.spv"));
pub(crate) static MASKED_MESHLETS_SHADOW_SPIRV: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/model_masked_meshlets_shadow.spv"
));

/// The opaque and masked vertex shaders again, by indices and by meshlets, with `normal_main`,
/// drawing normals for reflections.
pub(crate) static OPAQUE_NORMALS_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_opaque_normals.spv"));
pub(crate) static MASKED_NORMALS_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked_normals.spv"));
pub(crate) static OPAQUE_MESHLETS_NORMALS_SPIRV: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/model_opaque_meshlets_normals.spv"
));
pub(crate) static MASKED_MESHLETS_NORMALS_SPIRV: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/model_masked_meshlets_normals.spv"
));

/// The same through a mesh shader alone (`mesh_main`), for devices with them. Its pipelines take
/// their fragment shaders from the modules above
/// ([`GraphicsDesc::mesh`](gg_gpu::GraphicsDesc::mesh)).
pub(crate) static OPAQUE_MESH_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_opaque_mesh.spv"));
pub(crate) static MASKED_MESH_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked_mesh.spv"));

/// Draw the opaque and masked shaders' meshlets into the visibility buffer: through vertex shaders
/// (`meshlet_visibility_main`, `visibility_main`), or through a mesh shader
/// (`mesh_visibility_main`) and its fragment shader (`visibility_primitive_main`), each alone, as
/// `build.rs` says why.
pub(crate) static OPAQUE_VISIBILITY_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_opaque_visibility.spv"));
pub(crate) static MASKED_VISIBILITY_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked_visibility.spv"));
pub(crate) static OPAQUE_VISIBILITY_MESH_SPIRV: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/model_opaque_visibility_mesh.spv"
));
pub(crate) static MASKED_VISIBILITY_MESH_SPIRV: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/model_masked_visibility_mesh.spv"
));
pub(crate) static OPAQUE_VISIBILITY_PRIMITIVE_SPIRV: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/model_opaque_visibility_primitive.spv"
));
pub(crate) static MASKED_VISIBILITY_PRIMITIVE_SPIRV: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/model_masked_visibility_primitive.spv"
));

/// The modules drawing a model shader's meshlets ([`Meshlets`](crate::shader::Meshlets)): through
/// vertex shaders, with cut-out shadows or empty; through a mesh shader; into the visibility buffer
/// each way; and normals, by indices and through vertex shaders.
pub(crate) struct MeshletSpirv {
    pub(crate) vertices: &'static [u8],
    pub(crate) vertices_shadow: &'static [u8],
    pub(crate) mesh: &'static [u8],
    pub(crate) visibility: &'static [u8],
    pub(crate) visibility_mesh: &'static [u8],
    pub(crate) visibility_primitive: &'static [u8],
    pub(crate) normals: &'static [u8],
    pub(crate) vertices_normals: &'static [u8],
}

/// A model mesh's vertex: its position in mesh space, its texture coordinates, a linear RGBA color
/// multiplying the material's, its unit normal, and its unit tangent along the textures' X, with
/// the sign making the bitangent `cross(normal, tangent) * w` run along their Y, as glTF's
/// `TANGENT` has it. A mesh without a normal map may leave tangents as anything.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub struct ModelVertex {
    pub position: Vec3,
    pub uv: Vec2,
    pub color: Vec4,
    pub normal: Vec3,
    pub tangent: Vec4,
}

/// How a material covers what's behind it: glTF's `alphaMode`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    /// Wholly, whatever its alpha.
    Opaque,
    /// Wholly where alpha is at least `cutoff`, else not at all.
    Mask { cutoff: f32 },
    /// By straight alpha, sorted with other blending draws by world z.
    Blend,
}

/// A texture a material samples, and how: nearest texel of the nearest level, as glTF's `magFilter`
/// NEAREST asks, or else trilinear with up to 16 anisotropic samples; wrapping as `wrap` says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sampled {
    pub texture: Texture,
    pub nearest: bool,
    pub wrap: Wrap,
}

impl Sampled {
    /// The engine's white texture, leaving what it multiplies as is.
    pub const WHITE: Self = Self::of(Texture::WHITE);

    /// `texture`, filtered linearly, repeating.
    pub const fn of(texture: Texture) -> Self {
        Self {
            texture,
            nearest: false,
            wrap: Wrap::Repeat,
        }
    }

    fn sampler(self) -> u32 {
        match (self.nearest, self.wrap) {
            (true, Wrap::Clamp) => Sampler::NearestClamp.index(),
            (true, Wrap::Repeat) => Sampler::NearestRepeat.index(),
            (true, Wrap::Mirror) => Sampler::NearestMirror.index(),
            (false, wrap) => wrap.anisotropic(),
        }
    }
}

/// How a model's mesh looks, as glTF's metallic-roughness materials say: lit, its base color,
/// reflection, normal map, occlusion and emission; or unlit (KHR_materials_unlit), its base color
/// alone, as every material draws in a frame without lights. Each texture multiplies its factor,
/// sampled where the vertices say. `alpha` says how it covers what's behind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// Linear RGBA, straight alpha, multiplied by its texture's colors and the vertices'. A
    /// [`Workflow::SpecularGlossiness`]'s diffuse color.
    pub color: Vec4,
    pub color_texture: Sampled,
    /// How it reflects, multiplied by its surface texture's texels.
    pub workflow: Workflow,
    /// A [`Map::Surface`](crate::Map::Surface), read as stored, for
    /// [`Workflow::MetallicRoughness`]; sRGB colors for [`Workflow::SpecularGlossiness`].
    pub surface_texture: Sampled,
    /// A [`Map::Normal`](crate::Map::Normal), X and Y scaled by `normal_scale`. Without one, it
    /// faces as its vertices' normals say.
    pub normal_texture: Option<Sampled>,
    pub normal_scale: f32,
    /// A [`Map::Surface`](crate::Map::Surface) whose red darkens ambient light, not the frame's
    /// lights, by `occlusion_strength`: 0 none, 1 all.
    pub occlusion_texture: Sampled,
    pub occlusion_strength: f32,
    /// Emitted light, linear RGB, multiplied by its emissive texture's colors. Added as the view
    /// shows it, not scaled by exposure, so emissive white shows white whatever the light.
    pub emissive: Vec3,
    pub emissive_texture: Sampled,
    pub alpha: AlphaMode,
    /// Whether both faces draw; otherwise only fronts, wound counterclockwise seen from in front,
    /// or clockwise under a mirroring transform.
    pub double_sided: bool,
    /// Whether it draws its base color alone, whatever the frame's lights.
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

/// How a material's surface reflects light (its PBR workflow), with the factors its surface
/// texture's texels multiply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Workflow {
    /// glTF's metallic-roughness: `metallic` from 0 (dielectric) to 1 (metal), multiplied by the
    /// surface texture's blue; `roughness` from 0 (mirror) to 1, by its green.
    MetallicRoughness { metallic: f32, roughness: f32 },
    /// KHR_materials_pbrSpecularGlossiness: `specular`, the linear RGB reflected head-on,
    /// multiplied by the surface texture's colors; `glossiness` from 0 (rough) to 1 (mirror), by
    /// its alpha. It diffuses its base color times 1 less specular's strongest channel, shaded by
    /// the same BRDF with roughness 1 less glossiness.
    SpecularGlossiness { specular: Vec3, glossiness: f32 },
}

impl Default for Workflow {
    /// glTF's default: fully metallic and rough.
    fn default() -> Self {
        Self::MetallicRoughness {
            metallic: 1.0,
            roughness: 1.0,
        }
    }
}

/// A handle [`Materials::add`] handed out: its place in the GPU's materials table, which a model
/// draw's parameters carry.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, NoUninit)]
pub struct MaterialId {
    index: u32,
}

impl MaterialId {
    /// The order [`Materials::add`] handed it out in, from 0.
    pub fn index(self) -> u32 {
        self.index
    }
}

/// The materials models' draws index, in a GPU table. Each entry is written when added or replaced,
/// not copied per draw per frame. Handles come at once; [`Uploads::take`](crate::Uploads::take)
/// takes the writes for the frame to make before it draws. A renderer draws one `Materials`'
/// materials only. Handles are never reused, so the table holds every material a run added.
#[derive(Default)]
pub struct Materials {
    /// Written since last taken, in order: each material's index and entry.
    pub(crate) changes: Vec<(u32, Entry)>,
    /// How each material draws, which its draws take their shader and faces from.
    drawn: Vec<(AlphaMode, bool)>,
}

impl Materials {
    /// Adds `material`, which draws from [`MeshDraw::with_material`] take from the frame that
    /// writes it.
    pub fn add(&mut self, material: &Material) -> MaterialId {
        let index = u32::try_from(self.drawn.len()).expect("fewer than 2^32 materials");
        self.drawn.push((material.alpha, material.double_sided));
        self.changes.push((index, Entry::new(material)));
        MaterialId { index }
    }

    /// Puts `material` in place of `id`'s, replacing any writes still waiting; draws take it from
    /// the frame that writes it. Existing draws keep their shader and faces, so a change of alpha
    /// mode or sidedness needs them made again.
    ///
    /// # Panics
    ///
    /// - If `id` isn't one of these materials (another `Materials`' handles can be past them).
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

    /// How many it has handed out: the next index.
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
    /// A [`Workflow::MetallicRoughness`]'s; 0 for the other.
    metallic: f32,
    roughness: f32,
    normal_scale: f32,
    occlusion_strength: f32,
    /// The alpha below which the masked shader discards; the others read none.
    cutoff: f32,
    /// [`UNLIT`], [`NORMAL_MAPPED`] and [`SPECULAR_GLOSSINESS`].
    flags: u32,
    /// Each texture's index and sampler: base color, surface, normal map, occlusion and emissive.
    textures: [[u32; 2]; 5],
    /// A [`Workflow::SpecularGlossiness`]'s; 0 for the other.
    specular: Vec3,
    glossiness: f32,
}

/// Drawn unlit.
const UNLIT: u32 = 1;
/// Has a normal map.
const NORMAL_MAPPED: u32 = 2;
/// Its surface is a [`Workflow::SpecularGlossiness`].
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

/// The model shaders' parameters: `Params` in model.slang. Public only for its name and size, which
/// a snapshot's assets give as what a model's draws hold.
#[doc(hidden)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub struct Params {
    material: MaterialId,
}

impl MeshDraw {
    /// A draw of `mesh` with `material`, one of `materials`, through the engine's model shader for
    /// its alpha mode (which `shaders` adds when first asked), with its faces.
    ///
    /// # Panics
    ///
    /// - If `material` isn't one of `materials`.
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
        let packed = crate::ModelMesh::pack("a", &[corner(); 3], vec![0, 1, 2], Vec::new());
        let packed = packed.unwrap();
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
        // After the engine's sprite shader, each when first asked for.
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
        // Each draw's parameters are its material's table index.
        assert_eq!(masked.params(), 1u32.to_le_bytes());
        assert_eq!(opaque_again.params(), 3u32.to_le_bytes());
    }

    /// Each material's entry is written when added and replaced, the last waiting write replacing
    /// earlier ones; each texture samples as it says; and a material without a normal map is
    /// flagged so.
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
