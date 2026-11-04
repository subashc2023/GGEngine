//! Offscreen color images, which frames render into as targets, as headless runs do, later passes
//! of a frame sample and the CPU reads back; and what making an image takes, which a graph's own
//! images share.

use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use crate::buffer::Buffer;
use crate::device::Retired;
use crate::vma::{Allocation, Memory};
use crate::{COLOR_RANGE, Device, Format, copy_region, vk_error};

/// A color image in GPU memory, which frames render into as a target, their later passes sample
/// through its slot in the bindless set, and the CPU can read back. Dropping it retires it, and its
/// slot, until the GPU is done with it.
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
        assert!(
            width > 0 && height > 0,
            "a {width}x{height} image, but an image has an area"
        );
        let format = Self::FORMAT.color;
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC;
        let info = image_info([width, height], format, usage);
        let (raw, allocation) = device.allocator().create_image(&info, Memory::Gpu, name)?;
        let view = match image_view(device, raw, format, COLOR_RANGE, name) {
            Ok(view) => view,
            Err(error) => {
                device.retire(Retired::Image(raw, vk::ImageView::null(), allocation));
                return Err(error);
            }
        };
        // SAFETY: the view is of a live sampled image of the device, which shaders sample only
        // through a pass that samples it, once its barrier has put it in the shader-read-only
        // layout.
        let index = match unsafe { device.bindless().add(device.raw(), view, None, name) } {
            Ok(index) => index,
            Err(error) => {
                device.retire(Retired::Image(raw, view, allocation));
                return Err(error);
            }
        };
        Ok(Self {
            device: device.clone(),
            raw,
            view,
            allocation: Some(allocation),
            extent: vk::Extent2D { width, height },
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

    /// Its slot in the bindless set, which shaders may sample only in a pass that samples it.
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

/// How an image of `size` pixels, of `format`, for `usage`, is made: one level and one layer,
/// sampled once a pixel, in the GPU's own tiling.
pub(crate) fn image_info(
    size: [u32; 2],
    format: vk::Format,
    usage: vk::ImageUsageFlags,
) -> vk::ImageCreateInfo<'static> {
    let [width, height] = size;
    vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
}

/// A view of `range` of `image`, of `format`, which it names `name`, as it names the image.
pub(crate) fn image_view(
    device: &Device,
    image: vk::Image,
    format: vk::Format,
    range: vk::ImageSubresourceRange,
    name: &str,
) -> Result<vk::ImageView> {
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(range);
    // SAFETY: the image is live and bound to memory.
    let view = unsafe { device.raw().create_image_view(&view_info, None) }.map_err(|e| {
        let view = format_args!("image {name:?}'s view");
        vk_error("vkCreateImageView", view, e)
    })?;
    // SAFETY: the image and view are live.
    unsafe {
        device.set_name(image, name);
        device.set_name(view, name);
    }
    Ok(view)
}
