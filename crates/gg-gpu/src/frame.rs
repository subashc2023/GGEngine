//! Frames in flight. Each frame records its commands and submits them, and a timeline semaphore
//! paces them, so that the CPU gets at most `FRAMES_IN_FLIGHT` frames ahead of the GPU. A frame
//! can render into several targets, such as an app's windows, and presents all their swapchains at
//! once; its later passes can sample what its earlier ones rendered offscreen. It reads back a few
//! pixels of the IDs its passes render, which the frame two later, which reuses its resources once
//! it has finished, finds read. On a device that uploads through a transfer queue, a frame's copies
//! go in a submission of their own, which another timeline paces, and no frame waits for.

mod pass;
mod upload;

use std::ffi::{CStr, CString};
use std::slice;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;
use gg_core::{Error, Result};
use tracing::error;

use crate::buffer::Buffer;
use crate::vma::Memory;
use crate::{COLOR_RANGE, ColorImage, Device, IdImage, Swapchain, copy_region, vk_error};
use upload::Arena;

pub use pass::{Indices, Pass};
pub use upload::Upload;

/// How many frames the GPU works on at once.
pub(crate) const FRAMES_IN_FLIGHT: u64 = 2;

/// The most IDs a frame reads back ([`Frame::read_ids`]).
pub const MAX_READS: usize = 64;

/// The bytes of an ID read back: two 32-bit unsigned integers.
const ID_BYTES: usize = size_of::<[u32; 2]>();

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
    /// How many times copies have been submitted, counted apart from frames, since a frame may
    /// submit none.
    transfers_submitted: u64,
    /// The command buffers copies record into, each with the submission it was last in: a frame
    /// takes one whose submission has finished, or makes another, so that none waits.
    transfers: Vec<TransferCommands>,
    /// What the transfer timeline had reached as the frame being recorded began: the copies
    /// submitted up to then have finished.
    transfers_finished: u64,
    /// The submission of copies the frame being recorded waits on, the last of those that filled
    /// what it takes over from the transfer queue; 0 for none.
    transfer_wait: u64,
    /// What frames read IDs back into, [`MAX_READS`] of them for each slot, in the slots' order:
    /// made by the first read, so that apps that read none hold none.
    reads: Option<Buffer>,
    /// What the last frame to use the slot of the frame being begun read back, as that one finds
    /// it: [`Frame::ids_read`]'s.
    read: Read,
    /// What each slot's frames read offscreen images back into ([`Frame::read_image`]), in the
    /// slots' order.
    image_reads: Vec<ImageReads>,
    /// For each target of the frame being recorded, its image: the swapchain image's index, or 0
    /// for an offscreen image, and the layout the frame's commands so far leave it in. `None` for
    /// a swapchain that had no image ready.
    images: Vec<Option<(u32, vk::ImageLayout)>>,
    /// The last frame's list of targets, emptied, whose memory the next frame reuses.
    spare_targets: Vec<Target<'static>>,
    scratch: Scratch,
    /// How long the last `begin` waited, which [`Frames::waited`] describes.
    waited: Duration,
    /// Whether a frame begun is still being recorded: cleared as it's submitted or dropped, so
    /// that one forgotten instead is caught as the next begins.
    recording: bool,
    /// Whether a frame failed after acquiring an image, or to submit or present, which can leave
    /// the semaphores its acquires signaled with no wait pending: no frame begins after that.
    failed: bool,
}

/// What one frame in flight records into and synchronizes with.
struct Slot {
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    /// The pool's and the commands' name, kept to name the commands again as they're begun,
    /// without allocating.
    commands_name: CString,
    /// For each target, signaled when the swapchain image the frame renders into is ready for
    /// it. There are as many as the most targets a frame has had.
    acquired: Vec<vk::Semaphore>,
    /// The number of the frame that last began in the slot, 0 for none, and how many IDs it read
    /// back into the slot's part of `Frames::reads`.
    reads: (u64, usize),
}

/// A command buffer that copies on the transfer queue record into, for one submission at a time.
struct TransferCommands {
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    /// The pool's and the commands' name, kept to name the commands again as they're begun,
    /// without allocating.
    name: CString,
    /// The submission of copies it was last in; 0 for none. It's free once the transfer timeline
    /// has reached it.
    submitted: u64,
}

