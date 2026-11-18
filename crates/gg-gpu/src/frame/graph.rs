//! A frame's render graph: the passes a frame runs, in the order they're added, and how each uses
//! images (render into, compute into, sample, copy from) and buffers (shaders write and read, draws
//! take commands from). The frame places every barrier and layout transition from those uses as
//! each pass begins.
//!
//! The graph's own images and buffers last for a frame and are made from what the passes say of
//! them. Images whose uses don't overlap share memory, as `transients` says.

use std::ops::Range;

use ash::vk;
use gg_core::profile::Label;

use crate::{COLOR_RANGE, ColorImage, DEPTH_RANGE};

/// An image a [`Graph`]'s passes use: one of the frame's targets, or one of the graph's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageId(pub(crate) u32);

/// A buffer of a [`Graph`]'s own, which its passes use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferId(pub(crate) u32);

/// A pass a [`Graph`] runs: the frame records them in the order they were added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassId(pub(crate) u32);

/// The most bytes of a buffer a pass reads back ([`Graph::read_back`]).
pub const MAX_BUFFER_READ: u64 = 256;

/// The most buffers a frame reads back ([`Graph::read_back`]).
pub const MAX_BUFFER_READS: usize = 4;

/// What a frame runs: its passes, in order, each with the images it uses and how.
///
/// A frame places the barriers and layout transitions between passes from the image's last use
/// before a pass and the pass's own, so no pass records one itself. Clear it and add the frame's
/// passes every frame; it keeps its memory, so a frame that adds no more than the last allocates
/// nothing.
///
/// Its own images ([`Graph::color`], [`Graph::hdr`], [`Graph::ids`], [`Graph::motion`],
/// [`Graph::visibility`], [`Graph::depth`], [`Graph::scalar`], [`Graph::normals`]) last for a
/// frame. Each is undefined until a pass writes it, so its first use writes it whole, and passes
/// whose uses of two of them don't overlap may find them in the same memory. The frames keep them
/// for the frames after, which add them alike, by name, kind and size.
///
/// Its own buffers ([`Graph::buffer`]) last from frame to frame: the frames keep each for the
/// frames after that add one of its name no larger, holding what the last of them left there.
#[derive(Default)]
pub struct Graph {
    pub(crate) images: Vec<ImageDecl>,
    pub(crate) buffers: Vec<BufferDecl>,
    pub(crate) passes: Vec<PassDecl>,
    /// Each pass's image uses, one after another, in the order given.
    pub(crate) uses: Vec<(ImageId, Use)>,
    /// Each pass's buffer uses, likewise.
    pub(crate) buffer_uses: Vec<(BufferId, BufferUse)>,
}

/// A buffer of a graph's own: its debug name and its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BufferDecl {
    pub(crate) name: &'static str,
    pub(crate) size: u64,
}

/// An image of a graph's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageDecl {
    /// The frame's target of that index.
    Target(usize),
    /// One of the graph's own, `name` its debug name.
    Transient {
        name: &'static str,
        kind: ImageKind,
        size: [u32; 2],
    },
}

/// What a graph's own image holds, as [`Graph::image_named`] tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// Colors in [`ColorImage::FORMAT`]. Passes render into and blend over them, later ones
    /// sample them and frames read them back.
    Color,
    /// Colors of high dynamic range, as 16-bit floats in linear RGBA, [`Format::HDR`]. Passes
    /// render into and blend over them, past 1 too, and later ones sample them. Vulkan requires
    /// every device to render into the format, blend in it and sample it.
    ///
    /// [`Format::HDR`]: crate::Format::HDR
    Hdr,
    /// IDs beside a pass's colors: two 32-bit unsigned integers, the low and high halves of a
    /// 64-bit one. Passes render them, frames read pixels back and later passes sample them as
    /// unsigned integers. Vulkan requires every device to render into the format, copy from it and
    /// sample it.
    Ids,
    /// Motion beside a pass's colors and IDs, as 16-bit floats in RGBA, the format of [`Hdr`]'s.
    /// Passes render it, the pipelines that write depth write it, compute passes write it as
    /// storage, and later passes sample it.
    ///
    /// [`Hdr`]: Self::Hdr
    Motion,
    /// A visibility buffer: two 32-bit unsigned integers a pixel, in the IDs' format. A pass
    /// renders it as its color target, beside its IDs, telling what it drew there; later passes
    /// sample it as unsigned integers.
    Visibility,
    /// Depth as 32-bit floats that draws test and write, from 1 at the near plane to 0 far away,
    /// as the renderer's cameras project it. Later passes sample it by comparison, through the
    /// bindless set's comparison sampler, as a shadow map's is, or a texel at a time, as the view's
    /// is to build its depth pyramid. Every device the engine opens renders it and samples it both
    /// ways.
    Depth,
    /// A 32-bit float a pixel that compute passes write as storage and later passes sample, as
    /// the view's ambient occlusion is. Vulkan requires every device to store to the format from
    /// shaders and to sample it.
    Scalar,
    /// Unit normals in RGB, as [`Hdr`]'s format and uses, so passes render and write them alike;
    /// a show reads them as normals rather than light.
    ///
    /// [`Hdr`]: Self::Hdr
    Normals,
}

impl ImageKind {
    pub(crate) const fn format(self) -> vk::Format {
        match self {
            Self::Color => ColorImage::FORMAT.color,
            Self::Hdr | Self::Motion | Self::Normals => vk::Format::R16G16B16A16_SFLOAT,
            Self::Ids | Self::Visibility => vk::Format::R32G32_UINT,
            Self::Depth => vk::Format::D32_SFLOAT,
            Self::Scalar => vk::Format::R32_SFLOAT,
        }
    }

    /// What its images are made for: every use a pass may make of its kind.
    pub(crate) fn usage(self) -> vk::ImageUsageFlags {
        match self {
            Self::Color | Self::Ids => {
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
            }
            // Never read back: a frame reads back what it shows. Vulkan requires every device to
            // support storage in the format, which compute passes write.
            Self::Hdr | Self::Motion | Self::Normals => {
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::STORAGE
            }
            Self::Visibility => {
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
            }
            Self::Depth => {
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
            }
            Self::Scalar => vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
        }
    }

    pub(crate) fn range(self) -> vk::ImageSubresourceRange {
        match self {
            Self::Color
            | Self::Hdr
            | Self::Ids
            | Self::Motion
            | Self::Visibility
            | Self::Scalar
            | Self::Normals => COLOR_RANGE,
            Self::Depth => DEPTH_RANGE,
        }
    }

    /// Whether shaders may sample it, through a bindless slot.
    pub(crate) fn sampled(self) -> bool {
        self.usage().contains(vk::ImageUsageFlags::SAMPLED)
    }

    /// Whether compute shaders may write it, through a bindless storage slot.
    pub(crate) fn storage(self) -> bool {
        self.usage().contains(vk::ImageUsageFlags::STORAGE)
    }
}

/// How a pass uses an image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Use {
    /// Its color target, cleared first to this linear RGBA color (a visibility buffer, to 0, its
    /// only clear); with `None`, drawn over as the image's last use left it.
    Color(Option<[f32; 4]>),
    /// The IDs beside its color target, cleared first to 0, for none, unless `kept`: as the
    /// image's last use left them.
    Ids { kept: bool },
    /// The motion beside its color target and IDs, cleared first to 0, unless `kept`: as the
    /// image's last use left it.
    Motion { kept: bool },
    /// The depth its draws test and write, cleared first to 0, as far away as depth goes, unless
    /// `kept`: as the image's last use left it.
    Depth { kept: bool },
    /// Sampled by its fragment shaders, through the image's bindless slot.
    Sampled,
    /// Sampled by its compute shaders, through the image's bindless slot.
    ComputeSampled,
    /// Written and read by its compute shaders, a level at a time, through the storage slots of a
    /// storage image's levels or of a graph image of a kind they write, in the general layout.
    /// With `kept`, as its last use left it; otherwise that is discarded, for shaders that write
    /// every texel.
    Storage { kept: bool },
    /// Copied from, as a read back is.
    CopySource,
}

