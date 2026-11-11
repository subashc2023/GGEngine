//! Uploads. A frame copies parts of the meshes and textures being streamed to the GPU by the
//! device's upload path, as many bytes of each as its caller's budget allows, and takes each for
//! drawing once its last part is there: at once through the graphics queue or by host image copy,
//! and through a transfer queue, whose copies go in a submission of their own that no frame waits
//! for, in the first frame that begins after they've finished.
//!
//! The SAFETY comments here lean on what the parent module keeps: [`Frames::begin`] begins a slot's
//! commands, and resets its arena, only once the GPU has finished the slot's last frame, and they
//! record until the frame is submitted; `Frame::outside_pass` checks that no pass is open, which
//! only a pass's drop closes; `Frame::send` empties staging, whose buffers then retire with the
//! frame and with the copies being recorded, and submits the copies, whose command buffer is
//! reused only once the transfer timeline has passed their submission; and resources retire until
//! both the frames and the copies that may use them have finished.
//!
//! [`Frames::begin`]: crate::Frames::begin

use std::marker::PhantomData;
use std::slice;
use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use super::{Frame, TransferCommands, begin_recording, command_buffer};
use crate::buffer::Buffer;
use crate::texture::{MAX_LEVELS, Part, next_part};
use crate::vma::Memory;
use crate::{COLOR_RANGE, Device, GpuBuffer, Texture, UploadPath, color_layers, copy_region};

/// The stages that read textures: fragment shaders sample them, vertex shaders may too, as the
/// sprite shader queries their sizes, and compute shaders, as a sky's are prefiltered.
const TEXTURE_READERS: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::from_raw(
    vk::PipelineStageFlags2::VERTEX_SHADER.as_raw()
        | vk::PipelineStageFlags2::FRAGMENT_SHADER.as_raw()
        | vk::PipelineStageFlags2::COMPUTE_SHADER.as_raw(),
);

/// The stages that read buffers during a frame: shaders, through their addresses, mesh shaders
/// among them, which every stage before rasterizing takes in, and draws' indices.
const BUFFER_READERS: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::from_raw(
    TEXTURE_READERS.as_raw()
        | vk::PipelineStageFlags2::PRE_RASTERIZATION_SHADERS.as_raw()
        | vk::PipelineStageFlags2::INDEX_INPUT.as_raw(),
);

/// What reads buffers, as [`BUFFER_READERS`] read them.
const BUFFER_READS: vk::AccessFlags2 = vk::AccessFlags2::from_raw(
    vk::AccessFlags2::SHADER_STORAGE_READ.as_raw() | vk::AccessFlags2::INDEX_READ.as_raw(),
);

