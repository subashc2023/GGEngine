//! Uploads. A frame fills the meshes and textures it takes by the device's upload path: through a
//! transfer queue, it records their copies apart and submits them first, and its commands wait for
//! them.
//!
//! The SAFETY comments here lean on what the parent module keeps: [`Frames::begin`] begins a slot's
//! commands, and resets its arena, only once the GPU has finished the slot's last frame, and they
//! record until the frame is submitted; `Frame::outside_pass` checks that no pass is open, which
//! only a pass's drop closes; and `Frame::send` empties staging, whose buffers then retire with the
//! frame, submits the copies before the frame's commands, which wait for them, and waits for the
//! copies itself if the commands fail to submit.
//!
//! [`Frames::begin`]: crate::Frames::begin

use std::ffi::CStr;
use std::marker::PhantomData;
use std::slice;
use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use super::{Frame, begin_recording};
use crate::buffer::Buffer;
use crate::texture::MAX_LEVELS;
use crate::vma::Memory;
use crate::{COLOR_RANGE, Device, GpuBuffer, Texture, UploadPath, copy_region};

/// The stages that read textures: fragment shaders sample them, and vertex shaders may too, as
/// the sprite shader queries their sizes.
const TEXTURE_READERS: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::from_raw(
    vk::PipelineStageFlags2::VERTEX_SHADER.as_raw()
        | vk::PipelineStageFlags2::FRAGMENT_SHADER.as_raw(),
);

/// The stages that read buffers during a frame: shaders, through their addresses, and draws'
/// indices.
const BUFFER_READERS: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::from_raw(
    TEXTURE_READERS.as_raw() | vk::PipelineStageFlags2::INDEX_INPUT.as_raw(),
);

