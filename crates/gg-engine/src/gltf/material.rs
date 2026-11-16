//! A model's materials: their factors and textures, and the images those sample.

use std::io;
use std::path::Path;

use gg_core::Result;
use gg_core::math::Vec3;
use gg_renderer::{AlphaMode, Map, Workflow, Wrap};

use super::Reader;
use crate::image;

/// How a primitive looks: glTF's metallic-roughness or KHR_materials_pbrSpecularGlossiness
/// material. Base or diffuse color is the factor, texture and primitive's vertex colors multiplied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Material {
    /// The base color factor, or the diffuse factor, linear RGBA.
    pub(crate) color: [f32; 4],
    /// How metallic and rough it is, or its specular color and how glossy.
    pub(crate) workflow: Workflow,
    /// The normal map's scale of X and Y.
    pub(crate) normal_scale: f32,
    /// How much the occlusion map darkens, from 0 to 1.
    pub(crate) occlusion_strength: f32,
    /// The emissive factor, linear RGB, times KHR_materials_emissive_strength's strength.
    pub(crate) emissive: [f32; 3],
    /// Its textures, each at its [`Slot`]'s place.
    pub(crate) textures: [Option<Sampling>; 5],
    pub(crate) alpha: AlphaMode,
    /// Whether a triangle's back shows too; its front is where its vertices go counterclockwise.
    pub(crate) double_sided: bool,
    pub(crate) unlit: bool,
}

impl Default for Material {
    /// glTF's default for a primitive without a material: white, metallic, rough, untextured,
    /// opaque, single-sided.
    fn default() -> Self {
        Self {
            color: [1.0; 4],
            workflow: Workflow::default(),
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            textures: [None; 5],
            alpha: AlphaMode::Opaque,
            double_sided: false,
            unlit: false,
        }
    }
}

impl Material {
    /// Whether its numbers are all finite, as a model's must be.
    pub(crate) fn is_finite(&self) -> bool {
        let workflow = match self.workflow {
            Workflow::MetallicRoughness {
                metallic,
                roughness,
            } => metallic.is_finite() && roughness.is_finite(),
            Workflow::SpecularGlossiness {
                specular,
                glossiness,
            } => specular.is_finite() && glossiness.is_finite(),
        };
        let cutoff = match self.alpha {
            AlphaMode::Mask { cutoff } => cutoff,
            AlphaMode::Opaque | AlphaMode::Blend => 0.0,
        };
        let numbers = [self.normal_scale, self.occlusion_strength, cutoff].into_iter();
        workflow && (numbers.chain(self.color).chain(self.emissive)).all(f32::is_finite)
    }

    pub(crate) fn texture(&self, slot: Slot) -> Option<Sampling> {
        self.textures[slot as usize]
    }

    /// Like [`Slot::map`], but a specular-glossiness surface texture holds colors (specular
    /// sRGB-encoded), per KHR_materials_pbrSpecularGlossiness.
    pub(crate) fn map(&self, slot: Slot) -> Option<Map> {
        match (slot, self.workflow) {
            (Slot::Surface, Workflow::SpecularGlossiness { .. }) => None,
            _ => slot.map(),
        }
    }
}

/// A material's texture: the index in `images` of its image, and how it samples it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sampling {
    pub(crate) image: usize,
    /// Magnifies to the nearest texel, rather than blending the nearest four.
    pub(crate) nearest: bool,
    /// The same across and down.
    pub(crate) wrap: Wrap,
}

/// A material's textures, in the order its `textures` hold them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// Base color, or diffuse.
    Color,
    /// Metallic-roughness, or specular-glossiness.
    Surface,
    Normal,
    Occlusion,
    Emissive,
}

impl Slot {
    pub(crate) const ALL: [Self; 5] = [
        Self::Color,
        Self::Surface,
        Self::Normal,
        Self::Occlusion,
        Self::Emissive,
    ];

    /// What its texture's image holds in a metallic-roughness material: sRGB-encoded colors
    /// (`None`) or a map of data.
    pub(crate) fn map(self) -> Option<Map> {
        match self {
            Self::Color | Self::Emissive => None,
            Self::Surface | Self::Occlusion => Some(Map::Surface),
            Self::Normal => Some(Map::Normal),
        }
    }
}

/// A phrase for what an image holds, as a texture samples it.
pub(crate) fn holds(map: Option<Map>) -> &'static str {
    match map {
        None => "colors",
        Some(Map::Surface) => "values",
        Some(Map::Normal) => "a normal map",
    }
}