/// The least a part of a buffer takes, and what each part but the last is a multiple of. Vulkan
/// needs no alignment of a buffer copy's offsets or size, on a queue for transfers alone either
/// (only a copy into an image needs its buffer offset a multiple of 4 there,
/// VUID-vkCmdCopyBufferToImage-commandBuffer-07737, which staging's alignment gives); parts are
/// whole units of the staging arena's alignment so that they pack in it without padding.
const BUFFER_PART: u64 = Arena::ALIGNMENT;

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

    /// Writes `pixels` into the `size` pixels of `texture`'s own image starting at `origin`: rows
    /// of RGBA8, in the texture's format and tightly packed, top row first. The image's other
    /// pixels keep what they had, except on its first write, which leaves them undefined. Shaders
    /// see the new pixels from this frame's passes on. The pixels go through the frame's arena and
    /// the graphics queue, which suits small changes the frame uses at once, such as the UI's;
    /// [`Frame::stream_texture`] fills a whole image by the device's upload path, within a budget.
    /// Only RGBA8 textures are written: a compressed one is streamed, as its blocks of 4 by 4
    /// pixels can't be written a pixel at a time.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, a pass has begun in the frame, another device made the
    /// texture, its slot shows a stand-in rather than an image of its own, it's compressed, it has
    /// mip levels, whose others a write would leave undefined, the region doesn't fit in it, or
    /// `pixels` doesn't fill the region exactly.
    pub fn write_texture(
        &mut self,
        texture: &mut Texture,
        origin: [u32; 2],
        size: [u32; 2],
        pixels: &[u8],
    ) -> Result<()> {
        let commands = self.before_passes();
        self.frames
            .device
            .check_made("the texture written", texture.device());
        let image = texture.image_shown_mut("a texture written");
        assert!(
            !image.format.compressed(),
            "a compressed texture is streamed, not written, as its blocks of 4x4 pixels can't be \
             written a pixel at a time"
        );
        assert_eq!(
            image.levels, 1,
            "a texture with mip levels is streamed, not written"
        );
        let [width, height] = image.size();
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
        let device = &self.frames.device;
        // Earlier frames on this queue may still sample the image; the copy waits for them.
        let (old_layout, src_stage) = if image.written {
            (vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, TEXTURE_READERS)
        } else {
            (vk::ImageLayout::UNDEFINED, vk::PipelineStageFlags2::NONE)
        };
        let to_copy = to_copy(image.raw)
            .src_stage_mask(src_stage)
            .old_layout(old_layout);
        // SAFETY: the commands are recording outside a pass, the region fits the image, and the
        // staged pixels and the image live until the frame has finished: the arena is reset only
        // after that, and the image retires until then.
        unsafe {
            device.begin_label(commands, c"write texture");
            device.raw().cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&to_copy)),
            );
            copy_to_image(device, commands, staged.buffer, image.raw, &[region]);
            device.raw().cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(slice::from_ref(&to_read(image.raw))),
            );
            device.end_label(commands);
        }
        image.written = true;
        Ok(())
    }

    /// Writes `bytes` into `buffer` from byte `offset` on: through the frame's arena and the
    /// graphics queue, after what earlier frames read of it, which suits small changes to data the
    /// frame's draws read at once, such as a material's. The buffer's other bytes keep what they
    /// had, except on its first write, which leaves them undefined and takes the buffer for drawing,
    /// as [`Frame::finish_buffer`] takes one streamed. Shaders see the new bytes from this frame's
    /// passes on.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, a pass has begun in the frame, another device made the
    /// buffer, frames are streaming it, or the bytes don't fit in it.
    pub fn write_buffer(
        &mut self,
        buffer: &mut GpuBuffer,
        offset: u64,
        bytes: &[u8],
    ) -> Result<()> {
        let size = bytes.len() as u64;
        self.write_buffer_pieces(buffer, size, &[offset], bytes)
    }

    /// Writes `bytes`, in pieces of `piece` bytes each, into `buffer`, the `i`th piece from byte
    /// `offsets[i]` on, as [`write_buffer`](Self::write_buffer) writes them, in one copy: such as
    /// records scattered through a table, each where its index puts it. Pieces that overlap leave
    /// the later's bytes, as Vulkan copies them, undefined.
    ///
    /// # Panics
    ///
    /// As `write_buffer` does, for any piece; and if `bytes` aren't `offsets` pieces.
    pub fn write_buffer_pieces(
        &mut self,
        buffer: &mut GpuBuffer,
        piece: u64,
        offsets: &[u64],
        bytes: &[u8],
    ) -> Result<()> {
        let commands = self.before_passes();
        self.frames
            .device
            .check_made("the buffer written", buffer.device());
        assert_eq!(
            piece.checked_mul(offsets.len() as u64),
            Some(bytes.len() as u64),
            "a write's bytes are its pieces"
        );
        let size = buffer.size();
        for &offset in offsets {
            assert!(
                offset.checked_add(piece).is_some_and(|end| end <= size),
                "{piece} bytes at {offset} overflow a buffer of {size}"
            );
        }
        let written = buffer.written();
        let (copied, _) = buffer.progress();
        assert!(
            written || *copied == 0,
            "a buffer that frames are streaming isn't written"
        );
        // A copy has at least a byte.
        if bytes.is_empty() {
            return Ok(());
        }
        let staged = self.upload(bytes)?;
        let copies = &mut self.frames.scratch.copies;
        copies.clear();
        copies.extend((0..).zip(offsets).map(|(i, &offset)| vk::BufferCopy {
            src_offset: staged.offset + i * piece,
            dst_offset: offset,
            size: piece,
        }));
        // Earlier frames on this queue may still read the buffer, after all the compute queue's
        // passes read of it, which each frame's last batch here waits for, and an earlier write
        // may still be copying into it; the copy waits for both.
        let (src_stage, src_access) = if written {
            (
                BUFFER_READERS | vk::PipelineStageFlags2::COPY,
                vk::AccessFlags2::TRANSFER_WRITE,
            )
        } else {
            (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE)
        };
        let to_copy = vk::MemoryBarrier2::default()
            .src_stage_mask(src_stage)
            .src_access_mask(src_access)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE);
        let to_read = vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(BUFFER_READERS)
            .dst_access_mask(BUFFER_READS);
        // A pass on the compute queue waits for the batch the copy is in.
        self.frames.buffers_written = true;
        let device = &self.frames.device;
        let raw = device.raw();
        // SAFETY: the commands are recording outside a pass, each region fits the buffer and the
        // staged bytes, and the staged bytes and the buffer live until the frame has finished: the
        // arena is reset only after that, and the buffer retires until then.
        unsafe {
            device.begin_label(commands, c"write buffer");
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().memory_barriers(slice::from_ref(&to_copy)),
            );
            raw.cmd_copy_buffer(commands, staged.buffer, buffer.raw(), copies);
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().memory_barriers(slice::from_ref(&to_read)),
            );
            device.end_label(commands);
        }
        buffer.set_written();
        Ok(())
    }

    /// Copies the next part of `pixels`, all of the image [`Texture::refill`] made for `texture`,
    /// each of its mip levels largest first, in its format, tightly packed, after the level before,
    /// into that image, by the device's upload path, and returns how many bytes it copied: as many
    /// as fit `budget`, but at least the smallest whole part, so that each frame makes progress.
    /// A level of RGBA8 is its rows of pixels, top row first; a level of BC7 or BC5 is its rows of
    /// blocks of 4 by 4 pixels, 16 bytes each, top row first, a side that isn't a multiple of 4
    /// ending in a whole block ([`crate::TextureFormat::bytes`] says how many bytes that makes). A part is whole
    /// rows of one level, or the small levels of a mip tail together. The frame keeps nothing of
    /// `pixels`: the caller gives them again, whole, until the last part is copied. What's staged
    /// for a copy is freed once the GPU is done with it. Through a transfer queue, the copies go
    /// in a submission of their own, which no frame waits for. Shaders see the image once
    /// [`Frame::finish_texture`] has shown it in the texture's slot.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, a pass has begun in the frame, another device made the
    /// texture, no refill has made an image for it, `pixels` doesn't fill the image's levels
    /// exactly, or every part is copied already.
    pub fn stream_texture(
        &mut self,
        texture: &mut Texture,
        pixels: &[u8],
        budget: u64,
    ) -> Result<u64> {
        let commands = self.before_passes();
        let device = &self.frames.device;
        device.check_made("the texture streamed", texture.device());
        let path = device.upload_path();
        let unit = match path {
            UploadPath::HostImageCopy => 1,
            UploadPath::GraphicsQueue | UploadPath::TransferQueue => {
                device.copy_granularity().height
            }
        };
        let Some(pending) = texture.pending_mut() else {
            panic!("a texture is streamed once `Texture::refill` has made the image for it");
        };
        let total = pending.image.bytes();
        let [width, height] = pending.image.size();
        assert_eq!(
            pixels.len() as u64,
            total,
            "the pixels don't fill a {width}x{height} texture's {} levels in {:?}",
            pending.image.levels,
            pending.image.format,
        );
        assert!(
            pending.copied < total,
            "every part of the texture is copied already"
        );
        let format = pending.image.format;
        let part = next_part(format, pending.image.levels(), unit, pending.copied, budget);
        let first = pending.copied == 0;
        let last = pending.copied + part.bytes == total;
        let image = pending.image.raw;
        match path {
            UploadPath::HostImageCopy => {
                host_copy(&self.frames.device, image, pixels, &part, first)?;
            }
            UploadPath::GraphicsQueue => {
                let (staging, offset) =
                    self.stage(&pixels[part.offset as usize..][..part.bytes as usize])?;
                let regions = buffer_regions(&part, offset);
                let device = &self.frames.device;
                // SAFETY: the commands are recording outside a pass, the regions fit the image's
                // levels, and the staged pixels and the image live until the frame has finished:
                // staging and the image retire until then.
                unsafe {
                    device.begin_label(commands, c"stream texture");
                    if first {
                        device.raw().cmd_pipeline_barrier2(
                            commands,
                            &vk::DependencyInfo::default()
                                .image_memory_barriers(slice::from_ref(&to_copy(image))),
                        );
                    }
                    copy_to_image(device, commands, staging, image, &regions[..part.count]);
                    if last {
                        device.raw().cmd_pipeline_barrier2(
                            commands,
                            &vk::DependencyInfo::default()
                                .image_memory_barriers(slice::from_ref(&to_read(image))),
                        );
                    }
                    device.end_label(commands);
                }
            }
            UploadPath::TransferQueue => {
                let (staging, offset) =
                    self.stage(&pixels[part.offset as usize..][..part.bytes as usize])?;
                let regions = buffer_regions(&part, offset);
                let transfers = self.transfer_commands()?;
                let device = &self.frames.device;
                let [release, _] = texture_handover(handover_families(device), image);
                // SAFETY: the transfer commands are recording, the regions fit the image's levels,
                // and the staged pixels and the image live until the copies have finished: staging
                // and the image retire until then.
                unsafe {
                    device.begin_label(transfers, c"stream texture");
                    if first {
                        // Nothing has used the image, so it needs no ownership transfer.
                        device.raw().cmd_pipeline_barrier2(
                            transfers,
                            &vk::DependencyInfo::default()
                                .image_memory_barriers(slice::from_ref(&to_copy(image))),
                        );
                    }
                    copy_to_image(device, transfers, staging, image, &regions[..part.count]);
                    if last {
                        device.raw().cmd_pipeline_barrier2(
                            transfers,
                            &vk::DependencyInfo::default()
                                .image_memory_barriers(slice::from_ref(&release)),
                        );
                    }
                    device.end_label(transfers);
                }
                if last {
                    let pending = texture.pending_mut().expect("still streaming");
                    pending.released = self.frames.transfers_submitted + 1;
                }
            }
        }
        let pending = texture.pending_mut().expect("still streaming");
        pending.copied += part.bytes;
        Ok(part.bytes)
    }

    /// Shows the image streamed into `texture` in its slot, once its last part is there, in place
    /// of what the slot showed, a stand-in or an image of its own, which retires: at once if the
    /// last part went through the graphics queue or the host, and through a transfer queue once
    /// the copies have finished, as they had when this frame began, which this frame then takes
    /// over from it. Returns whether the texture now shows an image of its own, with none pending:
    /// so `true` as well for one that has shown its image already. The new image reaches the
    /// frame's bindless set at once, and each other set as its next frame begins, so frames before
    /// this one draw what the slot showed.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, another device made the texture, or a pass has begun in
    /// the frame and the image is to be shown, which every pass recorded before it would sample
    /// before it's ready: finish streaming before the frame's passes.
    pub fn finish_texture(&mut self, texture: &mut Texture) -> Result<bool> {
        self.outside_pass();
        let device = &self.frames.device;
        device.check_made("the texture finished", texture.device());
        let path = device.upload_path();
        let Some(pending) = texture.pending_mut() else {
            return Ok(texture.shown().is_some());
        };
        if pending.copied < pending.image.bytes() {
            return Ok(false);
        }
        let released = pending.released;
        if path == UploadPath::TransferQueue && released > self.frames.transfers_finished {
            return Ok(false);
        }
        assert!(
            !self.pass_begun,
            "a texture comes to show an image before the frame's passes, which would sample it \
             before it's ready"
        );
        let commands = self.before_passes();
        let pending = texture.take_pending().expect("just there");
        let mut image = pending.image;
        if path == UploadPath::TransferQueue {
            let [_, acquire] = texture_handover(handover_families(device), image.raw);
            // SAFETY: the commands are recording outside a pass, and the image lives until the
            // frame has finished: it retires until then.
            unsafe {
                device.begin_label(commands, c"finish texture");
                device.raw().cmd_pipeline_barrier2(
                    commands,
                    &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&acquire)),
                );
                device.end_label(commands);
            }
            self.frames.transfer_wait = self.frames.transfer_wait.max(released);
        }
        image.written = true;
        // SAFETY: the view is of a live sampled image, which retires by frames, so it lives until
        // this frame has finished, and is in the shader-read-only layout, as its last copy, or the
        // acquire just recorded, leaves it before any pass of this frame; the slot of the frame
        // being recorded is this one's, whose set's last frame has finished.
        unsafe {
            device
                .bindless()
                .rewrite(device.raw(), image.view, texture.index(), self.slot)
        };
        if let Some(old) = texture.show(image) {
            old.retire();
        }
        Ok(true)
    }

    /// Shows `stand_in`'s image in `texture`'s slot, in place of what it showed: the texture's own
    /// image, if it had one, retires, and so does any image being streamed into it, with whatever
    /// was copied. The stand-in reaches the frame's bindless set at once, and each other set as
    /// its next frame begins, so frames before this one draw what the slot showed. The texture is
    /// then as [`Texture::placed`] made it, holding the stand-in's image while its slot shows it,
    /// and can be refilled again.
    ///
    /// # Panics
    ///
    /// If another device made either texture, or `stand_in` has no image of its own.
    pub fn hide_texture(&mut self, texture: &mut Texture, stand_in: &Texture) {
        let device = &self.frames.device;
        device.check_made("the texture hidden", texture.device());
        device.check_made("the stand-in", stand_in.device());
        let stood_in = stand_in.image_shown("a stand-in");
        // SAFETY: the stand-in's view is of a live sampled image, which frames write before any
        // shader samples it, as for the texture it's of. The texture holds it from here on while
        // its slot shows it, and the image retires by frames once nothing holds it, so it lives
        // until every frame that may sample it through the slot has finished, however long the
        // stand-in itself does. The slot of the frame being recorded is this one's, whose set's
        // last frame has finished.
        unsafe {
            device
                .bindless()
                .rewrite(device.raw(), stood_in.view, texture.index(), self.slot)
        };
        texture.stand_in(stood_in);
        if let Some(pending) = texture.take_pending() {
            pending.image.retire();
        }
        if let Some(image) = texture.take_shown() {
            image.retire();
        }
    }

    /// Copies the next part of `bytes`, all of `buffer`'s, into it, by the device's upload path,
    /// and returns how many bytes it copied: as many as fit `budget`, in multiples of 16 bytes,
    /// but at least 16, or what's left, so that each frame makes progress. The frame keeps nothing
    /// of `bytes`: the caller gives them again, whole, until the last part is copied. What's staged
    /// for a copy is freed once the GPU is done with it. Through a transfer queue, the copies go in
    /// a submission of their own, which no frame waits for; a device that copies textures from
    /// the host copies buffers on the graphics queue. Shaders and draws see the buffer once
    /// [`Frame::finish_buffer`] has taken it.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, a pass has begun in the frame, another device made the
    /// buffer, frames have taken it for drawing, `bytes` doesn't fill it exactly, or every part is
    /// copied already.
    pub fn stream_buffer(
        &mut self,
        buffer: &mut GpuBuffer,
        bytes: &[u8],
        budget: u64,
    ) -> Result<u64> {
        let commands = self.before_passes();
        self.frames
            .device
            .check_made("the buffer streamed", buffer.device());
        assert!(
            !buffer.written(),
            "a buffer is streamed once, before frames take it for drawing"
        );
        let size = buffer.size();
        assert_eq!(
            bytes.len() as u64,
            size,
            "the bytes don't fill a buffer of {size}"
        );
        let target = buffer.raw();
        let (copied, _) = buffer.progress();
        let copied = *copied;
        assert!(copied < size, "every part of the buffer is copied already");
        let part = buffer_part(copied, size, budget);
        let (staging, offset) = self.stage(&bytes[copied as usize..][..part as usize])?;
        let region = vk::BufferCopy {
            src_offset: offset,
            dst_offset: copied,
            size: part,
        };
        let last = copied + part == size;
        match self.frames.device.upload_path() {
            UploadPath::TransferQueue => {
                let transfers = self.transfer_commands()?;
                let device = &self.frames.device;
                // SAFETY: the transfer commands are recording, the region fits the buffer, and the
                // staged bytes and the buffer live until the copies have finished: staging and the
                // buffer retire until then. Every queue shares the buffer, so the semaphore that
                // a frame which takes it waits on is all it needs.
                unsafe {
                    device.begin_label(transfers, c"stream buffer");
                    device
                        .raw()
                        .cmd_copy_buffer(transfers, staging, target, &[region]);
                    device.end_label(transfers);
                }
                if last {
                    let (_, released) = buffer.progress();
                    *released = self.frames.transfers_submitted + 1;
                }
            }
            UploadPath::HostImageCopy | UploadPath::GraphicsQueue => {
                // A pass on the compute queue waits for the batch the copy is in.
                self.frames.buffers_written = true;
                let device = &self.frames.device;
                let to_read = vk::MemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COPY)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(BUFFER_READERS)
                    .dst_access_mask(BUFFER_READS);
                let raw = device.raw();
                // SAFETY: the commands are recording outside a pass, the region fits the buffer,
                // and the staged bytes and the buffer live until the frame has finished: staging
                // and the buffer retire until then.
                unsafe {
                    device.begin_label(commands, c"stream buffer");
                    raw.cmd_copy_buffer(commands, staging, target, &[region]);
                    if last {
                        raw.cmd_pipeline_barrier2(
                            commands,
                            &vk::DependencyInfo::default()
                                .memory_barriers(slice::from_ref(&to_read)),
                        );
                    }
                    device.end_label(commands);
                }
            }
        }
        let (copied, _) = buffer.progress();
        *copied += part;
        Ok(part)
    }

    /// Takes `buffer` for drawing, once its last part is there: at once if it went through the
    /// graphics queue, and through a transfer queue once the copies have finished, as they had
    /// when this frame began, which this frame then takes it over from. Returns whether draws may
    /// take it from now on, in this frame's passes and later frames': so `true` as well for one
    /// taken already.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, or another device made the buffer.
    pub fn finish_buffer(&mut self, buffer: &mut GpuBuffer) -> Result<bool> {
        self.outside_pass();
        let device = &self.frames.device;
        device.check_made("the buffer finished", buffer.device());
        if buffer.written() {
            return Ok(true);
        }
        let size = buffer.size();
        let (copied, released) = buffer.progress();
        if *copied < size {
            return Ok(false);
        }
        if device.upload_path() == UploadPath::TransferQueue {
            let released = *released;
            if released > self.frames.transfers_finished {
                return Ok(false);
            }
            // Every queue shares the buffer, so the frame's wait for the copies, which makes what
            // they wrote visible to all its commands, is all it takes.
            self.frames.transfer_wait = self.frames.transfer_wait.max(released);
        }
        buffer.set_written();
        Ok(true)
    }

    /// How many submissions of copies on the transfer queue the GPU hadn't finished as this frame
    /// began: 0 on the other paths. What streams holds off while this is high, copying nothing
    /// in the frame, so that the copies in flight, their staging and their command buffers stay
    /// bounded when the copy engine falls behind the frames, as over a slow link or under
    /// contention; a copy engine that keeps up leaves at most the last frame's unfinished.
    pub fn uploads_behind(&self) -> u64 {
        self.frames.transfers_submitted - self.frames.transfers_finished
    }

    /// Copies `bytes` into staging, where they wait for the copy that fills a resource with them,
    /// and returns the buffer and offset they're at.
    fn stage(&mut self, bytes: &[u8]) -> Result<(vk::Buffer, u64)> {
        let staged = self.frames.staging.upload(&self.frames.device, bytes)?;
        Ok((staged.buffer, staged.offset))
    }

    /// The commands the frame's copies on the transfer queue record into, begun with the first:
    /// a command buffer of the ring whose last submission has finished, or another made, so that
    /// the frame never waits for one. The ring grows only while frames copy with submissions
    /// unfinished, which [`Frame::uploads_behind`] lets a caller hold off at.
    fn transfer_commands(&mut self) -> Result<vk::CommandBuffer> {
        if let Some(i) = self.transfers {
            return Ok(self.frames.transfers[i].commands);
        }
        let frames = &mut *self.frames;
        let free = (frames.transfers.iter()).position(|t| t.submitted <= frames.transfers_finished);
        let i = match free {
            Some(i) => i,
            None => {
                let family = (frames.device.transfer_family())
                    .expect("a device that uploads through a transfer queue has one");
                let name = format!("transfers {}", frames.transfers.len());
                let (pool, commands, name) = command_buffer(&frames.device, family, name)?;
                frames.transfers.push(TransferCommands {
                    pool,
                    commands,
                    name,
                    submitted: 0,
                });
                frames.transfers.len() - 1
            }
        };
        let transfers = &frames.transfers[i];
        // SAFETY: the transfer timeline has passed the submission the commands were last in, if
        // any, so the GPU is done with them and the pool's commands can be reset and recorded
        // again.
        unsafe {
            begin_recording(
                &frames.device,
                transfers.pool,
                transfers.commands,
                &transfers.name,
            )
        }?;
        frames.device.transfer_begun(frames.transfers_submitted + 1);
        self.transfers = Some(i);
        Ok(transfers.commands)
    }
}