impl<'a> Frame<'a> {
    /// Copies `bytes` into the frame's arena, where the GPU reads them during this frame:
    /// geometry, and data shaders read through its address. The upload borrows the frame's
    /// [`Frames`], so that no later frame draws from it; its address, pushed to a shader in a
    /// later frame, reads whatever has replaced it, or memory freed since, as the arena merges its
    /// buffers into one as it resets, which can fault the device: shaders are trusted with the
    /// addresses they're given, as [`GraphicsDesc::spirv`](crate::GraphicsDesc::spirv) says.
    ///
    /// [`Frames`]: crate::Frames
    pub fn upload(&mut self, bytes: &[u8]) -> Result<Upload<'a>> {
        self.frames.arenas[self.slot].upload(&self.frames.device, bytes)
    }

    /// Writes `pixels` into the `size` pixels of `texture` starting at `origin`: rows of RGBA8,
    /// in the texture's format and tightly packed, top row first. The texture's other pixels
    /// keep what they had, except on its first write, which leaves them undefined. Shaders see
    /// the new pixels from this frame's passes on. The pixels go through the frame's arena and
    /// the graphics queue, which suits small changes the frame uses at once, such as the UI's;
    /// [`Frame::fill_texture`] fills a whole new texture by the device's upload path.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, another device made the texture, it has mip levels,
    /// whose others a write would leave undefined, the region doesn't fit in it, or `pixels`
    /// doesn't fill the region exactly.
    pub fn write_texture(
        &mut self,
        texture: &mut Texture,
        origin: [u32; 2],
        size: [u32; 2],
        pixels: &[u8],
    ) -> Result<()> {
        self.outside_pass();
        self.frames
            .device
            .check_made("the texture written", texture.device());
        assert_eq!(
            texture.level_count(),
            1,
            "a texture with mip levels is filled, not written"
        );
        let [width, height] = texture.size();
        assert!(
            origin[0].checked_add(size[0]).is_some_and(|x| x <= width)
                && origin[1].checked_add(size[1]).is_some_and(|y| y <= height),
            "a {}x{} region at {origin:?} overflows a {width}x{height} texture",
            size[0],
            size[1],
        );
        assert_eq!(
            pixels.len() as u64,
            u64::from(size[0]) * u64::from(size[1]) * 4,
            "the pixels don't fill a {}x{} region",
            size[0],
            size[1],
        );
        // A copy has at least a pixel.
        if pixels.is_empty() {
            return Ok(());
        }
        let staged = self.upload(pixels)?;
        let region = copy_region(staged.offset, origin, size, 0);
        self.copy_to_texture(texture, staged.buffer, &[region], c"write texture");
        texture.set_written();
        Ok(())
    }

    /// Fills `texture`, which no frame has written, with `pixels`: all of it, each of its mip
    /// levels, largest first, as rows of RGBA8 in the texture's format, tightly packed, top row
    /// first, after the level before. Shaders see them from this frame's passes on, whichever way
    /// the device uploads: through a transfer queue, the frame waits for the copy. What's staged
    /// for a copy is freed once the frame has finished.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, another device made the texture, a frame has written
    /// it, or `pixels` doesn't fill its levels exactly.
    pub fn fill_texture(&mut self, texture: &mut Texture, pixels: &[u8]) -> Result<()> {
        self.outside_pass();
        self.frames
            .device
            .check_made("the texture filled", texture.device());
        let [width, height] = texture.size();
        assert!(
            !texture.written(),
            "a texture is filled once, before any frame writes it"
        );
        assert_eq!(
            pixels.len() as u64,
            texture.bytes(),
            "the pixels don't fill a {width}x{height} texture's {} levels",
            texture.level_count(),
        );
        match self.frames.device.upload_path() {
            UploadPath::HostImageCopy => texture.copy_from_host(pixels)?,
            UploadPath::GraphicsQueue => {
                let (staging, offset) = self.stage(pixels)?;
                let (regions, count) = level_regions(texture, offset);
                self.copy_to_texture(texture, staging, &regions[..count], c"fill texture");
            }
            UploadPath::TransferQueue => {
                let (staging, offset) = self.stage(pixels)?;
                let image = texture.raw();
                // Nothing has used the texture, and the copy replaces what it holds, so it needs
                // no ownership transfer.
                let to_copy = vk::ImageMemoryBarrier2::default()
                    .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(COLOR_RANGE);
                let (regions, count) = level_regions(texture, offset);
                let families = handover_families(&self.frames.device);
                let [release, acquire] = texture_handover(families, image);
                self.hand_over(
                    c"fill texture",
                    // SAFETY: the regions fit the texture's levels, and the staged pixels and the
                    // texture live until the frame has finished, which waits for the copy: staging
                    // and the texture retire until then.
                    |raw, transfers| unsafe {
                        raw.cmd_pipeline_barrier2(
                            transfers,
                            &vk::DependencyInfo::default()
                                .image_memory_barriers(slice::from_ref(&to_copy)),
                        );
                        raw.cmd_copy_buffer_to_image(
                            transfers,
                            staging,
                            image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            &regions[..count],
                        );
                    },
                    [
                        &vk::DependencyInfo::default()
                            .image_memory_barriers(slice::from_ref(&release)),
                        &vk::DependencyInfo::default()
                            .image_memory_barriers(slice::from_ref(&acquire)),
                    ],
                )?;
            }
        }
        texture.set_written();
        Ok(())
    }

    /// Fills `texture` anew with the pixels of another image, `size` pixels across and down, with
    /// the full chain of mip levels that [`mip_sizes`](crate::mip_sizes) gives if `mipmapped` and
    /// otherwise one, packed as [`Frame::fill_texture`] takes them: in place of what it held,
    /// whether or not a frame has filled or written it, in an image made for them, which keeps the
    /// texture's index, so that shaders that sample it by its index see them in each of this
    /// frame's passes, which all begin after it: the new image takes the texture's slot at once,
    /// which a pass recorded before would sample too, as the frame is submitted, before its copy.
    /// Frames before this one may still sample it, so it first waits for the GPU to finish them, a
    /// frame's time at most: it's for an asset reloaded as its file changes, not for every frame.
    /// The old image retires until this frame has finished.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, a pass has begun in the frame, another device made the
    /// texture, a side is 0 or more than [`MAX_TEXTURE_SIZE`](crate::MAX_TEXTURE_SIZE), or
    /// `pixels` doesn't fill the levels exactly.
    pub fn refill_texture(
        &mut self,
        texture: &mut Texture,
        size: [u32; 2],
        mipmapped: bool,
        pixels: &[u8],
    ) -> Result<()> {
        self.outside_pass();
        assert!(
            !self.pass_begun,
            "a texture is refilled before the frame's passes, which would sample its new image \
             before its copy"
        );
        self.frames
            .device
            .check_made("the texture refilled", texture.device());
        let [width, height] = size;
        let levels = if mipmapped { usize::MAX } else { 1 };
        let bytes: u64 = (crate::mip_sizes(width, height).take(levels))
            .map(|[width, height]| u64::from(width) * u64::from(height) * 4)
            .sum();
        // Before anything changes, so that the texture is never left unfilled.
        assert_eq!(
            pixels.len() as u64,
            bytes,
            "the pixels don't fill a {width}x{height} texture's levels"
        );
        let before = self.frames.submitted;
        if before > 0 {
            self.frames.wait(before)?;
            self.frames.device.frames_finished(before);
        }
        // SAFETY: the GPU has finished every frame before this one, and this one, still recording,
        // takes the texture's slot as it's written.
        unsafe { texture.remake(width, height, mipmapped) }?;
        self.fill_texture(texture, pixels)
    }

    /// Fills `buffer`, which no frame has written, with `bytes`: all of it. Shaders and draws see
    /// them from this frame's passes on, as [`Frame::fill_texture`] has it; a device that copies
    /// textures from the host copies buffers on the graphics queue.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, another device made the buffer, a frame has written
    /// it, or `bytes` doesn't fill it exactly.
    pub fn fill_buffer(&mut self, buffer: &mut GpuBuffer, bytes: &[u8]) -> Result<()> {
        self.outside_pass();
        self.frames
            .device
            .check_made("the buffer filled", buffer.device());
        assert!(
            !buffer.written(),
            "a buffer is filled once, before any frame writes it"
        );
        let size = buffer.size();
        assert_eq!(
            bytes.len() as u64,
            size,
            "the bytes don't fill a buffer of {size}"
        );
        let (staging, offset) = self.stage(bytes)?;
        let target = buffer.raw();
        let region = vk::BufferCopy {
            src_offset: offset,
            dst_offset: 0,
            size,
        };
        match self.frames.device.upload_path() {
            UploadPath::TransferQueue => {
                let families = handover_families(&self.frames.device);
                let [release, acquire] = buffer_handover(families, target);
                self.hand_over(
                    c"fill buffer",
                    // SAFETY: the region fits the buffer, and the staged bytes and the buffer live
                    // until the frame has finished, which waits for the copy: staging and the
                    // buffer retire until then.
                    |raw, transfers| unsafe {
                        raw.cmd_copy_buffer(transfers, staging, target, &[region]);
                    },
                    [
                        &vk::DependencyInfo::default()
                            .buffer_memory_barriers(slice::from_ref(&release)),
                        &vk::DependencyInfo::default()
                            .buffer_memory_barriers(slice::from_ref(&acquire)),
                    ],
                )?;
            }
            UploadPath::HostImageCopy | UploadPath::GraphicsQueue => {
                let device = &self.frames.device;
                let commands = self.frames.slots[self.slot].commands;
                let to_read = vk::MemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COPY)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(BUFFER_READERS)
                    .dst_access_mask(
                        vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::INDEX_READ,
                    );
                let raw = device.raw();
                // SAFETY: the commands are recording outside a pass, the region fits the buffer,
                // and the staged bytes and the buffer live until the frame has finished: staging
                // and the buffer retire until then.
                unsafe {
                    device.begin_label(commands, c"fill buffer");
                    raw.cmd_copy_buffer(commands, staging, target, &[region]);
                    raw.cmd_pipeline_barrier2(
                        commands,
                        &vk::DependencyInfo::default().memory_barriers(slice::from_ref(&to_read)),
                    );
                    device.end_label(commands);
                }
            }
        }
        buffer.set_written();
        Ok(())
    }

    /// Copies `bytes` into staging, where they wait for the copy that fills a resource with them,
    /// and returns the buffer and offset they're at.
    fn stage(&mut self, bytes: &[u8]) -> Result<(vk::Buffer, u64)> {
        let staged = self.frames.staging.upload(&self.frames.device, bytes)?;
        Ok((staged.buffer, staged.offset))
    }

    /// The commands the frame's copies on the transfer queue record into, begun with the first.
    fn transfer_commands(&mut self) -> Result<vk::CommandBuffer> {
        let slot = &self.frames.slots[self.slot];
        let (pool, transfers) = (slot.transfer_pool, slot.transfers);
        if !self.transferring {
            let (device, name) = (&self.frames.device, &slot.transfers_name);
            // SAFETY: the slot's last copies have finished, as the GPU has finished the slot's
            // previous frame, which waited for them, or that frame failed to submit and waited
            // for them itself; so the pool's commands can be reset and recorded again.
            unsafe { begin_recording(device, pool, transfers, name) }?;
            self.transferring = true;
        }
        Ok(transfers)
    }

    /// Records `copy` into the frame's copies on the transfer queue, then hands what it filled over
    /// to the graphics queue: `release` after the copy, and `acquire` in the frame's commands,
    /// before its passes, all under `label`. Recording both here keeps either from being left out,
    /// which nothing would catch for a buffer's acquire.
    fn hand_over(
        &mut self,
        label: &CStr,
        copy: impl FnOnce(&ash::Device, vk::CommandBuffer),
        [release, acquire]: [&vk::DependencyInfo<'_>; 2],
    ) -> Result<()> {
        let transfers = self.transfer_commands()?;
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        let raw = device.raw();
        // SAFETY: both command buffers are recording, the frame's outside a pass.
        unsafe { device.begin_label(transfers, label) };
        copy(raw, transfers);
        // SAFETY: as above.
        unsafe {
            raw.cmd_pipeline_barrier2(transfers, release);
            device.end_label(transfers);
            device.begin_label(commands, label);
            raw.cmd_pipeline_barrier2(commands, acquire);
            device.end_label(commands);
        }
        Ok(())
    }

    /// Records into the frame's commands, under `label`, a copy of the pixels staged in `buffer`
    /// into `texture`, by `regions`, between barriers that wait for what reads the texture and make
    /// the copy visible to it.
    fn copy_to_texture(
        &self,
        texture: &Texture,
        buffer: vk::Buffer,
        regions: &[vk::BufferImageCopy],
        label: &CStr,
    ) {
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        // Earlier frames on this queue may still sample the texture; the copy waits for them.
        let (old_layout, src_stage) = if texture.written() {
            (vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, TEXTURE_READERS)
        } else {
            (vk::ImageLayout::UNDEFINED, vk::PipelineStageFlags2::NONE)
        };
        let to_copy = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(src_stage)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(old_layout)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(texture.raw())
            .subresource_range(COLOR_RANGE);
        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(TEXTURE_READERS)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(texture.raw())
            .subresource_range(COLOR_RANGE);
        let raw = device.raw();
        // SAFETY: the commands are recording outside a pass, the regions fit the texture, and
        // the staged pixels and the texture live until the frame has finished: the arena is
        // reset only after that, staging and the texture retire until then.
        unsafe {
            device.begin_label(commands, label);
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&to_copy)),
            );
            raw.cmd_copy_buffer_to_image(
                commands,
                buffer,
                texture.raw(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                regions,
            );
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&to_read)),
            );
            device.end_label(commands);
        }
    }
}

