//! What the game's view shows in place of its light, to see how it's made, as the tool API's look
//! and captures ask for it: a part of what lit shading takes or makes, which shading.slang's `lit`
//! returns in place of its light, shown as drawn and nothing after it; or an image the frame makes,
//! by its debug name, which a pass of its own, as `shaders/show.slang` says, writes into colors of
//! the view's own in place of the view's, from a frame drawn as the game's is but for its
//! antialiasing, so that every pass's images are there to show. [`showing`] says how each frame
//! draws.

use std::sync::Arc;

use bytemuck::NoUninit;
use gg_core::{Result, label};
use gg_gpu::{ComputePipeline, Device, Frame, Graph, ImageId, ImageKind, PassId};

use crate::{Antialiasing, Grade, Lens, RenderData, Tonemapper};

static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/show.spv"));

/// How many pixels across and down a workgroup takes: `GROUP` in `shaders/show.slang`.
const GROUP: u32 = 8;

/// What the game's view shows in place of its light, as [`RenderData::show`] gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show<'a> {
    /// A part of what lit shading takes or makes, at each point of what models draw.
    Shading(Channel),
    /// An image the frame makes, by its debug name, as RenderDoc shows it, the last of that name:
    /// colors as they are, those of high dynamic range through the tonemapper; depth as `1 +
    /// log2(depth) / 24`; IDs and a visibility buffer as a color for each value; motion as a
    /// normal is; a float a pixel as its value; each value as [`Channel`] says the view stores
    /// one. An image the frame doesn't make shows black, warning `renderer.no_image` once.
    Image(&'a str),
}

/// A part of what lit shading takes or makes, which shading returns in place of its light: colors
/// as they are; values, from 0 to 1, a normal's axes from -1 to 1 taken as 0 to 1, as the view's
/// 8-bit sRGB image stores them as their bytes over 255; and light, exposed, without the haze, which
/// the frame's tonemapper maps. Shading.slang's `SHOW_` constants number them, as `Channel::index`
/// does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// The base color, its texture's, its vertices' and its factor's; a specular-glossiness
    /// material's diffuse color.
    BaseColor = 1,
    /// Which way it faces in the world, as shading turns it, its normal map's bumps among it.
    Normal,
    /// Which way it faces before its normal map.
    SurfaceNormal,
    /// How metallic it is: 0 for a specular-glossiness material.
    Metallic,
    /// How rough it is, as shading takes it, from 0.045.
    Roughness,
    /// The share of the light from all around that reaches it: its occlusion map's and the
    /// view's ambient occlusion's.
    Occlusion,
    /// The light it gives off.
    Emission,
    /// What it reflects head-on, as the frame's lights take it: a metallic-roughness material's
    /// 0.04 mixed with its base color by how metallic it is; a specular-glossiness material's
    /// specular color.
    SpecularColor,
    /// The light the frame's lights shade it with, shadows and all; none for what's unlit.
    DirectLight,
    /// The sky's light from all around, diffuse and specular, as occlusion darkens it and the
    /// view's reflections take the place of it.
    SkyLight,
    /// The view's reflections, as much as takes the place of the sky's specular light.
    Reflection,
    /// The light it diffuses, the frame's lights' and the sky's.
    DiffuseLight,
    /// The light it reflects: the frame's lights' highlights, the sky's specular light and the
    /// view's reflections.
    SpecularLight,
}

impl Channel {
    /// Its number, as shading.slang's `SHOW_` constants have it, which the frame's data carries:
    /// from 1, 0 being the view's light.
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

/// `data` as a frame renders it while it shows something in place of the view's light, with no
/// antialiasing, so that the view shows it as drawn, a pixel at each pixel's center. A part of
/// shading shows with no fog, depth of field, motion blur, bloom, grade or lens either, cleared to
/// black, without the sky, and through no tonemapper unless it shows light, so that the view's sRGB
/// image stores colors and values as they are. An image shows from a frame drawn as the game's is
/// but for that, so that the passes after the view's make theirs. Either way the frame keeps its
/// exposure as it adapts, metering none of it.
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

/// The pass in a frame's graph: the pass, the view's size, the image it shows and its kind, if the
/// frame makes it, and what it writes, which the view shows.
pub(crate) struct Plan {
    pass: PassId,
    size: [u32; 2],
    source: Option<(ImageId, ImageKind)>,
    shown: ImageId,
}

impl Plan {
    /// What the view shows.
    pub(crate) fn shown(&self) -> ImageId {
        self.shown
    }

    /// The tonemapper the view's shown through, of `tonemapper`, the frame's: none but for colors
    /// of high dynamic range, which are light.
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

    /// Declares in `graph` the pass that shows `source`, the last image of the graph's added so
    /// far of that name, and its kind, or black if there's none, in colors of the view's own of
    /// `size` pixels, made in `graph`.
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