const NEAREST: u32 = 9728;
const CLAMP_TO_EDGE: u32 = 33071;
const MIRRORED_REPEAT: u32 = 33648;
const REPEAT: u32 = 10497;

impl<'a, F: Fn(&Path, u64) -> io::Result<Vec<u8>>> Reader<'a, F> {
    /// Material `index`, and the TEXCOORD set its textures read, if any: the first in [`Slot`]'s
    /// order, since a primitive's vertices have one set. KHR_materials_pbrSpecularGlossiness
    /// replaces its metallic-roughness fallback. A texture reading another set, or whose image
    /// another texture samples as something else (colors, not a map), is left out and goes to
    /// `imported.ignored`.
    pub(super) fn material(&mut self, index: u32) -> Result<(Material, Option<u32>)> {
        let (model, gltf) = (self.model, self.gltf);
        let material = model.item(&gltf.materials, "material", index)?;
        let alpha = match material.alpha_mode.as_deref() {
            None | Some("OPAQUE") => AlphaMode::Opaque,
            Some("MASK") => {
                let cutoff = material.alpha_cutoff.unwrap_or(0.5);
                if cutoff < 0.0 {
                    return Err(model.bad(format!(
                        "material {index}'s alphaCutoff is {cutoff}, below 0"
                    )));
                }
                AlphaMode::Mask { cutoff }
            }
            Some("BLEND") => AlphaMode::Blend,
            Some(mode) => {
                return Err(model.bad(format!(
                    "material {index}'s alphaMode {mode:?} isn't glTF's"
                )));
            }
        };
        let pbr = &material.pbr_metallic_roughness;
        let extensions = &material.extensions;
        let strength = (extensions.emissive_strength.as_ref())
            .and_then(|strength| strength.emissive_strength)
            .unwrap_or(1.0);
        let emissive = material.emissive_factor.unwrap_or([0.0; 3]);
        let (color, workflow, color_texture, surface_texture) =
            match &extensions.specular_glossiness {
                Some(glossy) => (
                    glossy.diffuse_factor.unwrap_or([1.0; 4]),
                    Workflow::SpecularGlossiness {
                        specular: Vec3::from_array(glossy.specular_factor.unwrap_or([1.0; 3])),
                        glossiness: glossy.glossiness_factor.unwrap_or(1.0),
                    },
                    (glossy.diffuse_texture.as_ref(), "diffuseTexture"),
                    (
                        glossy.specular_glossiness_texture.as_ref(),
                        "specularGlossinessTexture",
                    ),
                ),
                None => (
                    pbr.base_color_factor.unwrap_or([1.0; 4]),
                    Workflow::MetallicRoughness {
                        metallic: pbr.metallic_factor.unwrap_or(1.0),
                        roughness: pbr.roughness_factor.unwrap_or(1.0),
                    },
                    (pbr.base_color_texture.as_ref(), "baseColorTexture"),
                    (
                        pbr.metallic_roughness_texture.as_ref(),
                        "metallicRoughnessTexture",
                    ),
                ),
            };
        let mut read = Material {
            color,
            workflow,
            normal_scale: (material.normal_texture.as_ref())
                .and_then(|normal| normal.scale)
                .unwrap_or(1.0),
            occlusion_strength: (material.occlusion_texture.as_ref())
                .and_then(|occlusion| occlusion.strength)
                .unwrap_or(1.0),
            emissive: emissive.map(|channel| channel * strength),
            alpha,
            double_sided: material.double_sided,
            unlit: extensions.unlit.is_some(),
            ..Material::default()
        };
        // The emissive factor times its strength can pass f32's range though each number is in it.
        if !read.is_finite() {
            return Err(model.bad(format!("material {index} has a number that isn't finite")));
        }
        let infos = [
            (Slot::Color, color_texture),
            (Slot::Surface, surface_texture),
            (
                Slot::Normal,
                (
                    (material.normal_texture.as_ref()).map(|n| &n.info),
                    "normalTexture",
                ),
            ),
            (
                Slot::Occlusion,
                (
                    (material.occlusion_texture.as_ref()).map(|o| &o.info),
                    "occlusionTexture",
                ),
            ),
            (
                Slot::Emissive,
                (material.emissive_texture.as_ref(), "emissiveTexture"),
            ),
        ];
        let mut set = None;
        for (slot, (info, name)) in infos {
            let Some(info) = info else {
                continue;
            };
            let read_set = *set.get_or_insert(info.tex_coord);
            if read_set != info.tex_coord {
                self.imported.ignored.push(format!(
                    "material {index}'s {name}, which reads TEXCOORD_{} where its others read \
                     TEXCOORD_{read_set}",
                    info.tex_coord
                ));
                continue;
            }
            let held = read.map(slot);
            read.textures[slot as usize] = self.sampling(index, name, held, info.index)?;
        }
        Ok((read, set))
    }