impl Use {
    /// What the use does to the image.
    pub(crate) fn access(self) -> Access {
        let color = |reads, discards| Access {
            stages: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            reads,
            writes: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            discards,
        };
        match self {
            Self::Color(Some(_)) | Self::Ids { kept: false } | Self::Motion { kept: false } => {
                color(vk::AccessFlags2::NONE, true)
            }
            // Blending reads what's there, as loading what's kept does.
            Self::Color(None) | Self::Ids { kept: true } | Self::Motion { kept: true } => {
                color(vk::AccessFlags2::COLOR_ATTACHMENT_READ, false)
            }
            // A clear or a load writes or reads at the early fragment tests, and a store writes at
            // the late ones.
            Self::Depth { kept } => Access {
                stages: vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                reads: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ,
                writes: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                discards: !kept,
            },
            Self::Sampled => Access {
                stages: vk::PipelineStageFlags2::FRAGMENT_SHADER,
                reads: vk::AccessFlags2::SHADER_SAMPLED_READ,
                writes: vk::AccessFlags2::NONE,
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                discards: false,
            },
            Self::ComputeSampled => Access {
                stages: vk::PipelineStageFlags2::COMPUTE_SHADER,
                ..Self::Sampled.access()
            },
            Self::Storage { kept } => Access {
                stages: vk::PipelineStageFlags2::COMPUTE_SHADER,
                reads: vk::AccessFlags2::SHADER_STORAGE_READ,
                writes: vk::AccessFlags2::SHADER_STORAGE_WRITE,
                layout: vk::ImageLayout::GENERAL,
                discards: !kept,
            },
            Self::CopySource => Access {
                stages: vk::PipelineStageFlags2::COPY,
                reads: vk::AccessFlags2::TRANSFER_READ,
                writes: vk::AccessFlags2::NONE,
                layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                discards: false,
            },
        }
    }

    fn attaches(self) -> bool {
        matches!(
            self,
            Self::Color(_) | Self::Ids { .. } | Self::Motion { .. } | Self::Depth { .. }
        )
    }
}

/// How a pass uses a buffer of the graph's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BufferUse {
    /// Read by its compute shaders, through the buffer's address.
    ComputeRead,
    /// Written and read by its compute shaders, likewise.
    ComputeWrite,
    /// Read by its draws' vertex, mesh or fragment shaders, through the buffer's address.
    Read,
    /// The commands its indirect draws take. Its draws' shaders may read them too, through the
    /// buffer's address, as a draw of meshlets reads those kept after its command.
    Indirect,
    /// Copied from, as a read back is.
    CopySource,
}

impl BufferUse {
    pub(crate) fn writes(self) -> bool {
        self == Self::ComputeWrite
    }

    /// What the use does to the buffer, in no layout, as buffers have none.
    pub(crate) fn access(self) -> Access {
        let access = |stages, reads, writes| Access {
            stages,
            reads,
            writes,
            layout: vk::ImageLayout::UNDEFINED,
            discards: false,
        };
        use vk::{AccessFlags2 as A, PipelineStageFlags2 as S};
        match self {
            Self::ComputeRead => access(S::COMPUTE_SHADER, A::SHADER_STORAGE_READ, A::NONE),
            Self::ComputeWrite => access(
                S::COMPUTE_SHADER,
                A::SHADER_STORAGE_READ,
                A::SHADER_STORAGE_WRITE,
            ),
            // Every stage before rasterizing, so that a mesh shader's reads are among them.
            Self::Read => access(
                S::PRE_RASTERIZATION_SHADERS | S::FRAGMENT_SHADER,
                A::SHADER_STORAGE_READ,
                A::NONE,
            ),
            Self::Indirect => access(
                S::DRAW_INDIRECT | S::PRE_RASTERIZATION_SHADERS | S::FRAGMENT_SHADER,
                A::INDIRECT_COMMAND_READ | A::SHADER_STORAGE_READ,
                A::NONE,
            ),
            Self::CopySource => access(S::COPY, A::TRANSFER_READ, A::NONE),
        }
    }
}

/// What a use does to an image: the stages it's in, the reads and writes they make there, the
/// layout, and whether it discards what the image held, as a clear does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Access {
    pub(crate) stages: vk::PipelineStageFlags2,
    pub(crate) reads: vk::AccessFlags2,
    pub(crate) writes: vk::AccessFlags2,
    pub(crate) layout: vk::ImageLayout,
    pub(crate) discards: bool,
}

/// A pass of a graph's.
pub(crate) struct PassDecl {
    pub(crate) label: Label,
    pub(crate) kind: PassKind,
    /// The queue it was added to run on.
    pub(crate) queue: Queue,
    /// Its uses, in [`Graph::uses`].
    pub(crate) uses: Range<usize>,
    /// Its buffer uses, in [`Graph::buffer_uses`].
    pub(crate) buffer_uses: Range<usize>,
}

/// A queue a pass runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Queue {
    Graphics,
    /// The device's separate compute queue where it has one; else the graphics queue.
    Compute,
}

impl Queue {
    /// Its index among a frame's queues.
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Graphics => 0,
            Self::Compute => 1,
        }
    }

    pub(crate) const fn other(self) -> Self {
        match self {
            Self::Graphics => Self::Compute,
            Self::Compute => Self::Graphics,
        }
    }
}

/// What a pass does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassKind {
    /// Draws into its attachments, as [`Frame::render`](crate::Frame::render) records it.
    Render,
    /// Reads pixels of IDs back, as [`Frame::read_ids`](crate::Frame::read_ids) does.
    ReadIds,
    /// Reads a color image back, as [`Frame::read_image`](crate::Frame::read_image) does.
    ReadImage,
    /// Dispatches compute shaders, as [`Frame::compute`](crate::Frame::compute) records them.
    Compute,
    /// Reads a buffer back, as [`Frame::read_back`](crate::Frame::read_back) does.
    ReadBack,
}

impl Graph {
    /// Empties it for the next frame, keeping its memory.
    pub fn clear(&mut self) {
        self.images.clear();
        self.buffers.clear();
        self.passes.clear();
        self.uses.clear();
        self.buffer_uses.clear();
    }

    /// The frame's `target`th target, as [`Frames::begin`](crate::Frames::begin) takes them: the
    /// same image each time it's asked for.
    pub fn target(&mut self, target: usize) -> ImageId {
        let decl = ImageDecl::Target(target);
        match self.images.iter().position(|&image| image == decl) {
            Some(i) => ImageId(i as u32),
            None => self.add_image(decl),
        }
    }

