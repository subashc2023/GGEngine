//! Storage images: images compute passes write a mip level at a time through storage slots in the
//! bindless sets, and shaders then sample whole, as a 2D image or a cube, through a texture slot.
//! Frames take them as targets, and they keep the layout frames leave them in.

use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use crate::device::Retired;
use crate::texture::mip_sizes;
use crate::vma::{Allocation, Memory};
use crate::{Device, vk_failed};

/// An image that compute passes write and shaders sample, in GPU memory.
///
/// [`StorageImage::FORMAT`], one layer for a 2D image or six for a cube, with mip levels. Its
/// contents are undefined until a pass writes them. Dropping it retires it and its slots until the
/// GPU is done with it.
pub struct StorageImage {
    device: Arc<Device>,
    raw: vk::Image,
    /// What shaders sample: every level, as a 2D image or a cube.
    view: vk::ImageView,
    /// Each level's view of every layer, which compute shaders write and read, and its storage
    /// slot.
    levels: Vec<(vk::ImageView, u32)>,
    /// Taken when the image is dropped.
    allocation: Option<Allocation>,
    size: [u32; 2],
    cube: bool,
    /// The layout the work submitted so far leaves the image in.
    layout: vk::ImageLayout,
    /// Its texture slot in the bindless sets, past the placed ones.
    index: u32,
    name: String,
}

impl StorageImage {
    /// The format of every storage image: 16-bit floats in linear RGBA, which Vulkan requires
    /// every device to store to from shaders, and to sample and filter.
    pub const FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

