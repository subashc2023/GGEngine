//! Frames in flight. Each frame records its commands and submits them, and a timeline semaphore
//! paces them, so that the CPU gets at most `FRAMES_IN_FLIGHT` frames ahead of the GPU. A frame
//! runs the passes of a [`Graph`], in order, which place every barrier between them, and can
//! render into several targets, such as an app's windows, presenting all their swapchains at once;
//! its later passes can sample what its earlier ones rendered offscreen. It reads back a few pixels
//! of the IDs its passes render, and a small buffer its compute shaders write, which the frame two
//! later, which reuses its resources once it has finished, finds read, with the times its passes
//! took on the GPU. On a device that uploads through a transfer queue, a frame's copies go in a
//! submission of their own, which another timeline paces, and no frame waits for.

mod compute;
pub(crate) mod graph;
mod pass;
mod timer;
mod transients;
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
use crate::{COLOR_RANGE, ColorImage, Device, StorageImage, Swapchain, copy_region, vk_error};
use graph::{Hazard, ImageDecl, PassKind, Use};
use timer::Timer;
use transients::Transients;
use upload::Arena;

pub use compute::Compute;
pub use graph::{BufferId, ComputePass, Graph, ImageId, MAX_BUFFER_READ, PassId, RenderPass};
pub use pass::{Indices, Pass};
pub use upload::Upload;

/// How many frames the GPU works on at once.
pub(crate) const FRAMES_IN_FLIGHT: u64 = 2;

/// The most IDs a frame reads back ([`Frame::read_ids`]).
pub const MAX_READS: usize = 64;

/// The bytes of an ID read back: two 32-bit unsigned integers.
const ID_BYTES: usize = size_of::<[u32; 2]>();