/// How many bytes the next part of a buffer of `size` bytes takes, after the first `copied`,
/// within `budget`: a multiple of [`BUFFER_PART`], at least one, unless it reaches the end.
fn buffer_part(copied: u64, size: u64, budget: u64) -> u64 {
    let left = size - copied;
    let want = budget / BUFFER_PART * BUFFER_PART;
    left.min(want.max(BUFFER_PART))
}

/// The copies of `part`'s regions from the staged bytes at `staged`, the part's first, into their
/// levels: the first `part.count` of them.
fn buffer_regions(part: &Part, staged: u64) -> [vk::BufferImageCopy; MAX_LEVELS] {
    let mut regions = [vk::BufferImageCopy::default(); MAX_LEVELS];
    for (copy, region) in regions.iter_mut().zip(part.regions()) {
        let offset = staged + (region.offset - part.offset);
        *copy = copy_region(offset, [0, region.y], region.size, region.level);
    }
    regions
}

/// Copies `regions` of the bytes staged in `buffer` into `image`, which is in the transfer
/// destination layout, in `commands`.
///
/// # Safety
///
/// `commands` must be recording, outside a pass, the regions must fit the image's levels and the
/// staged bytes, and both must live until the copy has finished.
unsafe fn copy_to_image(
    device: &Device,
    commands: vk::CommandBuffer,
    buffer: vk::Buffer,
    image: vk::Image,
    regions: &[vk::BufferImageCopy],
) {
    // SAFETY: the caller guarantees it.
    unsafe {
        device.raw().cmd_copy_buffer_to_image(
            commands,
            buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            regions,
        );
    }
}