/// The regions, the first `count` of them, of a copy of every level of `texture` from its pixels
/// at `offset` in a buffer, packed a level after another, largest first. Each is a whole level,
/// which any queue copies whatever its transfer granularity, AMD's 16 by 16 by 8 among them:
/// Vulkan lets a copy that reaches the image's edges ignore it (`minImageTransferGranularity`).
/// Each level's pixels start a multiple of 4 bytes after the staged ones, which start 16-byte
/// aligned, as a queue for transfers alone needs (VUID-vkCmdCopyBufferToImage-commandBuffer-07737).
fn level_regions(texture: &Texture, offset: u64) -> ([vk::BufferImageCopy; MAX_LEVELS], usize) {
    let mut regions = [vk::BufferImageCopy::default(); MAX_LEVELS];
    for ((level, (size, at)), region) in texture.levels().enumerate().zip(&mut regions) {
        *region = copy_region(offset + at, [0, 0], size, level as u32);
    }
    (regions, texture.level_count() as usize)
}

/// The families of the queues a resource is handed over between: the transfer queue's, then the
/// graphics queue's.
fn handover_families(device: &Device) -> [u32; 2] {
    let transfer = device
        .transfer_family()
        .expect("a device that uploads through a transfer queue has one");
    [transfer, device.graphics_family()]
}

