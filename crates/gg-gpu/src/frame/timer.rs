//! Each pass's time on the GPU, from timestamps written as it begins and ends, read back once its
//! frame has finished. With gg-core's `tracy` feature, Tracy shows one GPU timeline per queue. The
//! frame's time, from its first pass's start on either queue to its last's end, goes to
//! [`Frames::gpu_time`](crate::Frames::gpu_time).
//!
//! Both timestamps are written once everything before them on the queue has run through every
//! stage. A pass's time so takes in its barriers' waits, and a queue's passes follow one another
//! on its timeline, none beginning before the one before it ended.
//!
//! Tracy places a timeline against its CPU's clock by pairing a reading of the GPU's clock with its
//! own, as the timeline starts and again each second, so that it doesn't drift. The reading is
//! immediate through `VK_KHR_calibrated_timestamps` where the driver has it. Otherwise it's
//! taken once, by a timestamp the graphics queue writes, which its submission and wait's latency
//! puts behind.

use std::slice;
use std::time::{Duration, Instant};

use ash::vk;
use gg_core::Result;
use gg_core::profile::{self, GpuTimeline, Label};

use super::FRAMES_IN_FLIGHT;
use super::graph::Queue;
use crate::{Device, vk_error, vk_failed};

/// The most timestamps a frame writes on a queue, two per pass: Tracy numbers a timeline's
/// timestamps with 16 bits, and each frame in flight takes half.
const MOST_TIMESTAMPS: u32 = 1 << 15;

/// How often Tracy's timelines are given the GPU's clock again.
const SYNC_EVERY: Duration = Duration::from_secs(1);

/// The timestamps the frames write on a queue, and what they come to.
pub(crate) struct Timer {
    /// Names the queue's timeline and pools.
    name: &'static str,
    /// Each slot's query pool, null until a frame needs one, and how many timestamps it holds.
    pools: Vec<(vk::QueryPool, u32)>,
    /// How many timestamps each slot's last frame wrote, until they're read.
    written: Vec<u32>,
    /// What a slot's timestamps are read into.
    results: Vec<u64>,
    /// The bits of the queue's timestamps that count, those both queues' count; `None` for a
    /// queue that writes none, or that the device hasn't, whose passes go untimed.
    mask: Option<u64>,
    /// Tracy's timeline, with the feature, and when it was last given the GPU's clock.
    timeline: Option<(GpuTimeline, Instant)>,
    /// The first and the last timestamp of the last frame whose timestamps came back.
    span: Option<(u64, u64)>,
}

impl Timer {
    /// A timer for `device`'s frames' passes on `queue`. With gg-core's `tracy` feature, it reads
    /// the GPU's clock for Tracy's timeline.
    pub(crate) fn new(device: &Device, queue: Queue) -> Result<Self> {
        let graphics = device.timestamp_bits();
        let compute = device.compute_timestamp_bits();
        let (bits, name) = match queue {
            Queue::Graphics => (graphics, "graphics queue"),
            Queue::Compute => (compute, "compute queue"),
        };
        let both = if device.compute_family().is_some() {
            graphics.min(compute)
        } else {
            graphics
        };
        let mask = (bits > 0 && both > 0).then(|| u64::MAX >> (64 - both.min(64)));
        let timeline = match mask {
            Some(mask) if profile::ENABLED => {
                let now = match device.read_clock() {
                    Some(now) => now,
                    None => read_clock(device)?,
                };
                let period = device.limits().timestamp_period;
                let timeline = GpuTimeline::new(name, (now & mask) as i64, period);
                timeline.map(|timeline| (timeline, Instant::now()))
            }
            _ => None,
        };
        Ok(Self {
            name,
            pools: vec![(vk::QueryPool::null(), 0); FRAMES_IN_FLIGHT as usize],
            written: vec![0; FRAMES_IN_FLIGHT as usize],
            results: Vec::new(),
            mask,
            timeline,
            span: None,
        })
    }

