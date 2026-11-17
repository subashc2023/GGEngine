//! Frames in flight. Each frame records its commands and submits them, and a timeline semaphore
//! paces them, so the CPU gets at most `FRAMES_IN_FLIGHT` frames ahead of the GPU.
//!
//! - A frame runs the passes of a [`Graph`] in order, placing every barrier between them. It can
//!   render into several targets, such as an app's windows, presenting all their swapchains at
//!   once. Its later passes can sample what its earlier ones rendered offscreen.
//! - Passes added to run on the compute queue run there, on a device that has one, in batches of
//!   their own that semaphores order against the graphics queue's, as `batches` says.
//! - It reads back a few pixels of the IDs its passes render and small buffers its compute shaders
//!   write, with the times its passes took on the GPU. The frame two later, which reuses its
//!   resources once it has finished, finds them read.
//! - On a device that uploads through a transfer queue, a frame's copies go in a submission of
//!   their own, paced by another timeline that no frame waits for.

mod batches;
mod compute;
pub(crate) mod graph;
mod pass;
mod timer;
mod transients;
mod upload;

use std::ffi::{CStr, CString};
use std::mem::ManuallyDrop;
use std::ops::Range;
use std::slice;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;
use gg_core::{Error, Result};
use tracing::{error, warn};

use crate::bindless::PUSH_CONSTANTS;
use crate::buffer::Buffer;
use crate::swapchain::{Failure, failure};
use crate::vma::Memory;
use crate::{
    COLOR_RANGE, ColorImage, Device, StorageImage, Swapchain, copy_region, vk_error, vk_failed,
};
use batches::{Plan, Timelines};
use graph::{BufferDecl, Hazard, ImageDecl, PassKind, Queue, Use};
use timer::Timer;
use transients::Transients;
use upload::Arena;

pub use compute::Compute;
pub use graph::{
    BufferId, ComputePass, Graph, ImageId, ImageKind, MAX_BUFFER_READ, MAX_BUFFER_READS, PassId,
    RenderPass,
};
pub use pass::{Indices, Pass};
pub use upload::Upload;

/// How many frames the GPU works on at once.
pub(crate) const FRAMES_IN_FLIGHT: u64 = 2;

/// The most IDs a frame reads back ([`Frame::read_ids`]).
pub const MAX_READS: usize = 64;

/// The bytes of an ID read back: two 32-bit unsigned integers.
const ID_BYTES: usize = size_of::<[u32; 2]>();

/// What the frames' arenas hold, which [`Frame::upload`] copies into, and the most a frame has
/// taken of its own, in bytes, as [`Frames::arenas`] gives them. A run's log compares the two: an
/// arena grows by doubling and keeps what it grew to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arenas {
    /// The bytes of every slot's arena's buffers.
    pub held: u64,
    /// The most bytes one frame's uploads took of its arena, each from a multiple of 16.
    pub peak: u64,
}

