//! A frame's render graph: the passes a frame runs, in the order they're added, and how each uses
//! the images it renders into, computes into, samples and copies from, which the frame places
//! every barrier and layout transition from as each pass begins; and the images that are the
//! graph's own, which last for a frame, made from what the passes say of them, in memory that
//! those whose uses in a frame don't overlap share, as `transients` says.

use std::ops::Range;

use ash::vk;
use gg_core::profile::Label;

use crate::{COLOR_RANGE, ColorImage, DEPTH_RANGE};

/// An image a [`Graph`]'s passes use: one of the frame's targets, or one of the graph's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageId(pub(crate) u32);

/// A pass a [`Graph`] runs: the frame records them in the order they were added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassId(pub(crate) u32);

/// What a frame runs: its passes, in order, each with the images it uses and how. A frame places
/// the barriers and layout transitions between passes from those uses, the image's last use before
/// a pass and the pass's own, so that no pass records one itself. Clear it and add the frame's
/// passes again every frame; it keeps its memory, so that a frame that adds no more than the last
/// allocates nothing.
///
/// Its own images, [`Graph::color`], [`Graph::ids`] and [`Graph::depth`], last for a frame: each
/// is undefined until a pass of the frame writes it, so its first use writes it whole, and passes
/// whose uses of two of them don't overlap may find them in the same memory. The frames keep them
/// for the frames after, which add them alike, by their names, kinds and sizes.
#[derive(Default)]
pub struct Graph {
    pub(crate) images: Vec<ImageDecl>,
    pub(crate) passes: Vec<PassDecl>,
    /// Each pass's uses, one after another, in the order they were given.
    pub(crate) uses: Vec<(ImageId, Use)>,
}

/// An image of a graph's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageDecl {
    /// The frame's target of that index.
    Target(usize),
    /// One of the graph's own, `name` its debug name.
    Transient {
        name: &'static str,
        kind: Kind,
        size: [u32; 2],
    },
}

/// What a graph's own image holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Colors, in [`ColorImage::FORMAT`], which passes render into and blend over, later ones
    /// sample and frames read back.
    Color,
    /// Colors of high dynamic range, as 16-bit floats in linear RGBA, [`Format::HDR`], which passes
    /// render into and blend over, past 1 too, and later ones sample. Vulkan requires every device
    /// to render into the format, blend in it and sample it.
    ///
    /// [`Format::HDR`]: crate::Format::HDR
    Hdr,
    /// IDs beside a pass's colors, as two 32-bit unsigned integers, the low and high halves of a
    /// 64-bit one, which passes render, frames read pixels of back and later passes sample, as
    /// unsigned integers. Vulkan requires every device to render into the format, copy from it and
    /// sample it.
    Ids,
    /// Depth, as 32-bit floats, which draws test and write: from 1 at the near plane to 0 far
    /// away, as the renderer's cameras project it. Every device the engine opens renders it.
    Depth,
}

impl Kind {
    pub(crate) const fn format(self) -> vk::Format {
        match self {
            Self::Color => ColorImage::FORMAT.color,
            Self::Hdr => vk::Format::R16G16B16A16_SFLOAT,
            Self::Ids => vk::Format::R32G32_UINT,
            Self::Depth => vk::Format::D32_SFLOAT,
        }
    }

    /// What its images are made for: every use a pass may make of one of its kind.
    pub(crate) fn usage(self) -> vk::ImageUsageFlags {
        match self {
            Self::Color | Self::Ids => {
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
            }
            // Never read back: what a frame reads back is what it shows.
            Self::Hdr => vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
            Self::Depth => vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        }
    }

    pub(crate) fn range(self) -> vk::ImageSubresourceRange {
        match self {
            Self::Color | Self::Hdr | Self::Ids => COLOR_RANGE,
            Self::Depth => DEPTH_RANGE,
        }
    }

    /// Whether shaders may sample it, through a slot in the bindless set.
    pub(crate) fn sampled(self) -> bool {
        self.usage().contains(vk::ImageUsageFlags::SAMPLED)
    }
}