    /// Texture `texture`, which material `material` samples as its `name`, its image holding
    /// `wanted` (colors or a map): the image (MSFT_texture_dds's DDS one instead of its source)
    /// and how it's sampled. `None`, noted in `imported.ignored`, where another texture samples
    /// the image as something else.
    fn sampling(
        &mut self,
        material: u32,
        name: &str,
        wanted: Option<Map>,
        texture: u32,
    ) -> Result<Option<Sampling>> {
        let (model, gltf) = (self.model, self.gltf);
        let index = texture;
        let texture = model.item(&gltf.textures, "texture", index)?;
        let (mut wrap, mut nearest) = (Wrap::Repeat, false);
        if let Some(sampler_index) = texture.sampler {
            let sampler = model.item(&gltf.samplers, "sampler", sampler_index)?;
            let wrapping = |mode: Option<u32>| match mode.unwrap_or(REPEAT) {
                CLAMP_TO_EDGE => Ok(Wrap::Clamp),
                MIRRORED_REPEAT => Ok(Wrap::Mirror),
                REPEAT => Ok(Wrap::Repeat),
                mode => Err(model.bad(format!(
                    "sampler {sampler_index} has wrap mode {mode}, which glTF doesn't have"
                ))),
            };
            let (across, down) = (wrapping(sampler.wrap_s)?, wrapping(sampler.wrap_t)?);
            if across != down {
                return Err(model.unsupported(format!(
                    "sampler {sampler_index} wraps S and T differently, which the engine doesn't \
                     draw"
                )));
            }
            wrap = across;
            nearest = sampler.mag_filter == Some(NEAREST);
        }
        let dds = (texture.extensions.dds.as_ref()).and_then(|dds| dds.source);
        let source = dds.or(texture.source).ok_or_else(|| {
            model.unsupported(format!(
                "texture {index} has no source, the image the engine samples"
            ))
        })?;
        let image = self.image(source)?;
        let held = *self.holds.entry(image).or_insert(wanted);
        if held != wanted {
            self.imported.ignored.push(format!(
                "material {material}'s {name}, which samples image {source} as {} where another \
                 texture samples it as {}",
                holds(wanted),
                holds(held)
            ));
            return Ok(None);
        }
        Ok(Some(Sampling {
            image,
            nearest,
            wrap,
        }))
    }

    /// Image `index`, read once as far as its header for the loader to decode: by
    /// [`image::read`], or [`image::read_dds`] for a DDS file (told by its first bytes). Gives its
    /// place in `imported.images`.
    fn image(&mut self, index: u32) -> Result<usize> {
        if let Some(&at) = self.images.get(&index) {
            return Ok(at);
        }
        let (model, gltf) = (self.model, self.gltf);
        let image = model.item(&gltf.images, "image", index)?;
        // A byte past the most an image's file holds, so a longer one fails to decode.
        let most = image::MAX_FILE + 1;
        let bytes = match (&image.uri, image.buffer_view) {
            (Some(uri), None) => self.uri(&format!("image {index}"), uri, most)?,
            (None, Some(view)) => {
                let bytes = self.view(view)?.1;
                bytes[..bytes.len().min(most as usize)].to_vec()
            }
            _ => {
                return Err(model.bad(format!(
                    "image {index} has both a uri and a bufferView, or neither"
                )));
            }
        };
        let read = if image::is_dds(&bytes) {
            image::read_dds(bytes).map_err(|problem| {
                model.bad(format!(
                    "image {index} doesn't read as a DDS of BC7: {problem}"
                ))
            })?
        } else {
            image::read(bytes).map_err(|problem| {
                model.bad(format!(
                    "image {index} doesn't decode as a PNG or a JPEG: {problem}"
                ))
            })?
        };
        // Images sharing a source each decode it, so each counts; the file is held until decoded.
        self.memory.hold(read.pixels.len() + read.decoded_len())?;
        let at = self.imported.images.len();
        self.imported.images.push(read);
        self.images.insert(index, at);
        Ok(at)
    }
}