    /// The first and last timestamp of the last frame whose timestamps came back, the first in the
    /// bits that count and the last as many ticks after it as passed.
    /// `None` before one has, after a frame of no passes on the queue, and for a queue that
    /// writes no timestamps.
    pub(crate) fn span(&self) -> Option<(u64, u64)> {
        self.span
    }

    /// Reads the timestamps slot `slot`'s last frame wrote (it has finished), gives them to
    /// Tracy's timeline and takes the frame's span from them. Also gives Tracy the GPU's clock
    /// again, if it's time and the driver reads it at once.
    pub(crate) fn finished(&mut self, device: &Device, slot: usize) -> Result<()> {
        let written = std::mem::take(&mut self.written[slot]);
        let Some(mask) = self.mask else {
            return Ok(());
        };
        if let Some((timeline, synced)) = &mut self.timeline
            && synced.elapsed() >= SYNC_EVERY
            && let Some(now) = device.read_clock()
        {
            timeline.sync((now & mask) as i64);
            *synced = Instant::now();
        }
        if written == 0 {
            self.span = None;
            return Ok(());
        }
        let (pool, _) = self.pools[slot];
        self.results.clear();
        self.results.resize(written as usize, 0);
        // SAFETY: the pool is live, and its first `written` queries were reset and written by the
        // slot's last frame, which has finished.
        let read = unsafe {
            device.raw().get_query_pool_results(
                pool,
                0,
                &mut self.results,
                vk::QueryResultFlags::TYPE_64,
            )
        };
        match read {
            Ok(()) => {}
            // A frame that wasn't submitted, as one that failed, wrote none of them.
            Err(vk::Result::NOT_READY) => {
                self.span = None;
                return Ok(());
            }
            Err(e) => {
                let pool = format_args!("frame {slot}'s timestamps");
                return Err(vk_error("vkGetQueryPoolResults", pool, e));
            }
        }
        if let Some((timeline, _)) = &self.timeline {
            for (query, &timestamp) in (0..).zip(&self.results) {
                timeline.time(id(slot, query), (timestamp & mask) as i64);
            }
        }
        // Ticks counted past a wrap of the bits that count, if one came between.
        let ticks = self.results[self.results.len() - 1].wrapping_sub(self.results[0]) & mask;
        let first = self.results[0] & mask;
        self.span = Some((first, first.wrapping_add(ticks)));
        Ok(())
    }

    /// Readies slot `slot`'s pool for a frame of `passes` passes on the queue, making a larger pool
    /// if needed, and resets it. `commands`, the queue's first batch's, have begun recording.
    ///
    /// # Panics
    ///
    /// If the frame has more passes than [`MOST_TIMESTAMPS`] allows.
    pub(crate) fn begin_frame(
        &mut self,
        device: &Device,
        commands: vk::CommandBuffer,
        slot: usize,
        passes: usize,
    ) -> Result<()> {
        self.written[slot] = 0;
        if self.mask.is_none() {
            return Ok(());
        }
        let needed = u32::try_from(2 * passes)
            .ok()
            .filter(|&needed| needed <= MOST_TIMESTAMPS)
            .unwrap_or_else(|| {
                panic!(
                    "a frame of {passes} passes on a queue, but frames time at most {} passes",
                    MOST_TIMESTAMPS / 2
                )
            });
        let (pool, capacity) = &mut self.pools[slot];
        if needed > *capacity {
            let grown = needed.next_power_of_two().max(32);
            let info = vk::QueryPoolCreateInfo::default()
                .query_type(vk::QueryType::TIMESTAMP)
                .query_count(grown);
            let raw = device.raw();
            // SAFETY: the slot's last frame has finished, so nothing uses its pool, and the device
            // is live.
            let made = unsafe {
                raw.destroy_query_pool(*pool, None);
                *pool = vk::QueryPool::null();
                *capacity = 0;
                raw.create_query_pool(&info, None)
            };
            let name = format!("frame {slot} {} timestamps", self.name);
            *pool = made.map_err(vk_failed("vkCreateQueryPool", format_args!("{name:?}")))?;
            *capacity = grown;
            // SAFETY: the pool is live.
            unsafe { device.set_name(*pool, &name) };
        }
        if *capacity > 0 {
            // SAFETY: the commands are recording, outside a pass, and the pool is live.
            unsafe {
                device
                    .raw()
                    .cmd_reset_query_pool(commands, *pool, 0, *capacity)
            };
        }
        Ok(())
    }

