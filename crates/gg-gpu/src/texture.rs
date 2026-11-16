//! Textures: images shaders sample, each in a bindless slot showing its own image, or another
//! texture's as a stand-in until it has one.

use std::iter;
use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use crate::device::Retired;
use crate::vma::{Allocation, Memory};
use crate::{COLOR_RANGE, Device, UploadPath, vk_error};

/// The most pixels a side a texture has: what every Vulkan 1.4 device, the required version,
/// takes.
pub const MAX_TEXTURE_SIZE: u32 = 8192;

/// The most mip levels a texture has: a full chain of [`MAX_TEXTURE_SIZE`] pixels a side.
pub(crate) const MAX_LEVELS: usize = MAX_TEXTURE_SIZE.ilog2() as usize + 1;

/// The level sizes of a full mip chain of a `width` by `height` image, largest first: each half
/// the one before, rounded down but at least a pixel a side, down to a pixel. A mipmapped texture
/// has these levels.
pub fn mip_sizes(width: u32, height: u32) -> impl Iterator<Item = [u32; 2]> {
    iter::successors(Some([width, height]), |&[width, height]| {
        (width > 1 || height > 1).then(|| [(width / 2).max(1), (height / 2).max(1)])
    })
}

/// The sizes of the first `count` levels of a `width` by `height` image's mip chain, and where
/// each level's pixels, stored in `format`, start among all of them, packed in order.
fn levels(
    format: TextureFormat,
    width: u32,
    height: u32,
    count: u32,
) -> impl Iterator<Item = ([u32; 2], u64)> {
    let mut offset = 0;
    mip_sizes(width, height)
        .take(count as usize)
        .map(move |size| {
            let at = offset;
            offset += format.level_bytes(size);
            (size, at)
        })
}

/// How a texture's pixels are stored, and how they read in shaders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureFormat {
    /// 8-bit RGBA channels, read as they are, scaled to 0 to 1: for data, and for color a shader
    /// decodes itself.
    Rgba8Unorm,
    /// 8-bit RGBA channels, read as color decoded from sRGB to linear before filtering, alpha as
    /// it is: for color as images store it.
    Rgba8Srgb,
    /// Color compressed as BC7, in blocks of 4 by 4 pixels of 16 bytes each, sRGB-encoded as
    /// [`TextureFormat::Rgba8Srgb`]'s are: read decoded to linear before filtering, alpha as it
    /// is. For color as games' cooked textures store it. A side that isn't a multiple of 4, as a
    /// mip level's may not be, still ends in a whole block.
    Bc7Srgb,
    /// Data compressed as BC7, as [`TextureFormat::Bc7Srgb`]'s blocks are, read as
    /// [`TextureFormat::Rgba8Unorm`]'s: as cooked materials' data maps store it.
    Bc7Unorm,
    /// Two channels of data compressed as BC5, in blocks of 4 by 4 pixels of 16 bytes each, 8 per
    /// channel, read as red and green from 0 to 1, blue 0 and alpha 1: as cooked normal maps store
    /// their X and Y.
    Bc5Unorm,
    /// 16-bit floats in linear RGBA, 8 bytes a pixel, read as they are: for high-dynamic-range
    /// colors, as a sky's.
    Rgba16Float,
    /// High-dynamic-range colors compressed as BC6H, unsigned, in blocks of 4 by 4 pixels of 16
    /// bytes each, read as [`TextureFormat::Rgba16Float`]'s, alpha 1: as cooked skies store them.
    Bc6hUfloat,
}

impl TextureFormat {
    /// Every format.
    pub const ALL: [Self; 7] = [
        Self::Rgba8Unorm,
        Self::Rgba8Srgb,
        Self::Bc7Srgb,
        Self::Bc7Unorm,
        Self::Bc5Unorm,
        Self::Rgba16Float,
        Self::Bc6hUfloat,
    ];

