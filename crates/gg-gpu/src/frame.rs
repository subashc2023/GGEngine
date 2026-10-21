//! Frames in flight. Each frame records one command buffer and submits it, and a timeline
//! semaphore paces them, so that the CPU gets at most `FRAMES_IN_FLIGHT` frames ahead of the GPU.

use std::slice;
use std::sync::Arc;

use ash::vk;
use gg_core::Result;
use tracing::error;

use crate::{COLOR_RANGE, ColorImage, Device, Swapchain, vk_error};

/// How many frames the GPU works on at once.
const FRAMES_IN_FLIGHT: u64 = 2;

/// What a frame renders into.
pub enum Target<'a> {
    /// The next image of a window's swapchain, presented when the frame is submitted.
    Swapchain(&'a mut Swapchain),
    /// An offscreen image.
    Image(&'a mut ColorImage),
}

/// The resources of the frames in flight, and the timeline semaphore that paces them.
pub struct Frames {
    device: Arc<Device>,
    /// Reaches N when the Nth frame has finished on the GPU.
    timeline: vk::Semaphore,
    submitted: u64,
    slots: Vec<Slot>,
}

/// What one frame in flight records into and synchronizes with.
#[derive(Clone, Copy)]
struct Slot {
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    /// Signaled when the swapchain image the frame renders into is ready for it.
    acquired: vk::Semaphore,
}

impl Frames {
    pub fn new(device: &Arc<Device>) -> Result<Self> {
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
                    acquired: vk::Semaphore::null(),
                });
                let slot = frames.slots.last_mut().expect("a slot was just pushed");
                let allocate_info = vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1);
                slot.commands = raw
                    .allocate_command_buffers(&allocate_info)
                    .map_err(|e| vk_error("vkAllocateCommandBuffers", e))?[0];
                slot.acquired = raw
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                    .map_err(|e| vk_error("vkCreateSemaphore", e))?;
                device.set_name(pool, &format!("frame {i} commands"));
                device.set_name(slot.commands, &format!("frame {i} commands"));
                device.set_name(slot.acquired, &format!("frame {i} image acquired"));
            }
        }
        Ok(frames)
    }

    /// Starts a frame that renders into `target`, once the GPU has finished the frame that last
    /// used this one's resources. Returns `None` if there's nothing to render into this time,
    /// as with the swapchain of a minimized window.
    pub fn begin<'a>(&'a mut self, mut target: Target<'a>) -> Result<Option<Frame<'a>>> {
        let slot = (self.submitted % FRAMES_IN_FLIGHT) as usize;
        if let Some(previous) = self.submitted.checked_sub(FRAMES_IN_FLIGHT) {
            self.wait(previous + 1)?;
        }
        let Slot {
            pool,
            commands,
            acquired,
        } = self.slots[slot];
        let (image, layout) = match &mut target {
            // SAFETY: the semaphore's last wait was in the frame that last used this slot, which
            // has finished, so it's unsignaled with no wait pending.
            Target::Swapchain(swapchain) => match unsafe { swapchain.acquire(acquired) }? {
                Some(image) => (image, vk::ImageLayout::UNDEFINED),
                None => return Ok(None),
            },
            Target::Image(image) => (0, image.layout()),
        };
        let raw = self.device.raw();
        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the GPU has finished the slot's previous frame, so its commands can be reset
        // and recorded again.
        unsafe {
            raw.reset_command_pool(pool, vk::CommandPoolResetFlags::empty())
                .map_err(|e| vk_error("vkResetCommandPool", e))?;
            raw.begin_command_buffer(commands, &begin_info)
                .map_err(|e| vk_error("vkBeginCommandBuffer", e))?;
        }
        Ok(Some(Frame {
            frames: self,
            target,
            slot,
            image,
            layout,
            submitted: false,
        }))
    }

    /// Waits until the GPU has finished the `frame`th frame.
    fn wait(&self, frame: u64) -> Result<()> {
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

impl Drop for Frames {
    fn drop(&mut self) {
        if self.wait(self.submitted).is_err() {
            let _ = self.device.wait_idle();
        }
        let raw = self.device.raw();
        // SAFETY: the GPU has finished every frame, so nothing here is in use. Null handles,
        // left by a failed `new`, are allowed.
        unsafe {
            for slot in &self.slots {
                raw.destroy_semaphore(slot.acquired, None);
                raw.destroy_command_pool(slot.pool, None);
            }
            raw.destroy_semaphore(self.timeline, None);
        }
    }
}

/// A frame being recorded. Submitting it, or dropping it, sends its commands to the GPU and, for
/// a swapchain, presents the image.
#[must_use = "a frame's commands reach the GPU when it's submitted"]
pub struct Frame<'a> {
    frames: &'a mut Frames,
    target: Target<'a>,
    slot: usize,
    /// The swapchain image the frame renders into, or 0 for an offscreen image.
    image: u32,
    /// The layout the frame's commands so far leave the target in.
    layout: vk::ImageLayout,
    submitted: bool,
}