/// How a pass uses an image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Use {
    /// Its color target, cleared first to this color, in linear RGBA, or with `None` drawn over as
    /// the image's last use left it.
    Color(Option<[f32; 4]>),
    /// The IDs beside its color target, cleared first to 0, for none.
    Ids,
    /// The depth its draws test and write, cleared first to 0, as far away as depth goes.
    Depth,
    /// Sampled by its fragment shaders, through the image's slot in the bindless set.
    Sampled,
    /// Sampled by its compute shaders, through the image's slot in the bindless set.
    ComputeSampled,
    /// Written and read by its compute shaders, a level at a time, through the storage slots of a
    /// storage image's levels, in the general layout, as its last use left it.
    Storage,
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
            Self::Color(Some(_)) | Self::Ids => color(vk::AccessFlags2::NONE, true),
            // Blending reads what's there.
            Self::Color(None) => color(vk::AccessFlags2::COLOR_ATTACHMENT_READ, false),
            // A clear writes at the early fragment tests, and a store at the late ones.
            Self::Depth => Access {
                stages: vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                reads: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ,
                writes: vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                discards: true,
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
            Self::Storage => Access {
                stages: vk::PipelineStageFlags2::COMPUTE_SHADER,
                reads: vk::AccessFlags2::SHADER_STORAGE_READ,
                writes: vk::AccessFlags2::SHADER_STORAGE_WRITE,
                layout: vk::ImageLayout::GENERAL,
                discards: false,
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

    /// Whether it's one of a render pass's attachments.
    fn attaches(self) -> bool {
        matches!(self, Self::Color(_) | Self::Ids | Self::Depth)
    }
}

/// What a use does to an image: the stages it's used in, and the reads and writes they make there,
/// in a layout; and whether it discards what the image held, as a clear does.
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
    /// Its uses, in [`Graph::uses`].
    pub(crate) uses: Range<usize>,
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
}

impl Graph {
    /// Empties it for the next frame, keeping its memory.
    pub fn clear(&mut self) {
        self.images.clear();
        self.passes.clear();
        self.uses.clear();
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

    /// An image of the graph's own of colors, `size` pixels, named `name`: [`ColorImage::FORMAT`],
    /// as a target's, so that the same pipelines draw into either.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn color(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, Kind::Color, size)
    }

    /// An image of the graph's own of colors of high dynamic range, `size` pixels, named `name`:
    /// [`Format::HDR`](crate::Format::HDR)'s 16-bit floats, which keep what's brighter than 1 for
    /// a later pass to sample and map into a target's range, as tonemapping does. Frames don't
    /// read it back.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn hdr(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, Kind::Hdr, size)
    }

    /// An image of the graph's own of IDs, `size` pixels, named `name`, which a pass renders
    /// beside its colors, frames read pixels of back ([`Graph::read_ids`]) and later passes may
    /// sample, as unsigned integers.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn ids(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, Kind::Ids, size)
    }

    /// An image of the graph's own of depth, `size` pixels, named `name`, which a pass's draws
    /// test and write.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn depth(&mut self, name: &'static str, size: [u32; 2]) -> ImageId {
        self.transient(name, Kind::Depth, size)
    }

    fn transient(&mut self, name: &'static str, kind: Kind, size: [u32; 2]) -> ImageId {
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

    /// A pass that draws, named `label`, whose attachments and the images it samples the builder
    /// it returns takes, before [`RenderPass::add`] adds it.
    pub fn render(&mut self, label: Label) -> RenderPass<'_> {
        let start = self.uses.len();
        RenderPass {
            graph: self,
            label,
            start,
        }
    }

    /// A pass that dispatches compute shaders, named `label`, whose images the builder it returns
    /// takes, before [`ComputePass::add`] adds it.
    pub fn compute(&mut self, label: Label) -> ComputePass<'_> {
        let start = self.uses.len();
        ComputePass {
            graph: self,
            label,
            start,
        }
    }

    /// A pass, named `label`, that reads pixels of `ids` back, as an earlier pass left them, as
    /// [`Frame::read_ids`](crate::Frame::read_ids) records it.
    ///
    /// # Panics
    ///
    /// If `ids` isn't an image of IDs of the graph's own, or no earlier pass wrote it.
    pub fn read_ids(&mut self, label: Label, ids: ImageId) -> PassId {
        assert!(
            matches!(self.kind(ids), Some(Kind::Ids)),
            "pass {label:?} reads IDs back from an image of IDs of the graph's own, which image {} \
             isn't",
            ids.0
        );
        self.read(label, PassKind::ReadIds, ids)
    }

    /// A pass, named `label`, that reads `image` back, as an earlier pass left it, as
    /// [`Frame::read_image`](crate::Frame::read_image) records it: a target or a color image of
    /// the graph's own.
    ///
    /// # Panics
    ///
    /// If `image` is neither, or is one of the graph's own that no earlier pass wrote.
    pub fn read_image(&mut self, label: Label, image: ImageId) -> PassId {
        assert!(
            matches!(self.kind(image), None | Some(Kind::Color)),
            "pass {label:?} reads back a target or a color image, which image {} isn't",
            image.0
        );
        self.read(label, PassKind::ReadImage, image)
    }

    fn read(&mut self, label: Label, kind: PassKind, image: ImageId) -> PassId {
        let start = self.uses.len();
        self.uses.push((image, Use::CopySource));
        self.add_pass(label, kind, start)
    }

    /// Adds the pass whose uses start at `start`, once they're checked.
    fn add_pass(&mut self, label: Label, kind: PassKind, start: usize) -> PassId {
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
        self.passes.push(PassDecl { label, kind, uses });
        PassId(self.passes.len() as u32 - 1)
    }

    /// The kind of `image` if it's one of the graph's own; `None` for a target.
    pub(crate) fn kind(&self, image: ImageId) -> Option<Kind> {
        match self.images.get(image.0 as usize)? {
            ImageDecl::Target(_) => None,
            &ImageDecl::Transient { kind, .. } => Some(kind),
        }
    }

    /// `pass`'s uses.
    pub(crate) fn uses(&self, pass: PassId) -> &[(ImageId, Use)] {
        &self.uses[self.passes[pass.0 as usize].uses.clone()]
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

/// A pass that draws, being built: its color target, the IDs and depth beside it, if any, and the
/// images it samples. [`RenderPass::add`] adds it to its graph.
#[must_use = "a pass is in its graph once it's added"]
pub struct RenderPass<'g> {
    graph: &'g mut Graph,
    label: Label,
    /// Where its uses start in the graph's.
    start: usize,
}