/// The barriers that hand a texture the transfer queue filled over to the graphics queue: the
/// release, on the transfer queue after the copy, and the acquire, on the graphics queue before
/// the frame's passes. Vulkan requires them to match but for the stages and accesses each ignores
/// of the other's side, and changes the layout once, between them. Validation checks the layout
/// they leave only against what next uses the texture, which sampling through the bindless set
/// doesn't count as.
fn texture_handover(
    [transfer, graphics]: [u32; 2],
    image: vk::Image,
) -> [vk::ImageMemoryBarrier2<'static>; 2] {
    let barrier = vk::ImageMemoryBarrier2::default()
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_queue_family_index(transfer)
        .dst_queue_family_index(graphics)
        .image(image)
        .subresource_range(COLOR_RANGE);
    [
        barrier
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE),
        barrier
            .dst_stage_mask(TEXTURE_READERS)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ),
    ]
}

/// The release and acquire that hand a buffer the transfer queue filled over to the graphics
/// queue, as [`texture_handover`]'s do a texture.
fn buffer_handover(
    [transfer, graphics]: [u32; 2],
    buffer: vk::Buffer,
) -> [vk::BufferMemoryBarrier2<'static>; 2] {
    let barrier = vk::BufferMemoryBarrier2::default()
        .src_queue_family_index(transfer)
        .dst_queue_family_index(graphics)
        .buffer(buffer)
        .size(vk::WHOLE_SIZE);
    [
        barrier
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE),
        barrier
            .dst_stage_mask(BUFFER_READERS)
            .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::INDEX_READ),
    ]
}