    pub(crate) fn raw(self) -> vk::Format {
        match self {
            Self::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
            Self::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
            Self::Bc7Srgb => vk::Format::BC7_SRGB_BLOCK,
            Self::Bc7Unorm => vk::Format::BC7_UNORM_BLOCK,
            Self::Bc5Unorm => vk::Format::BC5_UNORM_BLOCK,
            Self::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
            Self::Bc6hUfloat => vk::Format::BC6H_UFLOAT_BLOCK,
        }
    }

    /// Whether it's compressed, in blocks of 4 by 4 pixels, which copies move whole: so it's
    /// streamed, never written a pixel at a time.
    pub fn compressed(self) -> bool {
        self.block_size() != [1, 1]
    }

    /// How many bytes fill a texture of `size` pixels in this format, as [`Frame::stream_texture`]
    /// takes them: levels packed largest first, the first alone unless `mipmapped`, else every
    /// level [`mip_sizes`] gives. A level of RGBA8 has 4 bytes a pixel, of 16-bit floats 8. A
    /// level of BC7, BC5 or BC6H has 16 bytes per block of 4 by 4 pixels, its sides rounded up to
    /// whole blocks, so a one-pixel level has a block. A level's bytes are a multiple of a pixel's
    /// or block's, so each level starts as aligned as the first, as copies need.
    ///
    /// [`Frame::stream_texture`]: crate::Frame::stream_texture
    pub fn bytes(self, size: [u32; 2], mipmapped: bool) -> u64 {
        let levels = if mipmapped { usize::MAX } else { 1 };
        (mip_sizes(size[0], size[1]).take(levels))
            .map(|level| self.level_bytes(level))
            .sum()
    }

    /// How many bytes fill one mip level of `size` pixels.
    fn level_bytes(self, [width, height]: [u32; 2]) -> u64 {
        let [bw, bh] = self.block_size();
        u64::from(width.div_ceil(bw)) * u64::from(height.div_ceil(bh)) * self.block_bytes()
    }

    /// The pixels across and down of the texel block copies move whole: 1 by 1 for RGBA8 and
    /// 16-bit floats, 4 by 4 for BC7, BC5 and BC6H.
    fn block_size(self) -> [u32; 2] {
        match self {
            Self::Rgba8Unorm | Self::Rgba8Srgb | Self::Rgba16Float => [1, 1],
            Self::Bc7Srgb | Self::Bc7Unorm | Self::Bc5Unorm | Self::Bc6hUfloat => [4, 4],
        }
    }

    /// The bytes of a texel block: each level's and each copy's pixels start a multiple of it
    /// after the first's.
    fn block_bytes(self) -> u64 {
        match self {
            Self::Rgba8Unorm | Self::Rgba8Srgb => 4,
            Self::Rgba16Float => 8,
            Self::Bc7Srgb | Self::Bc7Unorm | Self::Bc5Unorm | Self::Bc6hUfloat => 16,
        }
    }
}

/// An image shaders sample, in one of the [`TextureFormat`]s, through a bindless slot.
///
/// - Made with [`Texture::new`], it has an image of its own at once, one mip level or a full
///   chain, whose pixels are undefined until a frame writes them
///   ([`Frame::write_texture`](crate::Frame::write_texture)): the UI's textures.
/// - Made with [`Texture::placed`], its slot shows another texture's image, a stand-in, until an
///   image streamed into it a part at a time ([`Texture::refill`], then
///   [`Frame::stream_texture`](crate::Frame::stream_texture)) is shown in its place once its
///   last part is there ([`Frame::finish_texture`](crate::Frame::finish_texture)). Another
///   refill streams another image, shown when it's there; the one before shows until then. The
///   slot can show a stand-in again ([`Frame::hide_texture`](crate::Frame::hide_texture)).
///
/// Dropping it retires its images and slot until the GPU is done with them; an image other
/// textures' slots show as their stand-in retires once none does.
pub struct Texture {
    device: Arc<Device>,
    index: u32,
    /// Its own image, which its slot shows: none while the slot shows a stand-in.
    shown: Option<Image>,
    /// The stand-in's image its slot shows while it has none of its own, held so it lives while
    /// the slot shows it, however long the stand-in itself does.
    stood_in: Option<Arc<Owned>>,
    /// The image being streamed into it, shown once the last part is copied.
    pending: Option<Pending>,
    /// Its debug name, which its errors name it by.
    name: String,
}

