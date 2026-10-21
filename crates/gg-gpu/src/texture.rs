//! Textures: images shaders sample, each in a slot of the bindless set.

use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use crate::device::Retired;
use crate::vma::{Allocation, Memory};
use crate::{COLOR_RANGE, Device, vk_error};

/// How a texture's 8-bit RGBA channels read in shaders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFormat {
    /// Color is sRGB-encoded, and shaders read it decoded, as linear values.
    Rgba8Srgb,
    /// Shaders read the bytes as they are, scaled to 0 to 1: for data, and for color a shader
    /// decodes itself.
    Rgba8Unorm,
}

/// An RGBA image with 8-bit channels, that shaders sample through its index in the bindless
/// set. Frames write its pixels, which are undefined until then. Dropping it retires it until
/// the GPU is done with it.
pub struct Texture {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    /// Taken when the texture is dropped.
    allocation: Option<Allocation>,
    extent: vk::Extent2D,
    index: u32,
    /// Whether a frame has written it, which leaves it in the shader-read-only layout.
    written: bool,
}

impl Texture {
    /// A texture of `width` by `height` pixels, with `name` as its debug name.
    pub fn new(
        device: &Arc<Device>,
        width: u32,
        height: u32,
        format: TextureFormat,
        name: &str,
    ) -> Result<Self> {
        let extent = vk::Extent2D { width, height };
        let format = match format {
            TextureFormat::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
            TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        };
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(extent.into())
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST);
        let (raw, allocation) = device.allocator().create_image(&info, Memory::Gpu)?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(raw)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(info.format)
            .subresource_range(COLOR_RANGE);
        // SAFETY: the image is live and bound to memory.
        let view = match unsafe { device.raw().create_image_view(&view_info, None) } {
            Ok(view) => view,
            Err(error) => {
                device.retire(Retired::Image(raw, vk::ImageView::null(), allocation));
                return Err(vk_error("vkCreateImageView", error));
            }
        };
        // SAFETY: the view is of a live sampled image, which is in the shader-read-only layout
        // before any shader can sample it: frames write it first.
        let index = match unsafe { device.bindless().add(device.raw(), view) } {
            Ok(index) => index,
            Err(error) => {
                device.retire(Retired::Image(raw, view, allocation));
                return Err(error);
            }
        };
        // SAFETY: the image and view are live.
        unsafe {
            device.set_name(raw, name);
            device.set_name(view, name);
        }
        Ok(Self {
            device: device.clone(),
            raw,
            view,
            allocation: Some(allocation),
            extent,
            index,
            written: false,
        })
    }

    /// Where shaders find the texture in the bindless set's texture array.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The width and height in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
    }

    pub(crate) fn raw(&self) -> vk::Image {
        self.raw
    }

    pub(crate) fn written(&self) -> bool {
        self.written
    }

    pub(crate) fn set_written(&mut self) {
        self.written = true;
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            self.device.retire(Retired::Texture(
                self.raw, self.view, allocation, self.index,
            ));
        }
    }
}