impl RenderPass<'_> {
    /// Draws into `image`, a target or a color image of the graph's own, of either range, cleared
    /// first to `color`, in linear RGBA.
    pub fn clear(self, image: ImageId, color: [f32; 4]) -> Self {
        self.with(image, Use::Color(Some(color)))
    }

    /// Draws into `image`, a target or a color image of the graph's own, of either range, over
    /// what its last use left there, which its draws blend over: a target that no pass has drawn
    /// into since it was made, or since its swapchain's image was acquired, holds nothing to draw
    /// over.
    pub fn keep(self, image: ImageId) -> Self {
        self.with(image, Use::Color(None))
    }

    /// Draws the draws' IDs into `ids`, an image of IDs of the graph's own, cleared first to 0,
    /// which pipelines made for a format [`with_ids`](crate::Format::with_ids) write.
    pub fn ids(self, ids: ImageId) -> Self {
        self.with(ids, Use::Ids)
    }

    /// Tests the draws against `depth`, an image of depth of the graph's own, and writes theirs
    /// into it as their pipelines say, cleared first to 0, as far away as depth goes.
    pub fn depth(self, depth: ImageId) -> Self {
        self.with(depth, Use::Depth)
    }

    /// Samples `image` from the draws' fragment shaders, as an earlier pass left it, through the
    /// index [`Pass::sampled`](crate::Pass::sampled) gives: a target that's an offscreen image or a
    /// storage image, or a color image or an image of IDs of the graph's own.
    pub fn sample(self, image: ImageId) -> Self {
        self.with(image, Use::Sampled)
    }

    fn with(self, image: ImageId, use_: Use) -> Self {
        self.graph.uses.push((image, use_));
        self
    }

    /// Adds the pass to its graph.
    ///
    /// # Panics
    ///
    /// If it hasn't one color target, has more than one image of IDs or of depth, or uses an image
    /// twice, as one another kind of image, or as images of the graph's own of other sizes; or if
    /// it samples or draws over one of the graph's own images that no earlier pass wrote.
    pub fn add(self) -> PassId {
        let Self {
            graph,
            label,
            start,
        } = self;
        let uses = &graph.uses[start..];
        let count = |is: fn(&Use) -> bool| uses.iter().filter(|(_, use_)| is(use_)).count();
        assert!(
            count(|use_| matches!(use_, Use::Color(_))) == 1
                && count(|use_| *use_ == Use::Ids) <= 1
                && count(|use_| *use_ == Use::Depth) <= 1,
            "pass {label:?} draws into one color target, beside at most one image of IDs and one of \
             depth; it has {uses:?}"
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
                Use::Color(_) => matches!(kind, None | Some(Kind::Color | Kind::Hdr)),
                Use::Ids => kind == Some(Kind::Ids),
                Use::Depth => kind == Some(Kind::Depth),
                Use::Sampled => kind.is_none_or(Kind::sampled),
                Use::ComputeSampled | Use::Storage | Use::CopySource => {
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
        graph.add_pass(label, PassKind::Render, start)
    }
}

/// A pass that dispatches compute shaders, being built: the images it samples and those it writes.
/// [`ComputePass::add`] adds it to its graph.
#[must_use = "a pass is in its graph once it's added"]
pub struct ComputePass<'g> {
    graph: &'g mut Graph,
    label: Label,
    /// Where its uses start in the graph's.
    start: usize,
}