/// A texture's image in GPU memory, with its view.
pub(crate) struct Image {
    pub(crate) raw: vk::Image,
    pub(crate) view: vk::ImageView,
    /// The image, view and memory, which slots showing it as a stand-in hold too.
    owned: Arc<Owned>,
    pub(crate) extent: vk::Extent2D,
    pub(crate) levels: u32,
    pub(crate) format: TextureFormat,
    /// Whether a frame has filled or written it, which leaves it in the shader-read-only layout.
    pub(crate) written: bool,
}

/// A texture's image and view in GPU memory, held by its texture and by the textures whose slots
/// show it as their stand-in. It retires, until the GPU is done with the frames and copies that
/// may use it, once none holds it. A slot that stops showing it has the new view in the recording
/// frame's set at once and in each other set as its next frame begins, which retiring by frames
/// covers, as for a texture's own image.
pub(crate) struct Owned {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    /// Taken as it retires.
    allocation: Option<Allocation>,
}

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            (self.device).retire(Retired::Image(self.raw, self.view, allocation));
        }
    }
}

/// An image being streamed into a texture.
pub(crate) struct Pending {
    pub(crate) image: Image,
    /// How many bytes of its pixels have been copied, from the first.
    pub(crate) copied: u64,
    /// The submission of transfer-queue copies that hands it over to the graphics queue once its
    /// last part is recorded; 0 before.
    pub(crate) released: u64,
}