/// Bytes in a frame's arena, valid for the GPU during that frame, whose [`Frames`] it borrows.
///
/// [`Frames`]: crate::Frames
#[derive(Clone, Copy, Debug)]
pub struct Upload<'a> {
    pub(super) buffer: vk::Buffer,
    pub(super) offset: u64,
    pub(super) size: u64,
    address: vk::DeviceAddress,
    /// The handle of the device whose frames hold the bytes.
    pub(super) device: vk::Device,
    frames: PhantomData<&'a ()>,
}

impl Upload<'_> {
    /// Where shaders read the bytes, through a buffer pointer.
    pub fn address(&self) -> u64 {
        self.address
    }
}

/// Memory the CPU writes during a frame for the GPU to read in it, allocated by bumping an
/// offset. A frame's own arena holds geometry, data shaders read, and pixels on their way to
/// textures; it's reset when the frame's slot comes round again, and grows by adding buffers,
/// which the reset merges into one big enough for them all, so frames stop allocating once they
/// reach a steady state. Staging holds what fills meshes and textures, and is emptied as each
/// frame is submitted instead, so that it holds nothing once they stop coming.
pub(super) struct Arena {
    usage: vk::BufferUsageFlags,
    memory: Memory,
    /// Its buffers' debug name.
    name: String,
    /// The least a new buffer holds.
    least: u64,
    /// The newest buffer last; only it has room.
    buffers: Vec<Buffer>,
    /// How much of the newest buffer is used.
    used: u64,
}

impl Arena {
    /// Every upload starts at a multiple of this, which covers index buffers, texture copies, a
    /// transfer queue's copies and the data types shaders read.
    const ALIGNMENT: u64 = 16;

    /// The arena of slot `slot`'s frames, whose first buffer holds 1 MiB.
    pub(super) fn frame(slot: u64) -> Self {
        let usage = vk::BufferUsageFlags::INDEX_BUFFER
            | vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
            | vk::BufferUsageFlags::TRANSFER_SRC;
        Self::new(
            usage,
            Memory::Upload,
            format!("frame {slot} arena"),
            1 << 20,
        )
    }

    /// Staging, whose buffers hold at least 16 MiB, so that a scene of many meshes and textures
    /// takes few allocations.
    pub(super) fn staging() -> Self {
        let usage = vk::BufferUsageFlags::TRANSFER_SRC;
        Self::new(
            usage,
            Memory::Staging,
            "upload staging".to_owned(),
            16 << 20,
        )
    }

    fn new(usage: vk::BufferUsageFlags, memory: Memory, name: String, least: u64) -> Self {
        Self {
            usage,
            memory,
            name,
            least,
            buffers: Vec::new(),
            used: 0,
        }
    }

    /// Empties the arena, once the GPU has finished the frame that last used it.
    pub(super) fn reset(&mut self, device: &Arc<Device>) -> Result<()> {
        if self.buffers.len() > 1 {
            let total = self.buffers.iter().map(Buffer::size).sum();
            self.buffers.clear();
            self.grow(device, total)?;
        }
        self.used = 0;
        Ok(())
    }