impl ComputePass<'_> {
    /// Samples `image` from the compute shaders, as an earlier pass left it, through the index
    /// [`Compute::sampled`](crate::Compute::sampled) gives: a target that's an offscreen image or
    /// a storage image, or a color image or an image of IDs of the graph's own.
    pub fn sample(self, image: ImageId) -> Self {
        self.with(image, Use::ComputeSampled)
    }

    /// Writes and reads `image`, a storage image among the frame's targets, through its levels'
    /// storage slots, which [`Compute::storage`](crate::Compute::storage) gives, as its last use
    /// left it: what no pass has written since it was made is undefined.
    pub fn write(self, image: ImageId) -> Self {
        self.with(image, Use::Storage)
    }

    fn with(self, image: ImageId, use_: Use) -> Self {
        self.graph.uses.push((image, use_));
        self
    }

    /// Adds the pass to its graph.
    ///
    /// # Panics
    ///
    /// If it uses an image twice, writes one of the graph's own, which are no storage images,
    /// samples one of a kind shaders don't sample, or one that no earlier pass wrote.
    pub fn add(self) -> PassId {
        let Self {
            graph,
            label,
            start,
        } = self;
        let uses = &graph.uses[start..];
        for (i, &(image, use_)) in uses.iter().enumerate() {
            assert!(
                uses[..i].iter().all(|&(used, _)| used != image),
                "pass {label:?} uses image {} twice; an image is in one layout at a time",
                image.0
            );
            let kind = graph.kind(image);
            let fits = match use_ {
                Use::ComputeSampled => kind.is_none_or(Kind::sampled),
                // Only a target can be a storage image, as the frame checks.
                Use::Storage => kind.is_none(),
                _ => unreachable!("a compute pass samples and writes alone"),
            };
            assert!(
                fits,
                "pass {label:?} uses image {} as {use_:?}, but it's {}",
                image.0,
                kind.map_or_else(|| "a target".to_owned(), |kind| format!("{kind:?}"))
            );
        }
        graph.add_pass(label, PassKind::Compute, start)
    }
}

/// What a resource's earlier uses left for a later one to wait for: the last write, or layout
/// transition, and the reads since that have seen it. A write after them waits for both; a read,
/// for the write, unless its stages have seen it already.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Hazard {
    write_stages: vk::PipelineStageFlags2,
    write_access: vk::AccessFlags2,
    reads: vk::PipelineStageFlags2,
}

/// A barrier between an image's uses: what it waits for, what it makes wait, and the layout it
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
    /// The barrier on `range` of `image`.
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
    /// color attachment output stage, which a barrier from that stage then follows.
    pub(crate) const ACQUIRED: Self = Self {
        write_stages: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        write_access: vk::AccessFlags2::NONE,
        reads: vk::PipelineStageFlags2::NONE,
    };

    /// An image that anything before the frame may have written, as an offscreen target that a
    /// frame before rendered or the CPU read back.
    pub(crate) const ANYTHING: Self = Self {
        write_stages: vk::PipelineStageFlags2::ALL_COMMANDS,
        write_access: vk::AccessFlags2::MEMORY_WRITE,
        reads: vk::PipelineStageFlags2::NONE,
    };

    /// The barrier that lets `access` follow the uses this hazard and `layout` say came before, if
    /// it needs one, and what it leaves for the uses after, in `self` and `layout`.
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
            return None;
        }
        // A write, or a layout transition, which writes too, waits for the reads since the last
        // write as well as for the write itself; a read waits for the write alone.
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
            // A transition is made visible to the barrier's second scope, which the next barrier
            // then waits for.
            *self = Self {
                write_stages: access.stages,
                write_access: access.writes,
                reads: if access.reads.is_empty() {
                    vk::PipelineStageFlags2::NONE
                } else {
                    access.stages
                },
            };
        } else {
            self.reads |= access.stages;
        }
        Some(barrier)
    }

    /// What this and `other` left together, for a use that follows both: their writes and their
    /// reads.
    pub(crate) fn and(self, other: Self) -> Self {
        Self {
            write_stages: self.write_stages | other.write_stages,
            write_access: self.write_access | other.write_access,
            reads: self.reads | other.reads,
        }
    }

    /// What a barrier after every use so far waits for, as one before a present does: the last
    /// write and the reads since, with the write's access.
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
        // Two passes drawn over a target, then its read in the same layout from another stage
        // than any before, which isn't a transition: the write alone; and a draw over it after,
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
        assert!(refused(|graph| {
            let (target, depth) = (graph.target(0), graph.depth("depth", [4, 4]));
            graph
                .render(label())
                .clear(target, [0.0; 4])
                .depth(depth)
                .add();
            graph.render(label()).keep(target).sample(depth).add();
        }));
        assert!(!refused(|graph| {
            let target = graph.target(0);
            graph.render(label()).clear(target, [0.0; 4]).add();
            graph.render(label()).keep(target).add();
        }));
    }

    /// Colors of high dynamic range draw and are drawn over as colors are, beside IDs and depth,
    /// and are sampled, but not read back.
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
}