impl Texture {
    /// What every texture is made for: shaders sample it, and copies write it.
    pub(crate) const USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
        vk::ImageUsageFlags::SAMPLED.as_raw() | vk::ImageUsageFlags::TRANSFER_DST.as_raw(),
    );

    /// A texture `name` of `width` by `height` pixels, with the full [`mip_sizes`] chain if
    /// `mipmapped`, else one level, in a free unplaced bindless slot. It has an image of its own
    /// at once, whose pixels are undefined until a frame writes them.
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
        // The slot first: a refused one leaves no image behind.
        let bindless = device.bindless();
        let index = bindless.reserve(None, name)?;
        let image = match Image::new(device, width, height, format, mipmapped, name) {
            Ok(image) => image,
            Err(error) => {
                bindless.remove(index);
                return Err(error);
            }
        };
        // SAFETY: the view is of a live sampled image, which is in the shader-read-only layout
        // before any shader can sample it: frames write it first. No frame uses the slot, which
        // was free when it was reserved.
        unsafe { bindless.write(device.raw(), image.view, index) };
        Ok(Self {
            device: device.clone(),
            index,
            shown: Some(image),
            stood_in: None,
            pending: None,
            name: name.to_owned(),
        })
    }

    /// A texture `name` in placed slot `slot`, one of the first
    /// [`PLACED_TEXTURES`](crate::PLACED_TEXTURES), so its index is known before it's made. It
    /// has no image of its own: its slot shows the image `stand_in` shows, as a [`Texture::new`]
    /// texture does, until an image streamed into it is shown. The texture holds that image while
    /// its slot shows it, so the stand-in may be dropped, refilled or hidden meanwhile: the image
    /// retires once neither it nor a slot showing it holds it.
    ///
    /// # Panics
    ///
    /// If `slot` isn't placed, or is in use: by a texture, or by one dropped while the GPU may
    /// still sample it; or if `stand_in` has no image of its own, or another device made it.
    pub fn placed(device: &Arc<Device>, slot: u32, stand_in: &Texture, name: &str) -> Result<Self> {
        device.check_made("the stand-in", stand_in.device());
        let stood_in = stand_in.image_shown("a stand-in");
        let bindless = device.bindless();
        let index = bindless.reserve(Some(slot), name)?;
        // SAFETY: the view is of a live sampled image, which frames write before any shader
        // samples it, as for the texture it's of, and which the texture holds while its slot shows
        // it. No frame uses the slot, which was free when it was reserved.
        unsafe { bindless.write(device.raw(), stood_in.view, index) };
        Ok(Self {
            device: device.clone(),
            index,
            shown: None,
            stood_in: Some(stood_in.owned.clone()),
            pending: None,
            name: name.to_owned(),
        })
    }

    /// Makes an image in `format` for the texture to stream pixels into, `width` by `height`, with
    /// the full [`mip_sizes`] chain if `mipmapped`, else one level. Frames then copy its pixels a
    /// part at a time ([`Frame::stream_texture`](crate::Frame::stream_texture)) and, once the last
    /// is there, show it in the texture's slot
    /// ([`Frame::finish_texture`](crate::Frame::finish_texture)), replacing what it showed. An
    /// earlier image not yet shown retires, with whatever was copied into it. If it fails, the
    /// texture is as it was.
    ///
    /// # Panics
    ///
    /// If a side is 0, or more than [`MAX_TEXTURE_SIZE`].
    pub fn refill(
        &mut self,
        width: u32,
        height: u32,
        format: TextureFormat,
        mipmapped: bool,
    ) -> Result<()> {
        let image = Image::new(&self.device, width, height, format, mipmapped, &self.name)?;
        let pending = Pending {
            image,
            copied: 0,
            released: 0,
        };
        if let Some(old) = self.pending.replace(pending) {
            old.image.retire();
        }
        Ok(())
    }

    /// Drops the image being streamed into the texture, if any: it retires, with whatever was
    /// copied into it, once the frames and copies that may use it have finished. The slot keeps
    /// showing what it shows. For an image given up on before it's shown, as a replaced one is.
    pub fn discard(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.image.retire();
        }
    }

    /// How many bytes frames have copied into the image being streamed, from the first; 0 without
    /// one.
    pub fn streamed(&self) -> u64 {
        self.pending.as_ref().map_or(0, |pending| pending.copied)
    }

    /// Where shaders find the texture in the bindless sets' texture array.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The width and height in pixels of the first, largest level of the image it shows; `[0, 0]`
    /// while its slot shows a stand-in.
    pub fn size(&self) -> [u32; 2] {
        self.shown.as_ref().map_or([0, 0], Image::size)
    }

    /// The handle of the device that made it.
    pub(crate) fn device(&self) -> vk::Device {
        self.device.handle()
    }

    /// The image it shows, if it has one of its own.
    pub(crate) fn shown(&self) -> Option<&Image> {
        self.shown.as_ref()
    }

    /// The image it shows.
    ///
    /// # Panics
    ///
    /// If its slot shows a stand-in; `what` names the image in the message.
    pub(crate) fn image_shown(&self, what: &str) -> &Image {
        self.shown.as_ref().unwrap_or_else(|| {
            panic!(
                "{what} is a texture with an image of its own, but texture {:?}'s slot shows a \
                 stand-in: one `Texture::new` made, or one streamed into it and shown",
                self.name
            )
        })
    }

    /// As [`Texture::image_shown`], to write.
    pub(crate) fn image_shown_mut(&mut self, what: &str) -> &mut Image {
        self.image_shown(what);
        self.shown.as_mut().expect("just checked")
    }

    /// The image being streamed into it, if one is.
    pub(crate) fn pending_mut(&mut self) -> Option<&mut Pending> {
        self.pending.as_mut()
    }

    /// Shows `image` in the texture's own state, not the slot (the frame rewrites the slot, and
    /// comes first). Returns the image it showed, which the caller retires, and lets go of the
    /// stand-in's, which retires once nothing else holds it.
    pub(crate) fn show(&mut self, image: Image) -> Option<Image> {
        self.stood_in = None;
        self.shown.replace(image)
    }

    /// Shows `stand_in`'s image in the texture's own state, not the slot (the frame rewrites the
    /// slot, and comes first), holding it while the slot shows it and letting go of any earlier
    /// stand-in's. The caller takes and retires the image it showed of its own.
    pub(crate) fn stand_in(&mut self, stand_in: &Image) {
        self.stood_in = Some(stand_in.owned.clone());
    }

    /// Takes the image being streamed into it, if one is: to show, or to retire.
    pub(crate) fn take_pending(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    /// Takes the image it shows, if it has one of its own, to retire: its slot shows a stand-in.
    pub(crate) fn take_shown(&mut self) -> Option<Image> {
        self.shown.take()
    }
}

