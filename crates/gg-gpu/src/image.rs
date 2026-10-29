//! Offscreen images: color images, which headless runs render into, tests read back, and later
//! passes of a frame sample, such as the game's view; ID images, which passes render their draws'
//! IDs into beside their targets, for frames to read pixels of back and their later passes to
//! sample; and depth images, which passes test their draws against.

use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use crate::buffer::Buffer;
use crate::device::Retired;
use crate::vma::{Allocation, Memory};
use crate::{COLOR_RANGE, DEPTH_RANGE, Device, Format, copy_region, vk_error};

/// A color image in GPU memory, which frames render into, their later passes sample through its
/// slot in the bindless set, and the CPU can read back. Dropping it retires it, and its slot,
/// until the GPU is done with it.
pub struct ColorImage {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    /// Taken when the image is dropped.
    allocation: Option<Allocation>,
    extent: vk::Extent2D,
    /// The layout the work submitted so far leaves the image in.
    layout: vk::ImageLayout,
    /// Its slot in the bindless set, past the placed ones.
    index: u32,
    /// Its debug name, which its read-back and their errors are named after.
    name: String,
}

impl ColorImage {
    /// The format of every color image: 8-bit sRGB, as windows usually present, so what's read
    /// back matches what a window would show. The game's view is one, so games' pipelines are
    /// made for it.
    pub const FORMAT: Format = Format::new(vk::Format::R8G8B8A8_SRGB);

    /// An image of `width` by `height` pixels, in a free slot of the bindless set past the placed
    /// ones, with `name` as its debug name.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn new(device: &Arc<Device>, width: u32, height: u32, name: &str) -> Result<Self> {
        // SAFETY: shaders sample the image only through the index `Frame::begin_sampling`
        // returns, once it has put the image in the shader-read-only layout.
        let (raw, view, allocation, extent, index) =
            unsafe { allocate_sampled(device, [width, height], Self::FORMAT.color, name) }?;
        Ok(Self {
            device: device.clone(),
            raw,
            view,
            allocation: Some(allocation),
            extent,
            layout: vk::ImageLayout::UNDEFINED,
            index,
            name: name.to_owned(),
        })
    }

    /// Copies the image to the CPU as rows of RGBA8 pixels, sRGB-encoded and tightly packed, top
    /// row first. It waits for the GPU to finish everything submitted so far.
    pub fn read_back(&mut self) -> Result<Vec<u8>> {
        let size = u64::from(self.extent.width) * u64::from(self.extent.height) * 4;
        // The buffer and the commands that copy into it.
        let name = format!("{} read-back", self.name);
        let buffer = Buffer::new(
            &self.device,
            size,
            vk::BufferUsageFlags::TRANSFER_DST,
            Memory::Readback,
            &name,
        )?;
        let raw = self.device.raw();
        let to_source = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .old_layout(self.layout)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(self.raw)
            .subresource_range(COLOR_RANGE);
        let region = copy_region(0, [0, 0], self.size(), 0);
        let to_host = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .buffer(buffer.raw())
            .size(vk::WHOLE_SIZE);
        // SAFETY: the image and buffer are live, and the barriers make the copy wait for earlier
        // work on the image and the host read wait for the copy.
        unsafe {
            self.device.run_once(&name, |commands| {
                let image_barriers = [to_source];
                let buffer_barriers = [to_host];
                raw.cmd_pipeline_barrier2(
                    commands,
                    &vk::DependencyInfo::default().image_memory_barriers(&image_barriers),
                );
                raw.cmd_copy_image_to_buffer(
                    commands,
                    self.raw,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    buffer.raw(),
                    &[region],
                );
                raw.cmd_pipeline_barrier2(
                    commands,
                    &vk::DependencyInfo::default().buffer_memory_barriers(&buffer_barriers),
                );
            })?;
        }
        self.layout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
        buffer.read()
    }

    /// The width and height in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
    }

    /// The image's handle, for code that breaks the safe API's rules on purpose, such as tests
    /// that check validation catches mistakes.
    pub fn raw(&self) -> vk::Image {
        self.raw
    }

    /// The image, its view and its size, for recording commands that use it.
    pub(crate) fn parts(&self) -> (vk::Image, vk::ImageView, vk::Extent2D) {
        (self.raw, self.view, self.extent)
    }

    /// The handle of the device that made it.
    pub(crate) fn device(&self) -> vk::Device {
        self.device.handle()
    }

    pub(crate) fn layout(&self) -> vk::ImageLayout {
        self.layout
    }

    /// Records the layout that submitted work leaves the image in.
    pub(crate) fn set_layout(&mut self, layout: vk::ImageLayout) {
        self.layout = layout;
    }

    /// Its slot in the bindless set, which shaders may sample only as `Frame::begin_sampling`
    /// says.
    pub(crate) fn index(&self) -> u32 {
        self.index
    }
}

