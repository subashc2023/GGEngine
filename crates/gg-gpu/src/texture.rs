//! Textures: images shaders sample, each in a slot of the bindless set.

use std::iter;
use std::mem;
use std::slice;
use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use crate::device::Retired;
use crate::vma::{Allocation, Memory};
use crate::{COLOR_RANGE, Device, UploadPath, color_layers, vk_error};

/// The most pixels a side a texture has: what every Vulkan 1.4 device takes, the version the
/// engine requires.
pub const MAX_TEXTURE_SIZE: u32 = 8192;

/// The most mip levels a texture has: a full chain of [`MAX_TEXTURE_SIZE`] pixels a side.
pub(crate) const MAX_LEVELS: usize = MAX_TEXTURE_SIZE.ilog2() as usize + 1;

/// The sizes of the levels of a full mip chain of a `width` by `height` image, largest first: each
/// half the one before, rounded down but at least a pixel a side, down to a pixel. A texture made
/// mipmapped has these levels.
pub fn mip_sizes(width: u32, height: u32) -> impl Iterator<Item = [u32; 2]> {
    iter::successors(Some([width, height]), |&[width, height]| {
        (width > 1 || height > 1).then(|| [(width / 2).max(1), (height / 2).max(1)])
    })
}

/// The sizes of the first `count` levels of a `width` by `height` image's mip chain, and where each
/// one's pixels start among all of theirs, a level's after the one before's.
fn levels(width: u32, height: u32, count: u32) -> impl Iterator<Item = ([u32; 2], u64)> {
    let mut offset = 0;
    mip_sizes(width, height)
        .take(count as usize)
        .map(move |size @ [width, height]| {
            let at = offset;
            offset += u64::from(width) * u64::from(height) * 4;
            (size, at)
        })
}

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

/// An RGBA image with 8-bit channels, with one mip level or a full chain of them, that shaders
/// sample through its index in the bindless set. Frames fill its pixels, may write over those of
/// one with one level, and may refill it with another image, and they're undefined until then.
/// Dropping it retires it until the GPU is done with it.
pub struct Texture {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    /// Taken when the texture is dropped.
    allocation: Option<Allocation>,
    extent: vk::Extent2D,
    /// How many mip levels it has.
    levels: u32,
    format: vk::Format,
    index: u32,
    /// Whether a frame has filled or written it, which leaves it in the shader-read-only layout.
    written: bool,
    /// Its debug name, which its errors name it by.
    name: String,
}