    /// Writes the timestamp a pass named `label` begins at, into slot `slot`'s pool
    /// ([`Timer::begin_frame`] readied it), in `commands`, which run on the timer's queue after
    /// those it was readied in.
    pub(crate) fn begin(
        &mut self,
        device: &Device,
        commands: vk::CommandBuffer,
        slot: usize,
        label: Label,
    ) {
        if let Some(query) = self.write(device, commands, slot)
            && let Some((timeline, _)) = &self.timeline
        {
            timeline.begin(label, id(slot, query));
        }
    }

    /// Writes the timestamp the pass begun last ends at.
    pub(crate) fn end(&mut self, device: &Device, commands: vk::CommandBuffer, slot: usize) {
        if let Some(query) = self.write(device, commands, slot)
            && let Some((timeline, _)) = &self.timeline
        {
            timeline.end(id(slot, query));
        }
    }

    /// Writes the slot's next timestamp and returns its index; `None` where nothing's timed.
    fn write(&mut self, device: &Device, commands: vk::CommandBuffer, slot: usize) -> Option<u32> {
        self.mask?;
        let query = self.written[slot];
        let (pool, capacity) = self.pools[slot];
        assert!(
            query < capacity,
            "a frame writes the timestamps its passes were counted for"
        );
        // SAFETY: the commands are recording, and the query was reset this frame, earlier on the
        // same queue, and lies within the pool.
        unsafe {
            device.raw().cmd_write_timestamp2(
                commands,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                pool,
                query,
            );
        }
        self.written[slot] += 1;
        Some(query)
    }

    /// Destroys the pools.
    ///
    /// # Safety
    ///
    /// The GPU must have finished every frame.
    pub(crate) unsafe fn destroy(&self, device: &Device) {
        for &(pool, _) in &self.pools {
            // SAFETY: the caller guarantees nothing uses it; null pools are allowed.
            unsafe { device.raw().destroy_query_pool(pool, None) };
        }
    }
}

/// The number Tracy's timeline knows slot `slot`'s `query`th timestamp by.
fn id(slot: usize, query: u32) -> u16 {
    (slot as u32 * MOST_TIMESTAMPS + query) as u16
}

/// What the GPU's clock reads once the graphics queue has run a timestamp's write, which this
/// waits for.
fn read_clock(device: &Device) -> Result<u64> {
    let info = vk::QueryPoolCreateInfo::default()
        .query_type(vk::QueryType::TIMESTAMP)
        .query_count(1);
    // SAFETY: the device is live.
    let pool = unsafe { device.raw().create_query_pool(&info, None) }
        .map_err(vk_failed("vkCreateQueryPool", "\"clock\""))?;
    let mut timestamp = 0;
    // SAFETY: the pool is live; the commands reset its query before writing it, and the query is
    // read once they've run, after which nothing uses the pool.
    let read = unsafe {
        device
            .run_once("read the GPU's clock", |commands| {
                let raw = device.raw();
                raw.cmd_reset_query_pool(commands, pool, 0, 1);
                raw.cmd_write_timestamp2(commands, vk::PipelineStageFlags2::ALL_COMMANDS, pool, 0);
            })
            .and_then(|()| {
                let flags = vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT;
                let results = slice::from_mut(&mut timestamp);
                (device.raw().get_query_pool_results(pool, 0, results, flags))
                    .map_err(vk_failed("vkGetQueryPoolResults", "\"clock\""))
            })
    };
    // SAFETY: as above.
    unsafe { device.raw().destroy_query_pool(pool, None) };
    read.map(|()| timestamp)
}