    /// A graph image of colors, `size` pixels, named `name`, in [`ColorImage::FORMAT`] as a
    /// target's, so the same pipelines draw into either.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn color(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, ImageKind::Color, size)
    }

    /// A graph image of high dynamic range colors, `size` pixels, named `name`, in
    /// [`Format::HDR`](crate::Format::HDR)'s 16-bit floats. They keep what's brighter than 1 for a
    /// later pass to sample and map into a target's range, as tonemapping does. Passes draw into
    /// it and compute passes write it as storage. Frames don't read it back.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn hdr(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, ImageKind::Hdr, size)
    }

    /// A graph image of IDs, `size` pixels, named `name`. A pass renders it beside its colors,
    /// frames read pixels back ([`Graph::read_ids`]) and later passes may sample it as unsigned
    /// integers.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn ids(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, ImageKind::Ids, size)
    }

    /// A graph image of motion, `size` pixels, named `name`, 16-bit floats in RGBA. A pass renders
    /// it beside its colors and IDs ([`RenderPass::motion`]), through pipelines made for a format
    /// [`with_motion`](crate::Format::with_motion) that write depth. Compute passes write it as
    /// storage, and later passes sample it.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn motion(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, ImageKind::Motion, size)
    }

    /// A graph visibility buffer, `size` pixels, named `name`: two 32-bit unsigned integers a
    /// pixel. A pass draws into it as its color target ([`RenderPass::visibility`]), beside its
    /// IDs, through pipelines made for [`Format::VISIBILITY`](crate::Format::VISIBILITY), telling
    /// what it drew there. Later passes sample it as unsigned integers.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn visibility(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, ImageKind::Visibility, size)
    }

    /// A graph image of depth, `size` pixels, named `name`. A pass's draws test and write it, and
    /// later passes may sample it by comparison, through the bindless set's comparison sampler
    /// [`COMPARISON_SAMPLER`], as a shadow map is, or a texel at a time.
    ///
    /// [`COMPARISON_SAMPLER`]: crate::COMPARISON_SAMPLER
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn depth(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, ImageKind::Depth, size)
    }

    /// A graph image of a 32-bit float a pixel, `size` pixels, named `name`. Compute passes write
    /// it as storage, through a `RWTexture2DArray<float>` of format `r32f`, and later passes sample
    /// it, as the view's ambient occlusion is.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn scalar(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, ImageKind::Scalar, size)
    }

    /// A graph image of unit normals, `size` pixels, named `name`, made and used as
    /// [`Graph::hdr`]'s: passes draw into it and compute passes write it as storage.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn normals(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, ImageKind::Normals, size)
    }

    /// The graph's own images added so far, in order, each with its debug name, kind and size.
    /// [`Graph::image_named`] finds the last of a name.
    pub fn images(&self) -> impl Iterator<Item = (&'static str, ImageKind, [u32; 2])> + '_ {
        self.images.iter().filter_map(|image| match *image {
            ImageDecl::Target(_) => None,
            ImageDecl::Transient { name, kind, size } => Some((name, kind, size)),
        })
    }

    /// The last graph image added so far that's named `name`, and its kind, if any: so a pass added
    /// after it may sample it, as one that shows it does.
    pub fn image_named(&self, name: &str) -> Option<(ImageId, ImageKind)> {
        let found = self.images.iter().rposition(
            |image| matches!(image, ImageDecl::Transient { name: named, .. } if *named == name),
        )?;
        let id = ImageId(found as u32);
        self.kind(id).map(|kind| (id, kind))
    }

    fn transient(&mut self, name: &'static str, kind: ImageKind, size: [u32; 2]) -> ImageId {
        let [width, height] = size;
        assert!(
            width > 0 && height > 0,
            "image {name:?} is {width}x{height}, but an image has an area"
        );
        self.add_image(ImageDecl::Transient { name, kind, size })
    }

    fn add_image(&mut self, decl: ImageDecl) -> ImageId {
        self.images.push(decl);
        ImageId(self.images.len() as u32 - 1)
    }

    /// A graph buffer of `size` bytes, named `name`, in the GPU's own memory. Compute shaders write
    /// it through its address, as a pass that writes it says; later passes' shaders read it, draws
    /// take their commands from it, or frames read it back. The frames keep it by name: a pass
    /// that writes it finds what the last frame to add it left, and zeros where no frame has
    /// written since the frames made it. They make it anew after a frame that added none of its
    /// name, and may after one that added it smaller.
    ///
    /// # Panics
    ///
    /// - `size` is 0.
    /// - The graph has a buffer of that name already.
    pub fn buffer(&mut self, name: &'static str, size: u64) -> BufferId {
        assert!(
            size > 0,
            "buffer {name:?} is empty, but a buffer holds a byte"
        );
        assert!(
            self.buffers.iter().all(|decl| decl.name != name),
            "the graph has a buffer named {name:?} already; frames keep its buffers by their names"
        );
        self.buffers.push(BufferDecl { name, size });
        BufferId(self.buffers.len() as u32 - 1)
    }

    /// A pass that draws, named `label`. The builder takes its attachments, sampled images and
    /// read buffers before [`RenderPass::add`] adds it.
    pub fn render(&mut self, label: Label) -> RenderPass<'_> {
        let start = (self.uses.len(), self.buffer_uses.len());
        RenderPass {
            graph: self,
            label,
            start,
        }
    }

    /// A pass that dispatches compute shaders, named `label`. The builder takes its images and
    /// buffers before [`ComputePass::add`] adds it, to run on the graphics queue unless
    /// [`ComputePass::on_compute_queue`] says otherwise.
    pub fn compute(&mut self, label: Label) -> ComputePass<'_> {
        let start = (self.uses.len(), self.buffer_uses.len());
        ComputePass {
            graph: self,
            label,
            start,
            queue: Queue::Graphics,
        }
    }

    /// A pass, named `label`, that reads `buffer` back as an earlier pass left it, as
    /// [`Frame::read_back`](crate::Frame::read_back) records it.
    ///
    /// # Panics
    ///
    /// - `buffer` isn't the graph's, holds more than [`MAX_BUFFER_READ`] bytes, or no earlier pass
    ///   wrote it.
    /// - The graph reads back [`MAX_BUFFER_READS`] buffers already.
    pub fn read_back(&mut self, label: Label, buffer: BufferId) -> PassId {
        let reads = (self.passes.iter())
            .filter(|pass| pass.kind == PassKind::ReadBack)
            .count();
        assert!(
            reads < MAX_BUFFER_READS,
            "pass {label:?} reads back a buffer, but a frame reads back {MAX_BUFFER_READS} at most"
        );
        let size = (self.buffers.get(buffer.0 as usize))
            .map(|decl| decl.size)
            .unwrap_or_else(|| {
                panic!(
                    "pass {label:?} reads back buffer {}, not the graph's",
                    buffer.0
                )
            });
        assert!(
            size <= MAX_BUFFER_READ,
            "pass {label:?} reads back buffer {}, of {size} bytes, but a pass reads back \
             {MAX_BUFFER_READ} at most",
            buffer.0
        );
        let start = (self.uses.len(), self.buffer_uses.len());
        self.buffer_uses.push((buffer, BufferUse::CopySource));
        self.add_pass(label, PassKind::ReadBack, Queue::Graphics, start)
    }

    /// A pass, named `label`, that reads pixels of `ids` back as an earlier pass left them, as
    /// [`Frame::read_ids`](crate::Frame::read_ids) records it.
    ///
    /// # Panics
    ///
    /// If `ids` isn't a graph image of IDs, or no earlier pass wrote it.
    pub fn read_ids(&mut self, label: Label, ids: ImageId) -> PassId {
        assert!(
            matches!(self.kind(ids), Some(ImageKind::Ids)),
            "pass {label:?} reads IDs back from an image of IDs of the graph's own, which image {} \
             isn't",
            ids.0
        );
        self.read(label, PassKind::ReadIds, ids)
    }

    /// A pass, named `label`, that reads `image`, a target or a graph color image, back as an
    /// earlier pass left it, as [`Frame::read_image`](crate::Frame::read_image) records it.
    ///
    /// # Panics
    ///
    /// If `image` is neither, or is a graph image that no earlier pass wrote.
    pub fn read_image(&mut self, label: Label, image: ImageId) -> PassId {
        assert!(
            matches!(self.kind(image), None | Some(ImageKind::Color)),
            "pass {label:?} reads back a target or a color image, which image {} isn't",
            image.0
        );
        self.read(label, PassKind::ReadImage, image)
    }

    fn read(&mut self, label: Label, kind: PassKind, image: ImageId) -> PassId {
        let start = (self.uses.len(), self.buffer_uses.len());
        self.uses.push((image, Use::CopySource));
        self.add_pass(label, kind, Queue::Graphics, start)
    }

    /// Adds the pass on `queue`, whose image and buffer uses start at `start`, once checked.
    fn add_pass(
        &mut self,
        label: Label,
        kind: PassKind,
        queue: Queue,
        start: (usize, usize),
    ) -> PassId {
        let (start, buffers_start) = start;
        let uses = start..self.uses.len();
        for &(image, use_) in &self.uses[uses.clone()] {
            assert!(
                (image.0 as usize) < self.images.len(),
                "pass {label:?} uses image {}, which isn't the graph's",
                image.0
            );
            // What's undefined until a pass writes it can't be read before one has.
            let used = self.uses[..start].iter().any(|&(used, _)| used == image);
            assert!(
                self.kind(image).is_none() || used || use_.access().discards,
                "pass {label:?} uses image {} before any pass writes it, but it's the graph's own, \
                 undefined until then",
                image.0
            );
        }
        let buffer_uses = buffers_start..self.buffer_uses.len();
        let pass_uses = &self.buffer_uses[buffer_uses.clone()];
        for (i, &(buffer, use_)) in pass_uses.iter().enumerate() {
            assert!(
                (buffer.0 as usize) < self.buffers.len(),
                "pass {label:?} uses buffer {}, which isn't the graph's",
                buffer.0
            );
            assert!(
                pass_uses[..i].iter().all(|&(used, _)| used != buffer),
                "pass {label:?} uses buffer {} twice",
                buffer.0
            );
            let written = (self.buffer_uses[..buffers_start].iter())
                .any(|&(used, use_)| used == buffer && use_ == BufferUse::ComputeWrite);
            assert!(
                written || use_ == BufferUse::ComputeWrite,
                "pass {label:?} uses buffer {} before any pass of the frame writes it",
                buffer.0
            );
        }
        self.passes.push(PassDecl {
            label,
            kind,
            queue,
            uses,
            buffer_uses,
        });
        PassId(self.passes.len() as u32 - 1)
    }

    /// The kind of `image` if it's a graph image; `None` for a target.
    pub(crate) fn kind(&self, image: ImageId) -> Option<ImageKind> {
        match self.images.get(image.0 as usize)? {
            ImageDecl::Target(_) => None,
            &ImageDecl::Transient { kind, .. } => Some(kind),
        }
    }

    pub(crate) fn uses(&self, pass: PassId) -> &[(ImageId, Use)] {
        &self.uses[self.passes[pass.0 as usize].uses.clone()]
    }

    pub(crate) fn buffer_uses(&self, pass: PassId) -> &[(BufferId, BufferUse)] {
        &self.buffer_uses[self.passes[pass.0 as usize].buffer_uses.clone()]
    }

    /// The first and the last of the passes that use `image`, if any does.
    pub(crate) fn lifetime(&self, image: ImageId) -> Option<(usize, usize)> {
        let mut lifetime = None;
        for (i, pass) in self.passes.iter().enumerate() {
            if self.uses[pass.uses.clone()]
                .iter()
                .any(|&(used, _)| used == image)
            {
                let (first, _) = lifetime.unwrap_or((i, i));
                lifetime = Some((first, i));
            }
        }
        lifetime
    }
}