impl Texture {
    /// What every texture is made for: shaders sample it, and copies write it.
    pub(crate) const USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
        vk::ImageUsageFlags::SAMPLED.as_raw() | vk::ImageUsageFlags::TRANSFER_DST.as_raw(),
    );

    /// A texture of `width` by `height` pixels, with the full chain of mip levels that
    /// [`mip_sizes`] gives if `mipmapped`, and otherwise one, in a free slot of the bindless set
    /// past the placed ones, with `name` as its debug name.
    ///
    /// # Panics
    ///
    /// If a side is 0, or more than [`MAX_TEXTURE_SIZE`].
    pub fn new(
        device: &Arc<Device>,
        width: u32,
        height: u32,
        format: TextureFormat,
        mipmapped: bool,
        name: &str,
    ) -> Result<Self> {
        Self::make(device, None, width, height, format, mipmapped, name)
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
        mipmapped: bool,
        name: &str,
    ) -> Result<Self> {
        Self::make(device, Some(slot), width, height, format, mipmapped, name)
    }

    fn make(
        device: &Arc<Device>,
        slot: Option<u32>,
        width: u32,
        height: u32,
        format: TextureFormat,
        mipmapped: bool,
        name: &str,
    ) -> Result<Self> {
        let format = match format {
            TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
            TextureFormat::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
        };
        let (info, levels) = image_info(device, width, height, format, mipmapped);
        // The slot first: a refused one panics, which must leave no image behind.
        let bindless = device.bindless();
        let index = bindless.reserve(slot, name)?;
        let (raw, allocation, view) = match make_image(device, &info, name) {
            Ok(made) => made,
            Err(error) => {
                bindless.remove(index);
                return Err(error);
            }
        };
        // SAFETY: the view is of a live sampled image, which is in the shader-read-only layout
        // before any shader can sample it: frames fill or write it first. No frame uses the slot,
        // which was free when it was reserved.
        unsafe { bindless.write(device.raw(), view, index) };
        Ok(Self {
            device: device.clone(),
            raw,
            view,
            allocation: Some(allocation),
            extent: vk::Extent2D { width, height },
            levels,
            format,
            index,
            written: false,
            name: name.to_owned(),
        })
    }

    /// Makes the texture's image anew, `width` by `height` pixels, with the full chain of mip
    /// levels that [`mip_sizes`] gives if `mipmapped` and otherwise one, in its format, its slot
    /// taking its view at once: a frame then fills it as one no frame has written. The old image
    /// retires until the GPU is done with the frames that may use it. If it fails, the texture is
    /// as it was.
    ///
    /// # Safety
    ///
    /// No frame that the GPU may still run samples the texture: its slot changes at once, which the
    /// bindless set allows only for slots no submitted frame uses.
    ///
    /// # Panics
    ///
    /// If a side is 0, or more than [`MAX_TEXTURE_SIZE`].
    pub(crate) unsafe fn remake(&mut self, width: u32, height: u32, mipmapped: bool) -> Result<()> {
        let (info, levels) = image_info(&self.device, width, height, self.format, mipmapped);
        let (raw, allocation, view) = make_image(&self.device, &info, &self.name)?;
        let bindless = self.device.bindless();
        // SAFETY: the view is of a live sampled image, which frames fill before any shader samples
        // it, as for a new texture; the caller guarantees that no submitted frame uses the slot.
        unsafe { bindless.write(self.device.raw(), view, self.index) };
        let old = self.allocation.replace(allocation);
        let old = old.expect("a texture keeps its memory until it's dropped");
        let (old_raw, old_view) = (
            mem::replace(&mut self.raw, raw),
            mem::replace(&mut self.view, view),
        );
        self.device.retire(Retired::Image(old_raw, old_view, old));
        self.extent = vk::Extent2D { width, height };
        self.levels = levels;
        self.written = false;
        Ok(())
    }

    /// Where shaders find the texture in the bindless set's texture array.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The width and height in pixels, of its first and largest level.
    pub fn size(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
    }

    /// How many mip levels it has.
    pub(crate) fn level_count(&self) -> u32 {
        self.levels
    }

    /// Its mip levels' sizes, largest first, and where each one's pixels start among all of
    /// theirs, packed a level after the one before.
    pub(crate) fn levels(&self) -> impl Iterator<Item = ([u32; 2], u64)> {
        levels(self.extent.width, self.extent.height, self.levels)
    }

    /// How many bytes of RGBA8 fill all its levels.
    pub(crate) fn bytes(&self) -> u64 {
        let last = self.levels().last();
        last.map_or(0, |([width, height], at)| {
            at + u64::from(width) * u64::from(height) * 4
        })
    }

    pub(crate) fn raw(&self) -> vk::Image {
        self.raw
    }

    /// The handle of the device that made it.
    pub(crate) fn device(&self) -> vk::Device {
        self.device.handle()
    }

    pub(crate) fn written(&self) -> bool {
        self.written
    }

    pub(crate) fn set_written(&mut self) {
        self.written = true;
    }

    /// Writes `pixels`, every one of the texture's, its levels' packed largest first, from the CPU
    /// straight into its image, and leaves it in the shader-read-only layout: host image copy, on
    /// a device that uploads that way. The device's next submission makes the writes visible to
    /// it.
    ///
    /// # Panics
    ///
    /// If the device doesn't upload by host image copy, a frame has written the texture, or
    /// `pixels` doesn't fill its levels exactly.
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
            self.bytes(),
            "the pixels don't fill a {width}x{height} texture's {} levels",
            self.levels,
        );
        let transition = vk::HostImageLayoutTransitionInfoEXT::default()
            .image(self.raw)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .subresource_range(COLOR_RANGE);
        let texture = format_args!(
            "texture {:?} of {width}x{height} pixels in {} level(s)",
            self.name, self.levels
        );
        // SAFETY: the image is live and made for host transfers, which the device enabled, into
        // the shader-read-only layout, which it listed as one host copies write; no submission
        // has used it, since no frame has written it, and `&mut` keeps the engine off it.
        unsafe { functions.transition_image_layout(slice::from_ref(&transition)) }
            .map_err(|e| vk_error("vkTransitionImageLayoutEXT", texture, e))?;
        // A region for each level, each from where its pixels start.
        let mut regions = [vk::MemoryToImageCopyEXT::default(); MAX_LEVELS];
        for ((level, ([width, height], at)), region) in self.levels().enumerate().zip(&mut regions)
        {
            *region = vk::MemoryToImageCopyEXT::default()
                .host_pointer(pixels[at as usize..].as_ptr().cast())
                .image_subresource(color_layers(level as u32))
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                });
        }
        let info = vk::CopyMemoryToImageInfoEXT::default()
            .dst_image(self.raw)
            .dst_image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .regions(&regions[..self.levels as usize]);
        // SAFETY: as above, in the layout just changed to; the regions are the image's levels,
        // whose every texel `pixels`, tightly packed a level after another, was just checked to
        // hold.
        unsafe { functions.copy_memory_to_image(&info) }
            .map_err(|e| vk_error("vkCopyMemoryToImageEXT", texture, e))
    }
}

