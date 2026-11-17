//! What the game's view shows in place of its light, to see how it's made, as the tool API's look
//! and captures ask. Either a part of what lit shading takes or makes, which shading.slang's `lit`
//! returns in place of its light; or an image the frame makes, by debug name, which
//! `shaders/show.slang`'s pass writes into the view's own colors. [`showing`] says how each frame
//! draws.

use std::ops::Range;
use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, ImageKind, PassId};

use crate::{Antialiasing, Grade, Lens, RenderData, Tonemapper};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/show.spv"));

/// A workgroup's side, in pixels: `GROUP` in `shaders/show.slang`.
const GROUP: u32 = 8;

/// What the game's view shows in place of its light ([`RenderData::show`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show<'a> {
    /// A part of what lit shading takes or makes, at each point models draw.
    Shading(Channel),
    /// An image the frame makes, by its debug name as RenderDoc shows it, the last of that name:
    ///
    /// - Colors as they are, HDR ones through the tonemapper.
    /// - Depth as `1 + log2(depth) / 24`.
    /// - IDs and a visibility buffer as a color per value.
    /// - Motion as a normal is.
    /// - A one-float pixel as its value.
    /// - Values as [`Channel`] says the view stores them.
    ///
    /// One the frame doesn't make shows black, warning `renderer.no_image` once.
    Image(&'a str),
}

/// An image a frame made that [`Show::Image`] finds, as [`Renderer::images`] lists them.
///
/// It has its debug name, what it holds, which says how a show reads it, and its size in pixels,
/// which a show stretches over the view.
///
/// [`Renderer::images`]: crate::Renderer::images
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameImage {
    pub name: &'static str,
    pub kind: ImageKind,
    pub size: [u32; 2],
}

/// Lists into `images` the graph's own images added so far, in order, but those at the places
/// `unlisted`: each name once, at its first place, as its last image, which a show finds. It scans
/// those listed for each, of the tens a frame makes, only in a frame asked to list them.
pub(crate) fn list(images: &mut Vec<FrameImage>, graph: &Graph, unlisted: Range<usize>) {
    let listed = graph.images().enumerate();
    let listed = listed.filter(|(i, _)| !unlisted.contains(i));
    for (_, (name, kind, size)) in listed {
        let image = FrameImage { name, kind, size };
        match images.iter_mut().find(|listed| listed.name == name) {
            Some(listed) => *listed = image,
            None => images.push(image),
        }
    }
}

/// A part of what lit shading takes or makes, returned in place of its light:
///
/// - Colors as they are.
/// - Values from 0 to 1, a normal's axes taken from -1..1 to 0..1, as the view's 8-bit sRGB image
///   stores them, as bytes over 255.
/// - Light, exposed, without the haze, which the tonemapper maps.
///
/// An unlit material shows its base color, and black for the rest, which it doesn't read.
/// Shading.slang's `SHOW_` constants number them, as `Channel::index` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Its texture's, vertices' and factor's; a specular-glossiness material's diffuse color.
    BaseColor = 1,
    /// Which way it faces in the world, as shading turns it, normal map included.
    Normal,
    /// Which way it faces before its normal map.
    SurfaceNormal,
    /// 0 for a specular-glossiness material.
    Metallic,
    /// As shading takes it, from 0.045.
    Roughness,
    /// The share of light from all around that reaches it: its occlusion map's and the view's
    /// ambient occlusion's.
    Occlusion,
    /// The light it gives off.
    Emission,
    /// What it reflects head-on: a metallic-roughness material's 0.04 mixed with its base color by
    /// how metallic it is; a specular-glossiness material's specular color.
    SpecularColor,
    /// The frame's lights' shading, shadows and all.
    DirectLight,
    /// The sky's diffuse and specular light, as occlusion darkens it and the view's reflections
    /// replace it.
    SkyLight,
    /// The view's reflections, as much as replaces the sky's specular light.
    Reflection,
    /// Diffused light, the frame's lights' and the sky's.
    DiffuseLight,
    /// Reflected light: the frame's lights' highlights, the sky's specular light and the view's
    /// reflections.
    SpecularLight,
}

impl Channel {
    /// Its number in shading.slang's `SHOW_` constants, which the frame's data carries: from 1, 0
    /// being the view's light.
    pub(crate) fn index(self) -> u32 {
        self as u32
    }

    /// Whether it's light, which the tonemapper maps, rather than a color or a value.
    fn light(self) -> bool {
        matches!(
            self,
            Self::DirectLight
                | Self::SkyLight
                | Self::Reflection
                | Self::DiffuseLight
                | Self::SpecularLight
        )
    }
}

