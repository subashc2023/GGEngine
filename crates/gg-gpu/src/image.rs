//! Offscreen color images: what headless runs render into, and what tests read back.

use std::sync::Arc;

use ash::vk;
use gg_core::{Error, Result};

use crate::{COLOR_RANGE, Device, vk_error};

/// The format of offscreen images: 8-bit sRGB, as windows usually present, so what's read back
/// matches what a window would show.
const FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;

/// A color image in GPU memory, which frames render into and the CPU can read back.
pub struct ColorImage {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    memory: vk::DeviceMemory,
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
        let raw_device = device.raw();
        // SAFETY: the device is live and the info is valid.
        let raw = unsafe { raw_device.create_image(&info, None) }
            .map_err(|e| vk_error("vkCreateImage", e))?;
        // From here on, dropping the image destroys what has been made so far.
        let mut image = Self {
            device: device.clone(),
            raw,
            view: vk::ImageView::null(),
            memory: vk::DeviceMemory::null(),
            extent,
            layout: vk::ImageLayout::UNDEFINED,
        };
        // SAFETY: the image is live.
        let requirements = unsafe { raw_device.get_image_memory_requirements(raw) };
        image.memory = allocate(
            device,
            requirements,
            vk::MemoryPropertyFlags::empty(),
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
            name,
        )?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(raw)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(FORMAT)
            .subresource_range(COLOR_RANGE);
        // SAFETY: the memory was allocated for this image's requirements and the image isn't
        // bound yet; the view is made once it is.
        unsafe {
            raw_device
                .bind_image_memory(raw, image.memory, 0)
                .map_err(|e| vk_error("vkBindImageMemory", e))?;
            image.view = raw_device
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
        let buffer = ReadBuffer::new(&self.device, size, "read-back buffer")?;
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
            .buffer(buffer.raw)
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
                    buffer.raw,
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
        // Frames in flight may still render into the image.
        let _ = self.device.wait_idle();
        let raw = self.device.raw();
        // SAFETY: the GPU is idle, so nothing uses the image. Null handles, left by a failed
        // `new`, are allowed.
        unsafe {
            raw.destroy_image_view(self.view, None);
            raw.destroy_image(self.raw, None);
            raw.free_memory(self.memory, None);
        }
    }
}

/// A host-visible buffer that the GPU copies into for the CPU to read.
struct ReadBuffer {
    device: Arc<Device>,
    raw: vk::Buffer,
    memory: vk::DeviceMemory,
    size: u64,
}

impl ReadBuffer {
    fn new(device: &Arc<Device>, size: u64, name: &str) -> Result<Self> {
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let raw_device = device.raw();
        // SAFETY: the device is live and the info is valid.
        let raw = unsafe { raw_device.create_buffer(&info, None) }
            .map_err(|e| vk_error("vkCreateBuffer", e))?;
        let mut buffer = Self {
            device: device.clone(),
            raw,
            memory: vk::DeviceMemory::null(),
            size,
        };
        // SAFETY: the buffer is live.
        let requirements = unsafe { raw_device.get_buffer_memory_requirements(raw) };
        // Coherent memory needs no flushing, and cached memory makes the CPU's reads fast.
        buffer.memory = allocate(
            device,
            requirements,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            vk::MemoryPropertyFlags::HOST_CACHED,
            name,
        )?;
        // SAFETY: the memory was allocated for this buffer's requirements, and the buffer isn't
        // bound yet.
        unsafe {
            raw_device
                .bind_buffer_memory(raw, buffer.memory, 0)
                .map_err(|e| vk_error("vkBindBufferMemory", e))?;
            device.set_name(raw, name);
        }
        Ok(buffer)
    }

    /// Copies the buffer's contents out, once the GPU has finished writing them.
    fn read(&self) -> Result<Vec<u8>> {
        let raw = self.device.raw();
        // SAFETY: the memory is host-visible and coherent, isn't mapped elsewhere, and the GPU
        // is done writing it; the mapping covers `size` bytes until it's unmapped.
        unsafe {
            let mapped = raw
                .map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty())
                .map_err(|e| vk_error("vkMapMemory", e))?;
            let bytes =
                std::slice::from_raw_parts(mapped.cast::<u8>(), self.size as usize).to_vec();
            raw.unmap_memory(self.memory);
            Ok(bytes)
        }
    }
}

impl Drop for ReadBuffer {
    fn drop(&mut self) {
        // A copy into the buffer may still be running if waiting for it failed.
        let _ = self.device.wait_idle();
        let raw = self.device.raw();
        // SAFETY: the GPU is idle, so nothing uses the buffer. A null handle is allowed.
        unsafe {
            raw.destroy_buffer(self.raw, None);
            raw.free_memory(self.memory, None);
        }
    }
}

/// Allocates memory for `requirements` with the `required` properties, preferring the
/// `preferred` ones.
///
/// Each image and buffer gets an allocation of its own, which is fine for the few that headless
/// runs and tests make. Allocations are limited (to 4,096 on KosmicKrisp), so resources in
/// number go through VMA instead, from Phase 3.
fn allocate(
    device: &Device,
    requirements: vk::MemoryRequirements,
    required: vk::MemoryPropertyFlags,
    preferred: vk::MemoryPropertyFlags,
    name: &str,
) -> Result<vk::DeviceMemory> {
    let Some(memory_type) = device.memory_type(requirements.memory_type_bits, required, preferred)
    else {
        return Err(Error::new(
            "gpu.no_memory_type",
            format!("{} has no {required:?} memory for {name}", device.name()),
        ));
    };
    let info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(memory_type);
    // SAFETY: the device is live, and the memory type is one of its own.
    unsafe {
        let memory = device
            .raw()
            .allocate_memory(&info, None)
            .map_err(|e| vk_error("vkAllocateMemory", e))?;
        device.set_name(memory, name);
        Ok(memory)
    }
}