/// Writes `part` of `pixels`, which hold all of `image`'s, into it from the CPU, host image copy,
/// on a device that uploads that way, into the shader-read-only layout, which the image is
/// transitioned to by the host first, if `first`. The device's next submission makes the writes
/// visible to it.
fn host_copy(
    device: &Device,
    image: vk::Image,
    pixels: &[u8],
    part: &Part,
    first: bool,
) -> Result<()> {
    let functions = device
        .host_image_copy_fn()
        .expect("only a device that uploads by host image copy copies from the host");
    let texture = format_args!("image {image:?}");
    if first {
        let transition = vk::HostImageLayoutTransitionInfoEXT::default()
            .image(image)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .subresource_range(COLOR_RANGE);
        // SAFETY: the image is live and made for host transfers, which the device enabled, into
        // the shader-read-only layout, which it listed as one host copies write; no submission has
        // used it, since nothing has been copied into it, and `&mut` keeps the engine off it.
        unsafe { functions.transition_image_layout(slice::from_ref(&transition)) }
            .map_err(|e| crate::vk_error("vkTransitionImageLayoutEXT", texture, e))?;
    }
    // A region for each of the part's, each from where its bytes start. A row length and image
    // height of 0 pack rows tightly, in whole blocks for BC7. A region's extent is its rows',
    // which needn't be a multiple of a block's sides where it reaches the level's bottom
    // (VUID-VkCopyMemoryToImageInfo-dstImage-00207 and -00208).
    let mut regions = [vk::MemoryToImageCopyEXT::default(); MAX_LEVELS];
    for (copy, region) in regions.iter_mut().zip(part.regions()) {
        *copy = vk::MemoryToImageCopyEXT::default()
            .host_pointer(pixels[region.offset as usize..].as_ptr().cast())
            .image_subresource(color_layers(region.level))
            .image_offset(vk::Offset3D {
                x: 0,
                y: region.y as i32,
                z: 0,
            })
            .image_extent(vk::Extent3D {
                width: region.size[0],
                height: region.size[1],
                depth: 1,
            });
    }
    let info = vk::CopyMemoryToImageInfoEXT::default()
        .dst_image(image)
        .dst_image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .regions(&regions[..part.count]);
    // SAFETY: as above, in the layout the first part changed to; the regions are rows of the
    // image's levels, whose bytes `pixels`, tightly packed a level after another, was checked to
    // hold.
    unsafe { functions.copy_memory_to_image(&info) }
        .map_err(|e| crate::vk_error("vkCopyMemoryToImageEXT", texture, e))
}

