//! Textures: images shaders sample, each in a slot of the bindless set.

use std::slice;
use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use crate::device::Retired;
use crate::vma::{Allocation, Memory};
use crate::{COLOR_LAYERS, COLOR_RANGE, Device, UploadPath, vk_error};

/// The most pixels a side a texture has: what every Vulkan 1.4 device takes, the version the
/// engine requires.
pub const MAX_TEXTURE_SIZE: u32 = 8192;

/// How a texture's 8-bit RGBA channels read in shaders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFormat {
    /// Shaders read the bytes as they are, scaled to 0 to 1: for data, and for color a shader
    /// decodes itself.
    Rgba8Unorm,
    /// Shaders read color decoded from sRGB to linear, before it's filtered, and alpha as it is:
    /// for color as images store it.
    Rgba8Srgb,
}

/// An RGBA image with 8-bit channels, that shaders sample through its index in the bindless
/// set. Frames fill its pixels, and may write over them, and they're undefined until then.
/// Dropping it retires it until the GPU is done with it.
pub struct Texture {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    /// Taken when the texture is dropped.
    allocation: Option<Allocation>,
    extent: vk::Extent2D,
    index: u32,
    /// Whether a frame has filled or written it, which leaves it in the shader-read-only layout.
    written: bool,
}

impl Texture {
    /// What every texture is made for: shaders sample it, and copies write it.
    pub(crate) const USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
        vk::ImageUsageFlags::SAMPLED.as_raw() | vk::ImageUsageFlags::TRANSFER_DST.as_raw(),
    );

    /// A texture of `width` by `height` pixels, in a free slot of the bindless set past the placed
    /// ones, with `name` as its debug name.
    ///
    /// # Panics
    ///
    /// If a side is 0, or more than [`MAX_TEXTURE_SIZE`].
    pub fn new(
        device: &Arc<Device>,
        width: u32,
        height: u32,
        format: TextureFormat,
        name: &str,
    ) -> Result<Self> {
        Self::make(device, None, width, height, format, name)
    }

    /// A texture like [`Texture::new`]'s, in placed slot `slot`, one of the first
    /// [`PLACED_TEXTURES`](crate::PLACED_TEXTURES), so that its index is known before it's made.
    ///
    /// # Panics
    ///
    /// As [`Texture::new`] does; and if `slot` isn't placed, or is in use: by a texture, or by one
    /// dropped while the GPU may still sample it.
    pub fn placed(
        device: &Arc<Device>,
        slot: u32,
        width: u32,
        height: u32,
        format: TextureFormat,
        name: &str,
    ) -> Result<Self> {
        Self::make(device, Some(slot), width, height, format, name)
    }

    fn make(
        device: &Arc<Device>,
        slot: Option<u32>,
        width: u32,
        height: u32,
        format: TextureFormat,
        name: &str,
    ) -> Result<Self> {
        let sides = 1..=MAX_TEXTURE_SIZE;
        assert!(
            sides.contains(&width) && sides.contains(&height),
            "a {width}x{height} texture, but textures are 1 to {MAX_TEXTURE_SIZE} pixels a side"
        );
        let extent = vk::Extent2D { width, height };
        let format = match format {
            TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
            TextureFormat::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
        };
        // Host copies need it, and only devices whose drivers say it costs them nothing upload
        // that way, unless a test asks.
        let usage = if device.upload_path() == UploadPath::HostImageCopy {
            Self::USAGE | vk::ImageUsageFlags::HOST_TRANSFER_EXT
        } else {
            Self::USAGE
        };
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(extent.into())
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage);
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
        // before any shader can sample it: frames fill or write it first.
        let index = match unsafe { device.bindless().add(device.raw(), view, slot) } {
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
    pub(crate) fn size(&self) -> [u32; 2] {
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

    /// Writes `pixels`, every one of the texture's, from the CPU straight into its image, and
    /// leaves it in the shader-read-only layout: host image copy, on a device that uploads that
    /// way. The device's next submission makes the writes visible to it.
    ///
    /// # Panics
    ///
    /// If the device doesn't upload by host image copy, a frame has written the texture, or
    /// `pixels` doesn't fill it exactly.
    pub(crate) fn copy_from_host(&mut self, pixels: &[u8]) -> Result<()> {
        let functions = self
            .device
            .host_image_copy_fn()
            .expect("only a device that uploads by host image copy copies from the host");
        assert!(
            !self.written,
            "a texture is copied from the host before any frame writes it"
        );
        let vk::Extent2D { width, height } = self.extent;
        assert_eq!(
            pixels.len() as u64,
            u64::from(width) * u64::from(height) * 4,
            "the pixels don't fill a {width}x{height} texture",
        );
        let transition = vk::HostImageLayoutTransitionInfoEXT::default()
            .image(self.raw)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .subresource_range(COLOR_RANGE);
        // SAFETY: the image is live and made for host transfers, which the device enabled, into
        // the shader-read-only layout, which it listed as one host copies write; no submission
        // has used it, since no frame has written it, and `&mut` keeps the engine off it.
        unsafe { functions.transition_image_layout(slice::from_ref(&transition)) }
            .map_err(|e| vk_error("vkTransitionImageLayoutEXT", e))?;
        let region = vk::MemoryToImageCopyEXT::default()
            .host_pointer(pixels.as_ptr().cast())
            .image_subresource(COLOR_LAYERS)
            .image_extent(self.extent.into());
        let info = vk::CopyMemoryToImageInfoEXT::default()
            .dst_image(self.raw)
            .dst_image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .regions(slice::from_ref(&region));
        // SAFETY: as above, in the layout just changed to; the region is the whole image, whose
        // every texel `pixels`, tightly packed, was just checked to hold.
        unsafe { functions.copy_memory_to_image(&info) }
            .map_err(|e| vk_error("vkCopyMemoryToImageEXT", e))
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