/// The IDs a frame read back, as the next frame in its slot found them once it had finished.
struct Read {
    /// The frame's number; 0 for none.
    frame: u64,
    /// How many of `values` it read.
    count: usize,
    values: [[u32; 2]; MAX_READS],
}

/// The buffer a slot's frames read an offscreen image back into, made, or made larger, by the
/// first read that needs it, and what the last of them read.
#[derive(Default)]
struct ImageReads {
    buffer: Option<Buffer>,
    /// The number of the frame that read into it last, 0 for none, and the image's size.
    frame: u64,
    size: [u32; 2],
}

/// An offscreen image as a frame read it back ([`Frame::read_image`]), once the frame has
/// finished, which [`Frames::image_read`] gives.
pub struct ImageRead<'a> {
    size: [u32; 2],
    buffer: &'a Buffer,
}

impl ImageRead<'_> {
    /// The image's width and height in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Copies the image's pixels into `out`, without allocating: rows of RGBA8, sRGB-encoded and
    /// tightly packed, top row first.
    ///
    /// # Panics
    ///
    /// If `out` doesn't hold exactly the pixels' bytes.
    pub fn read_into(&self, out: &mut [u8]) -> Result<()> {
        let [width, height] = self.size;
        assert_eq!(
            out.len() as u64,
            u64::from(width) * u64::from(height) * 4,
            "a {width}x{height} image's pixels fill what they're read into"
        );
        self.buffer.read_into(0, out)
    }
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
    /// Makes the frames that render with `device`, and their resources in flight.
    ///
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
            .map_err(|e| vk_error("vkCreateSemaphore", "semaphore \"frame timeline\"", e))?;
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
            transfers: Vec::new(),
            transfers_finished: 0,
            transfer_wait: 0,
            reads: None,
            read: Read {
                frame: 0,
                count: 0,
                values: [[0; 2]; MAX_READS],
            },
            image_reads: (0..FRAMES_IN_FLIGHT)
                .map(|_| ImageReads::default())
                .collect(),
            images: Vec::new(),
            spare_targets: Vec::new(),
            scratch: Scratch::default(),
            waited: Duration::ZERO,
            recording: false,
            failed: false,
        };
        // SAFETY: the semaphore is live.
        unsafe { device.set_name(timeline, "frame timeline") };
        let transfer_family = device.transfer_family();
        if transfer_family.is_some() {
            // SAFETY: as above.
            frames.transfer_timeline = unsafe { raw.create_semaphore(&info, None) }
                .map_err(|e| vk_error("vkCreateSemaphore", "semaphore \"upload timeline\"", e))?;
            // SAFETY: the semaphore is live.
            unsafe { device.set_name(frames.transfer_timeline, "upload timeline") };
        }
        for i in 0..FRAMES_IN_FLIGHT {
            let name = format!("frame {i} commands");
            let (pool, commands, commands_name) =
                command_buffer(device, device.graphics_family(), name)?;
            frames.slots.push(Slot {
                pool,
                commands,
                commands_name,
                acquired: Vec::new(),
                reads: (0, 0),
            });
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
    /// Once the frame that last used this one's resources has finished, it copies what that frame
    /// read back ([`Frame::read_ids`]), which the frame begun gives ([`Frame::ids_read`]). A frame
    /// that doesn't begin leaves them for the next that does: the slot forgets them only as one
    /// begins.
    ///
    /// Fails with `gpu.frame_failed` once a frame has failed after acquiring an image, or to
    /// submit or present: the semaphores its acquires signaled may have no wait pending, so no
    /// frame can begin again, and the frames can only be dropped.
    ///
    /// # Panics
    ///
    /// If `targets` is empty or holds one another device made, or the last frame was forgotten
    /// rather than submitted or dropped.
    pub fn begin<'a>(
        &'a mut self,
        targets: impl IntoIterator<Item = Target<'a>>,
    ) -> Result<Option<Frame<'a>>> {
        assert!(
            !self.recording,
            "frame {} was forgotten rather than submitted or dropped, so its images were never \
             presented and the semaphores its acquires signaled can't be used again",
            self.submitted + 1
        );
        if self.failed {
            return Err(Error::new(
                "gpu.frame_failed",
                "a frame failed after acquiring an image, or to submit or present, which can \
                 leave its semaphores signaled, so no frame begins after it; the frames can only \
                 be dropped",
            ));
        }
        let slot = (self.submitted % FRAMES_IN_FLIGHT) as usize;
        self.waited = Duration::ZERO;
        if let Some(previous) = self.submitted.checked_sub(FRAMES_IN_FLIGHT) {
            let start = Instant::now();
            self.wait(previous + 1)?;
            self.waited = start.elapsed();
            self.device.frames_finished(previous + 1);
        }
        if self.transfer_timeline != vk::Semaphore::null() {
            // SAFETY: the timeline semaphore is live.
            let finished = unsafe {
                self.device
                    .raw()
                    .get_semaphore_counter_value(self.transfer_timeline)
            }
            .map_err(|e| {
                vk_error(
                    "vkGetSemaphoreCounterValue",
                    "semaphore \"upload timeline\"",
                    e,
                )
            })?;
            self.transfers_finished = finished;
            self.device.transfers_finished(finished);
        }
        self.transfer_wait = 0;
        // SAFETY: the slot's last frame has finished, so no submitted frame that the GPU may still
        // run uses its set.
        unsafe { self.device.bindless().apply(self.device.raw(), slot) };
        // The slot's last frame has finished, and its reads with it.
        self.find_read(slot)?;
        let mut list: Vec<Target<'a>> = std::mem::take(&mut self.spare_targets);
        list.extend(targets);
        assert!(!list.is_empty(), "a frame renders into at least one target");
        // Before any image is acquired, which a panic would leave unpresented.
        for target in &list {
            match target {
                Target::Swapchain(swapchain) => self
                    .device
                    .check_made("a swapchain among the targets", swapchain.device()),
                Target::Image(image) => self
                    .device
                    .check_made("an offscreen image among the targets", image.device()),
            }
        }
        let acquired = &mut self.slots[slot].acquired;
        while acquired.len() < list.len() {
            let name = format!("frame {slot} target {} image acquired", acquired.len());
            // SAFETY: the device is live.
            let semaphore = unsafe {
                self.device
                    .raw()
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|e| vk_error("vkCreateSemaphore", format_args!("semaphore {name:?}"), e))?;
            // SAFETY: the semaphore is live.
            unsafe { self.device.set_name(semaphore, &name) };
            acquired.push(semaphore);
        }
        self.images.clear();
        if let Err(error) = self.start(slot, &mut list) {
            // A swapchain image acquired before the failure goes unpresented, and its semaphore
            // stays signaled, so no frame may acquire with it again.
            self.failed |= (list.iter().zip(&self.images))
                .any(|(target, image)| matches!(target, Target::Swapchain(_)) && image.is_some());
            self.spare_targets = recycle(list);
            return Err(error);
        }
        if self.images.is_empty() {
            self.spare_targets = recycle(list);
            return Ok(None);
        }
        self.recording = true;
        self.slots[slot].reads = (self.submitted + 1, 0);
        Ok(Some(Frame {
            frames: self,
            targets: list,
            slot,
            submitted: false,
            transfers: None,
            rendering: false,
            pass_begun: false,
        }))
    }

    /// Acquires `targets`' images into `images`, as [`Frames::begin`] says, and, given one for
    /// the first, readies slot `slot`'s arena and commands for the frame and begins recording
    /// them. `images` stays empty if the first target has no image.
    fn start(&mut self, slot: usize, targets: &mut [Target<'_>]) -> Result<()> {
        for (i, target) in targets.iter_mut().enumerate() {
            let acquired = self.slots[slot].acquired[i];
            let image = match target {
                Target::Swapchain(swapchain) => {
                    // SAFETY: the semaphore is new, or the frame that last used this slot waited
                    // on it or left it unsignaled and has finished, so it's unsignaled with no
                    // wait pending: a frame forgotten or failed, which could have left it
                    // signaled, keeps frames from beginning.
                    unsafe { swapchain.acquire(acquired, i == 0, &mut self.waited) }?
                        .map(|image| (image, vk::ImageLayout::UNDEFINED))
                }
                Target::Image(image) => Some((0, image.layout())),
            };
            if i == 0 && image.is_none() {
                return Ok(());
            }
            self.images.push(image);
        }
        self.device.frame_begun(self.submitted + 1);
        self.arenas[slot].reset(&self.device)?;
        let raw = self.device.raw();
        let Slot {
            pool,
            commands,
            ref commands_name,
            ..
        } = self.slots[slot];
        let bindless = self.device.bindless();
        // SAFETY: the GPU has finished the slot's previous frame, so its commands can be reset
        // and recorded again; the sets and layout live as long as the device.
        unsafe {
            begin_recording(&self.device, pool, commands, commands_name)?;
            raw.cmd_bind_descriptor_sets(
                commands,
                vk::PipelineBindPoint::GRAPHICS,
                bindless.pipeline_layout(),
                0,
                &[bindless.set(slot)],
                &[],
            );
        }
        Ok(())
    }

    /// Copies what slot `slot`'s last frame read back, once it has finished, for the frame begun
    /// in the slot to give.
    fn find_read(&mut self, slot: usize) -> Result<()> {
        let (frame, count) = self.slots[slot].reads;
        self.read.frame = frame;
        self.read.count = count;
        if count == 0 {
            return Ok(());
        }
        let reads = (self.reads.as_ref()).expect("a frame that read IDs back made their buffer");
        let mut bytes = [0; MAX_READS * ID_BYTES];
        let bytes = &mut bytes[..count * ID_BYTES];
        reads.read_into(read_offset(slot, 0), bytes)?;
        let (words, _) = bytes.as_chunks::<4>();
        let (read, _) = words.as_chunks::<2>();
        for (value, &[low, high]) in self.read.values.iter_mut().zip(read) {
            *value = [u32::from_ne_bytes(low), u32::from_ne_bytes(high)];
        }
        Ok(())
    }

    /// How long the last [`Frames::begin`] waited: for the GPU to finish the frame that last used
    /// this one's resources, and for the display to let go of the first target's image.
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// How many frames have been submitted: the number of the last, as frames count from 1. A
    /// [`Frames::begin`] that returns `None` begins none.
    pub fn submitted(&self) -> u64 {
        self.submitted
    }

    /// What the `frame`th frame read back of an offscreen image ([`Frame::read_image`]), once the
    /// GPU has finished that frame, which this waits for: `None` if it read none, wasn't
    /// submitted, or a frame since has read back into its slot's buffer, as the frame two after it
    /// may.
    pub fn image_read(&self, frame: u64) -> Result<Option<ImageRead<'_>>> {
        let Some(slot) = frame.checked_sub(1) else {
            return Ok(None);
        };
        let reads = &self.image_reads[(slot % FRAMES_IN_FLIGHT) as usize];
        let read =
            (reads.buffer.as_ref()).filter(|_| reads.frame == frame && frame <= self.submitted);
        let Some(buffer) = read else {
            return Ok(None);
        };
        self.wait(frame)?;
        Ok(Some(ImageRead {
            size: reads.size,
            buffer,
        }))
    }

    /// Waits until the GPU has finished the `frame`th frame.
    fn wait(&self, frame: u64) -> Result<()> {
        wait_on(&self.device, self.timeline, "frame timeline", frame)
    }
}