/// `data` as a frame renders it to show something in place of the view's light, as
/// [`RenderData::show`] says. Without antialiasing, each pixel shows as drawn at its center. A part
/// of shading skips the tonemapper unless it's light, so the view's sRGB image stores colors and
/// values as they are. An image comes from a frame drawn as the game's but for antialiasing, so the
/// passes after the view's make their images.
pub(crate) fn showing(data: RenderData<'_>) -> RenderData<'_> {
    match data.show {
        None => data,
        Some(Show::Image(_)) => RenderData {
            antialiasing: Antialiasing::None,
            ..data
        },
        Some(Show::Shading(channel)) => RenderData {
            clear_color: [0.0, 0.0, 0.0, 1.0],
            tonemapper: if channel.light() {
                data.tonemapper
            } else {
                Tonemapper::None
            },
            antialiasing: Antialiasing::None,
            bloom: 0.0,
            grade: Grade::IDENTITY,
            fog: None,
            depth_of_field: None,
            motion_blur: None,
            lens: Lens::NONE,
            ..data
        },
    }
}

/// What the pass pushes: `Push` in `shaders/show.slang`, field for field.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
struct ShowPush {
    size: [u32; 2],
    source: u32,
    kind: u32,
    target: u32,
    unused: u32,
}

/// `kind`'s number, as `shaders/show.slang` has it; 0 is none.
fn kind(kind: ImageKind) -> u32 {
    match kind {
        ImageKind::Color => 1,
        ImageKind::Hdr => 2,
        ImageKind::Ids => 3,
        ImageKind::Motion => 4,
        ImageKind::Visibility => 5,
        ImageKind::Depth => 6,
        ImageKind::Scalar => 7,
    }
}

/// What shows an image the frame makes in the game's view.
pub(crate) struct ShowPass {
    pipeline: ComputePipeline,
}

/// The pass in a frame's graph, the view's size, the image shown and its kind if the frame makes
/// it, and what it writes, which the view shows.
pub(crate) struct Plan {
    pass: PassId,
    size: [u32; 2],
    source: Option<(ImageId, ImageKind)>,
    shown: ImageId,
}

impl Plan {
    pub(crate) fn shown(&self) -> ImageId {
        self.shown
    }

    /// The tonemapper the view shows through: `tonemapper`, the frame's, for HDR colors, which are
    /// light; none for the rest.
    pub(crate) fn tonemapper(&self, tonemapper: Tonemapper) -> Tonemapper {
        match self.source {
            Some((_, ImageKind::Hdr)) => tonemapper,
            _ => Tonemapper::None,
        }
    }
}

impl ShowPass {
    pub(crate) fn new(device: &Arc<Device>) -> Result<Self> {
        Ok(Self {
            pipeline: ComputePipeline::new(device, "show", SPIRV, c"show_main")?,
        })
    }

    /// Declares in `graph` the pass showing `source`, the graph's last image of that name so far,
    /// by its kind, or black if there's none, into colors of the view's own of `size` pixels.
    pub(crate) fn plan(
        graph: &mut Graph,
        source: Option<(ImageId, ImageKind)>,
        size: [u32; 2],
    ) -> Plan {
        let shown = graph.hdr("view shown", size);
        let pass = graph.compute(label!("show"));
        let pass = match source {
            Some((image, _)) => pass.sample(image),
            None => pass,
        };
        Plan {
            pass: pass.overwrite(shown).add(),
            size,
            source,
            shown,
        }
    }

    /// Records `plan`'s pass.
    pub(crate) fn record(&self, frame: &mut Frame<'_>, plan: &Plan) {
        let mut pass = frame.compute(plan.pass);
        let (source, kind) = plan.source.map_or((0, 0), |(image, image_kind)| {
            (pass.sampled(image), kind(image_kind))
        });
        let push = ShowPush {
            size: plan.size,
            source,
            kind,
            target: pass.storage(plan.shown, 0),
            unused: 0,
        };
        pass.set_pipeline(&self.pipeline);
        pass.push_constants(bytemuck::bytes_of(&push));
        let [across, down] = plan.size.map(|side| side.div_ceil(GROUP));
        pass.dispatch([across, down, 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name the graph has twice is listed once, at its first place, as its last image, which a
    /// show finds. A target, which has no name, isn't listed, nor are those at the places left out.
    #[test]
    fn a_name_twice_is_listed_once_as_the_last_of_it() {
        let mut graph = Graph::default();
        graph.target(0);
        graph.depth("depth", [4, 4]);
        graph.ids("ids", [4, 4]);
        graph.depth("depth", [2, 2]);
        graph.hdr("resolved", [4, 4]);
        let mut images = Vec::new();
        list(&mut images, &graph, 3..4);
        let want = [
            FrameImage {
                name: "depth",
                kind: ImageKind::Depth,
                size: [2, 2],
            },
            FrameImage {
                name: "ids",
                kind: ImageKind::Ids,
                size: [4, 4],
            },
        ];
        assert_eq!(images, want);
    }
}