/// A pass that draws, being built. [`RenderPass::add`] adds it to its graph.
///
/// It holds its color target and any IDs and depth beside it, or its depth alone, as a shadow map's
/// pass draws; the images it samples; and the buffers its shaders read and its draws take their
/// commands from.
#[must_use = "a pass is in its graph once it's added"]
pub struct RenderPass<'g> {
    graph: &'g mut Graph,
    label: Label,
    /// Where its image and buffer uses start in the graph's.
    start: (usize, usize),
}

impl RenderPass<'_> {
    /// Draws into `image`, a target or a graph color image, of either range, cleared first to
    /// `color`, in linear RGBA.
    pub fn clear(self, image: ImageId, color: [f32; 4]) -> Self {
        self.with(image, Use::Color(Some(color)))
    }

    /// Draws into `image` over what its last use left there, as [`clear`](Self::clear),
    /// [`visibility`](Self::visibility), [`ids`](Self::ids), [`motion`](Self::motion) or
    /// [`depth`](Self::depth) would but for the clear. `image` is a target or graph color image of
    /// either range, whose colors its draws blend over; a visibility buffer; an image of IDs or
    /// motion; or one of depth, which its draws test against. A target no pass has drawn into
    /// since it was made, or since its swapchain's image was acquired, holds nothing to draw over.
    pub fn keep(self, image: ImageId) -> Self {
        let use_ = match self.graph.kind(image) {
            Some(ImageKind::Ids) => Use::Ids { kept: true },
            Some(ImageKind::Motion) => Use::Motion { kept: true },
            Some(ImageKind::Depth) => Use::Depth { kept: true },
            None
            | Some(
                ImageKind::Color
                | ImageKind::Hdr
                | ImageKind::Normals
                | ImageKind::Visibility
                | ImageKind::Scalar,
            ) => Use::Color(None),
        };
        self.with(image, use_)
    }

    /// Draws into `visibility`, a graph visibility buffer, as the color target, cleared first to
    /// 0.
    pub fn visibility(self, visibility: ImageId) -> Self {
        self.with(visibility, Use::Color(Some([0.0; 4])))
    }

    /// Draws the draws' IDs into `ids`, a graph image of IDs, cleared first to 0. Pipelines made
    /// for a format [`with_ids`](crate::Format::with_ids) write them.
    pub fn ids(self, ids: ImageId) -> Self {
        self.with(ids, Use::Ids { kept: false })
    }

    /// Draws the draws' motion into `motion`, a graph image of motion, beside their IDs, cleared
    /// first to 0. Pipelines made for a format [`with_motion`](crate::Format::with_motion) write it
    /// where they write depth.
    pub fn motion(self, motion: ImageId) -> Self {
        self.with(motion, Use::Motion { kept: false })
    }

    /// Tests the draws against `depth`, a graph image of depth, and writes theirs into it as their
    /// pipelines say, cleared first to 0, as far away as depth goes.
    pub fn depth(self, depth: ImageId) -> Self {
        self.with(depth, Use::Depth { kept: false })
    }

    /// Samples `image` from the draws' fragment shaders as an earlier pass left it, through the
    /// index [`Pass::sampled`](crate::Pass::sampled) gives. It is an offscreen or storage target,
    /// or a graph image of colors, IDs or depth.
    pub fn sample(self, image: ImageId) -> Self {
        self.with(image, Use::Sampled)
    }

    /// Reads `buffer` from the draws' shaders as an earlier pass wrote it, through the address
    /// [`Pass::address`](crate::Pass::address) gives.
    pub fn read_buffer(self, buffer: BufferId) -> Self {
        self.graph.buffer_uses.push((buffer, BufferUse::Read));
        self
    }

    /// Takes the commands of indirect draws from `buffer` as an earlier pass wrote them, as
    /// [`Pass::draw_indexed_indirect`](crate::Pass::draw_indexed_indirect) draws them. Its draws'
    /// shaders may read it too, through the address [`Pass::address`](crate::Pass::address) gives.
    pub fn indirect(self, buffer: BufferId) -> Self {
        self.graph.buffer_uses.push((buffer, BufferUse::Indirect));
        self
    }

    fn with(self, image: ImageId, use_: Use) -> Self {
        self.graph.uses.push((image, use_));
        self
    }

    /// Adds the pass to its graph.
    ///
    /// # Panics
    ///
    /// - It hasn't one color target, beside at most one image of IDs, one of motion beside those,
    ///   and one of depth, or one image of depth alone.
    /// - It uses an image twice, as another kind of image, or as graph images of different sizes.
    /// - It clears a visibility buffer to anything but 0.
    /// - It samples or draws over a graph image no earlier pass wrote.
    /// - It uses a buffer twice, or one no earlier pass wrote.
    pub fn add(self) -> PassId {
        let Self {
            graph,
            label,
            start,
        } = self;
        let uses = &graph.uses[start.0..];
        let count = |is: fn(&Use) -> bool| uses.iter().filter(|(_, use_)| is(use_)).count();
        let (colors, ids, motions, depths) = (
            count(|use_| matches!(use_, Use::Color(_))),
            count(|use_| matches!(use_, Use::Ids { .. })),
            count(|use_| matches!(use_, Use::Motion { .. })),
            count(|use_| matches!(use_, Use::Depth { .. })),
        );
        // Motion is the third color attachment, so it's only beside IDs.
        assert!(
            (colors == 1 && ids <= 1 && motions <= ids && depths <= 1)
                || (colors, ids, motions, depths) == (0, 0, 0, 1),
            "pass {label:?} draws into one color target, beside at most one image of IDs, one of \
             motion beside those, and one of depth, or into one image of depth alone; it has \
             {uses:?}"
        );
        let mut size = None;
        for (i, &(image, use_)) in uses.iter().enumerate() {
            assert!(
                uses[..i].iter().all(|&(used, _)| used != image),
                "pass {label:?} uses image {} twice; an image is in one layout at a time",
                image.0
            );
            let kind = graph.kind(image);
            let fits = match use_ {
                // A visibility buffer clears to 0 alone, as its integers take no color.
                Use::Color(clear) if kind == Some(ImageKind::Visibility) => {
                    clear.is_none_or(|clear| clear == [0.0; 4])
                }
                Use::Color(_) => matches!(
                    kind,
                    None | Some(ImageKind::Color | ImageKind::Hdr | ImageKind::Normals)
                ),
                Use::Ids { .. } => kind == Some(ImageKind::Ids),
                Use::Motion { .. } => kind == Some(ImageKind::Motion),
                Use::Depth { .. } => kind == Some(ImageKind::Depth),
                Use::Sampled => kind.is_none_or(ImageKind::sampled),
                Use::ComputeSampled | Use::Storage { .. } | Use::CopySource => {
                    unreachable!("a render pass neither computes nor copies")
                }
            };
            assert!(
                fits,
                "pass {label:?} uses image {} as {use_:?}, but it's {}",
                image.0,
                kind.map_or_else(|| "a target".to_owned(), |kind| format!("{kind:?}"))
            );
            if use_.attaches()
                && let Some(&ImageDecl::Transient { size: own, .. }) =
                    graph.images.get(image.0 as usize)
            {
                assert!(
                    size.is_none_or(|size| size == own),
                    "pass {label:?} draws into images of different sizes, {:?} and {own:?}",
                    size.unwrap_or_default()
                );
                size = Some(own);
            }
        }
        graph.add_pass(label, PassKind::Render, Queue::Graphics, start)
    }
}