impl Image {
    /// An image of `width` by `height` pixels in `format` for a texture on `device`, with the full
    /// [`mip_sizes`] chain if `mipmapped`, else one level, in GPU memory; it and its view are
    /// named `name`.
    ///
    /// # Panics
    ///
    /// If a side is 0, or more than [`MAX_TEXTURE_SIZE`].
    fn new(
        device: &Arc<Device>,
        width: u32,
        height: u32,
        format: TextureFormat,
        mipmapped: bool,
        name: &str,
    ) -> Result<Self> {
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
        // Host copies need it; only devices whose drivers say it's free upload that way, unless
        // a test asks.
        let usage = if device.upload_path() == UploadPath::HostImageCopy {
            Texture::USAGE | vk::ImageUsageFlags::HOST_TRANSFER_EXT
        } else {
            Texture::USAGE
        };
        let extent = vk::Extent2D { width, height };
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format.raw())
            .extent(extent.into())
            .mip_levels(levels)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage);
        let (raw, allocation) = device.allocator().create_image(&info, Memory::Gpu, name)?;
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
        Ok(Self {
            raw,
            view,
            owned: Arc::new(Owned {
                device: device.clone(),
                raw,
                view,
                allocation: Some(allocation),
            }),
            extent,
            levels,
            format,
            written: false,
        })
    }

    /// The width and height in pixels, of its first and largest level.
    pub(crate) fn size(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
    }

    /// Its mip levels' sizes, largest first, and where each level's pixels start among all of
    /// them, packed in order. A level's bytes are a multiple of its format's pixel's or block's,
    /// so each starts as aligned as the first.
    pub(crate) fn levels(&self) -> impl Iterator<Item = ([u32; 2], u64)> {
        levels(
            self.format,
            self.extent.width,
            self.extent.height,
            self.levels,
        )
    }

    /// How many bytes of its format fill all its levels.
    pub(crate) fn bytes(&self) -> u64 {
        self.format.bytes(self.size(), self.levels > 1)
    }

    /// Retires the image until the GPU is done with the frames and copies that may use it, once no
    /// slot showing it as a stand-in holds it.
    pub(crate) fn retire(self) {
        drop(self);
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        // The slot first, retired by the same frames as the images it may show, which go once
        // nothing else holds them.
        self.device.retire(Retired::Slot(self.index));
        if let Some(pending) = self.pending.take() {
            pending.image.retire();
        }
        if let Some(image) = self.shown.take() {
            image.retire();
        }
        self.stood_in = None;
    }
}

/// The part of a texture's pixels one copy takes, as the image regions it fills: whole rows of
/// texel blocks of one level, or every level from one on when what's left of the texture fits a
/// copy, so a mip tail's small levels go together.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Part {
    pub(crate) regions: [Region; MAX_LEVELS],
    /// How many of `regions` there are.
    pub(crate) count: usize,
    /// Where its bytes start among the texture's, and how many: the regions', in order.
    pub(crate) offset: u64,
    pub(crate) bytes: u64,
}

/// A region of a part: rows of one level, from the top texel row `y` down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Region {
    pub(crate) level: u32,
    /// Where its bytes start among the texture's pixels, and how many.
    pub(crate) offset: u64,
    pub(crate) bytes: u64,
    pub(crate) y: u32,
    /// Its texels across (the level's) and down.
    pub(crate) size: [u32; 2],
}

impl Part {
    pub(crate) fn regions(&self) -> &[Region] {
        &self.regions[..self.count]
    }
}