/// The barrier that takes `image`, which nothing has used, into the transfer destination layout
/// for copies into it, on whichever queue.
fn to_copy(image: vk::Image) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .dst_stage_mask(vk::PipelineStageFlags2::COPY)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(COLOR_RANGE)
}

/// The barrier that makes the copies into `image` on the graphics queue visible to what reads it,
/// in the shader-read-only layout.
fn to_read(image: vk::Image) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(TEXTURE_READERS)
        .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .image(image)
        .subresource_range(COLOR_RANGE)
}

/// The families of the queues a texture is handed over between: the transfer queue's, then the
/// graphics queue's.
fn handover_families(device: &Device) -> [u32; 2] {
    let transfer = device
        .transfer_family()
        .expect("a device that uploads through a transfer queue has one");
    [transfer, device.graphics_family()]
}

/// The barriers that hand a texture the transfer queue filled over to the graphics queue: the
/// release, on the transfer queue after the last copy, and the acquire, on the graphics queue
/// before the passes of the frame that takes it, once the copies have finished. Vulkan requires
/// them to match but for the stages and accesses each ignores of the other's side, and changes
/// the layout once, between them. Validation checks the layout they leave only against what next
/// uses the texture, which sampling through the bindless sets doesn't count as.
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
/// reach a steady state. Staging holds the parts of meshes and textures a frame streams, and is
/// emptied as each frame is submitted instead, so that it holds nothing once they stop coming.
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
    /// What the uploads since it was last emptied took, each from a multiple of the alignment.
    taken: u64,
    /// The most `taken` has been.
    peak: u64,
}