/// A pass that dispatches compute shaders, being built: the images it samples and writes, the
/// buffers it reads and writes, and the queue it runs on. [`ComputePass::add`] adds it to its
/// graph.
#[must_use = "a pass is in its graph once it's added"]
pub struct ComputePass<'g> {
    graph: &'g mut Graph,
    label: Label,
    /// Where its image and buffer uses start in the graph's.
    start: (usize, usize),
    queue: Queue,
}

impl ComputePass<'_> {
    /// Runs the pass on the device's separate compute queue where it has one, as the RTX 4090 and
    /// AMD's GPUs do, else on the graphics queue, as on lavapipe. The frame records it in its place
    /// among the graph's passes either way.
    ///
    /// A pass that must follow a pass on the other queue that used a buffer last waits by a
    /// semaphore for every pass added before it on that queue. So a compute-queue pass runs beside
    /// the graphics passes added after the last it waits for and before the first waiting for it.
    ///
    /// It uses buffers alone: images would need handing over between the queues' families, which
    /// buffers, shared by every queue, don't.
    pub fn on_compute_queue(mut self) -> Self {
        self.queue = Queue::Compute;
        self
    }
    /// Samples `image` from the compute shaders as an earlier pass left it, through the index
    /// [`Compute::sampled`](crate::Compute::sampled) gives. It is an offscreen or storage target,
    /// or any graph image.
    pub fn sample(self, image: ImageId) -> Self {
        self.with(image, Use::ComputeSampled)
    }

    /// Writes and reads `image` through its levels' storage slots, which
    /// [`Compute::storage`](crate::Compute::storage) gives, as its last use left it. It is a
    /// storage target, undefined until a pass has written it since it was made, or a graph image
    /// of HDR colors, motion or scalars that an earlier pass wrote.
    pub fn write(self, image: ImageId) -> Self {
        self.with(image, Use::Storage { kept: true })
    }

    /// Writes `image` as [`write`](Self::write) does but discards what it held, for shaders that
    /// write each texel of the level they write. A graph image takes it as its first use.
    pub fn overwrite(self, image: ImageId) -> Self {
        self.with(image, Use::Storage { kept: false })
    }

    /// Reads `buffer` from the compute shaders as an earlier pass wrote it, through the address
    /// [`Compute::address`](crate::Compute::address) gives.
    pub fn read_buffer(self, buffer: BufferId) -> Self {
        self.graph
            .buffer_uses
            .push((buffer, BufferUse::ComputeRead));
        self
    }

    /// Writes and reads `buffer` from the compute shaders, through the address
    /// [`Compute::address`](crate::Compute::address) gives, as an earlier pass left it. Unwritten
    /// by the frame, it holds what the frames before left, as [`Graph::buffer`] says.
    pub fn write_buffer(self, buffer: BufferId) -> Self {
        self.graph
            .buffer_uses
            .push((buffer, BufferUse::ComputeWrite));
        self
    }

    fn with(self, image: ImageId, use_: Use) -> Self {
        self.graph.uses.push((image, use_));
        self
    }

    /// Adds the pass to its graph.
    ///
    /// # Panics
    ///
    /// - It uses an image twice.
    /// - It writes a graph image of a kind shaders don't write, or samples one they don't sample.
    /// - It keeps or samples a graph image no earlier pass wrote.
    /// - It runs on the compute queue and uses any image.
    /// - It uses a buffer twice, or reads one no earlier pass wrote.
    pub fn add(self) -> PassId {
        let Self {
            graph,
            label,
            start,
            queue,
        } = self;
        let uses = &graph.uses[start.0..];
        assert!(
            queue == Queue::Graphics || uses.is_empty(),
            "pass {label:?} runs on the compute queue, whose passes use buffers alone, but it uses \
             {uses:?}"
        );
        for (i, &(image, use_)) in uses.iter().enumerate() {
            assert!(
                uses[..i].iter().all(|&(used, _)| used != image),
                "pass {label:?} uses image {} twice; an image is in one layout at a time",
                image.0
            );
            let kind = graph.kind(image);
            let fits = match use_ {
                Use::ComputeSampled => kind.is_none_or(ImageKind::sampled),
                // A target is written only if it's a storage image, as the frame checks.
                Use::Storage { .. } => kind.is_none_or(ImageKind::storage),
                _ => unreachable!("a compute pass samples and writes alone"),
            };
            assert!(
                fits,
                "pass {label:?} uses image {} as {use_:?}, but it's {}",
                image.0,
                kind.map_or_else(|| "a target".to_owned(), |kind| format!("{kind:?}"))
            );
        }
        graph.add_pass(label, PassKind::Compute, queue, start)
    }
}

/// What a resource's earlier uses left for a later one to wait for: the last write or layout
/// transition, and the reads since that have seen it. A write waits for both; a read, for the
/// write, unless its stages have seen it already.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Hazard {
    write_stages: vk::PipelineStageFlags2,
    write_access: vk::AccessFlags2,
    reads: vk::PipelineStageFlags2,
}