/// How an image of `width` by `height` pixels in `format` is made for a texture on `device`, with
/// the full chain of mip levels that [`mip_sizes`] gives if `mipmapped` and otherwise one, and how
/// many levels it has.
///
/// # Panics
///
/// If a side is 0, or more than [`MAX_TEXTURE_SIZE`].
fn image_info(
    device: &Device,
    width: u32,
    height: u32,
    format: vk::Format,
    mipmapped: bool,
) -> (vk::ImageCreateInfo<'static>, u32) {
    let sides = 1..=MAX_TEXTURE_SIZE;
    assert!(
        sides.contains(&width) && sides.contains(&height),
        "a {width}x{height} texture, but textures are 1 to {MAX_TEXTURE_SIZE} pixels a side"
    );
    let levels = if mipmapped {
        mip_sizes(width, height).count() as u32
    } else {
        1
    };
    // Host copies need it, and only devices whose drivers say it costs them nothing upload that
    // way, unless a test asks.
    let usage = if device.upload_path() == UploadPath::HostImageCopy {
        Texture::USAGE | vk::ImageUsageFlags::HOST_TRANSFER_EXT
    } else {
        Texture::USAGE
    };
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(levels)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage);
    (info, levels)
}

/// Makes the image `info` describes in GPU memory, and a view of it, both named `name`.
fn make_image(
    device: &Device,
    info: &vk::ImageCreateInfo<'_>,
    name: &str,
) -> Result<(vk::Image, Allocation, vk::ImageView)> {
    let (raw, allocation) = device.allocator().create_image(info, Memory::Gpu, name)?;
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
            let view = format_args!("texture {name:?}'s view");
            return Err(vk_error("vkCreateImageView", view, error));
        }
    };
    // SAFETY: the image and view are live.
    unsafe {
        device.set_name(raw, name);
        device.set_name(view, name);
    }
    Ok((raw, allocation, view))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_chain_halves_each_side_down_to_a_pixel() {
        let sizes: Vec<_> = mip_sizes(211, 5).collect();
        let expected = [
            [211, 5],
            [105, 2],
            [52, 1],
            [26, 1],
            [13, 1],
            [6, 1],
            [3, 1],
            [1, 1],
        ];
        assert_eq!(sizes, expected);
        assert_eq!(mip_sizes(1, 1).collect::<Vec<_>>(), [[1, 1]]);
        assert_eq!(mip_sizes(1, MAX_TEXTURE_SIZE).count(), MAX_LEVELS);
    }

    #[test]
    fn each_levels_pixels_start_after_the_ones_before() {
        let at: Vec<_> = levels(5, 3, 3).collect();
        assert_eq!(at, [([5, 3], 0), ([2, 1], 60), ([1, 1], 68)]);
        // One level of a texture that isn't mipmapped.
        assert_eq!(levels(5, 3, 1).collect::<Vec<_>>(), [([5, 3], 0)]);
    }
}