/// The next part of a texture's pixels to copy, in `format`, in the levels `levels` gives as
/// [`Image::levels`] does, after the first `copied` bytes: as many bytes as fit `budget`, but at
/// least the smallest whole part, so each copy makes progress. A part is either:
/// - Whole rows of blocks of one level, their count a multiple of `unit`, the copying queue's
///   transfer granularity down, in blocks, unless it reaches the level's bottom: Vulkan lets a
///   copy's extent there be unaligned, and a compressed image's copy end inside a block
///   (VUID-vkCmdCopyBufferToImage-imageOffset-07738 and -dstImage-00207 and -00208).
/// - From the start of a level, every level left, when they all fit, so a mip tail of small
///   levels goes in one copy.
///
/// A part's bytes start where a level's or row's do, a multiple of the texel block's bytes after
/// the first (VUID-vkCmdCopyBufferToImage-dstImage-07975).
///
/// # Panics
///
/// If every byte is copied already.
pub(crate) fn next_part(
    format: TextureFormat,
    levels: impl Iterator<Item = ([u32; 2], u64)>,
    unit: u32,
    copied: u64,
    budget: u64,
) -> Part {
    let [bw, bh] = format.block_size();
    let unit = unit.max(1);
    let mut all = [([0; 2], 0); MAX_LEVELS];
    let mut count = 0;
    for (slot, level) in all.iter_mut().zip(levels) {
        *slot = level;
        count += 1;
    }
    let all = &all[..count];
    let total = all
        .last()
        .map_or(0, |&(size, at)| at + format.level_bytes(size));
    assert!(
        copied < total,
        "every part of the texture is copied already"
    );
    let level = (all.iter())
        .rposition(|&(_, at)| at <= copied)
        .expect("a level holds the next byte");
    let ([width, height], at) = all[level];
    let row_bytes = u64::from(width.div_ceil(bw)) * format.block_bytes();
    let rows = height.div_ceil(bh);
    debug_assert!(
        (copied - at).is_multiple_of(row_bytes),
        "a part ends at a row"
    );
    let row = ((copied - at) / row_bytes) as u32;
    let left = rows - row;
    let mut part = Part {
        regions: [Region::default(); MAX_LEVELS],
        count: 0,
        offset: copied,
        bytes: 0,
    };
    let smallest = u64::from(unit.min(left)) * row_bytes;
    if row == 0 && total - at <= budget.max(smallest) {
        for (i, &(size, at)) in all.iter().enumerate().skip(level) {
            part.regions[i - level] = Region {
                level: i as u32,
                offset: at,
                bytes: format.level_bytes(size),
                y: 0,
                size,
            };
        }
        part.count = count - level;
        part.bytes = total - at;
        return part;
    }
    let want = (budget / row_bytes).min(u64::from(u32::MAX)) as u32;
    let mut take = want / unit * unit;
    if take == 0 {
        take = unit;
    }
    let take = take.min(left);
    let y = row * bh;
    part.regions[0] = Region {
        level: level as u32,
        offset: at + u64::from(row) * row_bytes,
        bytes: u64::from(take) * row_bytes,
        y,
        size: [width, (take * bh).min(height - y)],
    };
    part.count = 1;
    part.bytes = part.regions[0].bytes;
    part
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
        let at: Vec<_> = levels(TextureFormat::Rgba8Srgb, 5, 3, 3).collect();
        assert_eq!(at, [([5, 3], 0), ([2, 1], 60), ([1, 1], 68)]);
        // One level of a texture that isn't mipmapped.
        let first = levels(TextureFormat::Rgba8Unorm, 5, 3, 1);
        assert_eq!(first.collect::<Vec<_>>(), [([5, 3], 0)]);
    }

    #[test]
    fn bc7_levels_are_whole_blocks_whatever_their_sides() {
        let bc7 = TextureFormat::Bc7Srgb;
        // 13x5 is 4 by 2 blocks, 6x2 is 2 by 1, and 3x1 and 1x1 are a block each.
        let at: Vec<_> = levels(bc7, 13, 5, 4).collect();
        assert_eq!(
            at,
            [([13, 5], 0), ([6, 2], 128), ([3, 1], 160), ([1, 1], 176)]
        );
        assert_eq!(bc7.bytes([13, 5], true), 192);
        assert_eq!(bc7.bytes([13, 5], false), 128);
        // A pixel, and a block's sides and one past them.
        assert_eq!(levels(bc7, 1, 1, 1).collect::<Vec<_>>(), [([1, 1], 0)]);
        assert_eq!(bc7.bytes([1, 1], false), 16);
        assert_eq!(bc7.bytes([1, 1], true), 16);
        assert_eq!(bc7.bytes([4, 4], false), 16);
        assert_eq!(bc7.bytes([5, 4], false), 32);
        assert_eq!(bc7.bytes([5, 5], false), 64);
        assert_eq!(bc7.bytes([8192, 8192], false), 8192 * 8192);
        // BC6H's blocks are BC7's size; a pixel of 16-bit floats is twice RGBA8's.
        let bc6h = TextureFormat::Bc6hUfloat;
        assert_eq!(bc6h.bytes([13, 5], true), bc7.bytes([13, 5], true));
        assert_eq!(TextureFormat::Rgba16Float.bytes([3, 2], true), (6 + 1) * 8);
    }

    #[test]
    fn bytes_fill_the_last_level_and_start_every_level_aligned() {
        for format in TextureFormat::ALL {
            // Copies need each level aligned to the format's texel block size.
            let alignment = format.block_bytes();
            for size in [[1, 1], [2, 2], [3, 7], [4, 4], [13, 5], [64, 1], [211, 5]] {
                let count = mip_sizes(size[0], size[1]).count() as u32;
                let all: Vec<_> = levels(format, size[0], size[1], count).collect();
                let ([width, height], at) = *all.last().expect("a level");
                let last = format.bytes([width, height], false);
                assert_eq!(at + last, format.bytes(size, true), "{format:?} {size:?}");
                // The second level's pixels start after the first alone.
                let first = all.get(1).map_or(at + last, |&(_, second)| second);
                assert_eq!(first, format.bytes(size, false), "{format:?} {size:?}");
                assert!(all.iter().all(|&(_, at)| at % alignment == 0), "{all:?}");
            }
        }
    }

    /// The parts copying a `size` texture in `format`, one per call, each within `budget` but for
    /// the smallest whole part, rows in multiples of `unit`: their regions and bytes.
    fn parts(
        format: TextureFormat,
        size: [u32; 2],
        mipmapped: bool,
        unit: u32,
        budget: u64,
    ) -> Vec<(Vec<Region>, u64)> {
        let count = if mipmapped {
            mip_sizes(size[0], size[1]).count() as u32
        } else {
            1
        };
        let total = format.bytes(size, mipmapped);
        let mut copied = 0;
        let mut parts = Vec::new();
        while copied < total {
            let levels = levels(format, size[0], size[1], count);
            let part = next_part(format, levels, unit, copied, budget);
            assert_eq!(part.offset, copied, "a part follows the one before");
            let bytes: u64 = part.regions().iter().map(|region| region.bytes).sum();
            assert_eq!(bytes, part.bytes, "a part's bytes are its regions'");
            parts.push((part.regions().to_vec(), part.bytes));
            copied += part.bytes;
        }
        assert_eq!(copied, total, "the parts cover the texture exactly");
        parts
    }

    /// Rows of 256 bytes, 16 to a 4 KiB budget, in four parts; the second level, 4 KiB, whole;
    /// then the tail of five levels, 1,364 bytes, at once.
    #[test]
    fn parts_are_whole_rows_within_the_budget_and_a_mip_tail_at_once() {
        let rgba8 = TextureFormat::Rgba8Srgb;
        let parts = parts(rgba8, [64, 64], true, 1, 4096);
        let rows = |y, height| Region {
            level: 0,
            offset: u64::from(y) * 256,
            bytes: u64::from(height) * 256,
            y,
            size: [64, height],
        };
        assert_eq!(parts[0], (vec![rows(0, 16)], 4096));
        assert_eq!(parts[3], (vec![rows(48, 16)], 4096));
        assert_eq!(parts[4].0[0].level, 1);
        assert_eq!(parts[4].1, 4096);
        let (tail, bytes) = &parts[5];
        assert_eq!(bytes, &1364);
        let levels: Vec<_> = tail.iter().map(|region| region.level).collect();
        assert_eq!(levels, [2, 3, 4, 5, 6]);
        assert_eq!(tail[0].offset, 4096 * 5);
        assert_eq!(
            tail[4],
            Region {
                level: 6,
                offset: 4096 * 5 + 1024 + 256 + 64 + 16,
                bytes: 4,
                y: 0,
                size: [1, 1],
            }
        );
        assert_eq!(parts.len(), 6);
        // A budget that holds the whole texture takes it at once.
        let whole = self::parts(rgba8, [64, 64], true, 1, 1 << 20);
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].0.len(), 7);
    }

    /// With a granularity of 8 rows, a part is 8 rows though the budget holds 3, and 16 where it
    /// holds 19; the rows left at a level's bottom go however many they are.
    #[test]
    fn parts_rows_are_multiples_of_the_granularity_unless_they_reach_the_bottom() {
        let rgba8 = TextureFormat::Rgba8Unorm;
        let parts = parts(rgba8, [64, 36], false, 8, 3 * 256);
        let heights: Vec<_> = parts
            .iter()
            .map(|(regions, _)| regions[0].size[1])
            .collect();
        assert_eq!(heights, [8, 8, 8, 8, 4]);
        let ys: Vec<_> = parts.iter().map(|(regions, _)| regions[0].y).collect();
        assert_eq!(ys, [0, 8, 16, 24, 32]);
        let parts = self::parts(rgba8, [64, 36], false, 8, 19 * 256);
        let heights: Vec<_> = parts
            .iter()
            .map(|(regions, _)| regions[0].size[1])
            .collect();
        assert_eq!(heights, [16, 16, 4]);
    }

    /// BC7's rows are rows of blocks, 4 texels down, a part's extent reaching the level's bottom
    /// inside a block. A 13x5 chain's first level, two block rows of 64 bytes, goes in one part
    /// under a granularity of 2 blocks though the budget holds one row; its tail goes together.
    #[test]
    fn bc7_parts_are_rows_of_blocks() {
        let bc7 = TextureFormat::Bc7Srgb;
        let parts = parts(bc7, [13, 5], true, 2, 64);
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts[0].0,
            [Region {
                level: 0,
                offset: 0,
                bytes: 128,
                y: 0,
                size: [13, 5],
            }]
        );
        let levels: Vec<_> = parts[1].0.iter().map(|region| region.level).collect();
        assert_eq!((levels, parts[1].1), (vec![1, 2, 3], 64));
        // A row of blocks at a time, the last reaching the bottom 1 texel down.
        let parts = self::parts(bc7, [8, 9], false, 1, 32);
        let rows: Vec<_> = parts
            .iter()
            .map(|(regions, _)| (regions[0].y, regions[0].size[1], regions[0].offset))
            .collect();
        assert_eq!(rows, [(0, 4, 0), (4, 4, 32), (8, 1, 64)]);
        // Every part's bytes start a multiple of a block's bytes in.
        for format in TextureFormat::ALL {
            for (regions, _) in self::parts(format, [37, 21], true, 3, 500) {
                for region in regions {
                    assert!(
                        region.offset.is_multiple_of(format.block_bytes()),
                        "{region:?}"
                    );
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "copied already")]
    fn a_texture_copied_whole_has_no_next_part() {
        let rgba8 = TextureFormat::Rgba8Srgb;
        let _ = next_part(rgba8, levels(rgba8, 2, 2, 1), 1, 16, 16);
    }
}
