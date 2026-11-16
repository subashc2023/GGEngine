//! A frame's batches: the runs of commands a queue records between waiting for the other queue and
//! being waited for. Passes run on the graphics queue, except those added to run on the compute
//! queue, on a device that has one.
//!
//! Each queue's batches signal a timeline of the queue's own as they end. A batch waits on the
//! other's timeline before it begins when its first pass uses a buffer the other queue used last:
//! one queue reads what the other wrote once it's written, and writes what the other read or wrote
//! once that's done. A frame plans its batches as it begins, from its graph's passes and what the
//! frames before left of each buffer, so recording fails nothing. It submits them in planning
//! order, which has each after the batches it waits for.
//!
//! A semaphore's wait takes in all the other queue did before its signal and all its own queue does
//! after the wait, so a batch waits only where it hasn't already waited for as much. What a frame
//! records outside its passes goes in the graphics queue's first batch. That batch ends before the
//! first compute-queue pass, so it may be waited for alone. The frame's timeline is signaled after
//! the graphics queue's last batch, which follows all the compute queue ran: done is done on both.

use super::graph::{Graph, PassId, Queue};

/// How far each queue's timeline is planned, and how far of the other's each has waited for, by
/// [`Queue::index`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Timelines {
    /// The value of each queue's last batch.
    pub(crate) made: [u64; 2],
    /// The highest value of the other queue's timeline each has waited for.
    pub(crate) waited: [u64; 2],
}

/// What a buffer's uses left for its next to wait for: the queue and batch of its last write, and
/// each queue's last batch that read it since.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Crossing {
    written: Option<(Queue, u64)>,
    read: [u64; 2],
}

impl Crossing {
    /// The value of the other queue's timeline a use on `queue` waits for: its last write, if the
    /// other queue made it, and for a write, the other's reads since; 0 for none.
    fn wait(&self, queue: Queue, writes: bool) -> u64 {
        let other = queue.other();
        let written = match self.written {
            Some((by, value)) if by == other => value,
            _ => 0,
        };
        let read = if writes { self.read[other.index()] } else { 0 };
        written.max(read)
    }

    /// Notes a use on `queue`, in its batch of value `value`.
    fn record(&mut self, queue: Queue, writes: bool, value: u64) {
        if writes {
            *self = Self {
                written: Some((queue, value)),
                read: [0; 2],
            };
        } else {
            self.read[queue.index()] = value;
        }
    }
}

/// A planned batch: its queue, its value on that queue's timeline, and the value of the other's
/// it waits for first, 0 for none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Batch {
    pub(crate) queue: Queue,
    pub(crate) value: u64,
    pub(crate) waits: u64,
}

/// A frame's batches in planning order, the first the graphics queue's, each pass's batch, and what
/// the frame leaves of each buffer and the timelines. Kept between frames, so a frame that plans no
/// more than the last allocates nothing.
#[derive(Default)]
pub(crate) struct Plan {
    pub(crate) batches: Vec<Batch>,
    /// Each pass's index in `batches`.
    pub(crate) of_pass: Vec<usize>,
    /// Each of the graph's buffers' crossing, as the frame leaves it.
    pub(crate) crossings: Vec<Crossing>,
    /// The timelines, as the frame leaves them.
    pub(crate) timelines: Timelines,
}