    /// Empties the arena and drops its buffers, which retire until the GPU has finished the frame
    /// being recorded.
    pub(super) fn clear(&mut self) {
        self.buffers.clear();
        self.used = 0;
    }

    fn upload<'a>(&mut self, device: &Arc<Device>, bytes: &[u8]) -> Result<Upload<'a>> {
        let size = bytes.len() as u64;
        let mut offset = self.used.next_multiple_of(Self::ALIGNMENT);
        let room = self.buffers.last().map_or(0, Buffer::size);
        if self.buffers.is_empty() || offset + size > room {
            self.grow(device, (room * 2).max(size).max(self.least))?;
            offset = 0;
        }
        let buffer = self.buffers.last_mut().expect("the arena just made room");
        buffer.write(offset, bytes);
        self.used = offset + size;
        Ok(Upload {
            buffer: buffer.raw(),
            offset,
            size,
            address: buffer.address() + offset,
            device: device.handle(),
            frames: PhantomData,
        })
    }

    fn grow(&mut self, device: &Arc<Device>, size: u64) -> Result<()> {
        let buffer = Buffer::new(device, size, self.usage, self.memory, &self.name)?;
        self.buffers.push(buffer);
        self.used = 0;
        Ok(())
    }

    /// Makes what the CPU wrote visible to the GPU.
    pub(super) fn flush(&self) -> Result<()> {
        for buffer in &self.buffers {
            buffer.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ash::vk::Handle;

    use super::*;

    #[test]
    fn a_handover_releases_and_acquires_alike_but_for_each_sides_stages() {
        use vk::AccessFlags2 as Access;
        use vk::PipelineStageFlags2 as Stage;
        // The transfer queue's family, then the graphics queue's.
        let families = [1, 0];

        let image = vk::Image::from_raw(7);
        let [release, acquire] = texture_handover(families, image);
        let alike = |barrier: &vk::ImageMemoryBarrier2<'_>| {
            let range = barrier.subresource_range;
            let whole = (range.base_mip_level, range.level_count, range.layer_count);
            let layouts = (barrier.old_layout, barrier.new_layout);
            let queues = (
                barrier.src_queue_family_index,
                barrier.dst_queue_family_index,
            );
            (layouts, queues, barrier.image, range.aspect_mask, whole)
        };
        let expected = (
            (
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            ),
            (1, 0),
            image,
            vk::ImageAspectFlags::COLOR,
            (0, vk::REMAINING_MIP_LEVELS, 1),
        );
        assert_eq!(alike(&release), expected);
        assert_eq!(alike(&acquire), expected);
        let sides = |barrier: &vk::ImageMemoryBarrier2<'_>| {
            let src = (barrier.src_stage_mask, barrier.src_access_mask);
            (src, (barrier.dst_stage_mask, barrier.dst_access_mask))
        };
        let none = (Stage::NONE, Access::NONE);
        assert_eq!(
            sides(&release),
            ((Stage::COPY, Access::TRANSFER_WRITE), none)
        );
        let sampled = (TEXTURE_READERS, Access::SHADER_SAMPLED_READ);
        assert_eq!(sides(&acquire), (none, sampled));

        let buffer = vk::Buffer::from_raw(9);
        let [release, acquire] = buffer_handover(families, buffer);
        let alike = |barrier: &vk::BufferMemoryBarrier2<'_>| {
            let queues = (
                barrier.src_queue_family_index,
                barrier.dst_queue_family_index,
            );
            (queues, barrier.buffer, barrier.offset, barrier.size)
        };
        let expected = ((1, 0), buffer, 0, vk::WHOLE_SIZE);
        assert_eq!(alike(&release), expected);
        assert_eq!(alike(&acquire), expected);
        let sides = |barrier: &vk::BufferMemoryBarrier2<'_>| {
            let src = (barrier.src_stage_mask, barrier.src_access_mask);
            (src, (barrier.dst_stage_mask, barrier.dst_access_mask))
        };
        assert_eq!(
            sides(&release),
            ((Stage::COPY, Access::TRANSFER_WRITE), none)
        );
        let read = Access::SHADER_STORAGE_READ | Access::INDEX_READ;
        assert_eq!(sides(&acquire), (none, (BUFFER_READERS, read)));
    }
}
