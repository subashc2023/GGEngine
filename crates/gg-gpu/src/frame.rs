//! Frames in flight. Each frame records one command buffer and submits it, and a timeline
//! semaphore paces them, so that the CPU gets at most `FRAMES_IN_FLIGHT` frames ahead of the GPU.
//! A frame can render into several targets, such as an app's windows, and presents all their
//! swapchains at once.

use std::marker::PhantomData;
use std::slice;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;
use gg_core::Result;
use tracing::error;

use crate::bindless::PUSH_CONSTANTS;
use crate::buffer::Buffer;
use crate::vma::Memory;
use crate::{
    COLOR_RANGE, ColorImage, Device, Format, GpuBuffer, Pipeline, Swapchain, Texture, vk_error,
};

/// How many frames the GPU works on at once.
const FRAMES_IN_FLIGHT: u64 = 2;

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
            arenas: (0..FRAMES_IN_FLIGHT).map(Arena::new).collect(),
            images: Vec::new(),
            spare_targets: Vec::new(),
            scratch: Scratch::default(),
            waited: Duration::ZERO,
        };
        // SAFETY: the semaphore is live.
        unsafe { device.set_name(timeline, "frame timeline") };
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
            }
        }
        Ok(frames)
    }

    /// Starts a frame that renders into `targets`, once the GPU has finished the frame that last
    /// used this one's resources. The first target paces the frame: for a swapchain, it waits
    /// until the display has let go of an image. The others are rendered into only if their
    /// swapchains have an image to give at once, so that a window on a slower display doesn't
    /// slow the frame. Returns `None` if the first target has nothing to render into this time,
    /// as with the swapchain of a minimized window.
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
        }))
    }

    /// How long the last [`Frames::begin`] waited: for the GPU to finish the frame that last used
    /// this one's resources, and for the display to let go of the first target's image.
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// Waits until the GPU has finished the `frame`th frame.
    fn wait(&self, frame: u64) -> Result<()> {
        let _zone = gg_core::zone!("wait for the GPU");
        let semaphores = [self.timeline];
        let values = [frame];
        let info = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        // SAFETY: the timeline semaphore is live.
        unsafe { self.device.raw().wait_semaphores(&info, u64::MAX) }
            .map_err(|e| vk_error("vkWaitSemaphores", e))
    }
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
        if self.wait(self.submitted).is_err() {
            let _ = self.device.wait_idle();
        }
        self.device.frames_finished(self.submitted);
        let raw = self.device.raw();
        // SAFETY: the GPU has finished every frame, so nothing here is in use. Null handles,
        // left by a failed `new`, are allowed.
        unsafe {
            for slot in &self.slots {
                for &semaphore in &slot.acquired {
                    raw.destroy_semaphore(semaphore, None);
                }
                raw.destroy_command_pool(slot.pool, None);
            }
            raw.destroy_semaphore(self.timeline, None);
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
    /// the new pixels from this frame's passes on.
    ///
    /// # Panics
    ///
    /// If the region doesn't fit in the texture, or `pixels` doesn't fill it exactly.
    pub fn write_texture(
        &mut self,
        texture: &mut Texture,
        origin: [u32; 2],
        size: [u32; 2],
        pixels: &[u8],
    ) -> Result<()> {
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
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        // Earlier frames on this queue may still sample the texture; the copy waits for them.
        let (old_layout, src_stage) = if texture.written() {
            (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
            )
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
        let region = vk::BufferImageCopy::default()
            .buffer_offset(staged.offset)
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_offset(vk::Offset3D {
                x: origin[0] as i32,
                y: origin[1] as i32,
                z: 0,
            })
            .image_extent(vk::Extent3D {
                width: size[0],
                height: size[1],
                depth: 1,
            });
        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(texture.raw())
            .subresource_range(COLOR_RANGE);
        let raw = device.raw();
        // SAFETY: the commands are recording outside a pass, the region fits the texture, and
        // the staged pixels and the texture live until the frame has finished: the arena is
        // reset only after that, and the texture retires until then.
        unsafe {
            device.begin_label(commands, c"write texture");
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&to_copy)),
            );
            raw.cmd_copy_buffer_to_image(
                commands,
                staged.buffer,
                texture.raw(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&to_read)),
            );
            device.end_label(commands);
        }
        texture.set_written();
        Ok(())
    }

    /// Writes `bytes` into `buffer`, starting `offset` bytes in. The buffer's other bytes keep
    /// what they had. Shaders and draws see the new bytes from this frame's passes on.
    ///
    /// # Panics
    ///
    /// If the bytes don't fit in the buffer.
    pub fn write_buffer(
        &mut self,
        buffer: &mut GpuBuffer,
        offset: u64,
        bytes: &[u8],
    ) -> Result<()> {
        let size = bytes.len() as u64;
        assert!(
            offset
                .checked_add(size)
                .is_some_and(|end| end <= buffer.size()),
            "{size} bytes at {offset} overflow a buffer of {}",
            buffer.size()
        );
        // A copy has at least a byte.
        if bytes.is_empty() {
            return Ok(());
        }
        let staged = self.upload(bytes)?;
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        // What reads a buffer during a frame: shaders, through its address, and draws' indices.
        let readers = vk::PipelineStageFlags2::VERTEX_SHADER
            | vk::PipelineStageFlags2::FRAGMENT_SHADER
            | vk::PipelineStageFlags2::INDEX_INPUT;
        // Earlier frames on this queue may still read the buffer, or copy into it; the copy waits
        // for them.
        let to_copy = vk::MemoryBarrier2::default()
            .src_stage_mask(readers | vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE);
        let region = vk::BufferCopy {
            src_offset: staged.offset,
            dst_offset: offset,
            size,
        };
        let to_read = vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(readers)
            .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_READ | vk::AccessFlags2::INDEX_READ);
        let raw = device.raw();
        // SAFETY: the commands are recording outside a pass, the region fits the buffer, and the
        // staged bytes and the buffer live until the frame has finished: the arena is reset only
        // after that, and the buffer retires until then.
        unsafe {
            device.begin_label(commands, c"write buffer");
            if buffer.written() {
                raw.cmd_pipeline_barrier2(
                    commands,
                    &vk::DependencyInfo::default().memory_barriers(slice::from_ref(&to_copy)),
                );
            }
            raw.cmd_copy_buffer(commands, staged.buffer, buffer.raw(), &[region]);
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().memory_barriers(slice::from_ref(&to_read)),
            );
            device.end_label(commands);
        }
        buffer.set_written();
        Ok(())
    }

    /// Starts rendering into the `target`th of the targets the frame began with, cleared to
    /// `color`, in linear RGBA. `label` names the pass in captures. Rendering ends when the pass
    /// is dropped. Returns `None` for a swapchain that had no image ready this frame; the first
    /// target always has one.
    ///
    /// # Panics
    ///
    /// If the frame has no `target`th target.
    pub fn begin_pass(
        &mut self,
        target: usize,
        label: &std::ffi::CStr,
        color: [f32; 4],
    ) -> Option<Pass<'_, 'a>> {
        let (index, _) = self.frames.images[target]?;
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        let ((image, view, extent), format) = match &self.targets[target] {
            Target::Swapchain(swapchain) => (swapchain.image(index), swapchain.format()),
            Target::Image(image) => (image.parts(), image.format()),
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
        let attachment = vk::RenderingAttachmentInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue { float32: color },
            });
        let rendering = vk::RenderingInfo::default()
            .render_area(extent.into())
            .layer_count(1)
            .color_attachments(slice::from_ref(&attachment));
        let viewport = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let raw = device.raw();
        // SAFETY: the commands are recording outside a pass, and the image and view live until
        // the frame has finished on the GPU: the swapchain waits for its frames before
        // destroying its images, and an offscreen image retires until then.
        unsafe {
            device.begin_label(commands, label);
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&barrier)),
            );
            raw.cmd_begin_rendering(commands, &rendering);
            raw.cmd_set_viewport(commands, 0, &[viewport]);
            raw.cmd_set_scissor(commands, 0, &[extent.into()]);
        }
        self.frames.images[target] = Some((index, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL));
        Some(Pass {
            frame: self,
            extent,
            format,
            pipeline_set: false,
        })
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
            images,
            scratch,
            ..
        } = &mut *self.frames;
        arenas[self.slot].flush()?;
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
        // SAFETY: the commands are recording.
        unsafe { raw.end_command_buffer(slot.commands) }
            .map_err(|e| vk_error("vkEndCommandBuffer", e))?;

        let command_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(slot.commands)];
        let submit = vk::SubmitInfo2::default()
            .wait_semaphore_infos(&scratch.waits)
            .command_buffer_infos(&command_infos)
            .signal_semaphore_infos(&scratch.signals);
        {
            let _zone = gg_core::zone!("submit");
            // SAFETY: what the submission uses lives until the timeline reaches `frame`: the slot
            // and its arena are reused only after waiting for it, and resources retire until then.
            unsafe { device.submit(slice::from_ref(&submit)) }?;
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

    /// The target's format, which the pipelines it draws with must be made for.
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
    /// If the pipeline isn't made for the target's format.
    pub fn set_pipeline(&mut self, pipeline: &Pipeline) {
        assert_eq!(
            pipeline.format(),
            self.format,
            "a pipeline is made for the format of the target it draws into"
        );
        self.pipeline_set = true;
        // SAFETY: the commands are recording inside a pass whose target has the pipeline's
        // format, and the pipeline retires until the frame has finished.
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
/// offset: geometry, data shaders read, and pixels on their way to textures. It's reset when
/// the frame's slot comes round again, and grows by adding buffers, which the reset merges
/// into one big enough for them all, so frames stop allocating once they reach a steady state.
struct Arena {
    slot: u64,
    /// The newest buffer last; only it has room.
    buffers: Vec<Buffer>,
    /// How much of the newest buffer is used.
    used: u64,
}

impl Arena {
    /// What a new arena's first buffer holds.
    const FIRST_SIZE: u64 = 1 << 20;
    /// Every upload starts at a multiple of this, which covers index buffers, texture copies and
    /// the data types shaders read.
    const ALIGNMENT: u64 = 16;

    fn new(slot: u64) -> Self {
        Self {
            slot,
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

    fn upload<'a>(&mut self, device: &Arc<Device>, bytes: &[u8]) -> Result<Upload<'a>> {
        let size = bytes.len() as u64;
        let mut offset = self.used.next_multiple_of(Self::ALIGNMENT);
        let room = self.buffers.last().map_or(0, Buffer::size);
        if self.buffers.is_empty() || offset + size > room {
            self.grow(device, (room * 2).max(size).max(Self::FIRST_SIZE))?;
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
        let usage = vk::BufferUsageFlags::INDEX_BUFFER
            | vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
            | vk::BufferUsageFlags::TRANSFER_SRC;
        let name = format!("frame {} arena", self.slot);
        self.buffers
            .push(Buffer::new(device, size, usage, Memory::Upload, &name)?);
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
