//! Frames in flight. Each frame records its commands and submits them, and a timeline semaphore
//! paces them, so that the CPU gets at most `FRAMES_IN_FLIGHT` frames ahead of the GPU. A frame
//! can render into several targets, such as an app's windows, and presents all their swapchains at
//! once; its later passes can sample what its earlier ones rendered offscreen. It fills the meshes
//! and textures it takes by the device's upload path: through a transfer queue, it records their
//! copies apart and submits them first, and its commands wait for them.

use std::ffi::CStr;
use std::marker::PhantomData;
use std::slice;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;
use gg_core::Result;
use tracing::error;

use crate::bindless::PUSH_CONSTANTS;
use crate::buffer::Buffer;
use crate::texture::MAX_LEVELS;
use crate::vma::Memory;
use crate::{
    COLOR_RANGE, ColorImage, DEPTH_RANGE, DepthImage, Device, Faces, Format, GpuBuffer, Pipeline,
    Swapchain, Texture, UploadPath, color_layers, vk_error,
};

/// How many frames the GPU works on at once.
const FRAMES_IN_FLIGHT: u64 = 2;

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

/// What a frame renders into.
pub enum Target<'a> {
    /// The next image of a window's swapchain, presented when the frame is submitted.
    Swapchain(&'a mut Swapchain),
    /// An offscreen image.
    Image(&'a mut ColorImage),
}

/// The resources of the frames in flight, and the timeline semaphore that paces them. A device
/// has one for its whole life; its resources retire by its frames.
pub struct Frames {
    device: Arc<Device>,
    /// Reaches N when the Nth frame has finished on the GPU.
    timeline: vk::Semaphore,
    submitted: u64,
    slots: Vec<Slot>,
    /// Each slot's arena.
    arenas: Vec<Arena>,
    staging: Arena,
    /// For a device that uploads through a transfer queue, reaches N when the Nth submission of
    /// copies has finished; null otherwise.
    transfer_timeline: vk::Semaphore,
    /// How many times copies have been submitted, counted apart from frames, since a frame that
    /// fails to submit leaves its number to the next.
    transfers_submitted: u64,
    /// For each target of the frame being recorded, its image: the swapchain image's index, or 0
    /// for an offscreen image, and the layout the frame's commands so far leave it in. `None` for
    /// a swapchain that had no image ready.
    images: Vec<Option<(u32, vk::ImageLayout)>>,
    /// The last frame's list of targets, emptied, whose memory the next frame reuses.
    spare_targets: Vec<Target<'static>>,
    scratch: Scratch,
    /// How long the last `begin` waited, which [`Frames::waited`] describes.
    waited: Duration,
}

/// What one frame in flight records into and synchronizes with.
struct Slot {
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    /// For each target, signaled when the swapchain image the frame renders into is ready for
    /// it. There are as many as the most targets a frame has had.
    acquired: Vec<vk::Semaphore>,
    /// For a device that uploads through a transfer queue, the pool and command buffer the frame's
    /// copies record into; null otherwise.
    transfer_pool: vk::CommandPool,
    transfers: vk::CommandBuffer,
}

/// The lists a submission and its present are built in, kept so that frames don't allocate.
#[derive(Default)]
struct Scratch {
    waits: Vec<vk::SemaphoreSubmitInfo<'static>>,
    signals: Vec<vk::SemaphoreSubmitInfo<'static>>,
    rendered: Vec<vk::Semaphore>,
    swapchains: Vec<vk::SwapchainKHR>,
    images: Vec<u32>,
    results: Vec<vk::Result>,
}

impl Frames {
    /// # Panics
    ///
    /// If the device has had a `Frames` before, whose frame numbers this one's would repeat.
    pub fn new(device: &Arc<Device>) -> Result<Self> {
        device.claim_frames();
        let raw = device.raw();
        let mut timeline_info =
            vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        let info = vk::SemaphoreCreateInfo::default().push_next(&mut timeline_info);
        // SAFETY: the device is live, with timeline semaphores enabled.
        let timeline = unsafe { raw.create_semaphore(&info, None) }
            .map_err(|e| vk_error("vkCreateSemaphore", e))?;
        // From here on, dropping `frames` destroys what has been made so far.
        let mut frames = Self {
            device: device.clone(),
            timeline,
            submitted: 0,
            slots: Vec::new(),
            arenas: (0..FRAMES_IN_FLIGHT).map(Arena::frame).collect(),
            staging: Arena::staging(),
            transfer_timeline: vk::Semaphore::null(),
            transfers_submitted: 0,
            images: Vec::new(),
            spare_targets: Vec::new(),
            scratch: Scratch::default(),
            waited: Duration::ZERO,
        };
        // SAFETY: the semaphore is live.
        unsafe { device.set_name(timeline, "frame timeline") };
        let transfer_family = device.transfer_family();
        if transfer_family.is_some() {
            // SAFETY: as above.
            frames.transfer_timeline = unsafe { raw.create_semaphore(&info, None) }
                .map_err(|e| vk_error("vkCreateSemaphore", e))?;
            // SAFETY: the semaphore is live.
            unsafe { device.set_name(frames.transfer_timeline, "upload timeline") };
        }
        for i in 0..FRAMES_IN_FLIGHT {
            let pool_info = vk::CommandPoolCreateInfo::default()
                .flags(vk::CommandPoolCreateFlags::TRANSIENT)
                .queue_family_index(device.graphics_family());
            // SAFETY: the device is live, and each object is made from live ones.
            unsafe {
                let pool = raw
                    .create_command_pool(&pool_info, None)
                    .map_err(|e| vk_error("vkCreateCommandPool", e))?;
                frames.slots.push(Slot {
                    pool,
                    commands: vk::CommandBuffer::null(),
                    acquired: Vec::new(),
                    transfer_pool: vk::CommandPool::null(),
                    transfers: vk::CommandBuffer::null(),
                });
                let slot = frames.slots.last_mut().expect("a slot was just pushed");
                let allocate_info = vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1);
                slot.commands = raw
                    .allocate_command_buffers(&allocate_info)
                    .map_err(|e| vk_error("vkAllocateCommandBuffers", e))?[0];
                device.set_name(pool, &format!("frame {i} commands"));
                device.set_name(slot.commands, &format!("frame {i} commands"));
                if let Some(family) = transfer_family {
                    let pool_info = pool_info.queue_family_index(family);
                    slot.transfer_pool = raw
                        .create_command_pool(&pool_info, None)
                        .map_err(|e| vk_error("vkCreateCommandPool", e))?;
                    let allocate_info = allocate_info.command_pool(slot.transfer_pool);
                    slot.transfers = raw
                        .allocate_command_buffers(&allocate_info)
                        .map_err(|e| vk_error("vkAllocateCommandBuffers", e))?[0];
                    device.set_name(slot.transfer_pool, &format!("frame {i} transfers"));
                    device.set_name(slot.transfers, &format!("frame {i} transfers"));
                }
            }
        }
        Ok(frames)
    }

    /// Starts a frame that renders into `targets`, once the GPU has finished the frame that last
    /// used this one's resources. The first target paces the frame: for a swapchain, it waits
    /// until the display has let go of an image. The others are rendered into only if their
    /// swapchains have an image to give at once, so that a window on a slower display doesn't
    /// slow the frame; an offscreen image is always rendered into. Returns `None` if the first
    /// target has nothing to render into this time, as with the swapchain of a minimized window.
    ///
    /// # Panics
    ///
    /// If `targets` is empty.
    pub fn begin<'a>(
        &'a mut self,
        targets: impl IntoIterator<Item = Target<'a>>,
    ) -> Result<Option<Frame<'a>>> {
        let slot = (self.submitted % FRAMES_IN_FLIGHT) as usize;
        self.waited = Duration::ZERO;
        if let Some(previous) = self.submitted.checked_sub(FRAMES_IN_FLIGHT) {
            let start = Instant::now();
            self.wait(previous + 1)?;
            self.waited = start.elapsed();
            self.device.frames_finished(previous + 1);
        }
        let mut list: Vec<Target<'a>> = std::mem::take(&mut self.spare_targets);
        list.extend(targets);
        assert!(!list.is_empty(), "a frame renders into at least one target");
        let acquired = &mut self.slots[slot].acquired;
        while acquired.len() < list.len() {
            // SAFETY: the device is live.
            let semaphore = unsafe {
                self.device
                    .raw()
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|e| vk_error("vkCreateSemaphore", e))?;
            let name = format!("frame {slot} target {} image acquired", acquired.len());
            // SAFETY: the semaphore is live.
            unsafe { self.device.set_name(semaphore, &name) };
            acquired.push(semaphore);
        }
        self.images.clear();
        for (i, target) in list.iter_mut().enumerate() {
            let acquired = self.slots[slot].acquired[i];
            let image = match target {
                Target::Swapchain(swapchain) => {
                    // SAFETY: the semaphore is new, or the frame that last used this slot waited
                    // on it or left it unsignaled and has finished, so it's unsignaled with no
                    // wait pending.
                    unsafe { swapchain.acquire(acquired, i == 0, &mut self.waited) }?
                        .map(|image| (image, vk::ImageLayout::UNDEFINED))
                }
                Target::Image(image) => Some((0, image.layout())),
            };
            if i == 0 && image.is_none() {
                break;
            }
            self.images.push(image);
        }
        if self.images.is_empty() {
            self.spare_targets = recycle(list);
            return Ok(None);
        }
        self.device.frame_begun(self.submitted + 1);
        self.arenas[slot].reset(&self.device)?;
        let raw = self.device.raw();
        let Slot { pool, commands, .. } = self.slots[slot];
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        let bindless = self.device.bindless();
        // SAFETY: the GPU has finished the slot's previous frame, so its commands can be reset
        // and recorded again; the set and layout live as long as the device.
        unsafe {
            raw.reset_command_pool(pool, vk::CommandPoolResetFlags::empty())
                .map_err(|e| vk_error("vkResetCommandPool", e))?;
            raw.begin_command_buffer(commands, &begin_info)
                .map_err(|e| vk_error("vkBeginCommandBuffer", e))?;
            raw.cmd_bind_descriptor_sets(
                commands,
                vk::PipelineBindPoint::GRAPHICS,
                bindless.pipeline_layout(),
                0,
                &[bindless.set()],
                &[],
            );
        }
        Ok(Some(Frame {
            frames: self,
            targets: list,
            slot,
            submitted: false,
            transferring: false,
        }))
    }

    /// How long the last [`Frames::begin`] waited: for the GPU to finish the frame that last used
    /// this one's resources, and for the display to let go of the first target's image.
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// Waits until the GPU has finished the `frame`th frame.
    fn wait(&self, frame: u64) -> Result<()> {
        wait_on(&self.device, self.timeline, frame)
    }
}