/// A barrier between an image's uses: what it waits for, what it makes wait, and the layouts it
/// takes the image from and to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Barrier {
    pub(crate) src_stages: vk::PipelineStageFlags2,
    pub(crate) src_access: vk::AccessFlags2,
    pub(crate) dst_stages: vk::PipelineStageFlags2,
    pub(crate) dst_access: vk::AccessFlags2,
    pub(crate) old_layout: vk::ImageLayout,
    pub(crate) new_layout: vk::ImageLayout,
}

impl Barrier {
    pub(crate) fn on(
        self,
        image: vk::Image,
        range: vk::ImageSubresourceRange,
    ) -> vk::ImageMemoryBarrier2<'static> {
        vk::ImageMemoryBarrier2::default()
            .src_stage_mask(self.src_stages)
            .src_access_mask(self.src_access)
            .dst_stage_mask(self.dst_stages)
            .dst_access_mask(self.dst_access)
            .old_layout(self.old_layout)
            .new_layout(self.new_layout)
            .image(image)
            .subresource_range(range)
    }
}

impl Hazard {
    /// A swapchain's image just acquired: the frame's submission waits for its semaphore at the
    /// color attachment output stage, which a barrier from that stage follows.
    pub(crate) const ACQUIRED: Self = Self {
        write_stages: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        write_access: vk::AccessFlags2::NONE,
        reads: vk::PipelineStageFlags2::NONE,
    };

    /// An image anything before the frame may have written, as an offscreen target an earlier
    /// frame rendered or the CPU read back.
    pub(crate) const ANYTHING: Self = Self {
        write_stages: vk::PipelineStageFlags2::ALL_COMMANDS,
        write_access: vk::AccessFlags2::MEMORY_WRITE,
        reads: vk::PipelineStageFlags2::NONE,
    };

    /// The barrier, if needed, that lets `access` follow the uses this hazard and `layout` say came
    /// before. Leaves what later uses wait for in `self` and `layout`.
    pub(crate) fn follow(
        &mut self,
        layout: &mut vk::ImageLayout,
        access: Access,
    ) -> Option<Barrier> {
        let old_layout = if access.discards {
            vk::ImageLayout::UNDEFINED
        } else {
            *layout
        };
        let writes = !access.writes.is_empty() || old_layout != access.layout;
        let unseen = !self.write_stages.is_empty() && !self.reads.contains(access.stages);
        *layout = access.layout;
        if !writes && !unseen {
            // A read with nothing to wait for, as of a buffer the other queue wrote, which a
            // semaphore waited for: a write after it waits for it all the same.
            self.reads |= access.stages;
            return None;
        }
        // A write or layout transition (which writes too) waits for the reads since the last
        // write and for the write; a read waits for the write alone.
        let src_stages = if writes {
            self.write_stages | self.reads
        } else {
            self.write_stages
        };
        let barrier = Barrier {
            src_stages,
            src_access: self.write_access,
            dst_stages: access.stages,
            dst_access: access.reads | access.writes,
            old_layout,
            new_layout: access.layout,
        };
        if writes {
            // A transition alone is made visible to the barrier's second scope, whose reads have
            // seen it; the next barrier waits for it. The use's own writes come after the barrier,
            // which no read has seen, even in the stages that made them.
            let seen = access.writes.is_empty() && !access.reads.is_empty();
            *self = Self {
                write_stages: access.stages,
                write_access: access.writes,
                reads: if seen {
                    access.stages
                } else {
                    vk::PipelineStageFlags2::NONE
                },
            };
        } else {
            self.reads |= access.stages;
        }
        Some(barrier)
    }

    /// What this and `other` left together, for a use that follows both.
    pub(crate) fn and(self, other: Self) -> Self {
        Self {
            write_stages: self.write_stages | other.write_stages,
            write_access: self.write_access | other.write_access,
            reads: self.reads | other.reads,
        }
    }

