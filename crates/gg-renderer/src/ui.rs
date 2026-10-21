//! egui's meshes, drawn over the frame.

use std::collections::HashMap;
use std::sync::Arc;

use epaint::textures::{TextureFilter, TextureWrapMode, TexturesDelta};
use epaint::{ClippedPrimitive, ImageData, ImageDelta, Primitive, TextureId};
use gg_core::Result;
use gg_gpu::{
    Blend, Device, Format, Frame, GraphicsDesc, Pass, Pipeline, Sampler, Texture, TextureFormat,
};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/egui.spv"));

/// One frame of egui's output: the meshes to draw, and the texture changes to apply first,
/// which the renderer takes, leaving them empty.
pub struct Ui<'a> {
    pub primitives: &'a [ClippedPrimitive],
    pub textures: &'a mut TexturesDelta,
    /// Physical pixels per egui point.
    pub pixels_per_point: f32,
}

/// Draws egui's meshes, and keeps the textures they sample, which every window's UI shares.
pub(crate) struct UiPass {
    device: Arc<Device>,
    /// A pipeline for each format of target drawn into.
    pipelines: Vec<(Format, Pipeline)>,
    textures: HashMap<TextureId, (Texture, Sampler)>,
    /// Texture changes not yet applied, in order: egui sends each once.
    to_set: Vec<(TextureId, ImageDelta)>,
    /// Textures to free once the changes before them are drawn.
    to_free: Vec<TextureId>,
}

impl UiPass {
    /// A pass that draws into targets of `format`.
    pub(crate) fn new(device: &Arc<Device>, format: Format) -> Result<Self> {
        let mut pass = Self {
            device: device.clone(),
            pipelines: Vec::new(),
            textures: HashMap::new(),
            to_set: Vec::new(),
            to_free: Vec::new(),
        };
        pass.add_format(format)?;
        Ok(pass)
    }

    /// Lets the pass draw into targets of `format` too.
    pub(crate) fn add_format(&mut self, format: Format) -> Result<()> {
        if self
            .pipelines
            .iter()
            .all(|&(made_for, _)| made_for != format)
        {
            let pipeline = Pipeline::graphics(
                &self.device,
                &GraphicsDesc {
                    name: "egui",
                    spirv: SPIRV,
                    vertex: c"vertex_main",
                    fragment: c"fragment_main",
                    format,
                    blend: Blend::PremultipliedAlpha,
                },
            )?;
            self.pipelines.push((format, pipeline));
        }
        Ok(())
    }

    /// Takes a frame's texture changes, to apply on the next frame that renders.
    pub(crate) fn queue(&mut self, textures: &mut TexturesDelta) {
        for (id, deltas) in textures.set.drain() {
            self.to_set
                .extend(deltas.into_iter().map(|delta| (id, delta)));
        }
        self.to_free.extend(textures.free.drain());
    }

    /// Applies the texture changes queued so far, which must happen before the frame's passes.
    pub(crate) fn write_textures(&mut self, frame: &mut Frame<'_>) -> Result<()> {
        for (id, delta) in self.to_set.drain(..) {
            let ImageData::Color(image) = &delta.image;
            let size = [image.size[0] as u32, image.size[1] as u32];
            let pixels: &[u8] = bytemuck::cast_slice(&image.pixels);
            let sampler = sampler(delta.options.magnification, delta.options.wrap_mode);
            let origin = match delta.pos {
                Some([x, y]) => [x as u32, y as u32],
                // A whole image replaces the texture, whose size may change with it.
                None => {
                    let name = format!("egui texture {id:?}");
                    let texture = Texture::new(
                        &self.device,
                        size[0],
                        size[1],
                        // egui's colors are sRGB-encoded, and the shader decodes them after
                        // multiplying, as egui expects.
                        TextureFormat::Rgba8Unorm,
                        &name,
                    )?;
                    self.textures.insert(id, (texture, sampler));
                    [0, 0]
                }
            };
            // egui patches only textures it has set whole before.
            if let Some((texture, current)) = self.textures.get_mut(&id) {
                *current = sampler;
                frame.write_texture(texture, origin, size, pixels)?;
            }
        }
        Ok(())
    }

