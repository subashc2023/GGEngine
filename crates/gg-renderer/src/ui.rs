//! egui's meshes, drawn over the frame.

use std::collections::HashMap;
use std::sync::Arc;

use epaint::textures::{TextureFilter, TextureWrapMode, TexturesDelta};
use epaint::{ClippedPrimitive, ImageData, ImageDelta, Primitive, TextureId};
use gg_core::Result;
use gg_gpu::{
    Blend, Device, Format, Frame, GraphicsDesc, MAX_TEXTURE_SIZE, Pass, Pipeline, Sampler, Texture,
    TextureFormat,
};
use tracing::warn;

use crate::Viewport;
use crate::view::{ViewCallback, ViewPass};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/egui.spv"));

/// One frame of egui's output: the meshes to draw, and the texture changes to apply first,
/// which the renderer takes, leaving them empty.
pub struct Ui<'a> {
    pub primitives: &'a [ClippedPrimitive],
    pub textures: &'a mut TexturesDelta,
    /// Physical pixels per egui point.
    pub pixels_per_point: f32,
}

/// The game's view, for the main window's UI to draw where it paints a [`ViewCallback`]: the pass
/// that draws it, its image's index this frame, and its size.
pub(crate) struct View<'a> {
    pub(crate) pass: &'a ViewPass,
    pub(crate) image: u32,
    pub(crate) size: [u32; 2],
}

/// Whether `primitives` paint a [`ViewCallback`].
pub(crate) fn draws_view(primitives: &[ClippedPrimitive]) -> bool {
    primitives.iter().any(|clipped| match &clipped.primitive {
        Primitive::Callback(callback) => callback.callback.is::<ViewCallback>(),
        Primitive::Mesh(_) => false,
    })
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
                    depth_write: false,
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
        let _zone = gg_core::zone!();
        for (id, delta) in self.to_set.drain(..) {
            let ImageData::Color(image) = &delta.image;
            let size = [image.size[0] as u32, image.size[1] as u32];
            let pixels: &[u8] = bytemuck::cast_slice(&image.pixels);
            let sampler = sampler(delta.options.magnification, delta.options.wrap_mode);
            let origin = match delta.pos {
                Some([x, y]) => [x as u32, y as u32],
                // A whole image replaces the texture, whose size may change with it.
                None => {
                    // One that makes none takes its place all the same, so that what samples it
                    // draws nothing.
                    if !makes_texture(id, image.size) {
                        self.textures.remove(&id);
                        continue;
                    }
                    let name = format!("egui texture {id:?}");
                    let texture = Texture::new(
                        &self.device,
                        size[0],
                        size[1],
                        // egui's colors are sRGB-encoded, and the shader decodes them after
                        // multiplying, as egui expects.
                        TextureFormat::Rgba8Unorm,
                        // Drawn at its size, and written in patches, which levels can't be.
                        false,
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

    /// Draws egui's meshes, with `ppp` physical pixels per point, and the game's `view`, given for
    /// the main window's UI alone while it has an area, where the UI paints a [`ViewCallback`].
    ///
    /// # Panics
    ///
    /// If the pass's target has a format the pass wasn't given.
    pub(crate) fn draw(
        &self,
        pass: &mut Pass<'_, '_>,
        primitives: &[ClippedPrimitive],
        ppp: f32,
        view: Option<View<'_>>,
    ) -> Result<()> {
        let _zone = gg_core::zone!();
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
            // The clip rectangle in pixels, rounded as egui's own renderers round it.
            let min_x = (clip_rect.min.x * ppp).round().clamp(0.0, width as f32) as u32;
            let min_y = (clip_rect.min.y * ppp).round().clamp(0.0, height as f32) as u32;
            let max_x = (clip_rect.max.x * ppp).round().clamp(0.0, width as f32) as u32;
            let max_y = (clip_rect.max.y * ppp).round().clamp(0.0, height as f32) as u32;
            if max_x <= min_x || max_y <= min_y {
                continue;
            }
            let clip = Viewport {
                origin: [min_x, min_y],
                size: [max_x - min_x, max_y - min_y],
            };
            let mesh = match primitive {
                Primitive::Mesh(mesh) => mesh,
                // Other callbacks paint with a renderer of their own, which the engine doesn't
                // offer.
                Primitive::Callback(callback) => {
                    if let Some(view) = &view
                        && callback.callback.is::<ViewCallback>()
                    {
                        // From the callback's top left, rounded as the engine rounds the view's
                        // rect to size its image.
                        let min = callback.rect.min.to_vec2() * ppp;
                        let origin = [min.x.round() as u32, min.y.round() as u32];
                        let viewport = Viewport {
                            origin,
                            size: view.size,
                        };
                        view.pass.draw(pass, viewport, view.image, Some(clip));
                        pass.set_viewport([0, 0], [width, height]);
                        pass.set_pipeline(pipeline);
                    }
                    continue;
                }
            };
            let Some((texture, sampler)) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            if mesh.indices.is_empty() {
                continue;
            }
            pass.set_scissor(clip.origin, clip.size);
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
            pass.draw_indexed(indices, 0, mesh.indices.len() as u32, 0, 1);
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

/// Whether a whole image of `size` pixels across and down makes egui's texture `id`: not if it has
/// no pixels, which show nothing, nor if a side is past [`MAX_TEXTURE_SIZE`], as no texture's is,
/// which egui checks only in debug builds, and which this warns of.
fn makes_texture(id: TextureId, size: [usize; 2]) -> bool {
    let sides = 1..=MAX_TEXTURE_SIZE as usize;
    if size.iter().all(|side| sides.contains(side)) {
        return true;
    }
    if !size.contains(&0) {
        warn!(
            code = "renderer.bad_texture",
            texture = ?id,
            "egui texture {id:?} is {}x{} pixels, but textures are 1 to {MAX_TEXTURE_SIZE} pixels \
             a side, so it draws nothing",
            size[0],
            size[1]
        );
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An image without pixels, or with a side past the most a texture has, which egui hands on in
    /// release builds, makes no texture, so that it never reaches `gg-gpu`'s, which would panic the
    /// render thread; every size between does.
    #[test]
    fn an_image_of_no_pixels_or_too_many_makes_no_texture() {
        let id = TextureId::Managed(1);
        let most = MAX_TEXTURE_SIZE as usize;
        for size in [
            [0, 0],
            [0, 16],
            [16, 0],
            [most + 1, 1],
            [1, most + 1],
            [0, most + 1],
        ] {
            assert!(!makes_texture(id, size), "{size:?}");
        }
        for size in [[1, 1], [3, 1], [most, most]] {
            assert!(makes_texture(id, size), "{size:?}");
        }
    }
}