impl Drop for ColorImage {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            self.device.retire(Retired::Texture(
                self.raw, self.view, allocation, self.index,
            ));
        }
    }
}

/// An image of IDs in GPU memory, which a pass renders beside its target, cleared to 0 as the pass
/// starts, each draw writing its own where it shows: the low and high halves of a 64-bit number,
/// such as an entity's. A frame whose pass rendered it reads pixels of it back, a few at a time,
/// with [`Frame::read_ids`], and then its later passes may sample it through its slot in the
/// bindless set, as [`Frame::begin_sampling_ids`] says. Dropping it retires it, and its slot,
/// until the GPU is done with it.
///
/// [`Frame::read_ids`]: crate::Frame::read_ids
/// [`Frame::begin_sampling_ids`]: crate::Frame::begin_sampling_ids
pub struct IdImage {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    /// Taken when the image is dropped.
    allocation: Option<Allocation>,
    extent: vk::Extent2D,
    /// The layout the work recorded so far leaves the image in.
    layout: vk::ImageLayout,
    /// The number of the frame whose pass last rendered into it; 0 for none.
    rendered: u64,
    /// Its slot in the bindless set, past the placed ones.
    index: u32,
}

impl IdImage {
    /// The format of every ID image: two 32-bit unsigned integers, the low and high halves of an
    /// ID. Vulkan requires every device to render into it, copy from it and sample it.
    pub(crate) const FORMAT: vk::Format = vk::Format::R32G32_UINT;

    /// An ID image of `width` by `height` pixels, in a free slot of the bindless set past the
    /// placed ones, with `name` as its debug name.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn new(device: &Arc<Device>, width: u32, height: u32, name: &str) -> Result<Self> {
        // SAFETY: shaders sample the image only through the index `Frame::begin_sampling_ids`
        // returns, once it has put the image in the shader-read-only layout, and only as unsigned
        // integers, as its format holds them.
        let (raw, view, allocation, extent, index) =
            unsafe { allocate_sampled(device, [width, height], Self::FORMAT, name) }?;
        Ok(Self {
            device: device.clone(),
            raw,
            view,
            allocation: Some(allocation),
            extent,
            layout: vk::ImageLayout::UNDEFINED,
            rendered: 0,
            index,
        })
    }

    /// The width and height in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
    }

    /// The image, its view and its size, for recording commands that use it.
    pub(crate) fn parts(&self) -> (vk::Image, vk::ImageView, vk::Extent2D) {
        (self.raw, self.view, self.extent)
    }

    /// The handle of the device that made it.
    pub(crate) fn device(&self) -> vk::Device {
        self.device.handle()
    }

    pub(crate) fn layout(&self) -> vk::ImageLayout {
        self.layout
    }

    /// Records the layout that the work recorded so far leaves the image in.
    pub(crate) fn set_layout(&mut self, layout: vk::ImageLayout) {
        self.layout = layout;
    }

    /// The number of the frame whose pass last rendered into it; 0 for none.
    pub(crate) fn rendered(&self) -> u64 {
        self.rendered
    }

    /// Records that a pass of frame `frame` renders into it, which leaves it in the layout it
    /// renders in.
    pub(crate) fn set_rendered(&mut self, frame: u64) {
        self.rendered = frame;
        self.layout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
    }

    /// Its slot in the bindless set, which shaders may sample only as
    /// `Frame::begin_sampling_ids` says.
    pub(crate) fn index(&self) -> u32 {
        self.index
    }
}

impl Drop for IdImage {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            let retired = Retired::Texture(self.raw, self.view, allocation, self.index);
            self.device.retire(retired);
        }
    }
}

