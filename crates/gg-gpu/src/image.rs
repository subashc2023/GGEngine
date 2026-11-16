//! Offscreen color images: frames render into them as targets, as headless runs do, later passes
//! sample them and the CPU reads them back. Also what making an image takes, which a graph's own
//! images share.

use std::sync::Arc;

use ash::vk;
use gg_core::{Error, Result};

use crate::buffer::Buffer;
use crate::device::Retired;
use crate::vma::{Allocation, Memory};
use crate::{COLOR_RANGE, Device, Format, copy_region, vk_failed};

/// A color image in GPU memory: frames render into it, their later passes sample it through its
/// bindless slot, and the CPU can read it back. Dropping it retires it and its slot until the GPU
/// is done with it.
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
    /// Its debug name; its read-backs and their errors are named after it.
    name: String,
}

impl ColorImage {
    /// The format of every color image: 8-bit sRGB, as windows usually present, so read-backs
    /// match what a window would show. The game's view is one, so games' pipelines are made for it.
    pub const FORMAT: Format = Format::new(vk::Format::R8G8B8A8_SRGB);

    /// An image of `width` by `height` pixels, in a free bindless slot past the placed ones, with
    /// `name` as its debug name. Fails with `gpu.image_too_large` if a side is past the device's
    /// most.
    ///
    /// # Panics
    ///
    /// If a side is 0.
    pub fn new(device: &Arc<Device>, width: u32, height: u32, name: &str) -> Result<Self> {
        assert!(
            width > 0 && height > 0,
            "a {width}x{height} image, but an image has an area"
        );
        check_size(device, [width, height], name)?;
        let format = Self::FORMAT.color;
        let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC;
        let info = image_info([width, height], format, usage);
        let (raw, allocation) = device.allocator().create_image(&info, Memory::Gpu, name)?;
        let shape = vk::ImageViewType::TYPE_2D;
        let view = match image_view(device, raw, shape, format, COLOR_RANGE, name) {
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

    /// Copies the image to the CPU as tightly packed rows of sRGB RGBA8 pixels, top row first.
    /// Waits for the GPU to finish everything submitted so far.
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

    /// The image's handle, for code that breaks the safe API's rules on purpose, such as tests of
    /// validation catching mistakes.
    pub fn raw(&self) -> vk::Image {
        self.raw
    }

    /// The image, its view and its size, for recording commands that use it.
    pub(crate) fn parts(&self) -> (vk::Image, vk::ImageView, vk::Extent2D) {
        (self.raw, self.view, self.extent)
    }

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

/// Checks that a one-layer 2D image `name` of `size` pixels fits `device`'s limits, failing with
/// `gpu.image_too_large` before Vulkan is asked to make it
/// (VUID-VkImageCreateInfo-extent-02252 and -02253).
pub(crate) fn check_size(device: &Device, size: [u32; 2], name: &str) -> Result<()> {
    let most = device.limits().max_image_dimension2_d;
    let [width, height] = size;
    if width <= most && height <= most {
        return Ok(());
    }
    Err(Error::new(
        "gpu.image_too_large",
        format!(
            "image {name:?} is {width}x{height}, but device {:?} makes 2D images of at most {most} \
             pixels a side",
            device.name()
        ),
    ))
}

/// An image of `size` pixels, `format` and `usage`: one level, one layer, one sample per pixel,
/// in the GPU's optimal tiling.
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

/// A view of `range` of `image`, shaped as `shape`, in `format`. It names the view and the image
/// `name`.
pub(crate) fn image_view(
    device: &Device,
    image: vk::Image,
    shape: vk::ImageViewType,
    format: vk::Format,
    range: vk::ImageSubresourceRange,
    name: &str,
) -> Result<vk::ImageView> {
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(shape)
        .format(format)
        .subresource_range(range);
    // SAFETY: the image is live and bound to memory.
    let view = unsafe { device.raw().create_image_view(&view_info, None) }.map_err(vk_failed(
        "vkCreateImageView",
        format_args!("image {name:?}'s view"),
    ))?;
    // SAFETY: the image and view are live.
    unsafe {
        device.set_name(image, name);
        device.set_name(view, name);
    }
    Ok(view)
}