/// Waits until `timeline` reaches `value`.
fn wait_on(device: &Device, timeline: vk::Semaphore, value: u64) -> Result<()> {
    let _zone = gg_core::zone!("wait for the GPU");
    let semaphores = [timeline];
    let values = [value];
    let info = vk::SemaphoreWaitInfo::default()
        .semaphores(&semaphores)
        .values(&values);
    // SAFETY: the timeline semaphore is live.
    unsafe { device.raw().wait_semaphores(&info, u64::MAX) }
        .map_err(|e| vk_error("vkWaitSemaphores", e))
}

/// Empties `targets` for the next frame's, which borrow for another lifetime. Collecting a
/// `vec::IntoIter` into a `Vec` of a type with the same layout reuses its memory.
fn recycle(mut targets: Vec<Target<'_>>) -> Vec<Target<'static>> {
    targets.clear();
    targets
        .into_iter()
        .map(|_| unreachable!("the list was emptied"))
        .collect()
}

impl Drop for Frames {
    fn drop(&mut self) {
        // A frame's commands wait for its copies, and a frame whose commands fail to submit waits
        // for them itself, but that wait can fail too.
        let waited = self.wait(self.submitted).and_then(|()| {
            if self.transfers_submitted > 0 {
                wait_on(
                    &self.device,
                    self.transfer_timeline,
                    self.transfers_submitted,
                )
            } else {
                Ok(())
            }
        });
        if waited.is_err() {
            let _ = self.device.wait_idle();
        }
        self.device.frames_finished(self.submitted);
        let raw = self.device.raw();
        // SAFETY: the GPU has finished every frame and every copy, so nothing here is in use.
        // Null handles, left by a failed `new` or by a device that doesn't upload through a
        // transfer queue, are allowed.
        unsafe {
            for slot in &self.slots {
                for &semaphore in &slot.acquired {
                    raw.destroy_semaphore(semaphore, None);
                }
                raw.destroy_command_pool(slot.pool, None);
                raw.destroy_command_pool(slot.transfer_pool, None);
            }
            raw.destroy_semaphore(self.timeline, None);
            raw.destroy_semaphore(self.transfer_timeline, None);
        }
    }
}

/// A frame being recorded. Submitting it, or dropping it, sends its commands to the GPU and
/// presents the images of its swapchains.
#[must_use = "a frame's commands reach the GPU when it's submitted"]
pub struct Frame<'a> {
    frames: &'a mut Frames,
    targets: Vec<Target<'a>>,
    slot: usize,
    submitted: bool,
    /// Whether the frame's copies on the transfer queue have begun recording.
    transferring: bool,
}