/// What the frames' own arenas hold, which [`Frame::upload`] copies into, and the most a frame has
/// taken of its own, in bytes, as [`Frames::arenas`] gives them: for a run's log, which compares the
/// two, as an arena grows by doubling and keeps what it grew to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arenas {
    /// The bytes of every frame slot's arena's buffers.
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
    /// What frames read buffers back into ([`Frame::read_back`]), [`MAX_BUFFER_READ`] bytes for
    /// each slot, in the slots' order: made by the first read.
    buffer_reads: Option<Buffer>,
    /// The number of the frame whose buffer read the frame being begun gives, 0 for none, how
    /// many bytes it read, and its bytes: [`Frame::buffer_read`]'s.
    buffer_read: (u64, usize, [u8; MAX_BUFFER_READ as usize]),
    /// What each slot's frames read offscreen images back into ([`Frame::read_image`]), in the
    /// slots' order.
    image_reads: Vec<ImageReads>,
    /// For each target of the frame being recorded, its image as the frame's commands so far
    /// leave it. `None` for a swapchain that had no image ready.
    images: Vec<Option<Tracked>>,
    /// The images of the graphs' own.
    transients: Transients,
    /// The passes' timestamps.
    timer: Timer,
    /// The barriers before the pass being recorded, kept so that frames don't allocate.
    barriers: Vec<vk::ImageMemoryBarrier2<'static>>,
    buffer_barriers: Vec<vk::BufferMemoryBarrier2<'static>>,
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
    /// How many bytes that frame read back into the slot's part of `Frames::buffer_reads`.
    buffer_read: usize,
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
    /// The regions of a buffer's write, as [`Frame::write_buffer_pieces`] copies them.
    copies: Vec<vk::BufferCopy>,
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
        // Before anything that `Frames`' drop would destroy, as a failure here would leave it.
        let timer = Timer::new(device)?;
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
            buffer_reads: None,
            buffer_read: (0, 0, [0; MAX_BUFFER_READ as usize]),
            image_reads: (0..FRAMES_IN_FLIGHT)
                .map(|_| ImageReads::default())
                .collect(),
            images: Vec::new(),
            transients: Transients::new(device),
            timer,
            barriers: Vec::new(),
            buffer_barriers: Vec::new(),
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
                buffer_read: 0,
            });
        }
        Ok(frames)
    }

    /// Starts a frame that runs `graph`'s passes, rendering into `targets`, which the graph's
    /// targets are by their indices, once the GPU has finished the frame that last used this
    /// one's resources. The first target paces the frame: for a swapchain, it waits until the
    /// display has let go of an image. The others are rendered into only if their swapchains have
    /// an image to give at once, so that a window on a slower display doesn't slow the frame; an
    /// offscreen image is always rendered into. Returns `None` if the first target has nothing to
    /// render into this time, as with the swapchain of a minimized window.
    ///
    /// The graph's own images are those an earlier frame made, for a graph that declared them
    /// alike, by their names, kinds and sizes, while the uses of those that share memory still
    /// don't overlap; otherwise they're made again, as the frame begins, and those before retire.
    /// A failure to make them fails the frame before it acquires anything.
    ///
    /// Once the frame that last used this one's resources has finished, it copies what that frame
    /// read back ([`Frame::read_ids`], [`Frame::read_back`]), which the frame begun gives
    /// ([`Frame::ids_read`], [`Frame::buffer_read`]), and the time its passes took on the GPU
    /// ([`Frames::gpu_time`]). A frame that doesn't begin leaves what was read for the next that
    /// does: the slot forgets it only as one begins.
    ///
    /// Fails with `gpu.frame_failed` once a frame has failed after acquiring an image, or to
    /// submit or present: the semaphores its acquires signaled may have no wait pending, so no
    /// frame can begin again, and the frames can only be dropped.
    ///
    /// # Panics
    ///
    /// If `targets` is empty or holds one another device made; if the graph uses a target that
    /// `targets` hasn't, or uses a swapchain's other than to draw into it; or if the last frame
    /// was forgotten rather than submitted or dropped.
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
        // The slot's last frame has finished, and its reads and timestamps with it.
        self.find_read(slot)?;
        self.timer.finished(&self.device, slot)?;
        let mut list: Vec<Target<'a>> = std::mem::take(&mut self.spare_targets);
        list.extend(targets);
        assert!(!list.is_empty(), "a frame renders into at least one target");
        // Before any image is acquired, which a panic or a failure would leave unpresented.
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
            self.spare_targets = recycle(list);
            return Err(error);
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
        if let Err(error) = self.start(slot, &mut list, graph.passes.len()) {
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
        self.slots[slot].buffer_read = 0;
        Ok(Some(Frame {
            frames: self,
            targets: list,
            graph,
            next: 0,
            slot,
            submitted: false,
            transfers: None,
            rendering: false,
            pass_begun: false,
        }))
    }

    /// Acquires `targets`' images into `images`, as [`Frames::begin`] says, and, given one for
    /// the first, readies slot `slot`'s arena, commands and timestamps for a frame of `passes`
    /// passes and begins recording them. `images` stays empty if the first target has no image.
    fn start(&mut self, slot: usize, targets: &mut [Target<'_>], passes: usize) -> Result<()> {
        for (i, target) in targets.iter_mut().enumerate() {
            let acquired = self.slots[slot].acquired[i];
            let image = match target {
                Target::Swapchain(swapchain) => {
                    // SAFETY: the semaphore is new, or the frame that last used this slot waited
                    // on it or left it unsignaled and has finished, so it's unsignaled with no
                    // wait pending: a frame forgotten or failed, which could have left it
                    // signaled, keeps frames from beginning.
                    unsafe { swapchain.acquire(acquired, i == 0, &mut self.waited) }?.map(|index| {
                        Tracked {
                            index,
                            layout: vk::ImageLayout::UNDEFINED,
                            hazard: Hazard::ACQUIRED,
                        }
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
            for bind_point in [
                vk::PipelineBindPoint::GRAPHICS,
                vk::PipelineBindPoint::COMPUTE,
            ] {
                raw.cmd_bind_descriptor_sets(
                    commands,
                    bind_point,
                    bindless.pipeline_layout(),
                    0,
                    &[bindless.set(slot)],
                    &[],
                );
            }
        }
        self.timer.begin_frame(&self.device, commands, slot, passes)
    }

    /// Copies what slot `slot`'s last frame read back, once it has finished, for the frame begun
    /// in the slot to give.
    fn find_read(&mut self, slot: usize) -> Result<()> {
        let (frame, count) = self.slots[slot].reads;
        let bytes = self.slots[slot].buffer_read;
        self.buffer_read.0 = frame;
        self.buffer_read.1 = bytes;
        if bytes > 0 {
            let reads = (self.buffer_reads.as_ref())
                .expect("a frame that read a buffer back made what it read into");
            let out = &mut self.buffer_read.2[..bytes];
            reads.read_into(slot as u64 * MAX_BUFFER_READ, out)?;
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

    /// How long the last [`Frames::begin`] waited: for the GPU to finish the frame that last used
    /// this one's resources, and for the display to let go of the first target's image.
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// The time on the GPU of the last frame whose timestamps came back, which a frame's begin
    /// reads from the frame two before it: from its first pass's beginning to its last's end,
    /// their barriers' waits among it. `None` before then, after a frame of no passes, and for a
    /// GPU whose graphics queue writes no timestamps.
    pub fn gpu_time(&self) -> Option<Duration> {
        self.timer.last()
    }

    /// The bytes of GPU memory the graphs' own images are in, as the last frame begun made them,
    /// or kept them: those whose uses in a frame don't overlap share it.
    pub fn transient_bytes(&self) -> u64 {
        self.transients.bytes()
    }

    /// How many frames have been submitted: the number of the last, as frames count from 1. A
    /// [`Frames::begin`] that returns `None` begins none.
    pub fn submitted(&self) -> u64 {
        self.submitted
    }

    /// What the frames' own arenas hold, and the most a frame has taken of its own, as [`Arenas`]
    /// says.
    pub fn arenas(&self) -> Arenas {
        Arenas {
            held: self.arenas.iter().map(Arena::held).sum(),
            peak: self.arenas.iter().map(Arena::peak).max().unwrap_or(0),
        }
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

/// Checks that `graph` uses only targets of `targets`, swapchains' only to draw into, storage
/// images only to write and sample, and only storage images to write.
///
/// # Panics
///
/// If it doesn't.
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
                |use_| matches!(use_, Use::Storage | Use::Sampled | Use::ComputeSampled),
            ),
            Target::Image(_) => (
                "an offscreen image, which no pass writes as storage",
                |use_| *use_ != Use::Storage,
            ),
        };
        let image = ImageId(i as u32);
        let misused = (graph.uses.iter()).find(|&&(used, use_)| used == image && !fits(&use_));
        if let Some((_, use_)) = misused {
            panic!("target {target} is {what}, but one uses it as {use_:?}");
        }
    }
}

/// A target's image, its view and its size: for a swapchain's, its image `index`.
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
            self.timer.destroy(&self.device);
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
    /// The passes it runs.
    graph: &'a Graph,
    /// The graph's next pass to record.
    next: usize,
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

    /// What the frame that last used this one's resources, two before it, read back of a buffer
    /// ([`Frame::read_back`]), which has finished: its number, and the buffer's bytes; none if it
    /// read none, and 0 for the number of no frame, as [`Frame::ids_read`] gives what it read of
    /// IDs.
    pub fn buffer_read(&self) -> (u64, &[u8]) {
        let (frame, bytes, read) = &self.frames.buffer_read;
        (*frame, &read[..*bytes])
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

    /// Checks that `pass` is the graph's next pass to record, and of `kind`, and counts it
    /// recorded.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, or `pass` isn't the next, or of `kind`.
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
        decl
    }

    /// Starts `pass`: its timestamp and its label, and the barriers its uses need after the uses
    /// before them, which put each image it uses in the layout it uses it in.
    ///
    /// # Panics
    ///
    /// If it draws over, samples or copies from a target that no pass has drawn into or written
    /// since it was made, or since its swapchain's image was acquired.
    fn open_pass(&mut self, pass: PassId) {
        let graph = self.graph;
        let decl = &graph.passes[pass.0 as usize];
        // Before anything is recorded, as a refusal records nothing. A storage image's first
        // write finds it undefined, as it says.
        for &(image, use_) in graph.uses(pass) {
            if let ImageDecl::Target(target) = graph.images[image.0 as usize] {
                let tracked = self.frames.images[target]
                    .as_ref()
                    .expect("a pass that records has its targets' images");
                assert!(
                    use_.access().discards
                        || use_ == Use::Storage
                        || tracked.layout != vk::ImageLayout::UNDEFINED,
                    "pass {:?} uses target {target} as {use_:?}, but no pass has drawn into it \
                     or written it since it was made, or since its swapchain's image was acquired",
                    decl.label
                );
            }
        }
        let commands = self.commands();
        let device = &self.frames.device;
        self.frames
            .timer
            .begin(device, commands, self.slot, decl.label);
        // SAFETY: the commands are recording outside a pass.
        unsafe { device.begin_label(commands, decl.label.name()) };
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
            if let Some(barrier) = transients.follow_buffer(buffer, use_.access()) {
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

    /// Ends the pass started last: its label, and its timestamp.
    fn close_pass(&mut self) {
        let commands = self.commands();
        let device = &self.frames.device;
        // SAFETY: the commands are recording outside a pass, with the pass's label open.
        unsafe { device.end_label(commands) };
        self.frames.timer.end(device, commands, self.slot);
    }

    /// `image`'s image, its view and its size, for recording commands that use it: for a target,
    /// the one the frame acquired.
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
        self.frames.slots[self.slot].commands
    }

    /// The index in the bindless set that shaders sample `image` through, which a pass samples.
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

    /// Records `pass`, a pass of the frame's graph that reads an image back, which must be the
    /// next of its passes to record: the image, as the passes before left it, for
    /// [`Frames::image_read`] to give once the frame has finished. The slot's first read, and a
    /// read of a larger image than it read before, makes the buffer it reads into, and fails if it
    /// can't be made.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, `pass` isn't the graph's next, or doesn't read an image
    /// back, or reads a target back that no pass has drawn into since it was made.
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
            // The slot's last frame has finished, and its read with it.
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
        let commands = self.frames.slots[self.slot].commands;
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

    /// Records `pass`, a pass of the frame's graph that reads IDs back, which must be the next of
    /// its passes to record: the IDs at `pixels`, each a pixel's column and row from its top left,
    /// as the passes before left them, the first of them, in order, up to [`MAX_READS`] a frame,
    /// less those this frame has read already. Returns how many it read. They're read once the
    /// frame has finished, which the frame that next uses its resources, two later, waits for as
    /// it begins: [`Frame::ids_read`] gives them there. The first read makes the buffer the frames
    /// read into, of a kibibyte, and fails if it can't be made.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, `pass` isn't the graph's next, or doesn't read IDs back,
    /// or a pixel is outside them.
    pub fn read_ids(&mut self, pass: PassId, pixels: &[[u32; 2]]) -> Result<usize> {
        let [(ids, _)] = *self.graph.uses(pass) else {
            panic!("pass {} reads IDs back from an image alone", pass.0);
        };
        // Before the pass is taken, as a refusal records nothing.
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
        // Whether it reads any, so that the passes after find the IDs where it leaves them.
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
            // read them in; the regions lie within the image and within the slot's part of the
            // buffer, past what the frame read before; and both live until the frame has
            // finished: the image retires until then, and the buffer lives as long as the frames,
            // which wait for theirs before they go.
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

    /// Records `pass`, a pass of the frame's graph that reads a buffer back, which must be the
    /// next of its passes to record: the buffer, as the passes before left it, read once the frame
    /// has finished, which the frame that next uses its resources, two later, waits for as it
    /// begins: [`Frame::buffer_read`] gives it there. A frame reads one buffer back; a later read
    /// takes the place of an earlier one's. The first read makes the buffer the frames read into,
    /// and fails if it can't be made.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, or `pass` isn't the graph's next, or doesn't read a
    /// buffer back.
    pub fn read_back(&mut self, pass: PassId) -> Result<()> {
        self.take_pass(pass, PassKind::ReadBack);
        let [(buffer, _)] = *self.graph.buffer_uses(pass) else {
            unreachable!("a read back uses its buffer alone");
        };
        if self.frames.buffer_reads.is_none() {
            let (device, memory) = (&self.frames.device, Memory::Readback);
            let size = MAX_BUFFER_READ * FRAMES_IN_FLIGHT;
            let usage = vk::BufferUsageFlags::TRANSFER_DST;
            let reads = Buffer::new(device, size, usage, memory, "buffers read")?;
            self.frames.buffer_reads = Some(reads);
        }
        self.open_pass(pass);
        let size = self.graph.buffers[buffer.0 as usize].size;
        let offset = self.slot as u64 * MAX_BUFFER_READ;
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
        // at most `MAX_BUFFER_READ` bytes, and within the slot's part of what it reads into; and
        // both live until the frame has finished: the graph's buffer retires until then, and what
        // it reads into lives as long as the frames, which wait for theirs before they go.
        unsafe {
            raw.cmd_copy_buffer(commands, source, read, slice::from_ref(&region));
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().buffer_memory_barriers(slice::from_ref(&to_host)),
            );
        }
        self.frames.slots[self.slot].buffer_read = size as usize;
        self.close_pass();
        Ok(())
    }

    /// The address that shaders reach `buffer`, one of the graph's own, through during the frame,
    /// for data that a pass reads where it is, as a table of where compute shaders write: a pass
    /// that reaches it must use it as its graph declared, as [`Pass::address`] and
    /// [`Compute::address`] give it.
    ///
    /// # Panics
    ///
    /// If `buffer` isn't the graph's.
    pub fn address(&self, buffer: BufferId) -> u64 {
        self.buffer_parts(buffer).0
    }

    /// The address of `buffer`, one of the graph's own, and its bytes, as the graph declared it.
    fn buffer_parts(&self, buffer: BufferId) -> (u64, u64) {
        let address = self.frames.transients.buffer(buffer).address();
        (address, self.graph.buffers[buffer.0 as usize].size)
    }

    /// Sends the frame's commands to the GPU and presents the images of its swapchains.
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
            // The present waits on a semaphore signaled after every command, the layout change
            // included, so the barrier needs no second scope.
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
            match (target, image) {
                (Target::Image(target), Some(Tracked { layout, .. })) => target.set_layout(*layout),
                (Target::Storage(target), Some(Tracked { layout, .. })) => {
                    target.set_layout(*layout);
                }
                _ => {}
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