impl Plan {
    /// Plans the batches of a frame of `graph`'s passes, after frames that left the timelines as
    /// `timelines` has them and each buffer as `crossing` gives it, by index. Uses the compute
    /// queue if `computes`, else the graphics queue alone.
    pub(crate) fn make(
        &mut self,
        graph: &Graph,
        computes: bool,
        crossing: impl Fn(usize) -> Crossing,
        timelines: Timelines,
    ) {
        self.batches.clear();
        self.of_pass.clear();
        self.crossings.clear();
        (self.crossings).extend((0..graph.buffers.len()).map(crossing));
        self.timelines = timelines;
        let queue_of = |queue| if computes { queue } else { Queue::Graphics };
        let mut open = [Some(self.begin(Queue::Graphics, 0)), None];
        if (graph.passes.iter()).any(|pass| queue_of(pass.queue) == Queue::Compute) {
            open[Queue::Graphics.index()] = None;
        }
        for (i, pass) in graph.passes.iter().enumerate() {
            let queue = queue_of(pass.queue);
            let (own, other) = (queue.index(), queue.other().index());
            let uses = graph.buffer_uses(PassId(i as u32));
            let wait = (uses.iter())
                .map(|&(buffer, use_)| self.crossings[buffer.0 as usize].wait(queue, use_.writes()))
                .max()
                .unwrap_or(0);
            if wait > self.timelines.waited[own] {
                // The other queue's batch being planned ends here if it's what's waited for.
                if open[other].is_some_and(|batch| self.batches[batch].value == wait) {
                    open[other] = None;
                }
                open[own] = Some(self.begin(queue, wait));
            }
            let batch = *open[own].get_or_insert_with(|| self.begin(queue, 0));
            let value = self.batches[batch].value;
            for &(buffer, use_) in uses {
                self.crossings[buffer.0 as usize].record(queue, use_.writes(), value);
            }
            self.of_pass.push(batch);
        }
        let computed = self.timelines.made[Queue::Compute.index()];
        if computed > self.timelines.waited[Queue::Graphics.index()] {
            self.begin(Queue::Graphics, computed);
        }
    }

    /// Plans a batch on `queue` that first waits for `waits` of the other's timeline; returns its
    /// index.
    fn begin(&mut self, queue: Queue, waits: u64) -> usize {
        let own = queue.index();
        self.timelines.made[own] += 1;
        let waited = &mut self.timelines.waited[own];
        *waited = (*waited).max(waits);
        self.batches.push(Batch {
            queue,
            value: self.timelines.made[own],
            waits,
        });
        self.batches.len() - 1
    }

