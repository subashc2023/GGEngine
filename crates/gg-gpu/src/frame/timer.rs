//! Each pass's time on the GPU, from timestamps the GPU writes as the pass begins and as it ends,
//! which come back once its frame has finished: Tracy's GPU timeline shows each pass, with
//! gg-core's `tracy` feature, and the frame's time on the GPU, from its first pass's beginning to
//! its last's end, goes to [`Frames::gpu_time`](crate::Frames::gpu_time).
//!
//! Both timestamps are written once everything before them has run through every stage, so that a
//! pass's time takes in its barriers' waits, and passes follow one another on the timeline, none
//! beginning before the one before it ended.

use std::slice;
use std::time::Duration;

use ash::vk;
use gg_core::Result;
use gg_core::profile::{self, GpuTimeline, Label};

use super::FRAMES_IN_FLIGHT;
use crate::{Device, vk_error};

/// The most timestamps a frame writes, two for each pass: Tracy numbers a timeline's timestamps
/// with 16 bits, and each frame in flight takes half of them.
const MOST_TIMESTAMPS: u32 = 1 << 15;

/// The timestamps the frames write, and what they come to.
pub(crate) struct Timer {
    /// Each slot's query pool, null until a frame needs one, and how many timestamps it holds.
    pools: Vec<(vk::QueryPool, u32)>,
    /// How many timestamps each slot's last frame wrote, until they're read.
    written: Vec<u32>,
    /// What a slot's timestamps are read into.
    results: Vec<u64>,
    /// The nanoseconds a tick of the GPU's clock takes, and the bits of its timestamps that count;
    /// `None` for a graphics queue that writes none, whose passes go untimed.
    clock: Option<(f64, u64)>,
    /// Tracy's timeline, with the feature.
    timeline: Option<GpuTimeline>,
    /// The time on the GPU of the last frame whose timestamps came back.
    last: Option<Duration>,
}

impl Timer {
    /// A timer for `device`'s frames, which, with gg-core's `tracy` feature, reads the GPU's clock
    /// once for Tracy's timeline, waiting for the GPU to write it.
    pub(crate) fn new(device: &Device) -> Result<Self> {
        let bits = device.timestamp_bits();
        let clock = (bits > 0).then(|| {
            let mask = u64::MAX >> (64 - bits.min(64));
            (f64::from(device.limits().timestamp_period), mask)
        });
        let timeline = match clock {
            Some((period, mask)) if profile::ENABLED => {
                let now = (read_clock(device)? & mask) as i64;
                GpuTimeline::new("graphics queue", now, period as f32)
            }
            _ => None,
        };
        Ok(Self {
            pools: vec![(vk::QueryPool::null(), 0); FRAMES_IN_FLIGHT as usize],
            written: vec![0; FRAMES_IN_FLIGHT as usize],
            results: Vec::new(),
            clock,
            timeline,
            last: None,
        })
    }

    /// The time on the GPU of the last frame whose timestamps came back: from its first pass's
    /// beginning to its last's end. `None` before one has, after a frame of no passes, and for a
    /// GPU whose graphics queue writes no timestamps.
    pub(crate) fn last(&self) -> Option<Duration> {
        self.last
    }

    /// Reads the timestamps that slot `slot`'s last frame wrote, which has finished, gives them to
    /// Tracy's timeline, and takes the frame's time from them.
    pub(crate) fn finished(&mut self, device: &Device, slot: usize) -> Result<()> {
        let written = std::mem::take(&mut self.written[slot]);
        let Some((period, mask)) = self.clock.filter(|_| written > 0) else {
            return Ok(());
        };
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
                self.last = None;
                return Ok(());
            }
            Err(e) => {
                let pool = format_args!("frame {slot}'s timestamps");
                return Err(vk_error("vkGetQueryPoolResults", pool, e));
            }
        }
        if let Some(timeline) = &self.timeline {
            for (query, &timestamp) in (0..).zip(&self.results) {
                timeline.time(id(slot, query), (timestamp & mask) as i64);
            }
        }
        let ticks = self.results[self.results.len() - 1].wrapping_sub(self.results[0]) & mask;
        self.last = Some(Duration::from_nanos((ticks as f64 * period) as u64));
        Ok(())
    }

    /// Readies slot `slot`'s pool for a frame of `passes` passes, whose `commands` have begun
    /// recording, making a larger one if it needs one, and resets it.
    ///
    /// # Panics
    ///
    /// If the frame has more passes than [`MOST_TIMESTAMPS`] times.
    pub(crate) fn begin_frame(
        &mut self,
        device: &Device,
        commands: vk::CommandBuffer,
        slot: usize,
        passes: usize,
    ) -> Result<()> {
        self.written[slot] = 0;
        if self.clock.is_none() {
            return Ok(());
        }
        let needed = u32::try_from(2 * passes)
            .ok()
            .filter(|&needed| needed <= MOST_TIMESTAMPS)
            .unwrap_or_else(|| {
                panic!(
                    "a frame of {passes} passes, but frames time at most {} passes",
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
            let name = format!("frame {slot} timestamps");
            *pool = made.map_err(|e| vk_error("vkCreateQueryPool", format_args!("{name:?}"), e))?;
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

    /// Writes the timestamp a pass named `label` begins at, into slot `slot`'s pool, which
    /// [`Timer::begin_frame`] readied for it.
    pub(crate) fn begin(
        &mut self,
        device: &Device,
        commands: vk::CommandBuffer,
        slot: usize,
        label: Label,
    ) {
        if let Some(query) = self.write(device, commands, slot)
            && let Some(timeline) = &self.timeline
        {
            timeline.begin(label, id(slot, query));
        }
    }

    /// Writes the timestamp the pass begun last ends at.
    pub(crate) fn end(&mut self, device: &Device, commands: vk::CommandBuffer, slot: usize) {
        if let Some(query) = self.write(device, commands, slot)
            && let Some(timeline) = &self.timeline
        {
            timeline.end(id(slot, query));
        }
    }

    /// Writes the slot's next timestamp, and returns which it is; `None` where nothing's timed.
    fn write(&mut self, device: &Device, commands: vk::CommandBuffer, slot: usize) -> Option<u32> {
        self.clock?;
        let query = self.written[slot];
        let (pool, capacity) = self.pools[slot];
        assert!(
            query < capacity,
            "a frame writes the timestamps its passes were counted for"
        );
        // SAFETY: the commands are recording, and the query was reset this frame and lies within
        // the pool.
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

/// What the GPU's clock reads, once the graphics queue has run a timestamp's write, which this
/// waits for.
fn read_clock(device: &Device) -> Result<u64> {
    let info = vk::QueryPoolCreateInfo::default()
        .query_type(vk::QueryType::TIMESTAMP)
        .query_count(1);
    // SAFETY: the device is live.
    let pool = unsafe { device.raw().create_query_pool(&info, None) }
        .map_err(|e| vk_error("vkCreateQueryPool", "\"clock\"", e))?;
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
                    .map_err(|e| vk_error("vkGetQueryPoolResults", "\"clock\"", e))
            })
    };
    // SAFETY: as above.
    unsafe { device.raw().destroy_query_pool(pool, None) };
    read.map(|()| timestamp)
}