    /// What a barrier after every use so far waits for, as one before a present does: the last
    /// write's access and stages, and the reads' stages since.
    pub(crate) fn last(&self) -> (vk::PipelineStageFlags2, vk::AccessFlags2) {
        (self.write_stages | self.reads, self.write_access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label() -> Label {
        gg_core::label!("test")
    }

    /// The barriers a sequence of uses needs, from `hazard` and a layout of `UNDEFINED`.
    fn barriers(mut hazard: Hazard, uses: &[Use]) -> Vec<Option<Barrier>> {
        let mut layout = vk::ImageLayout::UNDEFINED;
        (uses.iter())
            .map(|use_| hazard.follow(&mut layout, use_.access()))
            .collect()
    }

    use vk::AccessFlags2 as A;
    use vk::ImageLayout as L;
    use vk::PipelineStageFlags2 as S;

    #[test]
    fn a_view_cleared_sampled_twice_then_read_back_waits_where_it_must() {
        let clear = Use::Color(Some([0.0; 4]));
        let found = barriers(
            Hazard::default(),
            &[clear, Use::Sampled, Use::Sampled, Use::CopySource, clear],
        );
        let barrier = |src_stages, src_access, dst: Use, old_layout| {
            let access = dst.access();
            Some(Barrier {
                src_stages,
                src_access,
                dst_stages: access.stages,
                dst_access: access.reads | access.writes,
                old_layout,
                new_layout: access.layout,
            })
        };
        let color = S::COLOR_ATTACHMENT_OUTPUT;
        assert_eq!(
            found,
            [
                // Memory nothing used yet: the transition alone.
                barrier(S::NONE, A::NONE, clear, L::UNDEFINED),
                barrier(
                    color,
                    A::COLOR_ATTACHMENT_WRITE,
                    Use::Sampled,
                    L::COLOR_ATTACHMENT_OPTIMAL
                ),
                // Sampled again in the same stage, which has seen the write.
                None,
                // To another layout: after the reads, and the transition before them.
                barrier(
                    S::FRAGMENT_SHADER,
                    A::NONE,
                    Use::CopySource,
                    L::SHADER_READ_ONLY_OPTIMAL
                ),
                // Discarded: after the copy's read, from no layout.
                barrier(S::COPY, A::NONE, clear, L::UNDEFINED),
            ]
        );
    }

    #[test]
    fn a_read_in_a_new_stage_waits_for_the_write_and_a_write_after_reads_waits_for_them() {
        // Two passes drawn over a target, then a read in the same layout from a stage no earlier
        // use had, which isn't a transition and waits for the write alone; then a draw over it,
        // which waits for that read.
        let mut hazard = Hazard::ACQUIRED;
        let mut layout = L::UNDEFINED;
        let clear = Use::Color(Some([1.0; 4])).access();
        let first = hazard.follow(&mut layout, clear).expect("a transition");
        assert_eq!(
            (first.src_stages, first.src_access),
            (S::COLOR_ATTACHMENT_OUTPUT, A::NONE)
        );
        let kept = hazard.follow(&mut layout, Use::Color(None).access());
        let kept = kept.expect("blending reads what the clear wrote");
        assert_eq!(kept.old_layout, L::COLOR_ATTACHMENT_OPTIMAL);
        assert_eq!(
            kept.dst_access,
            A::COLOR_ATTACHMENT_READ | A::COLOR_ATTACHMENT_WRITE
        );
        let read = Access {
            stages: S::COMPUTE_SHADER,
            reads: A::SHADER_STORAGE_READ,
            writes: A::NONE,
            layout: L::COLOR_ATTACHMENT_OPTIMAL,
            discards: false,
        };
        let read_barrier = hazard
            .follow(&mut layout, read)
            .expect("the write is unseen there");
        assert_eq!(
            (read_barrier.src_stages, read_barrier.src_access),
            (S::COLOR_ATTACHMENT_OUTPUT, A::COLOR_ATTACHMENT_WRITE)
        );
        assert_eq!(hazard.follow(&mut layout, read), None, "seen already");
        let over = hazard
            .follow(&mut layout, Use::Color(None).access())
            .expect("a write");
        assert_eq!(
            over.src_stages,
            S::COLOR_ATTACHMENT_OUTPUT | S::COMPUTE_SHADER,
            "after the write and the read since"
        );
        assert_eq!(
            hazard.last(),
            (S::COLOR_ATTACHMENT_OUTPUT, A::COLOR_ATTACHMENT_WRITE)
        );
    }

    #[test]
    fn a_graphs_own_images_are_written_before_theyre_read() {
        let mut graph = Graph::default();
        let target = graph.target(0);
        assert_eq!(graph.target(0), target, "one image for each target");
        let view = graph.color("view", [4, 4]);
        let ids = graph.ids("ids", [4, 4]);
        let depth = graph.depth("depth", [4, 4]);
        let drawn = graph
            .render(label())
            .clear(view, [0.0; 4])
            .ids(ids)
            .depth(depth)
            .add();
        let read = graph.read_ids(label(), ids);
        let shown = graph.render(label()).keep(target).sample(view).add();
        assert_eq!((drawn, read, shown), (PassId(0), PassId(1), PassId(2)));
        assert_eq!(graph.uses(read), [(ids, Use::CopySource)]);
        assert_eq!(graph.lifetime(view), Some((0, 2)));
        assert_eq!(graph.lifetime(depth), Some((0, 0)));
        let unused = graph.color("unused", [1, 1]);
        assert_eq!(graph.lifetime(unused), None);
        let unwritten = graph.color("unwritten", [4, 4]);
        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            graph
                .render(label())
                .clear(target, [0.0; 4])
                .sample(unwritten)
                .add()
        }));
        assert!(refused.is_err(), "sampled before any pass wrote it");
    }

    #[test]
    fn passes_draw_into_one_color_target_of_one_size() {
        let refused = |build: fn(&mut Graph)| {
            let mut graph = Graph::default();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&mut graph))).is_err()
        };
        assert!(refused(|graph| {
            let ids = graph.ids("ids", [4, 4]);
            graph.render(label()).ids(ids).add();
        }));
        assert!(refused(|graph| {
            let (a, b) = (graph.color("a", [4, 4]), graph.color("b", [4, 4]));
            graph
                .render(label())
                .clear(a, [0.0; 4])
                .clear(b, [0.0; 4])
                .add();
        }));
        assert!(refused(|graph| {
            let (view, ids) = (graph.color("view", [4, 4]), graph.ids("ids", [8, 4]));
            graph.render(label()).clear(view, [0.0; 4]).ids(ids).add();
        }));
        assert!(refused(|graph| {
            let view = graph.color("view", [4, 4]);
            graph.render(label()).clear(view, [0.0; 4]).ids(view).add();
        }));
        assert!(!refused(|graph| {
            let target = graph.target(0);
            graph.render(label()).clear(target, [0.0; 4]).add();
            graph.render(label()).keep(target).add();
        }));
        // Depth alone draws without IDs, into one image of it.
        assert!(refused(|graph| {
            let (ids, depth) = (graph.ids("ids", [4, 4]), graph.depth("depth", [4, 4]));
            graph.render(label()).ids(ids).depth(depth).add();
        }));
        assert!(refused(|graph| {
            let (a, b) = (graph.depth("a", [4, 4]), graph.depth("b", [4, 4]));
            graph.render(label()).depth(a).depth(b).add();
        }));
        assert!(refused(|graph| {
            graph.render(label()).add();
        }));
    }

    /// Depth is drawn alone, as a shadow map's is, or beside a color target and IDs. Later passes
    /// sample it, by drawing or computing, or draw over it, keeping it as they keep IDs and colors:
    /// only what an earlier pass wrote can be kept.
    #[test]
    fn depth_draws_alone_or_beside_colors_and_later_passes_sample_or_keep_it() {
        let mut graph = Graph::default();
        let shadow = graph.depth("shadow", [8, 8]);
        let drawn = graph.render(label()).depth(shadow).add();
        let target = graph.target(0);
        let shaded = (graph.render(label()).clear(target, [0.0; 4]))
            .sample(shadow)
            .add();
        assert_eq!(graph.uses(drawn), [(shadow, Use::Depth { kept: false })]);
        assert_eq!(graph.uses(shaded)[1], (shadow, Use::Sampled));
        assert_eq!(graph.lifetime(shadow), Some((0, 1)));
        let view = graph.color("view", [8, 8]);
        let ids = graph.ids("ids", [8, 8]);
        let depth = graph.depth("depth", [8, 8]);
        (graph.render(label()).clear(view, [0.0; 4]))
            .ids(ids)
            .depth(depth)
            .add();
        let reduced = graph.compute(label()).sample(depth).add();
        let over = (graph.render(label()).keep(view))
            .keep(ids)
            .keep(depth)
            .add();
        assert_eq!(graph.uses(reduced), [(depth, Use::ComputeSampled)]);
        assert_eq!(
            graph.uses(over),
            [
                (view, Use::Color(None)),
                (ids, Use::Ids { kept: true }),
                (depth, Use::Depth { kept: true })
            ]
        );
        let kept = [Use::Ids { kept: true }, Use::Depth { kept: true }];
        assert!(kept.iter().all(|use_| !use_.access().discards));
        let unwritten = |kind: fn(&mut Graph) -> ImageId| {
            let mut graph = Graph::default();
            let image = kind(&mut graph);
            let target = graph.target(0);
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (graph.render(label()).clear(target, [0.0; 4]))
                    .keep(image)
                    .add()
            }))
            .is_err()
        };
        assert!(unwritten(|graph| graph.ids("ids", [4, 4])), "IDs kept");
        assert!(
            unwritten(|graph| graph.depth("depth", [4, 4])),
            "depth kept"
        );
    }

    /// A graph's buffers are written by compute passes before passes read them, draws take
    /// commands from them or frames read them back; they're named apart. Barriers between their
    /// uses wait for writes, and a write for the reads since, with no layouts.
    #[test]
    fn buffers_are_written_before_theyre_read() {
        let refused = |build: fn(&mut Graph)| {
            let mut graph = Graph::default();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&mut graph))).is_err()
        };
        assert!(!refused(|graph| {
            let (data, commands) = (graph.buffer("data", 64), graph.buffer("commands", 20));
            graph
                .compute(label())
                .write_buffer(data)
                .write_buffer(commands)
                .add();
            graph.compute(label()).read_buffer(data).add();
            let target = graph.target(0);
            (graph.render(label()).clear(target, [0.0; 4]))
                .read_buffer(data)
                .indirect(commands)
                .add();
            graph.read_back(label(), data);
        }));
        assert!(refused(|graph| {
            let data = graph.buffer("data", 64);
            graph.compute(label()).read_buffer(data).add();
        }));
        assert!(refused(|graph| {
            let data = graph.buffer("data", 64);
            graph.read_back(label(), data);
        }));
        assert!(refused(|graph| {
            let data = graph.buffer("data", 64);
            graph
                .compute(label())
                .write_buffer(data)
                .write_buffer(data)
                .add();
        }));
        assert!(refused(|graph| {
            graph.buffer("data", 64);
            graph.buffer("data", 32);
        }));
        assert!(refused(|graph| {
            graph.buffer("empty", 0);
        }));
        let mut hazard = Hazard::default();
        let mut layout = vk::ImageLayout::UNDEFINED;
        let write = BufferUse::ComputeWrite.access();
        let first = hazard.follow(&mut layout, write).expect("a write");
        assert_eq!(first.src_stages, S::NONE, "nothing to wait for");
        // Read in the stage that wrote it, by a later pass; the write's own pass doesn't wait.
        let read = (hazard.follow(&mut layout, BufferUse::ComputeRead.access()))
            .expect("a read of the write in its own stage");
        assert_eq!(
            (read.src_stages, read.src_access),
            (S::COMPUTE_SHADER, A::SHADER_STORAGE_WRITE)
        );
        let indirect = (hazard.follow(&mut layout, BufferUse::Indirect.access()))
            .expect("a read of the write");
        assert_eq!(
            (indirect.src_stages, indirect.src_access),
            (S::COMPUTE_SHADER, A::SHADER_STORAGE_WRITE)
        );
        assert_eq!(indirect.old_layout, indirect.new_layout, "no transition");
        let again = hazard
            .follow(&mut layout, write)
            .expect("a write after the read");
        let draws = S::DRAW_INDIRECT | S::PRE_RASTERIZATION_SHADERS | S::FRAGMENT_SHADER;
        assert_eq!(again.src_stages, S::COMPUTE_SHADER | draws);
    }

    /// A visibility buffer is a pass's color target beside its IDs and depth, cleared to 0 alone.
    /// A later pass draws over it and a compute pass samples it, writing HDR colors whole as
    /// their first use, for a later pass to draw over after the compute shader's writes. Compute
    /// passes keep only what an earlier pass wrote, and write no other kind of graph image.
    #[test]
    fn a_visibility_buffer_is_drawn_then_computed_into_colors() {
        let mut graph = Graph::default();
        let size = [4, 4];
        let (visibility, ids, depth, hdr) = (
            graph.visibility("visibility", size),
            graph.ids("ids", size),
            graph.depth("depth", size),
            graph.hdr("hdr", size),
        );
        let drawn = (graph.render(label()).visibility(visibility))
            .ids(ids)
            .depth(depth)
            .add();
        let again = (graph.render(label()).keep(visibility))
            .keep(ids)
            .keep(depth)
            .add();
        let shaded = graph
            .compute(label())
            .sample(visibility)
            .overwrite(hdr)
            .add();
        let over = graph.render(label()).keep(hdr).keep(ids).keep(depth).add();
        assert_eq!(
            graph.uses(drawn)[0],
            (visibility, Use::Color(Some([0.0; 4])))
        );
        assert_eq!(graph.uses(again)[0], (visibility, Use::Color(None)));
        assert_eq!(
            graph.uses(shaded),
            [
                (visibility, Use::ComputeSampled),
                (hdr, Use::Storage { kept: false })
            ]
        );
        assert_eq!(graph.uses(over)[0], (hdr, Use::Color(None)));
        let found = barriers(
            Hazard::default(),
            &[Use::Storage { kept: false }, Use::Color(None)],
        );
        let first = found[0].expect("a transition");
        assert_eq!(
            (first.old_layout, first.new_layout),
            (L::UNDEFINED, L::GENERAL)
        );
        let drawn_over = found[1].expect("a barrier after the writes");
        assert_eq!(
            (drawn_over.src_stages, drawn_over.src_access),
            (S::COMPUTE_SHADER, A::SHADER_STORAGE_WRITE)
        );
        assert_eq!(
            (drawn_over.old_layout, drawn_over.new_layout),
            (L::GENERAL, L::COLOR_ATTACHMENT_OPTIMAL)
        );
        let refused = |build: fn(&mut Graph)| {
            let mut graph = Graph::default();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&mut graph))).is_err()
        };
        assert!(refused(|graph| {
            let visibility = graph.visibility("visibility", [4, 4]);
            graph.render(label()).clear(visibility, [1.0; 4]).add();
        }));
        assert!(refused(|graph| {
            let hdr = graph.hdr("hdr", [4, 4]);
            graph.compute(label()).write(hdr).add();
        }));
        assert!(refused(|graph| {
            let ids = graph.ids("ids", [4, 4]);
            graph.compute(label()).overwrite(ids).add();
        }));
        assert!(!refused(|graph| {
            let hdr = graph.hdr("hdr", [4, 4]);
            graph.compute(label()).overwrite(hdr).add();
            graph.compute(label()).write(hdr).add();
        }));
    }

    /// An image of scalars is written by compute passes through a storage view of its own format
    /// and sampled by later passes, computing or drawing; never drawn into, kept as an attachment,
    /// or read back.
    #[test]
    fn scalars_are_computed_then_sampled_but_never_drawn_into() {
        assert_eq!(ImageKind::Scalar.format(), vk::Format::R32_SFLOAT);
        assert!(ImageKind::Scalar.storage() && ImageKind::Scalar.sampled());
        let mut graph = Graph::default();
        let (scalar, hdr) = (graph.scalar("scalar", [4, 4]), graph.hdr("hdr", [4, 4]));
        let written = graph.compute(label()).overwrite(scalar).add();
        let computed = graph.compute(label()).sample(scalar).overwrite(hdr).add();
        let drawn = graph.render(label()).keep(hdr).sample(scalar).add();
        assert_eq!(
            graph.uses(written),
            [(scalar, Use::Storage { kept: false })]
        );
        assert_eq!(graph.uses(computed)[0], (scalar, Use::ComputeSampled));
        assert_eq!(graph.uses(drawn)[1], (scalar, Use::Sampled));
        let refused = |build: fn(&mut Graph)| {
            let mut graph = Graph::default();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&mut graph))).is_err()
        };
        assert!(refused(|graph| {
            let scalar = graph.scalar("scalar", [4, 4]);
            graph.render(label()).clear(scalar, [0.0; 4]).add();
        }));
        assert!(refused(|graph| {
            let scalar = graph.scalar("scalar", [4, 4]);
            graph.compute(label()).overwrite(scalar).add();
            graph.render(label()).keep(scalar).add();
        }));
        assert!(refused(|graph| {
            let scalar = graph.scalar("scalar", [4, 4]);
            graph.compute(label()).overwrite(scalar).add();
            graph.read_image(label(), scalar);
        }));
    }

    /// HDR colors draw and are drawn over as colors are, beside IDs and depth, and are sampled but
    /// not read back.
    #[test]
    fn colors_of_high_dynamic_range_draw_and_sample_but_arent_read_back() {
        let mut graph = Graph::default();
        let (hdr, ids, depth) = (
            graph.hdr("hdr", [4, 4]),
            graph.ids("ids", [4, 4]),
            graph.depth("depth", [4, 4]),
        );
        (graph.render(label()).clear(hdr, [2.0, 0.5, 0.0, 1.0]))
            .ids(ids)
            .depth(depth)
            .add();
        graph.render(label()).keep(hdr).add();
        let view = graph.color("view", [4, 4]);
        let mapped = graph
            .render(label())
            .clear(view, [0.0; 4])
            .sample(hdr)
            .add();
        assert_eq!(graph.uses(mapped)[1], (hdr, Use::Sampled));
        assert_eq!(graph.lifetime(hdr), Some((0, 2)));
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            graph.read_image(label(), hdr)
        }));
        assert!(read.is_err(), "read back");
    }

    /// A graph image is found by name, the last of that name added, with its kind; a target, which
    /// has no name in the graph, and a name none has, aren't.
    #[test]
    fn images_are_found_by_their_names() {
        let mut graph = Graph::default();
        graph.target(0);
        let first = graph.depth("depth", [4, 4]);
        let ids = graph.ids("ids", [4, 4]);
        let last = graph.depth("depth", [2, 2]);
        assert_ne!(first, last);
        assert_eq!(graph.image_named("depth"), Some((last, ImageKind::Depth)));
        assert_eq!(graph.image_named("ids"), Some((ids, ImageKind::Ids)));
        assert_eq!(graph.image_named("view"), None);
        // Each listed in order, a name twice, without the target.
        let listed: Vec<_> = graph.images().collect();
        let want = [
            ("depth", ImageKind::Depth, [4, 4]),
            ("ids", ImageKind::Ids, [4, 4]),
            ("depth", ImageKind::Depth, [2, 2]),
        ];
        assert_eq!(listed, want);
        graph.clear();
        assert_eq!(graph.image_named("ids"), None, "cleared");
        assert_eq!(graph.images().count(), 0, "cleared");
    }
}
