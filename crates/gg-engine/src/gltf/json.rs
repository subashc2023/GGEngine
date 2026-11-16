use std::collections::BTreeMap;

use serde::Deserialize;
use serde::de::IgnoredAny;

/// The only field that a file of any glTF version is sure to have of the same type.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Versioned {
    pub(super) asset: Asset,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Asset {
    pub(super) version: String,
    pub(super) min_version: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Gltf {
    pub(super) extensions_used: Vec<String>,
    pub(super) extensions_required: Vec<String>,
    pub(super) scene: Option<u32>,
    pub(super) scenes: Vec<Scene>,
    pub(super) nodes: Vec<Node>,
    pub(super) meshes: Vec<Mesh>,
    pub(super) accessors: Vec<Accessor>,
    pub(super) buffer_views: Vec<BufferView>,
    pub(super) buffers: Vec<Buffer>,
    pub(super) materials: Vec<Material>,
    pub(super) textures: Vec<Texture>,
    pub(super) samplers: Vec<Sampler>,
    pub(super) images: Vec<Image>,
    pub(super) skins: Vec<Skin>,
    pub(super) animations: Vec<Animation>,
    pub(super) cameras: Vec<IgnoredAny>,
    pub(super) extensions: Extensions,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Extensions {
    #[serde(rename = "KHR_lights_punctual")]
    pub(super) lights_punctual: Option<LightsPunctual>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct LightsPunctual {
    pub(super) lights: Vec<Light>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Light {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) color: Option<[f32; 3]>,
    pub(super) intensity: Option<f32>,
    pub(super) range: Option<f32>,
    pub(super) spot: Option<Spot>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Spot {
    pub(super) inner_cone_angle: Option<f32>,
    pub(super) outer_cone_angle: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Scene {
    pub(super) nodes: Vec<u32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Node {
    pub(super) name: Option<String>,
    pub(super) children: Vec<u32>,
    pub(super) mesh: Option<u32>,
    pub(super) skin: Option<u32>,
    /// Column-major.
    pub(super) matrix: Option<[f32; 16]>,
    pub(super) translation: Option<[f32; 3]>,
    /// XYZW.
    pub(super) rotation: Option<[f32; 4]>,
    pub(super) scale: Option<[f32; 3]>,
    pub(super) weights: Vec<f32>,
    pub(super) extensions: NodeExtensions,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Skin {
    pub(super) inverse_bind_matrices: Option<u32>,
    pub(super) joints: Vec<u32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Animation {
    pub(super) name: Option<String>,
    pub(super) channels: Vec<AnimationChannel>,
    pub(super) samplers: Vec<AnimationSampler>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct AnimationChannel {
    pub(super) sampler: u32,
    pub(super) target: AnimationTarget,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct AnimationTarget {
    pub(super) node: Option<u32>,
    pub(super) path: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct AnimationSampler {
    pub(super) input: u32,
    pub(super) interpolation: Option<String>,
    pub(super) output: u32,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct NodeExtensions {
    #[serde(rename = "KHR_lights_punctual")]
    pub(super) lights_punctual: Option<NodeLight>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct NodeLight {
    pub(super) light: u32,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Mesh {
    pub(super) name: Option<String>,
    pub(super) primitives: Vec<Primitive>,
    pub(super) weights: Vec<f32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Primitive {
    pub(super) attributes: BTreeMap<String, u32>,
    pub(super) indices: Option<u32>,
    pub(super) material: Option<u32>,
    /// Triangles if it's `None`.
    pub(super) mode: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Accessor {
    pub(super) buffer_view: Option<u32>,
    pub(super) byte_offset: u64,
    pub(super) component_type: u32,
    pub(super) normalized: bool,
    pub(super) count: u64,
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) sparse: Option<IgnoredAny>,
}

impl Accessor {
    /// What its elements are, such as "of VEC3s of component type 5126", for errors.
    pub(super) fn describe(&self) -> String {
        let normalized = if self.normalized { ", normalized" } else { "" };
        format!(
            "of {}s of component type {}{normalized}",
            self.kind, self.component_type
        )
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct BufferView {
    pub(super) buffer: u32,
    pub(super) byte_offset: u64,
    pub(super) byte_length: u64,
    pub(super) byte_stride: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Buffer {
    pub(super) uri: Option<String>,
    pub(super) byte_length: u64,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Material {
    pub(super) pbr_metallic_roughness: Pbr,
    pub(super) normal_texture: Option<NormalTextureInfo>,
    pub(super) occlusion_texture: Option<OcclusionTextureInfo>,
    pub(super) emissive_texture: Option<TextureInfo>,
    pub(super) emissive_factor: Option<[f32; 3]>,
    pub(super) alpha_mode: Option<String>,
    pub(super) alpha_cutoff: Option<f32>,
    pub(super) double_sided: bool,
    pub(super) extensions: MaterialExtensions,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct MaterialExtensions {
    #[serde(rename = "KHR_materials_unlit")]
    pub(super) unlit: Option<IgnoredAny>,
    #[serde(rename = "KHR_materials_emissive_strength")]
    pub(super) emissive_strength: Option<EmissiveStrength>,
    #[serde(rename = "KHR_materials_pbrSpecularGlossiness")]
    pub(super) specular_glossiness: Option<SpecularGlossiness>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct SpecularGlossiness {
    pub(super) diffuse_factor: Option<[f32; 4]>,
    pub(super) diffuse_texture: Option<TextureInfo>,
    pub(super) specular_factor: Option<[f32; 3]>,
    pub(super) glossiness_factor: Option<f32>,
    pub(super) specular_glossiness_texture: Option<TextureInfo>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct EmissiveStrength {
    pub(super) emissive_strength: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Pbr {
    pub(super) base_color_factor: Option<[f32; 4]>,
    pub(super) base_color_texture: Option<TextureInfo>,
    pub(super) metallic_factor: Option<f32>,
    pub(super) roughness_factor: Option<f32>,
    pub(super) metallic_roughness_texture: Option<TextureInfo>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct TextureInfo {
    pub(super) index: u32,
    pub(super) tex_coord: u32,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct NormalTextureInfo {
    #[serde(flatten)]
    pub(super) info: TextureInfo,
    pub(super) scale: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct OcclusionTextureInfo {
    #[serde(flatten)]
    pub(super) info: TextureInfo,
    pub(super) strength: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Texture {
    pub(super) sampler: Option<u32>,
    pub(super) source: Option<u32>,
    pub(super) extensions: TextureExtensions,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct TextureExtensions {
    #[serde(rename = "MSFT_texture_dds")]
    pub(super) dds: Option<TextureDds>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct TextureDds {
    pub(super) source: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Sampler {
    pub(super) mag_filter: Option<u32>,
    pub(super) wrap_s: Option<u32>,
    pub(super) wrap_t: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Image {
    pub(super) uri: Option<String>,
    pub(super) buffer_view: Option<u32>,
}