impl<'a> Frame<'a> {
    /// Copies `bytes` into the frame's arena, where the GPU reads them during this frame:
    /// geometry, and data shaders read through its address. The upload borrows the frame's
    /// [`Frames`], so that no later frame draws from it; its address, pushed to a shader in a
    /// later frame, reads whatever has replaced it.
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
    /// If the texture has mip levels, whose others a write would leave undefined, the region
    /// doesn't fit in it, or `pixels` doesn't fill the region exactly.
    pub fn write_texture(
        &mut self,
        texture: &mut Texture,
        origin: [u32; 2],
        size: [u32; 2],
        pixels: &[u8],
    ) -> Result<()> {
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
        let region = texture_region(staged.offset, origin, size, 0);
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
    /// If a frame has written the texture, or `pixels` doesn't fill its levels exactly.
    pub fn fill_texture(&mut self, texture: &mut Texture, pixels: &[u8]) -> Result<()> {
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

    /// Fills `buffer`, which no frame has written, with `bytes`: all of it. Shaders and draws see
    /// them from this frame's passes on, as [`Frame::fill_texture`] has it; a device that copies
    /// textures from the host copies buffers on the graphics queue.
    ///
    /// # Panics
    ///
    /// If a frame has written the buffer, or `bytes` doesn't fill it exactly.
    pub fn fill_buffer(&mut self, buffer: &mut GpuBuffer, bytes: &[u8]) -> Result<()> {
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
            let raw = self.frames.device.raw();
            let begin_info = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            // SAFETY: the slot's last copies have finished, as the GPU has finished the slot's
            // previous frame, which waited for them, or that frame failed to submit and waited
            // for them itself; so the pool's commands can be reset and recorded again.
            unsafe {
                raw.reset_command_pool(pool, vk::CommandPoolResetFlags::empty())
                    .map_err(|e| vk_error("vkResetCommandPool", e))?;
                raw.begin_command_buffer(transfers, &begin_info)
                    .map_err(|e| vk_error("vkBeginCommandBuffer", e))?;
            }
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

    /// Starts rendering into the `target`th of the targets the frame began with, cleared to
    /// `color`, in linear RGBA, and with `depth`, if given, which its draws test and write as
    /// their pipelines say, cleared to 0, as far away as depth goes. `label` names the pass in
    /// captures. Rendering ends when the pass is dropped. Returns `None` for a swapchain that had
    /// no image ready this frame; the first target always has one.
    ///
    /// # Panics
    ///
    /// If the frame has no `target`th target, or `depth` isn't the target's size.
    pub fn begin_pass(
        &mut self,
        target: usize,
        label: &std::ffi::CStr,
        color: [f32; 4],
        depth: Option<&DepthImage>,
    ) -> Option<Pass<'_, 'a>> {
        let (index, _) = self.frames.images[target]?;
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        let ((image, view, extent), format) = match &self.targets[target] {
            Target::Swapchain(swapchain) => (swapchain.image(index), swapchain.format()),
            Target::Image(image) => (image.parts(), ColorImage::FORMAT),
        };
        let depth = depth.map(|depth| {
            let (image, view, size) = depth.parts();
            assert_eq!(size, extent, "a pass's depth image is its target's size");
            (image, view)
        });
        let format = if depth.is_some() {
            format.with_depth()
        } else {
            format
        };
        // The old contents are discarded. A swapchain image is ready once its acquire semaphore
        // is signaled, which the submission waits for at the color-attachment stage; an
        // offscreen image must wait for whatever used it before.
        let (src_stage, src_access) = match self.targets[target] {
            Target::Swapchain(_) => (
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::NONE,
            ),
            Target::Image(_) => (
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::AccessFlags2::MEMORY_WRITE,
            ),
        };
        let barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(src_stage)
            .src_access_mask(src_access)
            .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image(image)
            .subresource_range(COLOR_RANGE);
        // The depth's old contents are discarded too, once the last pass that had it is done with
        // it: its clear wrote it at the early fragment tests, and its store at the late ones.
        let tests = vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
        let depth_barrier = depth.map(|(image, _)| {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(tests)
                .src_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .dst_stage_mask(tests)
                .dst_access_mask(
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                        | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                )
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                .image(image)
                .subresource_range(DEPTH_RANGE)
        });
        let barriers = [barrier, depth_barrier.unwrap_or_default()];
        let barriers = &barriers[..1 + usize::from(depth_barrier.is_some())];
        let attachment = vk::RenderingAttachmentInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue { float32: color },
            });
        // Nothing reads depth after the pass.
        let depth_attachment = depth.map(|(_, view)| {
            vk::RenderingAttachmentInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .clear_value(vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: 0.0,
                        stencil: 0,
                    },
                })
        });
        let mut rendering = vk::RenderingInfo::default()
            .render_area(extent.into())
            .layer_count(1)
            .color_attachments(slice::from_ref(&attachment));
        if let Some(depth) = &depth_attachment {
            rendering = rendering.depth_attachment(depth);
        }
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let raw = device.raw();
        let (cull_mode, front_face) = Faces::Both.state();
        // SAFETY: the commands are recording outside a pass, and the images and views live until
        // the frame has finished on the GPU: the swapchain waits for its frames before
        // destroying its images, and an offscreen image, and a depth image, retire until then,
        // dropped while the frame records. Every pipeline takes its faces as dynamic state, which
        // a draw needs set, and binding one doesn't change.
        unsafe {
            device.begin_label(commands, label);
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(barriers),
            );
            raw.cmd_begin_rendering(commands, &rendering);
            raw.cmd_set_viewport(commands, 0, &[viewport]);
            raw.cmd_set_scissor(commands, 0, &[extent.into()]);
            raw.cmd_set_cull_mode(commands, cull_mode);
            raw.cmd_set_front_face(commands, front_face);
        }
        self.frames.images[target] = Some((index, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL));
        Some(Pass {
            frame: self,
            extent,
            format,
            pipeline_set: false,
        })
    }

    /// Lets the frame's later passes sample the `target`th target, as its last pass left it, from
    /// their fragment shaders, through the index this returns in the bindless set. Shaders may
    /// sample through the index only until the image's next pass, which renders into it and so
    /// takes it out of the layout the bindless set declares: after that pass, call this again.
    ///
    /// # Panics
    ///
    /// If the frame has no `target`th target, the target is a swapchain's, or it's an offscreen
    /// image no pass has rendered into since it was made or read back.
    pub fn begin_sampling(&mut self, target: usize) -> u32 {
        let Target::Image(image) = &self.targets[target] else {
            panic!("target {target} is a swapchain's, and only offscreen images are sampled");
        };
        let (index, layout) =
            self.frames.images[target].expect("an offscreen image is always there");
        match layout {
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL => {}
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => return image.index(),
            _ => panic!("target {target} is sampled before a pass renders into it"),
        }
        let (raw_image, _, _) = image.parts();
        let barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(raw_image)
            .subresource_range(COLOR_RANGE);
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        // SAFETY: the commands are recording outside a pass, which `Pass` borrowing the frame
        // ensures, and the image lives until the frame has finished: it retires until then.
        unsafe {
            device.begin_label(commands, c"begin sampling");
            device.raw().cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&barrier)),
            );
            device.end_label(commands);
        }
        self.frames.images[target] = Some((index, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL));
        image.index()
    }

    /// Sends the frame's commands to the GPU and presents the images of its swapchains.
    pub fn submit(mut self) -> Result<()> {
        self.finish()
    }

    fn finish(&mut self) -> Result<()> {
        // Set first, so that dropping the frame after a failure doesn't submit it again.
        self.submitted = true;
        let Frames {
            device,
            timeline,
            submitted,
            slots,
            arenas,
            staging,
            transfer_timeline,
            transfers_submitted,
            images,
            scratch,
            ..
        } = &mut *self.frames;
        arenas[self.slot].flush()?;
        staging.flush()?;
        // Its buffers retire with this frame, so they last until the GPU has finished it, copies
        // and all.
        staging.clear();
        let raw = device.raw();
        let slot = &slots[self.slot];
        let frame = *submitted + 1;
        scratch.waits.clear();
        scratch.signals.clear();
        scratch.rendered.clear();
        scratch.swapchains.clear();
        scratch.images.clear();
        scratch.signals.push(
            vk::SemaphoreSubmitInfo::default()
                .semaphore(*timeline)
                .value(frame)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
        );
        if self.transferring {
            // SAFETY: the transfer commands are recording.
            unsafe { raw.end_command_buffer(slot.transfers) }
                .map_err(|e| vk_error("vkEndCommandBuffer", e))?;
            let command_infos =
                [vk::CommandBufferSubmitInfo::default().command_buffer(slot.transfers)];
            // Without maintenance8, which ash's headers predate, a wait for an ownership
            // transfer's release is for all commands.
            let copied = vk::SemaphoreSubmitInfo::default()
                .semaphore(*transfer_timeline)
                .value(*transfers_submitted + 1)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS);
            let submit = vk::SubmitInfo2::default()
                .command_buffer_infos(&command_infos)
                .signal_semaphore_infos(slice::from_ref(&copied));
            {
                let _zone = gg_core::zone!("submit uploads");
                // SAFETY: what the copies use lives until they finish, which the frame's own
                // submission waits for, or the frame itself if that fails: the slot is reused only
                // after that, and staging and the resources retire until then.
                unsafe { device.submit_transfers(slice::from_ref(&submit)) }?;
            }
            *transfers_submitted += 1;
            scratch.waits.push(copied);
        }
        for (i, (target, image)) in self.targets.iter().zip(images.iter_mut()).enumerate() {
            let (Target::Swapchain(swapchain), Some((index, layout))) = (target, image) else {
                continue;
            };
            let (image, _, _) = swapchain.image(*index);
            // The present waits on a semaphore signaled after every command, the layout change
            // included, so the barrier needs no second scope.
            let barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .old_layout(*layout)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .image(image)
                .subresource_range(COLOR_RANGE);
            // SAFETY: the commands are recording, and the image is live.
            unsafe {
                raw.cmd_pipeline_barrier2(
                    slot.commands,
                    &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&barrier)),
                );
            }
            *layout = vk::ImageLayout::PRESENT_SRC_KHR;
            let rendered = swapchain.rendered(*index);
            scratch.waits.push(
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(slot.acquired[i])
                    .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT),
            );
            scratch.signals.push(
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(rendered)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
            );
            scratch.rendered.push(rendered);
            scratch.swapchains.push(swapchain.raw());
            scratch.images.push(*index);
        }
        let command_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(slot.commands)];
        let submit = vk::SubmitInfo2::default()
            .wait_semaphore_infos(&scratch.waits)
            .command_buffer_infos(&command_infos)
            .signal_semaphore_infos(&scratch.signals);
        // SAFETY: the commands are recording.
        let sent = unsafe { raw.end_command_buffer(slot.commands) }
            .map_err(|e| vk_error("vkEndCommandBuffer", e))
            .and_then(|()| {
                let _zone = gg_core::zone!("submit");
                // SAFETY: what the submission uses lives until the timeline reaches `frame`: the
                // slot and its arena are reused only after waiting for it, and resources retire
                // until then.
                unsafe { device.submit(slice::from_ref(&submit)) }
            });
        if let Err(error) = sent {
            // Nothing else waits for the frame's copies now, so it does, before their slot is
            // reused or their staging freed.
            if self.transferring
                && let Err(waited) = wait_on(device, *transfer_timeline, *transfers_submitted)
            {
                error!(code = waited.code(), "{waited:#}");
            }
            return Err(error);
        }
        *submitted = frame;
        for (target, image) in self.targets.iter_mut().zip(images.iter()) {
            if let (Target::Image(target), Some((_, layout))) = (target, image) {
                target.set_layout(*layout);
            }
        }
        if scratch.swapchains.is_empty() {
            return Ok(());
        }

        scratch.results.clear();
        scratch
            .results
            .resize(scratch.swapchains.len(), vk::Result::SUCCESS);
        let info = vk::PresentInfoKHR::default()
            .wait_semaphores(&scratch.rendered)
            .swapchains(&scratch.swapchains)
            .image_indices(&scratch.images)
            .results(&mut scratch.results);
        let presented = {
            let _zone = gg_core::zone!("present");
            // SAFETY: each image was acquired for this frame, whose submission signals its
            // semaphore.
            unsafe { device.present(&info) }
        };
        let mut results = scratch.results.iter();
        for (target, image) in self.targets.iter_mut().zip(images.iter()) {
            if let (Target::Swapchain(swapchain), Some(_)) = (target, image) {
                let result = results
                    .next()
                    .expect("a result for each swapchain presented");
                swapchain.presented(*result)?;
            }
        }
        match presented {
            // Each swapchain's own result says how its present went.
            Ok(_) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(()),
            Err(e) => Err(vk_error("vkQueuePresentKHR", e)),
        }
    }
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        // An acquired swapchain image must be presented, or its semaphore stays signaled.
        if !self.submitted
            && let Err(error) = self.finish()
        {
            error!(code = error.code(), "{error:#}");
        }
        self.frames.spare_targets = recycle(std::mem::take(&mut self.targets));
    }
}