impl Frame<'_> {
    /// Clears the whole target to `color`, in linear RGBA.
    pub fn clear(&mut self, color: [f32; 4]) {
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        let (image, view, extent) = match &self.target {
            Target::Swapchain(swapchain) => swapchain.image(self.image),
            Target::Image(image) => image.parts(),
        };
        // The old contents are discarded. A swapchain image is ready once its acquire semaphore
        // is signaled, which the submission waits for at the color-attachment stage; an
        // offscreen image must wait for whatever used it before.
        let (src_stage, src_access) = match self.target {
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
        let raw = device.raw();
        // SAFETY: the commands are recording, and the image and view live until the frame has
        // finished on the GPU: their owners wait for the GPU before destroying them.
        unsafe {
            device.begin_label(commands, c"clear");
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&barrier)),
            );
            raw.cmd_begin_rendering(commands, &rendering);
            raw.cmd_end_rendering(commands);
            device.end_label(commands);
        }
        self.layout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
    }

    /// Sends the frame's commands to the GPU and, for a swapchain, presents the image.
    pub fn submit(mut self) -> Result<()> {
        self.finish()
    }

    fn finish(&mut self) -> Result<()> {
        // Set first, so that dropping the frame after a failure doesn't submit it again.
        self.submitted = true;
        let frames = &mut *self.frames;
        let device = &frames.device;
        let raw = device.raw();
        let Slot {
            commands, acquired, ..
        } = frames.slots[self.slot];
        if let Target::Swapchain(swapchain) = &self.target {
            let (image, _, _) = swapchain.image(self.image);
            // The present waits on a semaphore signaled after every command, the layout change
            // included, so the barrier needs no second scope.
            let barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .old_layout(self.layout)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .image(image)
                .subresource_range(COLOR_RANGE);
            // SAFETY: the commands are recording, and the image is live.
            unsafe {
                raw.cmd_pipeline_barrier2(
                    commands,
                    &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&barrier)),
                );
            }
            self.layout = vk::ImageLayout::PRESENT_SRC_KHR;
        }
        // SAFETY: the commands are recording.
        unsafe { raw.end_command_buffer(commands) }
            .map_err(|e| vk_error("vkEndCommandBuffer", e))?;

        let frame = frames.submitted + 1;
        let command_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(commands)];
        let waits = [vk::SemaphoreSubmitInfo::default()
            .semaphore(acquired)
            .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
        let rendered = match &self.target {
            Target::Swapchain(swapchain) => swapchain.rendered(self.image),
            Target::Image(_) => vk::Semaphore::null(),
        };
        let signals = [
            vk::SemaphoreSubmitInfo::default()
                .semaphore(frames.timeline)
                .value(frame)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
            vk::SemaphoreSubmitInfo::default()
                .semaphore(rendered)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
        ];
        // Only a swapchain image has semaphores to wait on and signal besides the timeline.
        let (waits, signals) = match self.target {
            Target::Swapchain(_) => (&waits[..], &signals[..]),
            Target::Image(_) => (&[][..], &signals[..1]),
        };
        let submit = vk::SubmitInfo2::default()
            .wait_semaphore_infos(waits)
            .command_buffer_infos(&command_infos)
            .signal_semaphore_infos(signals);
        // SAFETY: what the submission uses lives until the timeline reaches `frame`: the slot is
        // reused only after waiting for it, and the target's owner waits for the GPU before
        // destroying it.
        unsafe { device.submit(slice::from_ref(&submit)) }?;
        frames.submitted = frame;
        match &mut self.target {
            // SAFETY: the image was acquired for this frame, whose submission signals its
            // semaphore.
            Target::Swapchain(swapchain) => unsafe { swapchain.present(self.image) },
            Target::Image(image) => {
                image.set_layout(self.layout);
                Ok(())
            }
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
    }
}