    /// Draws egui's meshes, with `ppp` physical pixels per point.
    ///
    /// # Panics
    ///
    /// If the pass's target has a format the pass wasn't given.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        primitives: &[ClippedPrimitive],
        ppp: f32,
    ) -> Result<()> {
        let [width, height] = pass.size();
        let screen = [width as f32 / ppp, height as f32 / ppp];
        let (_, pipeline) = self
            .pipelines
            .iter()
            .find(|&&(made_for, _)| made_for == pass.format())
            .expect("a pipeline for each format of target");
        // The whole target, whichever viewport drew before.
        pass.set_viewport([0, 0], [width, height]);
        pass.set_pipeline(pipeline);
        for ClippedPrimitive {
            clip_rect,
            primitive,
        } in primitives
        {
            // Callbacks paint with a renderer of their own, which the engine doesn't offer.
            let Primitive::Mesh(mesh) = primitive else {
                continue;
            };
            let Some((texture, sampler)) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            // The clip rectangle in pixels, rounded as egui's own renderers round it.
            let min_x = (clip_rect.min.x * ppp).round().clamp(0.0, width as f32) as u32;
            let min_y = (clip_rect.min.y * ppp).round().clamp(0.0, height as f32) as u32;
            let max_x = (clip_rect.max.x * ppp).round().clamp(0.0, width as f32) as u32;
            let max_y = (clip_rect.max.y * ppp).round().clamp(0.0, height as f32) as u32;
            if max_x <= min_x || max_y <= min_y || mesh.indices.is_empty() {
                continue;
            }
            pass.set_scissor([min_x, min_y], [max_x - min_x, max_y - min_y]);
            let vertices = pass.upload(bytemuck::cast_slice(&mesh.vertices))?;
            let indices = pass.upload(bytemuck::cast_slice(&mesh.indices))?;
            // The shader's `Push`: the vertices' address, the screen's size in points, then the
            // texture's and sampler's indices.
            let mut push = [0u8; 24];
            push[0..8].copy_from_slice(&vertices.address().to_ne_bytes());
            push[8..12].copy_from_slice(&screen[0].to_ne_bytes());
            push[12..16].copy_from_slice(&screen[1].to_ne_bytes());
            push[16..20].copy_from_slice(&texture.index().to_ne_bytes());
            push[20..24].copy_from_slice(&sampler.index().to_ne_bytes());
            pass.push_constants(&push);
            pass.draw_indexed(indices, 0, mesh.indices.len() as u32, 0);
        }
        Ok(())
    }

    /// Frees the textures egui is done with, once the frame has drawn every target's UI.
    pub(crate) fn free_textures(&mut self) {
        for id in self.to_free.drain(..) {
            // Retired until the GPU is done with the frames that sample it.
            self.textures.remove(&id);
        }
    }
}

/// The sampler for egui's texture options. The engine's samplers filter alike both ways, so
/// egui's minification filter, which its presets set to match, goes unused, as does its mipmap
/// filter: its textures have no mipmaps.
fn sampler(magnification: TextureFilter, wrap: TextureWrapMode) -> Sampler {
    use TextureFilter::{Linear, Nearest};
    use TextureWrapMode::{ClampToEdge, MirroredRepeat, Repeat};
    match (magnification, wrap) {
        (Linear, ClampToEdge) => Sampler::LinearClamp,
        (Nearest, ClampToEdge) => Sampler::NearestClamp,
        (Linear, Repeat) => Sampler::LinearRepeat,
        (Nearest, Repeat) => Sampler::NearestRepeat,
        (Linear, MirroredRepeat) => Sampler::LinearMirror,
        (Nearest, MirroredRepeat) => Sampler::NearestMirror,
    }
}