/// What a frame renders or computes into.
pub enum Target<'a> {
    /// The next image of a window's swapchain, presented when the frame is submitted.
    Swapchain(&'a mut Swapchain),
    /// An offscreen image.
    Image(&'a mut ColorImage),
    /// A storage image, which compute passes write and passes sample, but none draws into.
    Storage(&'a mut StorageImage),
}

/// The resources of the frames in flight, and the timeline semaphore that paces them. A device's
/// resources retire by its frames, so it has one `Frames` at a time, and once one has begun a
/// frame, no other for its whole life.
pub struct Frames {
    device: Arc<Device>,
    /// Reaches N when the Nth frame has finished on the GPU, on every queue.
    timeline: vk::Semaphore,
    submitted: u64,
    slots: Vec<Slot>,
    /// For a device with a compute queue, each queue's timeline, by [`Queue::index`]. Reaches N
    /// when the queue has run its Nth batch; null otherwise.
    queue_timelines: [vk::Semaphore; 2],
    /// How far the timelines are planned and waited for, as the last frame begun left them.
    timelines: Timelines,
    /// The batches of the frame being recorded.
    plan: Plan,
    /// The command buffer of each of its batches, as `plan` has them.
    batch_commands: Vec<vk::CommandBuffer>,
    /// Whether the frame being recorded has written a buffer outside its passes, in its first
    /// batch, which a compute-queue batch then waits for.
    buffers_written: bool,
    arenas: Vec<Arena>,
    staging: Arena,
    /// For a device that uploads through a transfer queue, reaches N when the Nth submission of
    /// copies has finished; null otherwise.
    transfer_timeline: vk::Semaphore,
    /// How many times copies have been submitted, counted apart from frames, since a frame may
    /// submit none.
    transfers_submitted: u64,
    /// The command buffers copies record into, each with the submission it was last in. A frame
    /// takes one whose submission has finished, or makes another, so none waits.
    transfers: Vec<TransferCommands>,
    /// What the transfer timeline had reached as the frame being recorded began: copies submitted
    /// up to then have finished.
    transfers_finished: u64,
    /// The submission of copies the frame being recorded waits on, the last that filled what it
    /// takes over from the transfer queue; 0 for none.
    transfer_wait: u64,
    /// What frames read IDs back into, [`MAX_READS`] for each slot, in slot order. Made by the
    /// first read, so apps that read none hold none.
    reads: Option<Buffer>,
    /// What the last frame to use the slot of the frame being begun read back:
    /// [`Frame::ids_read`]'s.
    read: Read,
    /// What frames read buffers back into ([`Frame::read_back`]): [`MAX_BUFFER_READ`] bytes for
    /// each of a frame's [`MAX_BUFFER_READS`], each slot's after the one before's. Made by the
    /// first read.
    buffer_reads: Option<Buffer>,
    /// The number of the frame whose buffer reads the frame being begun gives (0 for none), what
    /// it read, and the bytes of each read: [`Frame::buffer_read`]'s.
    buffer_read: (
        u64,
        BufferReads,
        [[u8; MAX_BUFFER_READ as usize]; MAX_BUFFER_READS],
    ),
    /// What each slot's frames read offscreen images back into ([`Frame::read_image`]), in slot
    /// order.
    image_reads: Vec<ImageReads>,
    /// For each target of the frame being recorded, its image as the commands so far leave it.
    /// `None` for a swapchain that had no image ready.
    images: Vec<Option<Tracked>>,
    transients: Transients,
    /// The passes' timestamps on each queue, by [`Queue::index`].
    timers: [Timer; 2],
    /// The barriers before the pass being recorded, kept so frames don't allocate.
    barriers: Vec<vk::ImageMemoryBarrier2<'static>>,
    buffer_barriers: Vec<vk::BufferMemoryBarrier2<'static>>,
    /// The last frame's list of targets, emptied, whose memory the next frame reuses.
    spare_targets: Vec<Target<'static>>,
    scratch: Scratch,
    /// How long the last `begin` waited, which [`Frames::waited`] describes.
    waited: Duration,
    /// Whether a frame begun is still being recorded: cleared as it's submitted or dropped, so a
    /// forgotten one is caught as the next begins.
    recording: bool,
    /// Whether a frame failed after acquiring an image (other than by another target's swapchain
    /// failing), failed to submit, or lost the device. That can leave the semaphores its acquires
    /// signaled with no wait pending, so no frame begins after. A swapchain's own failure is its
    /// alone: a failed acquire signals nothing and a present waits on nothing of the frame's. A
    /// failed submit can leave the frame's batches running, which the frame timeline never counts,
    /// so the drop waits for the whole device after one.
    failed: bool,
}

/// A target's image as the frame being recorded uses it.
#[derive(Clone, Copy, Debug)]
struct Tracked {
    /// The swapchain image's index, or 0 for an offscreen image.
    index: u32,
    /// The layout the frame's commands so far leave it in.
    layout: vk::ImageLayout,
    /// What its uses before left for its next to wait for.
    hazard: Hazard,
}

/// What one frame in flight records into and synchronizes with.
struct Slot {
    /// Each queue's command pool, by [`Queue::index`]; null for a queue the device hasn't.
    pools: [vk::CommandPool; 2],
    /// The command buffers made from each pool, which the frame's batches on its queue take in
    /// turn, each with its name, kept to name it again as it's begun without allocating.
    commands: [Vec<(vk::CommandBuffer, CString)>; 2],
    /// For each target, signaled when the swapchain image the frame renders into is ready. There
    /// are as many as the most targets a frame has had.
    acquired: Vec<vk::Semaphore>,
    /// The number of the frame that last began in the slot (0 for none), and how many IDs it read
    /// back into the slot's part of `Frames::reads`.
    reads: (u64, usize),
    /// The buffers that frame read back into the slot's part of `Frames::buffer_reads`.
    buffer_reads: BufferReads,
}

/// The buffers a frame read back, in order, each into its own part of its slot's: each one's name
/// and bytes read, and how many it read.
#[derive(Clone, Copy, Debug, Default)]
struct BufferReads {
    reads: [(&'static str, usize); MAX_BUFFER_READS],
    count: usize,
}

/// A command buffer that copies on the transfer queue record into, for one submission at a time.
struct TransferCommands {
    pool: vk::CommandPool,
    commands: vk::CommandBuffer,
    /// The pool's and the commands' name, kept to name the commands again as they're begun
    /// without allocating.
    name: CString,
    /// The submission of copies it was last in; 0 for none. Free once the transfer timeline has
    /// reached it.
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

/// The buffer a slot's frames read an offscreen image back into, made or made larger by the first
/// read that needs it, and what the last read.
#[derive(Default)]
struct ImageReads {
    buffer: Option<Buffer>,
    /// The number of the frame that read into it last (0 for none), and the image's size.
    frame: u64,
    size: [u32; 2],
}

/// An offscreen image as a frame read it back ([`Frame::read_image`]) once it finished, which
/// [`Frames::image_read`] gives.
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

/// A batch's submission: its waits and signals, in `Scratch::waits` and `Scratch::signals`, and
/// the value of the other queue's timeline it waits for, 0 for none.
struct Submission {
    waits: Range<usize>,
    signals: Range<usize>,
    waits_on: u64,
}

/// The lists submissions and their present are built in, kept so frames don't allocate.
#[derive(Default)]
struct Scratch {
    /// The regions of a buffer's write, as [`Frame::write_buffer_pieces`] copies them.
    copies: Vec<vk::BufferCopy>,
    waits: Vec<vk::SemaphoreSubmitInfo<'static>>,
    signals: Vec<vk::SemaphoreSubmitInfo<'static>>,
    commands: Vec<vk::CommandBufferSubmitInfo<'static>>,
    /// Each batch's submission.
    batches: Vec<Submission>,
    /// The batches submitted in a call, emptied between calls: their lists borrow the ones above.
    submits: Vec<vk::SubmitInfo2<'static>>,
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
    /// If the device has a `Frames`, or had one that began a frame, whose frame numbers this
    /// one's would repeat.
    pub fn new(device: &Arc<Device>) -> Result<Self> {
        // Before the claim: they hold nothing of the device's until a frame is timed, so neither a
        // failure here nor a refused claim leaves anything to destroy.
        let timers = [
            Timer::new(device, Queue::Graphics)?,
            Timer::new(device, Queue::Compute)?,
        ];
        device.claim_frames();
        // From here on, dropping `frames` destroys what has been made so far, and gives up the
        // claim, as it began no frame.
        let mut frames = Self {
            device: device.clone(),
            timeline: vk::Semaphore::null(),
            submitted: 0,
            slots: Vec::new(),
            queue_timelines: [vk::Semaphore::null(); 2],
            timelines: Timelines::default(),
            plan: Plan::default(),
            batch_commands: Vec::new(),
            buffers_written: false,
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
            buffer_reads: None,
            buffer_read: (
                0,
                BufferReads::default(),
                [[0; MAX_BUFFER_READ as usize]; MAX_BUFFER_READS],
            ),
            image_reads: (0..FRAMES_IN_FLIGHT)
                .map(|_| ImageReads::default())
                .collect(),
            images: Vec::new(),
            transients: Transients::new(device),
            timers,
            barriers: Vec::new(),
            buffer_barriers: Vec::new(),
            spare_targets: Vec::new(),
            scratch: Scratch::default(),
            waited: Duration::ZERO,
            recording: false,
            failed: false,
        };
        let raw = device.raw();
        let mut timeline_info =
            vk::SemaphoreTypeCreateInfo::default().semaphore_type(vk::SemaphoreType::TIMELINE);
        let info = vk::SemaphoreCreateInfo::default().push_next(&mut timeline_info);
        // SAFETY: the device is live, with timeline semaphores enabled.
        frames.timeline = unsafe { raw.create_semaphore(&info, None) }.map_err(vk_failed(
            "vkCreateSemaphore",
            "semaphore \"frame timeline\"",
        ))?;
        // SAFETY: the semaphore is live.
        unsafe { device.set_name(frames.timeline, "frame timeline") };
        let transfer_family = device.transfer_family();
        if transfer_family.is_some() {
            // SAFETY: as above.
            frames.transfer_timeline = unsafe { raw.create_semaphore(&info, None) }.map_err(
                vk_failed("vkCreateSemaphore", "semaphore \"upload timeline\""),
            )?;
            // SAFETY: the semaphore is live.
            unsafe { device.set_name(frames.transfer_timeline, "upload timeline") };
        }
        let compute_family = device.compute_family();
        if compute_family.is_some() {
            for (timeline, name) in (frames.queue_timelines.iter_mut())
                .zip(["graphics queue timeline", "compute queue timeline"])
            {
                // SAFETY: as above.
                *timeline = unsafe { raw.create_semaphore(&info, None) }.map_err(vk_failed(
                    "vkCreateSemaphore",
                    format_args!("semaphore {name:?}"),
                ))?;
                // SAFETY: the semaphore is live.
                unsafe { device.set_name(*timeline, name) };
            }
        }
        for i in 0..FRAMES_IN_FLIGHT {
            // In the list first, so dropping `frames` destroys the pools made.
            frames.slots.push(Slot {
                pools: [vk::CommandPool::null(); 2],
                commands: [Vec::new(), Vec::new()],
                acquired: Vec::new(),
                reads: (0, 0),
                buffer_reads: BufferReads::default(),
            });
            let slot = frames.slots.last_mut().expect("just pushed");
            let families = [Some(device.graphics_family()), compute_family];
            let names = ["graphics", "compute"];
            for ((pool, family), queue) in slot.pools.iter_mut().zip(families).zip(names) {
                if let Some(family) = family {
                    *pool = command_pool(device, family, &format!("frame {i} {queue}"))?;
                }
            }
        }
        Ok(frames)
    }

    /// Starts a frame that runs `graph`'s passes, rendering into `targets` (the graph's targets,
    /// by index), once the GPU has finished the frame that last used this one's resources.
    ///
    /// Targets:
    ///
    /// - The first paces the frame: a swapchain waits until the display lets go of an image.
    ///   Returns `None` if it has nothing to render into, as a minimized window's swapchain.
    /// - The others are rendered into only if their swapchains have an image free at once, so a
    ///   slower display doesn't slow the frame. An offscreen image always is.
    /// - A later swapchain lost for good ([`Swapchain::lost`]), or whose acquire fails other than
    ///   by device loss, is left out as one without an image free is, with a warning.
    /// - A swapchain whose surface was lost makes another for its window as it acquires.
    ///
    /// The graph's own images are an earlier frame's, if its graph declared them alike by name,
    /// kind and size and the uses of those sharing memory still don't overlap. Otherwise they're
    /// made again, and the old ones retire.
    ///
    /// Once this slot's last frame has finished, its read-backs ([`Frame::read_ids`],
    /// [`Frame::read_back`]) come out through [`Frame::ids_read`] and [`Frame::buffer_read`], and
    /// its pass times through [`Frames::gpu_time`]. The slot forgets them only as a frame begins.
    ///
    /// Fails before anything is acquired:
    ///
    /// - with the first target's error;
    /// - if the graph's images can't be made, as with `gpu.image_too_large` for a side past the
    ///   device's limit.
    ///
    /// Fails with `gpu.frame_failed` once a frame has failed after acquiring an image (but by
    /// another target's swapchain failing), failed to submit, or lost the device. Its acquires'
    /// semaphores may have no wait pending, so no frame can begin again: the frames can only be
    /// dropped. A swapchain's present that fails otherwise fails it alone.
    ///
    /// # Panics
    ///
    /// - `targets` is empty or holds one another device made.
    /// - The graph uses a target `targets` hasn't, or uses a swapchain's other than to draw into
    ///   it.
    /// - The last frame was forgotten rather than submitted or dropped.
    pub fn begin<'a>(
        &'a mut self,
        targets: impl IntoIterator<Item = Target<'a>>,
        graph: &'a Graph,
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
                "a frame failed after acquiring an image, or to submit, or the device was lost, \
                 which can leave its semaphores signaled, so no frame begins after it; the frames \
                 can only be dropped",
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
            .map_err(vk_failed(
                "vkGetSemaphoreCounterValue",
                "semaphore \"upload timeline\"",
            ))?;
            self.transfers_finished = finished;
            self.device.transfers_finished(finished);
        }
        self.transfer_wait = 0;
        // SAFETY: the slot's last frame has finished, so no submitted frame that the GPU may still
        // run uses its set.
        unsafe { self.device.bindless().apply(self.device.raw(), slot) };
        // The slot's last frame has finished, and its reads and timestamps with it.
        self.find_read(slot)?;
        for timer in &mut self.timers {
            timer.finished(&self.device, slot)?;
        }
        let mut list: Vec<Target<'a>> = std::mem::take(&mut self.spare_targets);
        list.extend(targets);
        assert!(!list.is_empty(), "a frame renders into at least one target");
        // Before any image is acquired: a panic or failure would leave it unpresented.
        for target in &list {
            match target {
                Target::Swapchain(swapchain) => self
                    .device
                    .check_made("a swapchain among the targets", swapchain.device()),
                Target::Image(image) => self
                    .device
                    .check_made("an offscreen image among the targets", image.device()),
                Target::Storage(image) => self
                    .device
                    .check_made("a storage image among the targets", image.device()),
            }
        }
        check_targets(graph, &list);
        if let Err(error) = self.transients.prepare(graph) {
            self.spare_targets = emptied(list);
            return Err(error);
        }
        // What the frame leaves of the buffers and the timelines takes effect only once it begins.
        let transients = &self.transients;
        let computes = self.device.compute_family().is_some();
        let crossing = |buffer| transients.crossing(BufferId(buffer as u32));
        self.plan.make(graph, computes, crossing, self.timelines);
        let acquired = &mut self.slots[slot].acquired;
        while acquired.len() < list.len() {
            let name = format!("frame {slot} target {} image acquired", acquired.len());
            // SAFETY: the device is live.
            let semaphore = unsafe {
                self.device
                    .raw()
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(vk_failed(
                "vkCreateSemaphore",
                format_args!("semaphore {name:?}"),
            ))?;
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
            self.spare_targets = emptied(list);
            return Err(error);
        }
        if self.images.is_empty() {
            self.spare_targets = emptied(list);
            return Ok(None);
        }
        self.timelines = self.plan.timelines;
        self.transients.cross(&self.plan.crossings);
        self.buffers_written = false;
        self.recording = true;
        self.slots[slot].reads = (self.submitted + 1, 0);
        self.slots[slot].buffer_reads = BufferReads::default();
        Ok(Some(Frame {
            frames: self,
            targets: list,
            graph,
            next: 0,
            slot,
            submitted: false,
            transfers: None,
            commands: vk::CommandBuffer::null(),
            queue: Queue::Graphics,
            rendering: false,
            pass_begun: false,
        }))
    }

    /// Acquires `targets`' images into `images`, as [`Frames::begin`] says. Given one for the
    /// first, readies slot `slot`'s arena and the planned batches' commands and timestamps, and
    /// begins recording them. `images` stays empty if the first target has no image.
    fn start(&mut self, slot: usize, targets: &mut [Target<'_>]) -> Result<()> {
        for (i, target) in targets.iter_mut().enumerate() {
            let acquired = self.slots[slot].acquired[i];
            let image = match target {
                // Left out, as one without an image free is: it presents nothing again.
                Target::Swapchain(swapchain) if i > 0 && swapchain.lost() => None,
                Target::Swapchain(swapchain) => {
                    // SAFETY: the semaphore is new, or the frame that last used this slot waited
                    // on it or left it unsignaled and has finished, so it's unsignaled with no
                    // wait pending. A forgotten or failed frame, which could have left it
                    // signaled, keeps frames from beginning.
                    let acquired = unsafe { swapchain.acquire(acquired, i == 0, &mut self.waited) };
                    skip_lost(acquired, i)?.map(|index| Tracked {
                        index,
                        layout: vk::ImageLayout::UNDEFINED,
                        hazard: Hazard::ACQUIRED,
                    })
                }
                Target::Image(image) => Some(Tracked {
                    index: 0,
                    layout: image.layout(),
                    hazard: Hazard::ANYTHING,
                }),
                Target::Storage(image) => Some(Tracked {
                    index: 0,
                    layout: image.layout(),
                    hazard: Hazard::ANYTHING,
                }),
            };
            if i == 0 && image.is_none() {
                return Ok(());
            }
            self.images.push(image);
        }
        self.device.frame_begun(self.submitted + 1);
        self.arenas[slot].reset(&self.device)?;
        let device = &self.device;
        let raw = device.raw();
        let Slot {
            pools, commands, ..
        } = &mut self.slots[slot];
        for &pool in pools
            .iter()
            .filter(|&&pool| pool != vk::CommandPool::null())
        {
            // SAFETY: the GPU has finished the slot's previous frame, so nothing uses the pool's
            // commands.
            unsafe { raw.reset_command_pool(pool, vk::CommandPoolResetFlags::empty()) }.map_err(
                vk_failed("vkResetCommandPool", format_args!("frame {slot}'s pool")),
            )?;
        }
        let mut passes = [0; 2];
        for &batch in &self.plan.of_pass {
            passes[self.plan.batches[batch].queue.index()] += 1;
        }
        let bindless = device.bindless();
        let mut taken = [0; 2];
        self.batch_commands.clear();
        for batch in &self.plan.batches {
            let queue = batch.queue.index();
            let (made, nth) = (&mut commands[queue], taken[queue]);
            taken[queue] += 1;
            if made.len() == nth {
                let name = ["graphics", "compute"][queue];
                let name = format!("frame {slot} {name} {nth}");
                made.push(allocate_commands(device, pools[queue], name)?);
            }
            let (batch_commands, ref name) = made[nth];
            // The graphics queue runs compute passes too.
            let bind_points: &[_] = match batch.queue {
                Queue::Graphics => &[
                    vk::PipelineBindPoint::GRAPHICS,
                    vk::PipelineBindPoint::COMPUTE,
                ],
                Queue::Compute => &[vk::PipelineBindPoint::COMPUTE],
            };
            // SAFETY: the command buffer's pool was just reset, and the sets and layout live as
            // long as the device; the push constants are the layout's whole range, in every stage.
            unsafe {
                begin_commands(device, batch_commands, name)?;
                for &bind_point in bind_points {
                    raw.cmd_bind_descriptor_sets(
                        batch_commands,
                        bind_point,
                        bindless.pipeline_layout(),
                        0,
                        &[bindless.set(slot)],
                        &[],
                    );
                }
                // Every push constant set to zeros before any pass: without maintenance4, a draw
                // or dispatch whose shader uses one a pass didn't push is invalid
                // (VUID-vkCmdDraw-maintenance4-08602), and with it reads what's undefined. Every
                // pipeline shares the layout, so what's pushed holds across the batch.
                raw.cmd_push_constants(
                    batch_commands,
                    bindless.pipeline_layout(),
                    vk::ShaderStageFlags::ALL,
                    0,
                    &[0; PUSH_CONSTANTS as usize],
                );
            }
            if nth == 0 {
                let timer = &mut self.timers[queue];
                timer.begin_frame(device, batch_commands, slot, passes[queue])?;
            }
            self.batch_commands.push(batch_commands);
        }
        Ok(())
    }

    /// Copies what slot `slot`'s last frame read back, once finished, for the frame begun in the
    /// slot to give.
    fn find_read(&mut self, slot: usize) -> Result<()> {
        let (frame, count) = self.slots[slot].reads;
        let buffers = self.slots[slot].buffer_reads;
        self.buffer_read.0 = frame;
        self.buffer_read.1 = buffers;
        let read = buffers.reads[..buffers.count].iter();
        for ((i, &(_, bytes)), out) in read.enumerate().zip(&mut self.buffer_read.2) {
            let reads = (self.buffer_reads.as_ref())
                .expect("a frame that read a buffer back made what it read into");
            reads.read_into(buffer_read_offset(slot, i), &mut out[..bytes])?;
        }
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

    /// How long the last [`Frames::begin`] waited for the GPU to finish the frame that last used
    /// its resources, and for the display to let go of the first target's image.
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// The GPU time of the last frame whose timestamps came back, which a frame's begin reads from
    /// the frame two before it: from its first pass's beginning, on either queue, to its last's
    /// end, barrier and semaphore waits included. `None` before then, after a frame of no passes,
    /// and for a GPU whose graphics queue writes no timestamps.
    pub fn gpu_time(&self) -> Option<Duration> {
        let spans = self.timers.iter().filter_map(Timer::span);
        let (first, last) = spans.reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))?;
        let period = f64::from(self.device.limits().timestamp_period);
        Some(Duration::from_nanos(
            (last.saturating_sub(first) as f64 * period) as u64,
        ))
    }

    /// The bytes of GPU memory the graphs' own images are in, as the last frame begun made or
    /// kept them. Images whose uses in a frame don't overlap share it.
    pub fn transient_bytes(&self) -> u64 {
        self.transients.bytes()
    }

    /// How many frames have been submitted: the number of the last, as frames count from 1. A
    /// [`Frames::begin`] that returns `None` begins none.
    pub fn submitted(&self) -> u64 {
        self.submitted
    }

    /// What the frames' arenas hold, and the most a frame has taken of its own, as [`Arenas`]
    /// says.
    pub fn arenas(&self) -> Arenas {
        Arenas {
            held: self.arenas.iter().map(Arena::held).sum(),
            peak: self.arenas.iter().map(Arena::peak).max().unwrap_or(0),
        }
    }

    /// What the `frame`th frame read back of an offscreen image ([`Frame::read_image`]), once the
    /// GPU has finished that frame, which this waits for. `None` if it read none, wasn't
    /// submitted, or a later frame (the one two after, say) has read into its slot's buffer.
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

/// What a frame takes of acquiring the image of its swapchain target `target`: the image or none,
/// as acquired.
///
/// For a target after the first, a failure other than the device's loss is logged and gives none,
/// so the other targets' images, acquired already, are presented and the frame goes on without
/// the window. Its swapchain tries again at the next acquire or is lost for good, as [`Failure`]
/// says. Any other failure fails the frame, the first target's before it has acquired anything.
fn skip_lost(acquired: Result<Option<u32>>, target: usize) -> Result<Option<u32>> {
    match acquired {
        Err(error) if target > 0 && failure(&error) != Failure::Device => {
            warn!(
                code = error.code(),
                target, "{error:#}; the frame goes on without the swapchain"
            );
            Ok(None)
        }
        acquired => acquired,
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
    unsafe { device.raw().wait_semaphores(&info, u64::MAX) }.map_err(vk_failed(
        "vkWaitSemaphores",
        format_args!("semaphore {name:?} reaching {value}"),
    ))
}

/// Where the `read`th ID that slot `slot`'s frame reads back goes in `Frames::reads`.
fn read_offset(slot: usize, read: usize) -> u64 {
    ((slot * MAX_READS + read) * ID_BYTES) as u64
}

/// Where the `read`th buffer that slot `slot`'s frame reads back goes in `Frames::buffer_reads`.
fn buffer_read_offset(slot: usize, read: usize) -> u64 {
    (slot * MAX_BUFFER_READS + read) as u64 * MAX_BUFFER_READ
}

/// Checks that `graph` uses only targets of `targets`, swapchains' only to draw into, storage
/// images only to write and sample, and only storage images to write.
///
/// # Panics
///
/// If a check fails.
fn check_targets(graph: &Graph, targets: &[Target<'_>]) {
    for (i, &decl) in graph.images.iter().enumerate() {
        let ImageDecl::Target(target) = decl else {
            continue;
        };
        let Some(found) = targets.get(target) else {
            panic!(
                "the graph uses target {target}, but the frame has {} targets",
                targets.len()
            );
        };
        let (what, fits): (_, fn(&Use) -> bool) = match found {
            Target::Swapchain(_) => ("a swapchain's, which passes only draw into", |use_| {
                matches!(use_, Use::Color(_))
            }),
            Target::Storage(_) => (
                "a storage image, which passes only write and sample",
                |use_| {
                    matches!(
                        use_,
                        Use::Storage { .. } | Use::Sampled | Use::ComputeSampled
                    )
                },
            ),
            Target::Image(_) => (
                "an offscreen image, which no pass writes as storage",
                |use_| !matches!(use_, Use::Storage { .. }),
            ),
        };
        let image = ImageId(i as u32);
        let misused = (graph.uses.iter()).find(|&&(used, use_)| used == image && !fits(&use_));
        if let Some((_, use_)) = misused {
            panic!("target {target} is {what}, but one uses it as {use_:?}");
        }
    }
}

/// A target's image, view and size; for a swapchain's, its image `index`.
fn target_parts(target: &Target<'_>, index: u32) -> (vk::Image, vk::ImageView, vk::Extent2D) {
    match target {
        Target::Swapchain(swapchain) => swapchain.image(index),
        Target::Image(image) => image.parts(),
        Target::Storage(image) => image.parts(),
    }
}

/// What a target's barriers cover: the whole of its image, every level and layer.
fn target_range(target: &Target<'_>) -> vk::ImageSubresourceRange {
    match target {
        Target::Swapchain(_) | Target::Image(_) => COLOR_RANGE,
        Target::Storage(image) => image.range(),
    }
}

/// Empties `list` for a next use that borrows for another lifetime, as the next frame's targets
/// do, keeping its memory so frames don't allocate. `T` and `U` are one type but for their
/// lifetimes. Not done by collecting, whose reuse of the memory std doesn't promise.
fn emptied<T, U>(mut list: Vec<T>) -> Vec<U> {
    const {
        assert!(size_of::<T>() == size_of::<U>() && align_of::<T>() == align_of::<U>());
    }
    list.clear();
    let mut list = ManuallyDrop::new(list);
    // SAFETY: the memory is the global allocator's, made by a `Vec` for `capacity` values of a
    // type as large and as aligned as `U`, as `from_raw_parts` requires, or, without capacity, a
    // pointer as aligned as `U` that isn't null, which it requires then; it holds none; the old
    // `Vec` is never dropped, so the new one alone frees it.
    unsafe { Vec::from_raw_parts(list.as_mut_ptr().cast::<U>(), 0, list.capacity()) }
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
        // The copies run apart from the frames, so both are waited for; a wait can fail. Frames
        // that submitted none may have failed to make their timeline.
        let frames_waited = if self.submitted > 0 {
            self.wait(self.submitted)
        } else {
            Ok(())
        };
        let waited = frames_waited.and_then(|()| {
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
        // After a failed frame, the device is waited for whole too: a failed submit may follow
        // others of the frame's batches, which run past the last frame submitted, as the frame
        // timeline's signal is in its last batch.
        if self.failed || waited.is_err() {
            let _ = self.device.wait_idle();
        }
        self.device.frames_finished(self.submitted);
        self.device.transfers_finished(self.transfers_submitted);
        let raw = self.device.raw();
        // SAFETY: the GPU has finished every frame, on every queue, and every copy, so nothing here
        // is in use. Null handles, left by a failed `new` or by a device that doesn't upload
        // through a transfer queue or has no compute queue, are allowed. The command pools go
        // before the timers' query pools, as a forgotten frame's command buffer, still recording,
        // writes timestamps to them: Vulkan allows either order, but KosmicKrisp, freeing such a
        // buffer, ends its Metal encoder, which reads the timestamps' query pool, and crashes if
        // it's gone.
        unsafe {
            for slot in &self.slots {
                for &semaphore in &slot.acquired {
                    raw.destroy_semaphore(semaphore, None);
                }
                for &pool in &slot.pools {
                    raw.destroy_command_pool(pool, None);
                }
            }
            for transfers in &self.transfers {
                raw.destroy_command_pool(transfers.pool, None);
            }
            for timer in &self.timers {
                timer.destroy(&self.device);
            }
            raw.destroy_semaphore(self.timeline, None);
            raw.destroy_semaphore(self.transfer_timeline, None);
            for &timeline in &self.queue_timelines {
                raw.destroy_semaphore(timeline, None);
            }
        }
        // Last: another `Frames` may then claim the device's frames, if these began none.
        self.device.release_frames();
    }
}

/// A frame being recorded. Submitting or dropping it sends its commands to the GPU and presents
/// its swapchains' images; forgetting it is caught as the next frame begins.
#[must_use = "a frame's commands reach the GPU when it's submitted"]
pub struct Frame<'a> {
    frames: &'a mut Frames,
    targets: Vec<Target<'a>>,
    graph: &'a Graph,
    /// The graph's next pass to record.
    next: usize,
    slot: usize,
    submitted: bool,
    /// The command buffer the frame's transfer-queue copies record into, among
    /// `Frames::transfers`, once begun.
    transfers: Option<usize>,
    /// The commands of the batch of the pass recorded last, and its queue.
    commands: vk::CommandBuffer,
    queue: Queue,
    /// Whether a pass is open, which only its drop closes. The frame records nothing of its own
    /// while one is, so a forgotten pass is caught at the frame's next call.
    rendering: bool,
    /// Whether a pass has begun in the frame. What the frame records outside its passes, its
    /// writes and uploads, goes in its first batch, which runs before them all, so it's recorded
    /// before them. A texture that comes to show an image takes it into its slot in the frame's
    /// bindless set at once, which every pass recorded before it would sample as the frame is
    /// submitted, before what makes the image ready.
    pass_begun: bool,
}

impl<'a> Frame<'a> {
    /// The frame's number: frames count from 1, as [`Frames::submitted`] counts them.
    pub fn number(&self) -> u64 {
        self.frames.submitted + 1
    }

    /// What the finished frame that last used this one's resources, two before it, read back
    /// ([`Frame::read_ids`]): its number, and the IDs it read, in order. None if it read none; the
    /// number is 0 for no frame, as before the first two. A frame that doesn't begin, as a
    /// minimized window's doesn't, leaves them for the next that does.
    pub fn ids_read(&self) -> (u64, &[[u32; 2]]) {
        let read = &self.frames.read;
        (read.frame, &read.values[..read.count])
    }

    /// What the finished frame that last used this one's resources, two before it, read back of
    /// the buffer named `name` ([`Frame::read_back`]): its number, and the buffer's bytes as its
    /// last read of that name found them. None if it read none of that name; the number is 0 for
    /// no frame, as [`Frame::ids_read`] gives.
    pub fn buffer_read(&self, name: &str) -> (u64, &[u8]) {
        let (frame, buffers, bytes) = &self.frames.buffer_read;
        let reads = buffers.reads[..buffers.count].iter().zip(bytes);
        let read = reads.rev().find(|((read, _), _)| *read == name);
        (
            *frame,
            read.map_or(&[][..], |((_, size), bytes)| &bytes[..*size]),
        )
    }

    /// Checks that no pass is open: only outside one does the frame record its own commands or
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

    /// Checks that `pass` is the graph's next pass to record and of `kind`, and counts it
    /// recorded.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, or `pass` isn't the next or of `kind`.
    fn take_pass(&mut self, pass: PassId, kind: PassKind) -> &'a graph::PassDecl {
        self.outside_pass();
        let graph = self.graph;
        let decl = (graph.passes.get(pass.0 as usize)).expect("a pass of the frame's graph");
        assert!(
            pass.0 as usize == self.next,
            "a frame records its graph's passes in the order they were added: pass {} next, not \
             pass {} ({:?})",
            self.next,
            pass.0,
            decl.label
        );
        assert_eq!(
            decl.kind, kind,
            "pass {:?} is recorded as what it was added as",
            decl.label
        );
        self.next += 1;
        self.pass_begun = true;
        let plan = &self.frames.plan;
        let batch = plan.of_pass[pass.0 as usize];
        (self.commands, self.queue) =
            (self.frames.batch_commands[batch], plan.batches[batch].queue);
        decl
    }

    /// The commands of the frame's first batch, the graphics queue's, which what it records
    /// outside its passes goes in, to run before them all.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, or a pass has begun in the frame: its writes and uploads
    /// are recorded before its passes.
    fn before_passes(&self) -> vk::CommandBuffer {
        self.outside_pass();
        assert!(
            !self.pass_begun,
            "a frame records its writes and uploads before its passes, which they run before"
        );
        self.frames.batch_commands[0]
    }

    /// Starts `pass`: its timestamp and label, and the barriers its uses need after the uses
    /// before them, which put each image it uses in its layout.
    ///
    /// # Panics
    ///
    /// If it draws over, samples or copies from a target no pass has drawn into or written since
    /// it was made, or since its swapchain's image was acquired.
    fn open_pass(&mut self, pass: PassId) {
        let graph = self.graph;
        let decl = &graph.passes[pass.0 as usize];
        // Before anything is recorded: a refusal records nothing. A storage image's first write
        // finds it undefined, as it says.
        for &(image, use_) in graph.uses(pass) {
            if let ImageDecl::Target(target) = graph.images[image.0 as usize] {
                let tracked = self.frames.images[target]
                    .as_ref()
                    .expect("a pass that records has its targets' images");
                assert!(
                    use_.access().discards
                        || matches!(use_, Use::Storage { .. })
                        || tracked.layout != vk::ImageLayout::UNDEFINED,
                    "pass {:?} uses target {target} as {use_:?}, but no pass has drawn into it \
                     or written it since it was made, or since its swapchain's image was acquired",
                    decl.label
                );
            }
        }
        let commands = self.commands();
        let device = &self.frames.device;
        let timer = &mut self.frames.timers[self.queue.index()];
        timer.begin(device, commands, self.slot, decl.label);
        // SAFETY: the commands are recording outside a pass.
        unsafe { device.begin_label(commands, decl.label.name()) };
        for &(buffer, _) in graph.buffer_uses(pass) {
            if let Some(raw) = self.frames.transients.fill(buffer, self.queue) {
                // SAFETY: the commands are recording outside a pass, and the buffer, made to be
                // filled, lives until the frame has finished.
                unsafe {
                    (self.frames.device.raw()).cmd_fill_buffer(commands, raw, 0, vk::WHOLE_SIZE, 0)
                };
            }
        }
        let mut barriers = std::mem::take(&mut self.frames.barriers);
        barriers.clear();
        for &(image, use_) in graph.uses(pass) {
            let access = use_.access();
            let (raw, range) = match graph.images[image.0 as usize] {
                ImageDecl::Target(target) => {
                    let tracked = self.frames.images[target]
                        .as_mut()
                        .expect("a pass that records has its targets' images");
                    if let Some(barrier) = tracked.hazard.follow(&mut tracked.layout, access) {
                        let found = &self.targets[target];
                        let (raw, ..) = target_parts(found, tracked.index);
                        barriers.push(barrier.on(raw, target_range(found)));
                    }
                    continue;
                }
                ImageDecl::Transient { kind, .. } => {
                    (self.frames.transients.image(image).raw, kind.range())
                }
            };
            if let Some(barrier) = self.frames.transients.follow(image, access) {
                barriers.push(barrier.on(raw, range));
            }
        }
        let mut buffer_barriers = std::mem::take(&mut self.frames.buffer_barriers);
        buffer_barriers.clear();
        for &(buffer, use_) in graph.buffer_uses(pass) {
            let transients = &mut self.frames.transients;
            if let Some(barrier) = transients.follow_buffer(buffer, self.queue, use_.access()) {
                buffer_barriers.push(
                    vk::BufferMemoryBarrier2::default()
                        .src_stage_mask(barrier.src_stages)
                        .src_access_mask(barrier.src_access)
                        .dst_stage_mask(barrier.dst_stages)
                        .dst_access_mask(barrier.dst_access)
                        .buffer(transients.buffer(buffer).raw())
                        .size(vk::WHOLE_SIZE),
                );
            }
        }
        if !barriers.is_empty() || !buffer_barriers.is_empty() {
            // SAFETY: the commands are recording outside a pass, and the images and buffers live
            // until the frame has finished.
            unsafe {
                self.frames.device.raw().cmd_pipeline_barrier2(
                    commands,
                    &vk::DependencyInfo::default()
                        .image_memory_barriers(&barriers)
                        .buffer_memory_barriers(&buffer_barriers),
                );
            }
        }
        self.frames.barriers = barriers;
        self.frames.buffer_barriers = buffer_barriers;
    }

    /// Ends the pass started last: its label and timestamp.
    fn close_pass(&mut self) {
        let commands = self.commands();
        let device = &self.frames.device;
        // SAFETY: the commands are recording outside a pass, with the pass's label open.
        unsafe { device.end_label(commands) };
        let timer = &mut self.frames.timers[self.queue.index()];
        timer.end(device, commands, self.slot);
    }

    /// `image`'s image, view and size, for recording commands that use it; for a target, the one
    /// the frame acquired.
    ///
    /// # Panics
    ///
    /// If it's a swapchain's that had no image ready.
    fn parts(&self, image: ImageId) -> (vk::Image, vk::ImageView, vk::Extent2D) {
        match self.graph.images[image.0 as usize] {
            ImageDecl::Target(target) => {
                let tracked = self.frames.images[target].expect("the target has an image");
                target_parts(&self.targets[target], tracked.index)
            }
            ImageDecl::Transient { size, .. } => {
                let made = self.frames.transients.image(image);
                let [width, height] = size;
                (made.raw, made.view, vk::Extent2D { width, height })
            }
        }
    }

    fn commands(&self) -> vk::CommandBuffer {
        self.commands
    }

    fn sampled_index(&self, image: ImageId) -> u32 {
        match self.graph.images[image.0 as usize] {
            ImageDecl::Target(target) => match &self.targets[target] {
                Target::Image(image) => image.index(),
                Target::Storage(image) => image.index(),
                Target::Swapchain(_) => unreachable!("only offscreen targets are sampled"),
            },
            ImageDecl::Transient { .. } => (self.frames.transients.image(image).index)
                .expect("only kinds that shaders sample are sampled"),
        }
    }

    /// Records `pass`, which must be the graph's next pass to record and read an image back: the
    /// image as the passes before left it, for [`Frames::image_read`] to give once the frame has
    /// finished. The slot's first read, and a read of a larger image than before, makes the buffer
    /// it reads into, and fails if it can't be made.
    ///
    /// # Panics
    ///
    /// - A forgotten pass is still open.
    /// - `pass` isn't the graph's next, or doesn't read an image back.
    /// - It reads back a target no pass has drawn into since it was made.
    pub fn read_image(&mut self, pass: PassId) -> Result<()> {
        self.take_pass(pass, PassKind::ReadImage);
        let [(image, _)] = *self.graph.uses(pass) else {
            unreachable!("a read uses its image alone");
        };
        let number = self.number();
        let (raw_image, _, extent) = self.parts(image);
        let size = [extent.width, extent.height];
        let bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
        let reads = &mut self.frames.image_reads[self.slot];
        if reads
            .buffer
            .as_ref()
            .is_none_or(|buffer| buffer.size() < bytes)
        {
            // The slot's last frame has finished, and its read.
            let name = format!("frame {} image read", self.slot);
            let usage = vk::BufferUsageFlags::TRANSFER_DST;
            let device = &self.frames.device;
            reads.buffer = Some(Buffer::new(device, bytes, usage, Memory::Readback, &name)?);
        }
        self.open_pass(pass);
        let reads = &mut self.frames.image_reads[self.slot];
        let buffer = (reads.buffer.as_ref())
            .map(Buffer::raw)
            .expect("the buffer was just made");
        let to_host = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .buffer(buffer)
            .size(bytes);
        let region = copy_region(0, [0, 0], size, 0);
        let commands = self.commands;
        let raw = self.frames.device.raw();
        // SAFETY: the commands are recording outside a pass, with the image in the layout copies
        // read it in; the region is the whole image and lies within the buffer, which is at least
        // its size; and both live until the frame has finished: the image retires until then, and
        // the buffer is replaced only once the slot's next frame begins, which waits for this one.
        unsafe {
            raw.cmd_copy_image_to_buffer(
                commands,
                raw_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().buffer_memory_barriers(slice::from_ref(&to_host)),
            );
        }
        reads.frame = number;
        reads.size = size;
        self.close_pass();
        Ok(())
    }

    /// Records `pass`, which must be the graph's next pass to record and read IDs back: the IDs at
    /// `pixels`, each a pixel's column and row from its top left, as the passes before left them.
    /// It reads the first in order, up to [`MAX_READS`] a frame less those this frame has read
    /// already, and returns how many. They're read once the frame has finished, which the frame
    /// that next uses its resources, two later, waits for as it begins: [`Frame::ids_read`] gives
    /// them there. The first read makes the buffer the frames read into, of a kibibyte, and fails
    /// if it can't be made.
    ///
    /// # Panics
    ///
    /// - A forgotten pass is still open.
    /// - `pass` isn't the graph's next, or doesn't read IDs back.
    /// - A pixel is outside the IDs.
    pub fn read_ids(&mut self, pass: PassId, pixels: &[[u32; 2]]) -> Result<usize> {
        let [(ids, _)] = *self.graph.uses(pass) else {
            panic!("pass {} reads IDs back from an image alone", pass.0);
        };
        // Before the pass is taken: a refusal records nothing.
        if let ImageDecl::Transient { size, .. } = self.graph.images[ids.0 as usize] {
            let [width, height] = size;
            if let Some(pixel) = pixels.iter().find(|&&[x, y]| x >= width || y >= height) {
                panic!("pixel {pixel:?} is outside the {width}x{height} IDs read");
            }
        }
        self.take_pass(pass, PassKind::ReadIds);
        let (image, _, _) = self.parts(ids);
        let read = self.frames.slots[self.slot].reads.1;
        let count = pixels.len().min(MAX_READS - read);
        if count > 0 && self.frames.reads.is_none() {
            let (device, memory) = (&self.frames.device, Memory::Readback);
            let size = read_offset(FRAMES_IN_FLIGHT as usize, 0);
            let usage = vk::BufferUsageFlags::TRANSFER_DST;
            let reads = Buffer::new(device, size, usage, memory, "ids read")?;
            self.frames.reads = Some(reads);
        }
        // Whether it reads any, so the passes after find the IDs where it leaves them.
        self.open_pass(pass);
        if count > 0 {
            let buffer = (self.frames.reads.as_ref())
                .map(Buffer::raw)
                .expect("the buffer was just made");
            let mut regions = [vk::BufferImageCopy::default(); MAX_READS];
            for (i, (region, &[x, y])) in regions.iter_mut().zip(&pixels[..count]).enumerate() {
                *region = copy_region(read_offset(self.slot, read + i), [x, y], [1, 1], 0);
            }
            let to_host = vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ)
                .buffer(buffer)
                .offset(read_offset(self.slot, read))
                .size((count * ID_BYTES) as u64);
            let commands = self.commands();
            let raw = self.frames.device.raw();
            // SAFETY: the commands are recording outside a pass, with the IDs in the layout copies
            // read them in; the regions lie within the image and the slot's part of the buffer,
            // past what the frame read before; and both live until the frame has finished: the
            // image retires until then, and the buffer lives as long as the frames, which wait
            // for theirs before they go.
            unsafe {
                raw.cmd_copy_image_to_buffer(
                    commands,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    buffer,
                    &regions[..count],
                );
                raw.cmd_pipeline_barrier2(
                    commands,
                    &vk::DependencyInfo::default()
                        .buffer_memory_barriers(slice::from_ref(&to_host)),
                );
            }
            self.frames.slots[self.slot].reads.1 += count;
        }
        self.close_pass();
        Ok(count)
    }

    /// Records `pass`, which must be the graph's next pass to record and read a buffer back: the
    /// buffer as the passes before left it, read once the frame has finished, which the frame that
    /// next uses its resources, two later, waits for as it begins. [`Frame::buffer_read`] gives it
    /// there, by the buffer's name. A frame reads up to [`MAX_BUFFER_READS`] buffers back, each
    /// into its own part. The first read makes the buffer the frames read into, and fails if it
    /// can't be made.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, or `pass` isn't the graph's next or doesn't read a
    /// buffer back.
    pub fn read_back(&mut self, pass: PassId) -> Result<()> {
        self.take_pass(pass, PassKind::ReadBack);
        let [(buffer, _)] = *self.graph.buffer_uses(pass) else {
            unreachable!("a read back uses its buffer alone");
        };
        if self.frames.buffer_reads.is_none() {
            let (device, memory) = (&self.frames.device, Memory::Readback);
            let size = MAX_BUFFER_READ * MAX_BUFFER_READS as u64 * FRAMES_IN_FLIGHT;
            let usage = vk::BufferUsageFlags::TRANSFER_DST;
            let reads = Buffer::new(device, size, usage, memory, "buffers read")?;
            self.frames.buffer_reads = Some(reads);
        }
        self.open_pass(pass);
        let BufferDecl { name, size } = self.graph.buffers[buffer.0 as usize];
        // The graph reads back no more buffers than that, as it checked.
        let read = &mut self.frames.slots[self.slot].buffer_reads;
        let offset = buffer_read_offset(self.slot, read.count);
        read.reads[read.count] = (name, size as usize);
        read.count += 1;
        let source = self.frames.transients.buffer(buffer).raw();
        let read = (self.frames.buffer_reads.as_ref())
            .map(Buffer::raw)
            .expect("the buffer was just made");
        let region = vk::BufferCopy {
            src_offset: 0,
            dst_offset: offset,
            size,
        };
        let to_host = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .buffer(read)
            .offset(offset)
            .size(size);
        let commands = self.commands();
        let raw = self.frames.device.raw();
        // SAFETY: the commands are recording outside a pass, after the barrier that lets the copy
        // read the buffer; the region lies within the graph's buffer, which the graph checked holds
        // at most `MAX_BUFFER_READ` bytes, and within the read's part of the slot's part of what it
        // reads into; and both live until the frame has finished: the graph's buffer retires until
        // then, and what it reads into lives as long as the frames, which wait for theirs before
        // they go.
        unsafe {
            raw.cmd_copy_buffer(commands, source, read, slice::from_ref(&region));
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().buffer_memory_barriers(slice::from_ref(&to_host)),
            );
        }
        self.close_pass();
        Ok(())
    }

    /// The address shaders reach `buffer`, one of the graph's own, through during the frame, for
    /// data a pass reads in place, as a table of where compute shaders write. A pass that reaches
    /// it must use it as its graph declared, as [`Pass::address`] and [`Compute::address`] give
    /// it.
    ///
    /// # Panics
    ///
    /// If `buffer` isn't the graph's.
    pub fn address(&self, buffer: BufferId) -> u64 {
        self.buffer_parts(buffer).0
    }

    /// The address of `buffer`, one of the graph's own, and its declared bytes.
    fn buffer_parts(&self, buffer: BufferId) -> (u64, u64) {
        let address = self.frames.transients.buffer(buffer).address();
        (address, self.graph.buffers[buffer.0 as usize].size)
    }

    /// Sends the frame's commands to the GPU and presents its swapchains' images.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, or a pass of the frame's graph wasn't recorded.
    pub fn submit(mut self) -> Result<()> {
        self.outside_pass();
        assert!(
            self.next == self.graph.passes.len(),
            "a frame records every pass of its graph, but pass {} ({:?}) wasn't",
            self.next,
            self.graph.passes.get(self.next).map(|pass| pass.label)
        );
        self.finish()
    }

    /// Ends the frame: submits and presents it, unless a forgotten pass is still open. Its
    /// rendering never ended, so the frame's commands can't end either and its images go
    /// unpresented; that's an error rather than a panic, since a drop may already be unwinding.
    /// After a failure to submit, or the device's loss, no frame begins again. A present that
    /// fails otherwise is its swapchain's alone, as [`Frame::present`] says, since the submission
    /// waited on what the frame's acquires signaled.
    fn finish(&mut self) -> Result<()> {
        // Set first, so dropping the frame after a failure doesn't submit it again.
        self.submitted = true;
        self.frames.recording = false;
        let sent = if self.rendering {
            Err(Error::new(
                "gpu.pass_forgotten",
                "a pass was forgotten rather than dropped, so its rendering never ended and the \
                 frame can't be submitted; its images go unpresented",
            ))
        } else {
            self.send()
        };
        if sent.is_err() {
            self.frames.failed = true;
            return sent;
        }
        let presented = self.present();
        self.frames.failed |= fails_frames(&presented);
        presented
    }

    /// Submits the frame's copies and batches, readying its swapchains' images to present.
    fn send(&mut self) -> Result<()> {
        let Frames {
            submitted,
            arenas,
            staging,
            scratch,
            ..
        } = &mut *self.frames;
        arenas[self.slot].flush()?;
        staging.flush()?;
        // Its buffers retire with this frame and the copies being recorded, so they last until
        // the GPU has finished both.
        staging.clear();
        let frame = *submitted + 1;
        scratch.waits.clear();
        scratch.signals.clear();
        scratch.commands.clear();
        scratch.batches.clear();
        scratch.rendered.clear();
        scratch.swapchains.clear();
        scratch.images.clear();
        self.submit_uploads(frame)?;
        // The presents' barriers end the graphics queue's last batch, which signals what the
        // presents wait on after every command, layout changes included, so the barriers need no
        // second scope.
        let last = self.frames.plan.last_graphics();
        self.ready_presents(last);
        self.plan_submissions(frame, last);
        self.end_batches(frame)?;
        self.submit_batches(frame)?;
        self.frames.submitted = frame;
        for (target, image) in self.targets.iter_mut().zip(self.frames.images.iter()) {
            match (target, image) {
                (Target::Image(target), Some(Tracked { layout, .. })) => target.set_layout(*layout),
                (Target::Storage(target), Some(Tracked { layout, .. })) => {
                    target.set_layout(*layout);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Ends and submits the frame's copies on the transfer queue, if it recorded any.
    fn submit_uploads(&mut self, frame: u64) -> Result<()> {
        let Frames {
            device,
            transfer_timeline,
            transfers_submitted,
            transfers,
            ..
        } = &mut *self.frames;
        let raw = device.raw();
        if let Some(i) = self.transfers {
            let transfer = &mut transfers[i];
            // SAFETY: the transfer commands are recording.
            unsafe { raw.end_command_buffer(transfer.commands) }.map_err(vk_failed(
                "vkEndCommandBuffer",
                format_args!("command buffer {:?}", transfer.name),
            ))?;
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
        Ok(())
    }

    /// Ends the graphics queue's `last` batch with each swapchain image's barrier to present, and
    /// lists what the present waits on and shows.
    fn ready_presents(&mut self, last: usize) {
        let Frames {
            device,
            images,
            scratch,
            batch_commands,
            ..
        } = &mut *self.frames;
        let raw = device.raw();
        for (target, image) in self.targets.iter().zip(images.iter_mut()) {
            let (
                Target::Swapchain(swapchain),
                Some(Tracked {
                    index,
                    layout,
                    hazard,
                }),
            ) = (target, image)
            else {
                continue;
            };
            let (image, _, _) = swapchain.image(*index);
            let (src_stages, src_access) = hazard.last();
            let barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(src_stages)
                .src_access_mask(src_access)
                .old_layout(*layout)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .image(image)
                .subresource_range(COLOR_RANGE);
            // SAFETY: the commands are recording, and the image is live.
            unsafe {
                raw.cmd_pipeline_barrier2(
                    batch_commands[last],
                    &vk::DependencyInfo::default().image_memory_barriers(slice::from_ref(&barrier)),
                );
            }
            *layout = vk::ImageLayout::PRESENT_SRC_KHR;
            scratch.rendered.push(swapchain.rendered(*index));
            scratch.swapchains.push(swapchain.raw());
            scratch.images.push(*index);
        }
    }

    /// Lists each batch's waits, signals and commands for frame `frame`, whose timeline batch
    /// `last` signals.
    fn plan_submissions(&mut self, frame: u64, last: usize) {
        let Frames {
            timeline,
            slots,
            transfer_timeline,
            transfer_wait,
            images,
            scratch,
            queue_timelines,
            timelines,
            plan,
            batch_commands,
            buffers_written,
            ..
        } = &mut *self.frames;
        let acquired = &slots[self.slot].acquired;
        // Each batch's waits and signals. A semaphore's wait takes in all the other queue did
        // before it signaled, and makes it visible to all its own queue does after, through every
        // stage.
        let all = vk::PipelineStageFlags2::ALL_COMMANDS;
        let first_compute = (plan.batches.iter()).position(|batch| batch.queue == Queue::Compute);
        for (i, batch) in plan.batches.iter().enumerate() {
            let (own, other) = (batch.queue.index(), batch.queue.other().index());
            let (waits, signals) = (scratch.waits.len(), scratch.signals.len());
            let first = i == 0 || Some(i) == first_compute;
            let mut waits_on = batch.waits;
            if Some(i) == first_compute && *buffers_written {
                // What the frame wrote outside its passes, in its first batch, the graphics
                // queue's, which a compute-queue pass may read.
                waits_on = waits_on.max(plan.batches[0].value);
                timelines.waited[own] = timelines.waited[own].max(waits_on);
            }
            if waits_on > 0 {
                let semaphore = queue_timelines[other];
                let wait = vk::SemaphoreSubmitInfo::default().semaphore(semaphore);
                scratch.waits.push(wait.value(waits_on).stage_mask(all));
            }
            if first && *transfer_wait > 0 {
                // For what the frame takes over from the transfer queue, whose copies have
                // finished already: a texture's ownership transfer still needs the semaphore
                // between its halves. Without maintenance8, which ash's headers predate, a wait
                // for an ownership transfer's release is for all commands.
                let wait = vk::SemaphoreSubmitInfo::default().semaphore(*transfer_timeline);
                scratch
                    .waits
                    .push(wait.value(*transfer_wait).stage_mask(all));
            }
            if i == 0 {
                for (&semaphore, (target, image)) in
                    acquired.iter().zip(self.targets.iter().zip(images.iter()))
                {
                    if let (Target::Swapchain(_), Some(_)) = (target, image) {
                        let stage = vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT;
                        let wait = vk::SemaphoreSubmitInfo::default().semaphore(semaphore);
                        scratch.waits.push(wait.stage_mask(stage));
                    }
                }
            }
            if queue_timelines[own] != vk::Semaphore::null() {
                let signal = vk::SemaphoreSubmitInfo::default().semaphore(queue_timelines[own]);
                scratch
                    .signals
                    .push(signal.value(batch.value).stage_mask(all));
            }
            if i == last {
                let signal = vk::SemaphoreSubmitInfo::default().semaphore(*timeline);
                scratch.signals.push(signal.value(frame).stage_mask(all));
                for &rendered in &scratch.rendered {
                    let signal = vk::SemaphoreSubmitInfo::default().semaphore(rendered);
                    scratch.signals.push(signal.stage_mask(all));
                }
            }
            let commands = vk::CommandBufferSubmitInfo::default().command_buffer(batch_commands[i]);
            scratch.commands.push(commands);
            scratch.batches.push(Submission {
                waits: waits..scratch.waits.len(),
                signals: signals..scratch.signals.len(),
                waits_on,
            });
        }
    }

    fn end_batches(&self, frame: u64) -> Result<()> {
        let Frames {
            device,
            batch_commands,
            ..
        } = &*self.frames;
        let raw = device.raw();
        for (i, &commands) in batch_commands.iter().enumerate() {
            // SAFETY: the commands are recording, outside a pass.
            unsafe { raw.end_command_buffer(commands) }.map_err(vk_failed(
                "vkEndCommandBuffer",
                format_args!("frame {frame}'s batch {i}"),
            ))?;
        }
        Ok(())
    }

    fn submit_batches(&mut self, frame: u64) -> Result<()> {
        let Frames {
            device,
            scratch,
            timelines,
            plan,
            ..
        } = &mut *self.frames;
        // Runs of one queue's batches, each after the other's batches it waits for, which were
        // planned before it. The compute queue's go first where they can, so its work begins as
        // soon as it can. Each queue's batches up to the frame's are submitted.
        let mut done = [Queue::Graphics, Queue::Compute].map(|queue| {
            let first = plan.batches.iter().find(|batch| batch.queue == queue);
            first.map_or(timelines.made[queue.index()], |batch| batch.value - 1)
        });
        let mut next = [0; 2];
        let mut left = plan.batches.len();
        let mut submits = emptied(std::mem::take(&mut scratch.submits));
        let mut sent = Ok(());
        while left > 0 && sent.is_ok() {
            let ready = [Queue::Compute, Queue::Graphics]
                .into_iter()
                .find(|&queue| {
                    let (own, other) = (queue.index(), queue.other().index());
                    submits.clear();
                    let mut at = next[own];
                    while let Some(i) =
                        (at..plan.batches.len()).find(|&i| plan.batches[i].queue == queue)
                    {
                        let Submission {
                            waits,
                            signals,
                            waits_on,
                        } = &scratch.batches[i];
                        if *waits_on > done[other] {
                            break;
                        }
                        submits.push(
                            vk::SubmitInfo2::default()
                                .wait_semaphore_infos(&scratch.waits[waits.clone()])
                                .command_buffer_infos(slice::from_ref(&scratch.commands[i]))
                                .signal_semaphore_infos(&scratch.signals[signals.clone()]),
                        );
                        done[own] = plan.batches[i].value;
                        at = i + 1;
                    }
                    next[own] = at;
                    !submits.is_empty()
                });
            let queue = ready.expect("a frame's batches each wait for batches planned before it");
            left -= submits.len();
            let _zone = gg_core::zone!("submit");
            let work = format_args!("frame {frame}");
            // SAFETY: what the submissions use lives until the frame's timeline reaches `frame`,
            // which follows every batch: the slot and its arena are reused only after waiting for
            // it, and resources retire until then.
            sent = unsafe {
                match queue {
                    Queue::Graphics => device.submit(&submits, work),
                    Queue::Compute => device.submit_compute(&submits, work),
                }
            };
        }
        scratch.submits = emptied(submits);
        // The frame's copies, if any, run on regardless: the frames' drop waits for them.
        sent
    }

    /// Presents the frame's swapchains' images, which its submission readied. Each swapchain's
    /// result says how its own present went; each swapchain tries again at its next acquire, or is
    /// lost for good, as [`Failure`] says. Returns the device's loss, whether a swapchain's result
    /// or the whole present's said it, else the first target's failure that lost its swapchain for
    /// good. The other failures are logged, as [`heard`] sorts them.
    fn present(&mut self) -> Result<()> {
        let Frames {
            device,
            submitted,
            images,
            scratch,
            ..
        } = &mut *self.frames;
        let frame = *submitted;
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
        // A present that failed for lack of memory enqueued nothing and may not have said so of
        // each swapchain: each left as presented takes the failure, so none goes on with an image
        // it never presented.
        if let Err(
            e @ (vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
        ) = presented
        {
            for result in &mut scratch.results {
                if *result == vk::Result::SUCCESS {
                    *result = e;
                }
            }
        }
        let (mut device_lost, mut first) = (None, None);
        // The device's loss, which no swapchain's own result need say.
        if let Err(e @ vk::Result::ERROR_DEVICE_LOST) = presented {
            let swapchains = scratch.swapchains.len();
            let subject = format_args!("frame {frame}'s presents to {swapchains} swapchain(s)");
            device_lost = Some(vk_error("vkQueuePresentKHR", subject, e));
        }
        let mut results = scratch.results.iter();
        for (i, (target, image)) in self.targets.iter_mut().zip(images.iter()).enumerate() {
            if let (Target::Swapchain(swapchain), Some(_)) = (target, image) {
                let result = results
                    .next()
                    .expect("a result for each swapchain presented");
                // Every swapchain hears how its present went, whatever another's did.
                let Err(error) = swapchain.presented(*result) else {
                    continue;
                };
                match heard(i, &error, swapchain.lost()) {
                    Heard::Device => drop(device_lost.get_or_insert(error)),
                    Heard::Returned => drop(first.get_or_insert(error)),
                    Heard::Logged => warn!(
                        code = error.code(),
                        target = i,
                        "{error:#}; the frames go on without the swapchain until it's made again"
                    ),
                }
            }
        }
        outcome(device_lost, first)
    }
}

/// How a frame's present takes the failure of target `target`'s swapchain, which `lost` says is
/// lost for good after it.
#[derive(Debug, PartialEq, Eq)]
enum Heard {
    /// The device's loss, which the present returns before any other failure and which fails every
    /// frame after.
    Device,
    /// The first target's swapchain lost for good, which the present returns unless the device was
    /// lost.
    Returned,
    /// Logged: another target's, or one the swapchain tries again from at its next acquire.
    Logged,
}

fn heard(target: usize, error: &Error, lost: bool) -> Heard {
    match failure(error) {
        Failure::Device => Heard::Device,
        _ if target == 0 && lost => Heard::Returned,
        _ => Heard::Logged,
    }
}

/// What a frame's present returns, given the device's loss and the first target's failure it
/// heard ([`heard`]), if any. The device's loss wins, failing the frames after as [`fails_frames`]
/// says, with the first target's logged.
fn outcome(device_lost: Option<Error>, first: Option<Error>) -> Result<()> {
    match (device_lost, first) {
        (Some(device_lost), first) => {
            if let Some(error) = first {
                warn!(code = error.code(), "{error:#}");
            }
            Err(device_lost)
        }
        (None, Some(error)) => Err(error),
        (None, None) => Ok(()),
    }
}

/// Whether a frame's present that went as `presented` keeps frames from beginning. Only the
/// device's loss does: the frame's submission waited on what its acquires signaled, and a
/// swapchain's other failure is its alone.
fn fails_frames(presented: &Result<()>) -> bool {
    presented
        .as_ref()
        .is_err_and(|error| failure(error) == Failure::Device)
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        // An acquired swapchain image must be presented, or its semaphore stays signaled.
        if !self.submitted
            && let Err(error) = self.finish()
        {
            error!(code = error.code(), "{error:#}");
        }
        self.frames.spare_targets = emptied(std::mem::take(&mut self.targets));
    }
}

/// A command pool of `family`'s, named `name`, which its errors give. Its command buffers are each
/// recorded for one submission at a time.
fn command_pool(device: &Device, family: u32, name: &str) -> Result<vk::CommandPool> {
    let pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::TRANSIENT)
        .queue_family_index(family);
    // SAFETY: the device is live.
    let pool = unsafe { device.raw().create_command_pool(&pool_info, None) }.map_err(vk_failed(
        "vkCreateCommandPool",
        format_args!("command pool {name:?}"),
    ))?;
    // SAFETY: the pool is live.
    unsafe { device.set_name(pool, name) };
    Ok(pool)
}

/// A command buffer made from `pool`, a pool of the device's, named `name`, which its errors give.
/// The name comes back as a C string, to name the commands again as they're begun without
/// allocating.
fn allocate_commands(
    device: &Device,
    pool: vk::CommandPool,
    name: String,
) -> Result<(vk::CommandBuffer, CString)> {
    let name = CString::new(name).expect("names have no nul");
    let allocate_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: the pool is live, and the caller's to allocate from.
    let commands =
        unsafe { device.raw().allocate_command_buffers(&allocate_info) }.map_err(vk_failed(
            "vkAllocateCommandBuffers",
            format_args!("command buffer {name:?}"),
        ))?[0];
    // SAFETY: the command buffer is live.
    unsafe { device.set_c_name(commands, &name) };
    Ok((commands, name))
}

/// A command pool of `family`'s and its one command buffer, for one submission at a time, as
/// [`command_pool`] and [`allocate_commands`] make them, both named `name`.
fn command_buffer(
    device: &Device,
    family: u32,
    name: String,
) -> Result<(vk::CommandPool, vk::CommandBuffer, CString)> {
    let pool = command_pool(device, family, &name)?;
    match allocate_commands(device, pool, name) {
        Ok((commands, name)) => Ok((pool, commands, name)),
        Err(error) => {
            // SAFETY: the pool is live, and nothing was made from it.
            unsafe { device.raw().destroy_command_pool(pool, None) };
            Err(error)
        }
    }
}

/// Resets `pool` and begins recording `commands`, its command buffer, for one submission, as
/// [`begin_commands`] does. Both are named `name`, which their errors give.
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
    // SAFETY: the caller guarantees it.
    unsafe {
        (device
            .raw()
            .reset_command_pool(pool, vk::CommandPoolResetFlags::empty()))
        .map_err(vk_failed(
            "vkResetCommandPool",
            format_args!("command pool {name:?}"),
        ))?;
        begin_commands(device, commands, name)
    }
}

/// Begins recording `commands`, whose pool was reset since its last recording, for one
/// submission, naming it for captures `name`, which its errors give.
///
/// # Safety
///
/// `commands` must be the device's, its pool reset since the GPU last ran it.
unsafe fn begin_commands(device: &Device, commands: vk::CommandBuffer, name: &CStr) -> Result<()> {
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the caller guarantees it.
    unsafe {
        (device.raw().begin_command_buffer(commands, &begin_info)).map_err(vk_failed(
            "vkBeginCommandBuffer",
            format_args!("command buffer {name:?}"),
        ))?;
        device.name_begun(commands, name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list emptied for another lifetime keeps its memory, as frames keep their lists of targets
    /// and submissions; one never filled stays without memory.
    #[test]
    fn emptied_lists_keep_their_memory() {
        let borrowed = String::from("a target");
        let mut list: Vec<&str> = Vec::with_capacity(3);
        list.push(&borrowed);
        let (memory, capacity) = (list.as_ptr().addr(), list.capacity());
        let next: Vec<&'static str> = emptied(list);
        assert!(next.is_empty());
        assert_eq!((next.as_ptr().addr(), next.capacity()), (memory, capacity));
        let never_filled: Vec<&'static str> = emptied(Vec::<&str>::new());
        assert_eq!(never_filled.capacity(), 0);
    }

    /// A swapchain after the first that's lost is left out and the frame goes on with the images
    /// acquired before it; the first target's loss, or the device's, fails the frame.
    #[test]
    fn a_swapchain_lost_alone_is_left_out_unless_its_the_first_target() {
        let failed = |result| vk_error("vkAcquireNextImageKHR", "a swapchain", result);
        let surface_lost = || Err(failed(vk::Result::ERROR_SURFACE_LOST_KHR));
        assert_eq!(skip_lost(surface_lost(), 1).ok(), Some(None));
        assert_eq!(skip_lost(surface_lost(), 2).ok(), Some(None));
        let memory = Err(failed(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY));
        assert_eq!(skip_lost(memory, 1).ok(), Some(None));
        let first = skip_lost(surface_lost(), 0).expect_err("the frame fails");
        assert_eq!(first.code(), "gpu.surface_lost");
        let device_lost = Err(failed(vk::Result::ERROR_DEVICE_LOST));
        let device = skip_lost(device_lost, 1).expect_err("the frame fails");
        assert_eq!(device.code(), "gpu.device_lost");
        assert_eq!(skip_lost(Ok(Some(3)), 1).ok(), Some(Some(3)));
        assert_eq!(skip_lost(Ok(None), 0).ok(), Some(None));
    }

    /// The device's loss in a present wins over the first target's failure, however the targets
    /// come, so the frames after fail. Otherwise the first target's swapchain lost for good is
    /// returned, and the rest, which their swapchains try again from, are logged.
    #[test]
    fn a_presents_device_loss_wins_over_a_surface_lost() {
        let failed = |result| vk_error("vkQueuePresentKHR", "a swapchain", result);
        let surface = failed(vk::Result::ERROR_SURFACE_LOST_KHR);
        let device = failed(vk::Result::ERROR_DEVICE_LOST);
        assert_eq!(heard(0, &surface, true), Heard::Returned);
        assert_eq!(heard(0, &surface, false), Heard::Logged);
        assert_eq!(heard(1, &surface, true), Heard::Logged);
        assert_eq!(heard(1, &device, false), Heard::Device);
        assert_eq!(heard(0, &device, false), Heard::Device);
        // Target 0's surface lost for good, then target 1's device lost.
        let presented = outcome(Some(device), Some(surface));
        assert_eq!(
            presented.as_ref().err().map(Error::code),
            Some("gpu.device_lost")
        );
        assert!(fails_frames(&presented));
        let first = outcome(None, Some(failed(vk::Result::ERROR_SURFACE_LOST_KHR)));
        assert_eq!(
            first.as_ref().err().map(Error::code),
            Some("gpu.surface_lost")
        );
        assert!(!fails_frames(&first));
        assert!(outcome(None, None).is_ok());
    }

    /// Only the device's loss in a frame's present keeps frames from beginning; a swapchain whose
    /// present fails otherwise is lost alone.
    #[test]
    fn only_the_devices_loss_in_a_present_fails_the_frames_after() {
        let failed = |result| Err(vk_error("vkQueuePresentKHR", "a swapchain", result));
        assert!(fails_frames(&failed(vk::Result::ERROR_DEVICE_LOST)));
        assert!(!fails_frames(&failed(vk::Result::ERROR_SURFACE_LOST_KHR)));
        assert!(!fails_frames(&failed(vk::Result::ERROR_OUT_OF_HOST_MEMORY)));
        assert!(!fails_frames(&Ok(())));
    }
}
