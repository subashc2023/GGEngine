//! Offscreen color images: what headless runs render into, and what tests read back.

use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use crate::buffer::Buffer;
use crate::device::Retired;
use crate::vma::{Allocation, Memory};
use crate::{COLOR_RANGE, Device, vk_error};

/// The format of offscreen images: 8-bit sRGB, as windows usually present, so what's read back
/// matches what a window would show.
pub(crate) const FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;

/// A color image in GPU memory, which frames render into and the CPU can read back. Dropping it
/// retires it until the GPU is done with it.
pub struct ColorImage {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    /// Taken when the image is dropped.
    allocation: Option<Allocation>,
    extent: vk::Extent2D,
    /// The layout the work submitted so far leaves the image in.
    layout: vk::ImageLayout,
}

impl ColorImage {
    /// An image of `width` by `height` pixels, with `name` as its debug name.
    pub fn new(device: &Arc<Device>, width: u32, height: u32, name: &str) -> Result<Self> {
        let extent = vk::Extent2D { width, height };
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(FORMAT)
            .extent(extent.into())
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC);
        let (raw, allocation) = device.allocator().create_image(&info, Memory::Gpu)?;
        // From here on, dropping the image retires what has been made so far.
        let mut image = Self {
            device: device.clone(),
            raw,
            view: vk::ImageView::null(),
            allocation: Some(allocation),
            extent,
            layout: vk::ImageLayout::UNDEFINED,
        };
        let view_info = vk::ImageViewCreateInfo::default()
            .image(raw)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(FORMAT)
            .subresource_range(COLOR_RANGE);
        // SAFETY: the image is live and bound to memory.
        unsafe {
            image.view = device
                .raw()
                .create_image_view(&view_info, None)
                .map_err(|e| vk_error("vkCreateImageView", e))?;
            device.set_name(raw, name);
            device.set_name(image.view, name);
        }
        Ok(image)
    }

    /// Copies the image to the CPU as rows of RGBA8 pixels, sRGB-encoded and tightly packed, top
    /// row first. It waits for the GPU to finish everything submitted so far.
    pub fn read_back(&mut self) -> Result<Vec<u8>> {
        let size = u64::from(self.extent.width) * u64::from(self.extent.height) * 4;
        let buffer = Buffer::new(
            &self.device,
            size,
            vk::BufferUsageFlags::TRANSFER_DST,
            Memory::Readback,
            "read-back buffer",
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
        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_extent(self.extent.into());
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
            self.device.run_once(|commands| {
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

    /// The image's format, which pipelines that draw into it are made for.
    pub fn format(&self) -> crate::Format {
        crate::Format(FORMAT)
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

    pub(crate) fn layout(&self) -> vk::ImageLayout {
        self.layout
    }

    /// Records the layout that submitted work leaves the image in.
    pub(crate) fn set_layout(&mut self, layout: vk::ImageLayout) {
        self.layout = layout;
    }
}

impl Drop for ColorImage {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            self.device
                .retire(Retired::Image(self.raw, self.view, allocation));
        }
    }
}