/// The region of a copy of `size` pixels, at `offset` in a buffer, into mip level `level` of a
/// texture at `origin`.
fn texture_region(
    offset: u64,
    origin: [u32; 2],
    size: [u32; 2],
    level: u32,
) -> vk::BufferImageCopy {
    vk::BufferImageCopy::default()
        .buffer_offset(offset)
        .image_subresource(color_layers(level))
        .image_offset(vk::Offset3D {
            x: origin[0] as i32,
            y: origin[1] as i32,
            z: 0,
        })
        .image_extent(vk::Extent3D {
            width: size[0],
            height: size[1],
            depth: 1,
        })
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
        *region = texture_region(offset + at, [0, 0], size, level as u32);
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

/// Rendering into a frame's target, which ends when the pass is dropped. Draws read their
/// geometry from [`Upload`]s through push constants, and their textures from the bindless set.
pub struct Pass<'p, 'a> {
    frame: &'p mut Frame<'a>,
    extent: vk::Extent2D,
    format: Format,
    /// Whether a pipeline is set, which draws need.
    pipeline_set: bool,
}

impl<'a> Pass<'_, 'a> {
    /// The target's width and height in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
    }

    /// The formats of its target and its depth image, if it has one, which the pipelines it draws
    /// with must be made for.
    pub fn format(&self) -> Format {
        self.format
    }

    /// As [`Frame::upload`].
    pub fn upload(&mut self, bytes: &[u8]) -> Result<Upload<'a>> {
        self.frame.upload(bytes)
    }

    /// Sets the pipeline the draws that follow use.
    ///
    /// # Panics
    ///
    /// If the pipeline isn't made for the pass's formats.
    pub fn set_pipeline(&mut self, pipeline: &Pipeline) {
        assert_eq!(
            pipeline.format(),
            self.format,
            "a pipeline is made for the formats of the pass it draws in: its target's and its depth \
             image's"
        );
        self.pipeline_set = true;
        // SAFETY: the commands are recording inside a pass whose formats, its target's and its
        // depth image's, are the pipeline's, and the pipeline retires until the frame has finished.
        unsafe {
            self.frame.frames.device.raw().cmd_bind_pipeline(
                self.commands(),
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.raw(),
            );
        }
    }

    /// Maps clip space onto the `size` pixels at `origin` for the draws that follow, and keeps
    /// them inside those pixels, clipped to the target.
    ///
    /// # Panics
    ///
    /// If `size` has no area, which a viewport can't have.
    pub fn set_viewport(&mut self, origin: [u32; 2], size: [u32; 2]) {
        assert!(
            size[0] > 0 && size[1] > 0,
            "a viewport has an area; got {size:?}"
        );
        let viewport = vk::Viewport {
            x: origin[0] as f32,
            y: origin[1] as f32,
            width: size[0] as f32,
            height: size[1] as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        // SAFETY: the commands are recording, and the viewport has an area.
        unsafe {
            self.frame
                .frames
                .device
                .raw()
                .cmd_set_viewport(self.commands(), 0, &[viewport]);
        }
        self.set_scissor(origin, size);
    }

    /// Has the draws that follow draw only the triangles `faces` names: a pass starts drawing
    /// both.
    pub fn set_faces(&mut self, faces: Faces) {
        let (cull_mode, front_face) = faces.state();
        let raw = self.frame.frames.device.raw();
        let commands = self.commands();
        // SAFETY: the commands are recording, and every pipeline takes its faces as dynamic state.
        unsafe {
            raw.cmd_set_cull_mode(commands, cull_mode);
            raw.cmd_set_front_face(commands, front_face);
        }
    }

    /// Limits the draws that follow to the `size` pixels at `origin`, clipped to the target.
    pub fn set_scissor(&mut self, origin: [u32; 2], size: [u32; 2]) {
        let x = origin[0].min(self.extent.width);
        let y = origin[1].min(self.extent.height);
        let scissor = vk::Rect2D {
            offset: vk::Offset2D {
                x: x as i32,
                y: y as i32,
            },
            extent: vk::Extent2D {
                width: size[0].min(self.extent.width - x),
                height: size[1].min(self.extent.height - y),
            },
        };
        // SAFETY: the commands are recording, and the scissor lies within the target.
        unsafe {
            self.frame
                .frames
                .device
                .raw()
                .cmd_set_scissor(self.commands(), 0, &[scissor]);
        }
    }

    /// Sets the push constants, from offset 0, that the draws which follow see in every stage.
    ///
    /// # Panics
    ///
    /// If `bytes` is longer than 128 bytes or isn't a whole number of 4-byte words.
    pub fn push_constants(&mut self, bytes: &[u8]) {
        assert!(
            bytes.len() <= PUSH_CONSTANTS as usize && bytes.len().is_multiple_of(4),
            "push constants are up to {PUSH_CONSTANTS} bytes, in 4-byte words; got {}",
            bytes.len()
        );
        let device = &self.frame.frames.device;
        // SAFETY: the commands are recording, and the range lies within the layout's.
        unsafe {
            device.raw().cmd_push_constants(
                self.commands(),
                device.bindless().pipeline_layout(),
                vk::ShaderStageFlags::ALL,
                0,
                bytes,
            );
        }
    }

    /// Draws `vertices` vertices, `instances` times, with the pipeline set last: the vertex shader
    /// sees each vertex's index, from 0, as `SV_VulkanVertexID`, and each instance's as
    /// `SV_VulkanInstanceID`.
    ///
    /// # Panics
    ///
    /// If no pipeline is set.
    pub fn draw(&mut self, vertices: u32, instances: u32) {
        assert!(self.pipeline_set, "a draw needs a pipeline set first");
        // SAFETY: the commands are recording inside a pass with a pipeline set.
        unsafe {
            self.frame
                .frames
                .device
                .raw()
                .cmd_draw(self.commands(), vertices, instances, 0, 0);
        }
    }

    /// Draws `count` of `indices`, starting `first` indices in, `instances` times, with the
    /// pipeline set last: the vertex shader sees each instance's index, from 0, as
    /// `SV_VulkanInstanceID`. Each index is offset by `vertex_offset` before the vertex shader
    /// sees it.
    ///
    /// # Panics
    ///
    /// If no pipeline is set, or the indices drawn run past the end of `indices`.
    pub fn draw_indexed<'i>(
        &mut self,
        indices: impl Into<Indices<'i>>,
        first: u32,
        count: u32,
        vertex_offset: i32,
        instances: u32,
    ) {
        let indices = indices.into();
        assert!(self.pipeline_set, "a draw needs a pipeline set first");
        assert!(
            u64::from(first) + u64::from(count) <= indices.count,
            "{count} indices from index {first} run past the {} there are",
            indices.count
        );
        // SAFETY: the commands are recording inside a pass with a pipeline set, the indices
        // drawn are within their buffer, and it lives until the frame has finished: an upload's
        // arena is reset only after that, and a GPU buffer retires until then.
        unsafe {
            let raw = self.frame.frames.device.raw();
            raw.cmd_bind_index_buffer(
                self.commands(),
                indices.buffer,
                indices.offset,
                vk::IndexType::UINT32,
            );
            raw.cmd_draw_indexed(self.commands(), count, instances, first, vertex_offset, 0);
        }
    }

    fn commands(&self) -> vk::CommandBuffer {
        self.frame.frames.slots[self.frame.slot].commands
    }
}