/// Waits until `timeline`, the semaphore named `name`, reaches `value`.
fn wait_on(device: &Device, timeline: vk::Semaphore, name: &str, value: u64) -> Result<()> {
    let _zone = gg_core::zone!("wait for the GPU");
    let semaphores = [timeline];
    let values = [value];
    let info = vk::SemaphoreWaitInfo::default()
        .semaphores(&semaphores)
        .values(&values);
    // SAFETY: the timeline semaphore is live.
    unsafe { device.raw().wait_semaphores(&info, u64::MAX) }.map_err(|e| {
        let reached = format_args!("semaphore {name:?} reaching {value}");
        vk_error("vkWaitSemaphores", reached, e)
    })
}

/// Where the `read`th ID that slot `slot`'s frame reads back goes in `Frames::reads`.
fn read_offset(slot: usize, read: usize) -> u64 {
    ((slot * MAX_READS + read) * ID_BYTES) as u64
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
        if self.recording {
            error!(
                code = "gpu.frame_forgotten",
                frame = self.submitted + 1,
                "A frame was forgotten rather than submitted or dropped, so its images were never presented"
            );
        }
        // The copies run apart from the frames, so both are waited for, but a wait can fail.
        let waited = self.wait(self.submitted).and_then(|()| {
            if self.transfers_submitted > 0 {
                wait_on(
                    &self.device,
                    self.transfer_timeline,
                    "upload timeline",
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
        self.device.transfers_finished(self.transfers_submitted);
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
            }
            for transfers in &self.transfers {
                raw.destroy_command_pool(transfers.pool, None);
            }
            raw.destroy_semaphore(self.timeline, None);
            raw.destroy_semaphore(self.transfer_timeline, None);
        }
    }
}

/// A frame being recorded. Submitting it, or dropping it, sends its commands to the GPU and
/// presents the images of its swapchains; forgetting it instead is caught as the next frame
/// begins.
#[must_use = "a frame's commands reach the GPU when it's submitted"]
pub struct Frame<'a> {
    frames: &'a mut Frames,
    targets: Vec<Target<'a>>,
    slot: usize,
    submitted: bool,
    /// The command buffer the frame's copies on the transfer queue record into, among
    /// `Frames::transfers`, once they've begun.
    transfers: Option<usize>,
    /// Whether a pass is open, which only its drop closes: the frame records nothing of its own
    /// while one is, so a pass forgotten rather than dropped is caught at the frame's next call.
    rendering: bool,
    /// Whether a pass has begun in the frame: a texture that comes to show an image takes it into
    /// its slot in the frame's bindless set at once, which every pass recorded before it would
    /// sample as the frame is submitted, before what makes the image ready, so it comes before
    /// them all.
    pass_begun: bool,
}

