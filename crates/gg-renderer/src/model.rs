//! Models' meshes: the engine's vertex type and shader for meshes with materials, as glTF files
//! describe them, drawn unlit until the renderer has lights.

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3, Vec4};
use gg_gpu::{Sampler, Wrap};

use crate::{Mesh, MeshDraw, Shaders, Texture};

/// The model shaders, which the build compiled from `shaders/model.slang`, for materials that
/// blend, and from the two files that include it, for opaque and masked ones.
pub(crate) static BLENDED_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/model.spv"));
pub(crate) static OPAQUE_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_opaque.spv"));
pub(crate) static MASKED_SPIRV: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/model_masked.spv"));

/// A vertex of a model's mesh: where it is in the mesh's space, where it samples its material's
/// texture, and a color the material's is multiplied by, in linear RGBA.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub struct ModelVertex {
    pub position: Vec3,
    pub uv: Vec2,
    pub color: Vec4,
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

/// How a model's mesh is drawn: unlit, in its color times its texture's, sampled where its
/// vertices say and multiplied by their colors, covering what's behind it as `alpha` says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// In linear RGBA, alpha not multiplied in.
    pub color: Vec4,
    pub texture: Texture,
    /// Whether the texture is sampled at its nearest texel, of its nearest mip level, as glTF's
    /// `magFilter` NEAREST asks; otherwise filtered linearly within and between its levels, with
    /// up to 16 samples along the way the surface recedes.
    pub nearest: bool,
    pub wrap: Wrap,
    pub alpha: AlphaMode,
    /// Whether both of its triangles' faces draw; otherwise only their fronts, those whose corners
    /// wind counterclockwise seen from in front, or clockwise under a transform that mirrors them.
    pub double_sided: bool,
}

impl Default for Material {
    /// glTF's default material: white, opaque and single-sided.
    fn default() -> Self {
        Self {
            color: Vec4::ONE,
            texture: Texture::WHITE,
            nearest: false,
            wrap: Wrap::Repeat,
            alpha: AlphaMode::Opaque,
            double_sided: false,
        }
    }
}

/// The model shaders' parameters: `Material` in model.slang. Public for its name and size alone,
/// which a snapshot's assets give as what a model's draws hold.
#[doc(hidden)]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub struct Params {
    color: Vec4,
    texture: Texture,
    sampler: u32,
    /// The alpha below which the masked shader discards; the others read none.
    cutoff: f32,
}

impl MeshDraw {
    /// A draw of `mesh` with `material`, through the engine's model shader for its alpha mode,
    /// which `shaders` adds the first time one asks for it.
    pub fn with_material(
        mesh: Mesh<ModelVertex>,
        material: &Material,
        shaders: &mut Shaders,
    ) -> Self {
        let sampler = match (material.nearest, material.wrap) {
            (true, Wrap::Clamp) => Sampler::NearestClamp.index(),
            (true, Wrap::Repeat) => Sampler::NearestRepeat.index(),
            (true, Wrap::Mirror) => Sampler::NearestMirror.index(),
            (false, wrap) => wrap.anisotropic(),
        };
        let cutoff = match material.alpha {
            AlphaMode::Mask { cutoff } => cutoff,
            AlphaMode::Opaque | AlphaMode::Blend => 0.0,
        };
        let params = Params {
            color: material.color,
            texture: material.texture,
            sampler,
            cutoff,
        };
        let mut draw = Self::new(mesh, shaders.model(material.alpha), params);
        draw.culls = !material.double_sided;
        draw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Meshes;

    #[test]
    fn a_material_draws_with_the_model_shader_for_its_alpha_added_once() {
        let mut meshes = Meshes::default();
        let mut shaders = Shaders::default();
        let corner = ModelVertex {
            position: Vec3::ZERO,
            uv: Vec2::ZERO,
            color: Vec4::ONE,
        };
        let mesh = meshes.add("a", &[corner; 3], &[0, 1, 2]).unwrap();
        let mut draw = |alpha, double_sided| {
            let material = Material {
                alpha,
                double_sided,
                ..Material::default()
            };
            MeshDraw::with_material(mesh, &material, &mut shaders)
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
            shaders.added.len(),
            4,
            "the sprite shader and three model ones"
        );
        // Masked materials write depth where they draw, as opaque ones do.
        assert!(opaque.batch().is_some() && masked.batch().is_some());
        assert_eq!(blended.batch(), None);
        assert_eq!((opaque.culls, masked.culls), (true, false));
        let params = Params {
            color: Vec4::ONE,
            texture: Texture::WHITE,
            sampler: Wrap::Repeat.anisotropic(),
            cutoff: 0.25,
        };
        assert_eq!(masked.params(), bytemuck::bytes_of(&params));
    }

    #[test]
    fn a_material_sampled_at_its_nearest_texel_wraps_as_it_says() {
        let mut meshes = Meshes::default();
        let mut shaders = Shaders::default();
        let corner = ModelVertex {
            position: Vec3::ZERO,
            uv: Vec2::ZERO,
            color: Vec4::ONE,
        };
        let mesh = meshes.add("a", &[corner; 3], &[0, 1, 2]).unwrap();
        let mut params = |nearest, wrap| {
            let material = Material {
                nearest,
                wrap,
                ..Material::default()
            };
            let draw = MeshDraw::with_material(mesh, &material, &mut shaders);
            draw.params().to_vec()
        };
        let sampling = |sampler| {
            let params = Params {
                color: Vec4::ONE,
                texture: Texture::WHITE,
                sampler,
                cutoff: 0.0,
            };
            bytemuck::bytes_of(&params).to_vec()
        };
        let nearest = [
            (Wrap::Clamp, Sampler::NearestClamp),
            (Wrap::Repeat, Sampler::NearestRepeat),
            (Wrap::Mirror, Sampler::NearestMirror),
        ];
        for (wrap, sampler) in nearest {
            assert_eq!(params(true, wrap), sampling(sampler.index()), "{wrap:?}");
        }
        let anisotropic = sampling(Wrap::Mirror.anisotropic());
        assert_eq!(params(false, Wrap::Mirror), anisotropic);
    }
}