impl Drop for Pass<'_, '_> {
    fn drop(&mut self) {
        let device = &self.frame.frames.device;
        // SAFETY: the commands are recording inside the pass this ends, and its label.
        unsafe {
            device.raw().cmd_end_rendering(self.commands());
            device.end_label(self.commands());
        }
    }
}

/// Bytes in a frame's arena, valid for the GPU during that frame, whose [`Frames`] it borrows.
#[derive(Clone, Copy, Debug)]
pub struct Upload<'a> {
    buffer: vk::Buffer,
    offset: u64,
    size: u64,
    address: vk::DeviceAddress,
    frames: PhantomData<&'a ()>,
}

impl Upload<'_> {
    /// Where shaders read the bytes, through a buffer pointer.
    pub fn address(&self) -> u64 {
        self.address
    }
}

/// The 32-bit indices a draw takes: an [`Upload`] of the frame's, or a [`GpuBuffer`] from its
/// start.
#[derive(Clone, Copy, Debug)]
pub struct Indices<'a> {
    buffer: vk::Buffer,
    offset: u64,
    /// How many indices there are.
    count: u64,
    /// The indices borrow what holds them, so that it outlives the draw's recording.
    holder: PhantomData<&'a ()>,
}

impl<'a> From<Upload<'a>> for Indices<'a> {
    fn from(upload: Upload<'a>) -> Self {
        Self {
            buffer: upload.buffer,
            offset: upload.offset,
            count: upload.size / 4,
            holder: PhantomData,
        }
    }
}

impl<'a> From<&'a GpuBuffer> for Indices<'a> {
    fn from(buffer: &'a GpuBuffer) -> Self {
        Self {
            buffer: buffer.raw(),
            offset: 0,
            count: buffer.size() / 4,
            holder: PhantomData,
        }
    }
}

/// Memory the CPU writes during a frame for the GPU to read in it, allocated by bumping an
/// offset. A frame's own arena holds geometry, data shaders read, and pixels on their way to
/// textures; it's reset when the frame's slot comes round again, and grows by adding buffers,
/// which the reset merges into one big enough for them all, so frames stop allocating once they
/// reach a steady state. Staging holds what fills meshes and textures, and is emptied as each
/// frame is submitted instead, so that it holds nothing once they stop coming.
struct Arena {
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
    fn frame(slot: u64) -> Self {
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
    fn staging() -> Self {
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
    fn reset(&mut self, device: &Arc<Device>) -> Result<()> {
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
    fn clear(&mut self) {
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
    fn flush(&self) -> Result<()> {
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