/// A depth image in GPU memory, which a pass tests its draws against and writes their depths
/// into, cleared as the pass starts and kept for that pass alone. Dropping it retires it until the
/// GPU is done with it.
pub struct DepthImage {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    /// Taken when the image is dropped.
    allocation: Option<Allocation>,
    extent: vk::Extent2D,
}

impl DepthImage {
    /// The format of every depth image: 32-bit floats, which keep their precision far away, as the
    /// renderer's cameras project depth, from 1 at the near plane to 0 far away. Every device the
    /// engine opens can render into it.
    pub(crate) const FORMAT: vk::Format = vk::Format::D32_SFLOAT;

    /// A depth image of `width` by `height` pixels, with `name` as its debug name.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn new(device: &Arc<Device>, width: u32, height: u32, name: &str) -> Result<Self> {
        let usage = vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
        let (raw, view, allocation, extent) = allocate(
            device,
            [width, height],
            Self::FORMAT,
            usage,
            DEPTH_RANGE,
            name,
        )?;
        Ok(Self {
            device: device.clone(),
            raw,
            view,
            allocation: Some(allocation),
            extent,
        })
    }

    /// The image, its view and its size, for recording commands that use it.
    pub(crate) fn parts(&self) -> (vk::Image, vk::ImageView, vk::Extent2D) {
        (self.raw, self.view, self.extent)
    }

    /// The handle of the device that made it.
    pub(crate) fn device(&self) -> vk::Device {
        self.device.handle()
    }
}

impl Drop for DepthImage {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            let retired = Retired::Image(self.raw, self.view, allocation);
            self.device.retire(retired);
        }
    }
}

/// A color image of `size` pixels in GPU memory, of `format`, that passes render into, frames
/// copy from and shaders sample, with its view in a free slot of the bindless set past the placed
/// ones, all named `name`. It returns what [`allocate`] does, and the slot's index.
///
/// # Safety
///
/// Shaders must sample the image only in the shader-read-only layout, as for `Bindless::write`.
///
/// # Panics
///
/// If a side is 0.
unsafe fn allocate_sampled(
    device: &Device,
    size: [u32; 2],
    format: vk::Format,
    name: &str,
) -> Result<(vk::Image, vk::ImageView, Allocation, vk::Extent2D, u32)> {
    let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
        | vk::ImageUsageFlags::SAMPLED
        | vk::ImageUsageFlags::TRANSFER_SRC;
    let (raw, view, allocation, extent) = allocate(device, size, format, usage, COLOR_RANGE, name)?;
    // SAFETY: the view is of a live sampled image of the device, and the caller guarantees how
    // shaders sample it.
    match unsafe { device.bindless().add(device.raw(), view, None, name) } {
        Ok(index) => Ok((raw, view, allocation, extent, index)),
        Err(error) => {
            device.retire(Retired::Image(raw, view, allocation));
            Err(error)
        }
    }
}

/// An image of `size` pixels in GPU memory, of `format`, for `usage`, and a view of its `range`,
/// both named `name`.
///
/// # Panics
///
/// If a side is 0.
fn allocate(
    device: &Device,
    size: [u32; 2],
    format: vk::Format,
    usage: vk::ImageUsageFlags,
    range: vk::ImageSubresourceRange,
    name: &str,
) -> Result<(vk::Image, vk::ImageView, Allocation, vk::Extent2D)> {
    let [width, height] = size;
    assert!(
        width > 0 && height > 0,
        "a {width}x{height} image, but an image has an area"
    );
    let extent = vk::Extent2D { width, height };
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(extent.into())
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage);
    let (raw, allocation) = device.allocator().create_image(&info, Memory::Gpu, name)?;
    let view_info = vk::ImageViewCreateInfo::default()
        .image(raw)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(range);
    // SAFETY: the image is live and bound to memory.
    match unsafe { device.raw().create_image_view(&view_info, None) } {
        Ok(view) => {
            // SAFETY: the image and view are live.
            unsafe {
                device.set_name(raw, name);
                device.set_name(view, name);
            }
            Ok((raw, view, allocation, extent))
        }
        Err(error) => {
            device.retire(Retired::Image(raw, vk::ImageView::null(), allocation));
            let view = format_args!("image {name:?}'s view");
            Err(vk_error("vkCreateImageView", view, error))
        }
    }
}