impl Arena {
    /// Every upload starts at a multiple of this, which covers index buffers, texture copies, BC7's
    /// 16-byte blocks among them, a transfer queue's copies and the data types shaders read.
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

    /// Staging, whose buffers hold at least 16 MiB, a frame's budget of parts, so that a frame's
    /// streaming takes one allocation.
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
            taken: 0,
            peak: 0,
        }
    }

    /// Empties the arena, once the GPU has finished the frame that last used it.
    pub(super) fn reset(&mut self, device: &Arc<Device>) -> Result<()> {
        if self.buffers.len() > 1 {
            let total = self.held();
            self.buffers.clear();
            self.grow(device, total)?;
        }
        self.used = 0;
        self.taken = 0;
        Ok(())
    }

    /// Empties the arena and drops its buffers, which retire until the GPU has finished the frame
    /// being recorded and the copies it recorded.
    pub(super) fn clear(&mut self) {
        self.buffers.clear();
        self.used = 0;
        self.taken = 0;
    }

    /// The bytes of its buffers.
    pub(super) fn held(&self) -> u64 {
        self.buffers.iter().map(Buffer::size).sum()
    }

    /// The most the uploads between two of its resets have taken of it, each from a multiple of
    /// [`ALIGNMENT`](Self::ALIGNMENT): a frame's, for a frame's own arena.
    pub(super) fn peak(&self) -> u64 {
        self.peak
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
        self.taken += size.next_multiple_of(Self::ALIGNMENT);
        self.peak = self.peak.max(self.taken);
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
    }

    /// A buffer's parts are 16 bytes at least and in multiples of 16 within the budget, but for
    /// the last, which reaches the end.
    #[test]
    fn buffer_parts_are_multiples_of_sixteen_bytes_until_the_end() {
        assert_eq!(buffer_part(0, 100, 50), 48);
        assert_eq!(buffer_part(48, 100, 50), 48);
        assert_eq!(buffer_part(96, 100, 50), 4);
        assert_eq!(buffer_part(0, 100, 3), 16);
        assert_eq!(buffer_part(0, 12, 3), 12);
        assert_eq!(buffer_part(0, 100, 1 << 20), 100);
    }
}