    /// An image of `size` pixels with the first `levels` of the chain [`mip_sizes`] gives: a cube
    /// of six faces that size if `cube`, else a 2D image. It takes a free texture slot in the
    /// bindless sets past the placed ones, and a storage slot per level. `name` is its debug name.
    ///
    /// # Panics
    ///
    /// If a side is 0 or more than [`MAX_TEXTURE_SIZE`](crate::MAX_TEXTURE_SIZE), `levels` is 0 or
    /// more than its chain has, or a cube's sides differ.
    pub fn new(
        device: &Arc<Device>,
        size: [u32; 2],
        levels: u32,
        cube: bool,
        name: &str,
    ) -> Result<Self> {
        let [width, height] = size;
        let sides = 1..=crate::MAX_TEXTURE_SIZE;
        assert!(
            sides.contains(&width) && sides.contains(&height),
            "storage image {name:?} is {width}x{height}, but images are 1 to {} pixels a side",
            crate::MAX_TEXTURE_SIZE
        );
        let chain = mip_sizes(width, height).count() as u32;
        assert!(
            (1..=chain).contains(&levels),
            "storage image {name:?} has {levels} levels, but its chain has 1 to {chain}"
        );
        assert!(
            !cube || width == height,
            "storage image {name:?} is a cube, whose faces are square, but it's {width}x{height}"
        );
        let layers = if cube { 6 } else { 1 };
        let info = vk::ImageCreateInfo::default()
            .flags(if cube {
                vk::ImageCreateFlags::CUBE_COMPATIBLE
            } else {
                vk::ImageCreateFlags::empty()
            })
            .image_type(vk::ImageType::TYPE_2D)
            .format(Self::FORMAT)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(levels)
            .array_layers(layers)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE);
        let (raw, allocation) = device.allocator().create_image(&info, Memory::Gpu, name)?;
        // SAFETY: the image is live.
        unsafe { device.set_name(raw, name) };
        let (shape, all) = if cube {
            (vk::ImageViewType::CUBE, range(6))
        } else {
            (vk::ImageViewType::TYPE_2D, range(1))
        };
        let view = match make_view(device, raw, shape, all, name) {
            Ok(view) => view,
            Err(error) => {
                device.retire(Retired::Image(raw, vk::ImageView::null(), allocation));
                return Err(error);
            }
        };
        // SAFETY: the view is of a live sampled image of the device, which shaders sample only
        // once a frame has left it in the shader-read-only layout, as `sampled` says.
        let index = match unsafe { device.bindless().add(device.raw(), view, None, name) } {
            Ok(index) => index,
            Err(error) => {
                device.retire(Retired::Image(raw, view, allocation));
                return Err(error);
            }
        };
        // From here on, dropping `image` retires what's made so far.
        let mut image = Self {
            device: device.clone(),
            raw,
            view,
            levels: Vec::new(),
            allocation: Some(allocation),
            size,
            cube,
            layout: vk::ImageLayout::UNDEFINED,
            index,
            name: name.to_owned(),
        };
        for level in 0..levels {
            let range = vk::ImageSubresourceRange {
                base_mip_level: level,
                level_count: 1,
                ..all
            };
            let name = format!("{name} level {level}");
            let level_view =
                make_view(device, raw, vk::ImageViewType::TYPE_2D_ARRAY, range, &name)?;
            // SAFETY: the view is of one level of a live storage image of the device, which shaders
            // write only in a pass that declares it, which puts it in the general layout.
            let slot = unsafe {
                device
                    .bindless()
                    .add_storage(device.raw(), level_view, &name)
            };
            match slot {
                Ok(slot) => image.levels.push((level_view, slot)),
                Err(error) => {
                    // SAFETY: nothing has used the view, which no slot holds.
                    unsafe { device.raw().destroy_image_view(level_view, None) };
                    return Err(error);
                }
            }
        }
        Ok(image)
    }

    /// Its first level's width and height in pixels; a face's for a cube.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// How many mip levels it has.
    pub fn levels(&self) -> u32 {
        self.levels.len() as u32
    }

    /// Its slot in the bindless sets, for fragment shaders in any pass to sample as a `Texture2D`
    /// or, for a cube, a `TextureCube`. `Some` once the frames submitted have left it in the
    /// shader-read-only layout, as a last use that sampled it does; `None` until then, as while
    /// its last use wrote it. A pass that declares it gets its slot from the pass.
    pub fn sampled(&self) -> Option<u32> {
        (self.layout == vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL).then_some(self.index)
    }

    /// The image, its sampled view and its size, for recording commands that use it.
    pub(crate) fn parts(&self) -> (vk::Image, vk::ImageView, vk::Extent2D) {
        let [width, height] = self.size;
        (self.raw, self.view, vk::Extent2D { width, height })
    }

    /// Every level of every layer: what its barriers cover.
    pub(crate) fn range(&self) -> vk::ImageSubresourceRange {
        range(if self.cube { 6 } else { 1 })
    }

    /// Its texture slot: shaders may sample it only in a pass that samples it, or once a frame has
    /// left it in the shader-read-only layout.
    pub(crate) fn index(&self) -> u32 {
        self.index
    }

    /// Level `level`'s storage slot.
    ///
    /// # Panics
    ///
    /// If it hasn't that level.
    pub(crate) fn storage(&self, level: u32) -> u32 {
        let Some(&(_, slot)) = self.levels.get(level as usize) else {
            panic!(
                "storage image {:?} has {} levels, not a level {level}",
                self.name,
                self.levels.len()
            );
        };
        slot
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
}

impl Drop for StorageImage {
    fn drop(&mut self) {
        // The levels' views before the image they're of.
        for &(view, slot) in &self.levels {
            self.device.retire(Retired::StorageView(view, slot));
        }
        if let Some(allocation) = self.allocation.take() {
            self.device.retire(Retired::Texture(
                self.raw, self.view, allocation, self.index,
            ));
        }
    }
}

/// Every level of `layers` layers of a color image.
fn range(layers: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: vk::REMAINING_MIP_LEVELS,
        base_array_layer: 0,
        layer_count: layers,
    }
}

/// A view `name` of `range` of `image`, in [`StorageImage::FORMAT`], shaped as `shape`.
fn make_view(
    device: &Device,
    image: vk::Image,
    shape: vk::ImageViewType,
    range: vk::ImageSubresourceRange,
    name: &str,
) -> Result<vk::ImageView> {
    let info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(shape)
        .format(StorageImage::FORMAT)
        .subresource_range(range);
    // SAFETY: the image is live and bound to memory.
    let view = unsafe { device.raw().create_image_view(&info, None) }.map_err(vk_failed(
        "vkCreateImageView",
        format_args!("storage image view {name:?}"),
    ))?;
    // SAFETY: the view is live.
    unsafe { device.set_name(view, name) };
    Ok(view)
}