impl<'a> Frame<'a> {
    /// The frame's number: frames count from 1, as [`Frames::submitted`] counts them.
    pub fn number(&self) -> u64 {
        self.frames.submitted + 1
    }

    /// What the frame that last used this one's resources, two before it, read back
    /// ([`Frame::read_ids`]), which has finished: its number, and the IDs it read, in the order
    /// it read them; none if it read none, and 0 for the number of no frame, as before the first
    /// two. A frame that doesn't begin, as a minimized window's doesn't, leaves them for the next
    /// that does.
    pub fn ids_read(&self) -> (u64, &[[u32; 2]]) {
        let read = &self.frames.read;
        (read.frame, &read.values[..read.count])
    }

    /// Checks that no pass is open: only outside one does the frame record its own commands, or
    /// end them.
    ///
    /// # Panics
    ///
    /// If one is: a pass forgotten rather than dropped, whose rendering never ended.
    fn outside_pass(&self) {
        assert!(
            !self.rendering,
            "a pass was forgotten rather than dropped, so its rendering never ended; the frame can \
             neither record outside it nor be submitted"
        );
    }

    /// Lets the frame's later passes sample the `target`th target, as its last pass left it, from
    /// their fragment shaders, through the index this returns in the bindless set. Shaders may
    /// sample through the index only until the image's next pass, which renders into it and so
    /// takes it out of the layout the bindless set declares: after that pass, call this again.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, the frame has no `target`th target, the target is a
    /// swapchain's, or it's an offscreen image no pass has rendered into since it was made or
    /// read back.
    pub fn begin_sampling(&mut self, target: usize) -> u32 {
        self.outside_pass();
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
        // SAFETY: the commands are recording outside a pass, as checked, and the image lives until
        // the frame has finished: it retires until then.
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

    /// Lets the frame's later passes sample `ids`, as this frame's pass left them, from their
    /// fragment shaders, as unsigned integers, through the index this returns in the bindless set:
    /// once the frame has read back what it reads of them ([`Frame::read_ids`]). Shaders may sample
    /// through the index only until the image's next pass, which renders into it and so takes it
    /// out of the layout the bindless set declares: after that pass, call this again.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, another device made `ids`, or no pass of this frame has
    /// rendered into it.
    pub fn begin_sampling_ids(&mut self, ids: &mut IdImage) -> u32 {
        self.outside_pass();
        let number = self.number();
        let device = &self.frames.device;
        device.check_made("the ID image sampled", ids.device());
        assert!(
            ids.rendered() == number,
            "IDs are sampled in the frame whose pass rendered them, and no pass of frame {number} \
             has"
        );
        let (src_stage, src_access) = match ids.layout() {
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => return ids.index(),
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL => (
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            ),
            // Read back, by copies that wrote nothing of it.
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL => {
                (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::NONE)
            }
            layout => unreachable!("a frame's pass leaves its IDs in no layout such as {layout:?}"),
        };
        let (image, _, _) = ids.parts();
        let barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(src_stage)
            .src_access_mask(src_access)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(ids.layout())
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(COLOR_RANGE);
        let commands = self.frames.slots[self.slot].commands;
        // SAFETY: the commands are recording outside a pass, as checked, and the image lives until
        // the frame has finished: it retires until then.
        unsafe {
            device.begin_label(commands, c"begin sampling ids");
            device.raw().cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&barrier)),
            );
            device.end_label(commands);
        }
        ids.set_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        ids.index()
    }

    /// Reads the `target`th target back, as the frame's passes left it, for [`Frames::image_read`]
    /// to give once the frame has finished, and lets the frame's later passes sample it, as
    /// [`Frame::begin_sampling`] does, through the index this returns. The slot's first read, and
    /// a read of a larger image than it read before, makes the buffer it reads into, and fails if
    /// it can't be made.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, the frame has no `target`th target, the target is a
    /// swapchain's, or it's an offscreen image no pass has rendered into since it was made or
    /// read back.
    pub fn read_image(&mut self, target: usize) -> Result<u32> {
        self.outside_pass();
        let number = self.number();
        let Target::Image(image) = &self.targets[target] else {
            panic!("target {target} is a swapchain's, and only offscreen images are read back");
        };
        let Frames {
            device,
            slots,
            image_reads,
            images,
            ..
        } = &mut *self.frames;
        let (index, layout) = images[target].expect("an offscreen image is always there");
        let (src_stage, src_access) = match layout {
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL => (
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            ),
            // Sampled, which wrote nothing of it.
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => (
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::AccessFlags2::NONE,
            ),
            _ => panic!("target {target} is read back before a pass renders into it"),
        };
        let size = image.size();
        let bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
        let reads = &mut image_reads[self.slot];
        if reads
            .buffer
            .as_ref()
            .is_none_or(|buffer| buffer.size() < bytes)
        {
            // The slot's last frame has finished, and its read with it.
            let name = format!("frame {} image read", self.slot);
            let usage = vk::BufferUsageFlags::TRANSFER_DST;
            reads.buffer = Some(Buffer::new(device, bytes, usage, Memory::Readback, &name)?);
        }
        let buffer = (reads.buffer.as_ref())
            .map(Buffer::raw)
            .expect("the buffer was just made");
        let (raw_image, _, _) = image.parts();
        let to_copy = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(src_stage)
            .src_access_mask(src_access)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .old_layout(layout)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(raw_image)
            .subresource_range(COLOR_RANGE);
        let to_sample = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(raw_image)
            .subresource_range(COLOR_RANGE);
        let to_host = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .buffer(buffer)
            .size(bytes);
        let region = copy_region(0, [0, 0], size, 0);
        let commands = slots[self.slot].commands;
        let raw = device.raw();
        // SAFETY: the commands are recording outside a pass, as checked; the region is the whole
        // image and lies within the buffer, which is at least its size; and both live until the
        // frame has finished: the image retires until then, and the buffer is replaced only once
        // the slot's next frame begins, which waits for this one.
        unsafe {
            device.begin_label(commands, c"read image");
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&to_copy)),
            );
            raw.cmd_copy_image_to_buffer(
                commands,
                raw_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(slice::from_ref(&to_sample))
                    .buffer_memory_barriers(slice::from_ref(&to_host)),
            );
            device.end_label(commands);
        }
        images[target] = Some((index, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL));
        reads.frame = number;
        reads.size = size;
        Ok(image.index())
    }

    /// Reads back the IDs at `pixels` of `ids`, each a pixel's column and row from its top left,
    /// as this frame's last pass into it left them: the first of them, in order, up to
    /// [`MAX_READS`] a frame, less those this frame has read already. Returns how many it read.
    /// They're read once the frame has finished, which the frame that next uses its resources,
    /// two later, waits for as it begins: [`Frame::ids_read`] gives them there. The first read
    /// makes the buffer the frames read into, of a kibibyte, and fails if it can't be made.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, another device made `ids`, no pass of this frame has
    /// rendered into it, the frame samples it already ([`Frame::begin_sampling_ids`]), or a pixel
    /// is outside it.
    pub fn read_ids(&mut self, ids: &mut IdImage, pixels: &[[u32; 2]]) -> Result<usize> {
        self.outside_pass();
        let number = self.number();
        let device = &self.frames.device;
        device.check_made("the ID image read", ids.device());
        assert!(
            ids.rendered() == number,
            "IDs are read back in the frame whose pass rendered them, and no pass of frame \
             {number} has"
        );
        assert!(
            ids.layout() != vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            "IDs are read back before the frame samples them"
        );
        let [width, height] = ids.size();
        if let Some(pixel) = pixels.iter().find(|&&[x, y]| x >= width || y >= height) {
            panic!("pixel {pixel:?} is outside the {width}x{height} IDs read");
        }
        let read = self.frames.slots[self.slot].reads.1;
        let count = pixels.len().min(MAX_READS - read);
        if count == 0 {
            return Ok(0);
        }
        if self.frames.reads.is_none() {
            let (device, memory) = (&self.frames.device, Memory::Readback);
            let size = read_offset(FRAMES_IN_FLIGHT as usize, 0);
            let usage = vk::BufferUsageFlags::TRANSFER_DST;
            let reads = Buffer::new(device, size, usage, memory, "ids read")?;
            self.frames.reads = Some(reads);
        }
        let buffer = (self.frames.reads.as_ref())
            .map(Buffer::raw)
            .expect("the buffer was just made");
        let (image, _, _) = ids.parts();
        let mut regions = [vk::BufferImageCopy::default(); MAX_READS];
        for (i, (region, &[x, y])) in regions.iter_mut().zip(&pixels[..count]).enumerate() {
            *region = copy_region(read_offset(self.slot, read + i), [x, y], [1, 1], 0);
        }
        // From the pass that rendered them, unless an earlier read this frame took them from it.
        let to_copy = (ids.layout() == vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL).then(|| {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .image(image)
                .subresource_range(COLOR_RANGE)
        });
        let to_host = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .buffer(buffer)
            .offset(read_offset(self.slot, read))
            .size((count * ID_BYTES) as u64);
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        let raw = device.raw();
        // SAFETY: the commands are recording outside a pass; the regions lie within the image,
        // which a pass of this frame rendered, and within the slot's part of the buffer, past
        // what the frame read before; and both live until the frame has finished: the image
        // retires until then, and the buffer lives as long as the frames, which wait for theirs
        // before they go.
        unsafe {
            device.begin_label(commands, c"read ids");
            if let Some(to_copy) = &to_copy {
                raw.cmd_pipeline_barrier2(
                    commands,
                    &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(to_copy)),
                );
            }
            raw.cmd_copy_image_to_buffer(
                commands,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &regions[..count],
            );
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().buffer_memory_barriers(slice::from_ref(&to_host)),
            );
            device.end_label(commands);
        }
        ids.set_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        self.frames.slots[self.slot].reads.1 += count;
        Ok(count)
    }

    /// Sends the frame's commands to the GPU and presents the images of its swapchains.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open.
    pub fn submit(mut self) -> Result<()> {
        self.outside_pass();
        self.finish()
    }

    /// Ends the frame: submits and presents it, unless a forgotten pass is still open, whose
    /// rendering never ended, so that the frame's commands can't end either and its images go
    /// unpresented; that's an error rather than a panic, since a drop may already be unwinding.
    /// After any failure, no frame begins again.
    fn finish(&mut self) -> Result<()> {
        // Set first, so that dropping the frame after a failure doesn't submit it again.
        self.submitted = true;
        self.frames.recording = false;
        let finished = if self.rendering {
            Err(Error::new(
                "gpu.pass_forgotten",
                "a pass was forgotten rather than dropped, so its rendering never ended and the \
                 frame can't be submitted; its images go unpresented",
            ))
        } else {
            self.send()
        };
        self.frames.failed |= finished.is_err();
        finished
    }

    /// Submits the frame's copies and commands, and presents its swapchains' images.
    fn send(&mut self) -> Result<()> {
        let Frames {
            device,
            timeline,
            submitted,
            slots,
            arenas,
            staging,
            transfer_timeline,
            transfers_submitted,
            transfers,
            transfer_wait,
            images,
            scratch,
            ..
        } = &mut *self.frames;
        arenas[self.slot].flush()?;
        staging.flush()?;
        // Its buffers retire with this frame and with the copies being recorded, so they last
        // until the GPU has finished both.
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
        if let Some(i) = self.transfers {
            let transfer = &mut transfers[i];
            // SAFETY: the transfer commands are recording.
            unsafe { raw.end_command_buffer(transfer.commands) }.map_err(|e| {
                let subject = format_args!("command buffer {:?}", transfer.name);
                vk_error("vkEndCommandBuffer", subject, e)
            })?;
            let command_infos =
                [vk::CommandBufferSubmitInfo::default().command_buffer(transfer.commands)];
            let number = *transfers_submitted + 1;
            let copied = vk::SemaphoreSubmitInfo::default()
                .semaphore(*transfer_timeline)
                .value(number)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS);
            let submit = vk::SubmitInfo2::default()
                .command_buffer_infos(&command_infos)
                .signal_semaphore_infos(slice::from_ref(&copied));
            {
                let _zone = gg_core::zone!("submit uploads");
                let work = format_args!("frame {frame}'s uploads");
                // SAFETY: what the copies use lives until they finish: the command buffer is
                // reused only once the timeline has passed this submission, and staging and the
                // resources retire until then.
                unsafe { device.submit_transfers(slice::from_ref(&submit), work) }?;
            }
            transfer.submitted = number;
            *transfers_submitted = number;
            device.transfer_submitted(number);
        }
        if *transfer_wait > 0 {
            // For what the frame takes over from the transfer queue, whose copies have finished
            // already: the ownership transfer still needs the semaphore between its halves.
            // Without maintenance8, which ash's headers predate, a wait for an ownership transfer's
            // release is for all commands.
            scratch.waits.push(
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(*transfer_timeline)
                    .value(*transfer_wait)
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
            );
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
            .map_err(|e| {
                let subject = format_args!("command buffer {:?}", slot.commands_name);
                vk_error("vkEndCommandBuffer", subject, e)
            })
            .and_then(|()| {
                let _zone = gg_core::zone!("submit");
                // SAFETY: what the submission uses lives until the timeline reaches `frame`: the
                // slot and its arena are reused only after waiting for it, and resources retire
                // until then.
                unsafe { device.submit(slice::from_ref(&submit), format_args!("frame {frame}")) }
            });
        // The frame's copies, if any, run on regardless: the frames' drop waits for them.
        sent?;
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
            Err(e) => {
                let swapchains = scratch.swapchains.len();
                let subject = format_args!("frame {frame}'s presents to {swapchains} swapchain(s)");
                Err(vk_error("vkQueuePresentKHR", subject, e))
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
        self.frames.spare_targets = recycle(std::mem::take(&mut self.targets));
    }
}