    /// The last of the graphics queue's batches, which the frame's timeline is signaled after.
    pub(crate) fn last_graphics(&self) -> usize {
        (self.batches.iter())
            .rposition(|batch| batch.queue == Queue::Graphics)
            .expect("a frame's first batch is the graphics queue's")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::graph::BufferId;
    use gg_core::label;

    const G: Queue = Queue::Graphics;
    const C: Queue = Queue::Compute;

    fn batch(queue: Queue, value: u64, waits: u64) -> Batch {
        Batch {
            queue,
            value,
            waits,
        }
    }

    /// A frame as the renderer's are: a compute-queue pass that writes a buffer, beside graphics
    /// passes, then a graphics pass that reads it, then one more.
    fn frame(graph: &mut Graph) -> BufferId {
        graph.clear();
        let shadows = graph.buffer("shadows", 64);
        let clusters = graph.buffer("clusters", 64);
        graph.compute(label!("cull")).write_buffer(shadows).add();
        let target = graph.target(0);
        (graph.render(label!("shadow")).clear(target, [0.0; 4]))
            .read_buffer(shadows)
            .add();
        (graph.compute(label!("clusters")).on_compute_queue())
            .write_buffer(clusters)
            .add();
        (graph.render(label!("view")).keep(target))
            .read_buffer(clusters)
            .add();
        graph.render(label!("ui")).keep(target).add();
        clusters
    }

    fn plan(graph: &Graph, computes: bool, crossings: &[Crossing], timelines: Timelines) -> Plan {
        let mut plan = Plan::default();
        let crossing = |i: usize| crossings.get(i).copied().unwrap_or_default();
        plan.make(graph, computes, crossing, timelines);
        plan
    }

    #[test]
    fn a_frame_without_a_compute_queue_is_one_batch() {
        let mut graph = Graph::default();
        frame(&mut graph);
        let plan = plan(&graph, false, &[], Timelines::default());
        assert_eq!(plan.batches, [batch(G, 1, 0)]);
        assert_eq!(plan.of_pass, [0; 5]);
        assert_eq!(plan.last_graphics(), 0);
    }

    /// The graphics queue's first batch ends before the passes. The compute pass's batch begins
    /// without waiting for the graphics passes before it. The graphics pass that reads what it
    /// wrote begins the frame's last batch, which waits for it.
    #[test]
    fn a_compute_pass_runs_beside_the_graphics_passes_before_what_takes_its_buffer() {
        let mut graph = Graph::default();
        let clusters = frame(&mut graph);
        let first = plan(&graph, true, &[], Timelines::default());
        assert_eq!(
            first.batches,
            [
                batch(G, 1, 0),
                batch(G, 2, 0),
                batch(C, 1, 0),
                batch(G, 3, 1)
            ]
        );
        assert_eq!(first.of_pass, [1, 1, 2, 3, 3]);
        assert_eq!(first.last_graphics(), 3);
        assert_eq!(
            first.timelines,
            Timelines {
                made: [3, 1],
                waited: [1, 0],
            }
        );
        // The next frame's compute pass writes what the last frame's view read, so it waits for
        // the graphics queue's last batch, which the frame's own batches all follow.
        let crossings = first.crossings.clone();
        let next = plan(&graph, true, &crossings, first.timelines);
        assert_eq!(
            next.batches,
            [
                batch(G, 4, 0),
                batch(G, 5, 0),
                batch(C, 2, 3),
                batch(G, 6, 2)
            ]
        );
        assert_eq!(
            next.crossings[clusters.0 as usize],
            Crossing {
                written: Some((C, 2)),
                read: [6, 0],
            }
        );
    }

    /// A graphics batch at the frame's end waits for the compute work nothing else took, so the
    /// frame's timeline follows all the frame ran. A compute-queue pass that reads what a graphics
    /// pass wrote waits for it, ending the graphics batch there.
    #[test]
    fn the_frame_ends_after_all_the_compute_queue_ran() {
        let mut graph = Graph::default();
        let data = graph.buffer("data", 64);
        let target = graph.target(0);
        graph.compute(label!("write")).write_buffer(data).add();
        graph.render(label!("draw")).clear(target, [0.0; 4]).add();
        (graph.compute(label!("read")).on_compute_queue())
            .read_buffer(data)
            .add();
        graph.render(label!("more")).keep(target).add();
        let plan = plan(&graph, true, &[], Timelines::default());
        assert_eq!(
            plan.batches,
            [
                batch(G, 1, 0),
                batch(G, 2, 0),
                batch(C, 1, 2),
                batch(G, 3, 0),
                batch(G, 4, 1)
            ]
        );
        assert_eq!(plan.of_pass, [1, 1, 2, 3]);
        assert_eq!(plan.last_graphics(), 4);
    }

    /// A write waits for the other queue's reads since the last write, not only the write; a queue
    /// doesn't wait again for what it has waited for.
    #[test]
    fn a_write_waits_for_the_reads_on_the_other_queue_and_waits_arent_repeated() {
        let mut crossing = Crossing::default();
        crossing.record(C, true, 4);
        assert_eq!(crossing.wait(G, false), 4, "a read after the other's write");
        assert_eq!(crossing.wait(C, true), 0, "its own queue's write");
        crossing.record(G, false, 7);
        assert_eq!(crossing.wait(C, false), 0, "reads don't wait for reads");
        assert_eq!(crossing.wait(C, true), 7, "a write after the other's read");

        let mut graph = Graph::default();
        let (a, b) = (graph.buffer("a", 4), graph.buffer("b", 4));
        graph
            .compute(label!("both"))
            .write_buffer(a)
            .write_buffer(b)
            .add();
        for buffer in [a, b] {
            (graph.compute(label!("read")).on_compute_queue())
                .read_buffer(buffer)
                .add();
        }
        let plan = plan(&graph, true, &[], Timelines::default());
        // The second read waits for no more than the first.
        assert_eq!(
            plan.batches,
            [
                batch(G, 1, 0),
                batch(G, 2, 0),
                batch(C, 1, 2),
                batch(G, 3, 1)
            ]
        );
        assert_eq!(plan.of_pass, [1, 2, 2]);
    }
}