/// A command pool of `family`'s and its one command buffer, for one submission at a time, both
/// named `name`, which their errors give; the name comes back as a C string, to name the commands
/// again as they're begun without allocating.
fn command_buffer(
    device: &Device,
    family: u32,
    name: String,
) -> Result<(vk::CommandPool, vk::CommandBuffer, CString)> {
    let name = CString::new(name).expect("names have no nul");
    let pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::TRANSIENT)
        .queue_family_index(family);
    let raw = device.raw();
    // SAFETY: the device is live, and each object is made from live ones; a pool whose command
    // buffer can't be allocated is destroyed, unused.
    unsafe {
        let pool = raw.create_command_pool(&pool_info, None).map_err(|e| {
            let subject = format_args!("command pool {name:?}");
            vk_error("vkCreateCommandPool", subject, e)
        })?;
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let commands = match raw.allocate_command_buffers(&allocate_info) {
            Ok(commands) => commands[0],
            Err(e) => {
                raw.destroy_command_pool(pool, None);
                let subject = format_args!("command buffer {name:?}");
                return Err(vk_error("vkAllocateCommandBuffers", subject, e));
            }
        };
        device.set_c_name(pool, &name);
        device.set_c_name(commands, &name);
        Ok((pool, commands, name))
    }
}

/// Resets `pool` and begins recording `commands`, its command buffer, for one submission, naming
/// it again for captures. Both have `name` as their debug name, which their errors give.
///
/// # Safety
///
/// `pool` must be the device's, `commands` one of its command buffers, and the GPU done with them.
unsafe fn begin_recording(
    device: &Device,
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    name: &CStr,
) -> Result<()> {
    let raw = device.raw();
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the caller guarantees it.
    unsafe {
        raw.reset_command_pool(pool, vk::CommandPoolResetFlags::empty())
            .map_err(|e| {
                let subject = format_args!("command pool {name:?}");
                vk_error("vkResetCommandPool", subject, e)
            })?;
        raw.begin_command_buffer(commands, &begin_info)
            .map_err(|e| {
                let subject = format_args!("command buffer {name:?}");
                vk_error("vkBeginCommandBuffer", subject, e)
            })?;
        device.name_begun(commands, name);
    }
    Ok(())
}
