//! A graph's own images and buffers, for a frame.
//!
//! Images are made from the kinds and sizes the passes declare, each at the start of a block of
//! memory shared by images whose uses in a frame don't overlap. They're kept for later frames that
//! declare them alike, while uses within a block still don't overlap. A block remembers what the
//! last use of any image in it left, which the first use of another, in the frame or the next,
//! waits for. Each image also remembers what its own last use left, which its next waits for too.
//!
//! Buffers each have memory of their own, kept for later frames that declare one of their name no
//! larger. Each remembers what its last use on each queue left, and its uses across the two, which
//! frames plan their batches by.

use std::sync::Arc;

use ash::vk;
use gg_core::Result;
use tracing::trace;

use super::batches::Crossing;
use super::graph::{
    Access, Barrier, BufferId, Graph, Hazard, ImageDecl, ImageId, ImageKind, Queue,
};
use crate::Device;
use crate::buffer::Buffer;
use crate::device::Retired;
use crate::image::{check_size, image_info, image_view};
use crate::vma::{Allocation, Memory};

/// The images and buffers made for the graphs' own, and the blocks of memory the images are in.
pub(crate) struct Transients {
    device: Arc<Device>,
    made: Vec<Made>,
    blocks: Vec<Block>,
    /// For each of the graph's images, by index, the one made for it, if it's the graph's own.
    of: Vec<Option<usize>>,
    /// The first and last passes that use each of the graph's images, if any does.
    lifetimes: Vec<Option<(usize, usize)>>,
    /// Which of `made` the graph's images have matched so far, as it's prepared.
    taken: Vec<bool>,
    buffers: Vec<MadeBuffer>,
    /// For each of the graph's buffers, by index, the one made for it, in `buffers`.
    buffer_of: Vec<usize>,
}

/// A buffer made for one of a graph's own.
struct MadeBuffer {
    name: &'static str,
    buffer: Buffer,
    /// What its last use on each queue left, by [`Queue::index`], in the frame or one before.
    /// The queue's next use waits for it by a barrier, a frame's first across to the frame
    /// before's last. A write on one queue leaves nothing on the other for a barrier to wait for:
    /// a semaphore orders what comes before it there and what comes after.
    hazards: [Hazard; 2],
    /// Its uses on the two queues, which a use on one waits for on the other by a semaphore.
    crossing: Crossing,
}

/// An image made for one of a graph's own.
pub(crate) struct Made {
    name: &'static str,
    pub(crate) kind: ImageKind,
    pub(crate) size: [u32; 2],
    pub(crate) raw: vk::Image,
    pub(crate) view: vk::ImageView,
    /// Its slot in the bindless set, if shaders sample its kind.
    pub(crate) index: Option<u32>,
    /// Its view as a storage image (a 2D array of its one layer, as shaders declare them) and that
    /// view's storage slot in the bindless set, if compute shaders write its kind.
    pub(crate) storage: Option<(vk::ImageView, u32)>,
    /// The block it's in, in `Transients::blocks`.
    block: usize,
    /// The layout the frame's commands so far leave it in: undefined as each frame begins.
    pub(crate) layout: vk::ImageLayout,
    /// What its own last use left, which its next waits for as well as for its block's. A barrier
    /// on another image that used the block since is that image's, and leaves this one's own
    /// writes (a depth image's at the fragment tests) unwaited for, as synchronization validation
    /// finds.
    hazard: Hazard,
}

/// Memory that images are made in.
struct Block {
    /// Taken as it's retired.
    allocation: Option<Allocation>,
    /// Its bytes.
    size: u64,
    /// What the last use of any image in it left.
    hazard: Hazard,
}

impl Transients {
    pub(crate) fn new(device: &Arc<Device>) -> Self {
        Self {
            device: device.clone(),
            made: Vec::new(),
            blocks: Vec::new(),
            of: Vec::new(),
            lifetimes: Vec::new(),
            taken: Vec::new(),
            buffers: Vec::new(),
            buffer_of: Vec::new(),
        }
    }

    /// Readies the images for `graph`'s own. Those made for an earlier frame are kept if each was
    /// made by name, kind and size, and the uses of those sharing a block don't overlap. Otherwise
    /// new ones are made in new blocks, the old retiring until the frames using them have
    /// finished. Each is undefined until a pass of the frame writes it.
    pub(crate) fn prepare(&mut self, graph: &Graph) -> Result<()> {
        let _zone = gg_core::zone!();
        self.lifetimes.clear();
        (self.lifetimes).extend((0..graph.images.len()).map(|i| graph.lifetime(ImageId(i as u32))));
        self.taken.clear();
        self.taken.resize(self.made.len(), false);
        self.of.clear();
        let mut matched = true;
        for decl in &graph.images {
            let ImageDecl::Transient { name, kind, size } = *decl else {
                self.of.push(None);
                continue;
            };
            let found = (self.made.iter().zip(&self.taken)).position(|(made, &taken)| {
                !taken && (made.name, made.kind, made.size) == (name, kind, size)
            });
            if let Some(found) = found {
                self.taken[found] = true;
            }
            matched &= found.is_some();
            self.of.push(found);
        }
        if !matched || self.overlap() {
            self.make(graph)?;
        }
        for made in &mut self.made {
            made.layout = vk::ImageLayout::UNDEFINED;
        }
        self.prepare_buffers(graph)
    }

    /// Readies the buffers for `graph`'s own: the one made earlier of the same name if it holds
    /// enough bytes, else a new one of the next power of two bytes, so that a buffer declared a
    /// little larger than last frame's isn't remade every frame. Old buffers no buffer of the
    /// graph's takes retire until the frames using them have finished.
    fn prepare_buffers(&mut self, graph: &Graph) -> Result<()> {
        let wanted = |made: &MadeBuffer| {
            (graph.buffers.iter())
                .any(|decl| decl.name == made.name && decl.size <= made.buffer.size())
        };
        self.buffers.retain(wanted);
        self.buffer_of.clear();
        for decl in &graph.buffers {
            let found = self.buffers.iter().position(|made| made.name == decl.name);
            let index = match found {
                Some(found) => found,
                None => {
                    let usage = vk::BufferUsageFlags::STORAGE_BUFFER
                        | vk::BufferUsageFlags::INDIRECT_BUFFER
                        | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
                        | vk::BufferUsageFlags::TRANSFER_SRC;
                    let size = decl.size.next_power_of_two();
                    let buffer = Buffer::new(&self.device, size, usage, Memory::Gpu, decl.name)?;
                    trace!(name = decl.name, bytes = size, "Transient buffer made");
                    self.buffers.push(MadeBuffer {
                        name: decl.name,
                        buffer,
                        hazards: [Hazard::default(); 2],
                        crossing: Crossing::default(),
                    });
                    self.buffers.len() - 1
                }
            };
            self.buffer_of.push(index);
        }
        Ok(())
    }

    /// Whether the uses of two of the graph's images overlap in a block they share.
    fn overlap(&self) -> bool {
        let used = (self.of.iter().zip(&self.lifetimes))
            .filter_map(|(&made, &lifetime)| Some((self.made[made?].block, lifetime?)));
        used.clone().enumerate().any(|(i, (block, (first, last)))| {
            used.clone()
                .skip(i + 1)
                .any(|(other, (other_first, other_last))| {
                    other == block && first <= other_last && other_first <= last
                })
        })
    }

    /// Makes the images for `graph`'s own and their blocks, retiring those made before.
    fn make(&mut self, graph: &Graph) -> Result<()> {
        // Before anything retires, so that a refused graph leaves the images made before.
        for decl in &graph.images {
            if let &ImageDecl::Transient { name, size, .. } = decl {
                check_size(&self.device, size, name)?;
            }
        }
        self.retire();
        let raw = self.device.raw();
        let mut wanted = Vec::new();
        for (i, decl) in graph.images.iter().enumerate() {
            if let &ImageDecl::Transient { kind, size, .. } = decl {
                let info = image_info(size, kind.format(), kind.usage());
                let query = vk::DeviceImageMemoryRequirements::default().create_info(&info);
                let mut requirements = vk::MemoryRequirements2::default();
                // SAFETY: the device is live, and the info describes an image it can make.
                unsafe { raw.get_device_image_memory_requirements(&query, &mut requirements) };
                wanted.push((i, requirements.memory_requirements, self.lifetimes[i]));
            }
        }
        let shares: Vec<_> = wanted.iter().map(|&(_, need, life)| (need, life)).collect();
        let (blocks, block_of) = assign(&shares);
        for (i, requirements) in blocks.iter().enumerate() {
            let name = format!("transient block {i}");
            let allocation = self
                .device
                .allocator()
                .allocate_memory(requirements, &name)?;
            self.blocks.push(Block {
                allocation: Some(allocation),
                size: requirements.size,
                hazard: Hazard::default(),
            });
        }
        for (&(i, _, _), block) in wanted.iter().zip(block_of) {
            let ImageDecl::Transient { name, kind, size } = graph.images[i] else {
                unreachable!("only the graph's own were wanted");
            };
            let info = image_info(size, kind.format(), kind.usage());
            let allocation = (self.blocks[block].allocation.as_ref())
                .expect("a block is retired only with its images");
            // SAFETY: the block meets the requirements of every image assigned to it.
            let image = unsafe {
                (self.device.allocator()).create_aliasing_image(allocation, &info, name)
            }?;
            let (format, range) = (kind.format(), kind.range());
            let flat = vk::ImageViewType::TYPE_2D;
            let view = match image_view(&self.device, image, flat, format, range, name) {
                Ok(view) => view,
                Err(error) => {
                    self.device
                        .retire(Retired::Alias(image, vk::ImageView::null(), None));
                    return Err(error);
                }
            };
            let index = if kind.sampled() {
                // SAFETY: the view is of a live sampled image of the device, which shaders sample
                // only through a pass that samples it, once its barrier has put it in the
                // shader-read-only layout.
                match unsafe { self.device.bindless().add(raw, view, None, name) } {
                    Ok(index) => Some(index),
                    Err(error) => {
                        self.device.retire(Retired::Alias(image, view, None));
                        return Err(error);
                    }
                }
            } else {
                None
            };
            let storage = if kind.storage() {
                let layers = vk::ImageViewType::TYPE_2D_ARRAY;
                let made = image_view(&self.device, image, layers, format, range, name).and_then(
                    |storage| {
                        // SAFETY: the view is of a live storage image of the device, which shaders
                        // write only through a pass that writes it, once its barrier has put it in
                        // the general layout; the slot is free as it's taken.
                        let slot =
                            unsafe { self.device.bindless().add_storage(raw, storage, name) };
                        slot.map(|slot| (storage, slot)).inspect_err(|_| {
                            // SAFETY: nothing has used the view, which no slot holds.
                            unsafe { raw.destroy_image_view(storage, None) };
                        })
                    },
                );
                match made {
                    Ok(storage) => Some(storage),
                    Err(error) => {
                        self.device.retire(Retired::Alias(image, view, index));
                        return Err(error);
                    }
                }
            } else {
                None
            };
            self.made.push(Made {
                name,
                kind,
                size,
                raw: image,
                view,
                index,
                storage,
                block,
                layout: vk::ImageLayout::UNDEFINED,
                hazard: Hazard::default(),
            });
            self.of[i] = Some(self.made.len() - 1);
        }
        let (images, blocks, bytes) = (self.made.len(), self.blocks.len(), self.bytes());
        trace!(images, blocks, bytes, "Transient images made");
        Ok(())
    }

    /// Retires every image and block until the frames using them have finished.
    fn retire(&mut self) {
        for made in self.made.drain(..) {
            // Its storage view before the image it's of.
            if let Some((view, slot)) = made.storage {
                self.device.retire(Retired::StorageView(view, slot));
            }
            (self.device).retire(Retired::Alias(made.raw, made.view, made.index));
        }
        for block in self.blocks.drain(..) {
            if let Some(allocation) = block.allocation {
                self.device.retire(Retired::Memory(allocation));
            }
        }
        self.of.iter_mut().for_each(|made| *made = None);
    }

    /// The image made for the graph's `image`.
    ///
    /// # Panics
    ///
    /// If `image` isn't one of the graph's own.
    pub(crate) fn image(&self, image: ImageId) -> &Made {
        &self.made[self.made_for(image)]
    }

    /// The barrier, if needed, that lets `access`, a use of the graph's `image`, follow what the
    /// uses so far of the image and its block left. The use then leaves its own for both, and the
    /// layout it leaves the image in.
    ///
    /// # Panics
    ///
    /// If `image` isn't one of the graph's own.
    pub(crate) fn follow(&mut self, image: ImageId, access: Access) -> Option<Barrier> {
        let made = &mut self.made[self.of[image.0 as usize].expect("one of the graph's own")];
        let block = &mut self.blocks[made.block].hazard;
        // Within a frame they're the same once the image's first use has left them: no other
        // image uses the block between its uses.
        let mut hazard = made.hazard.and(*block);
        let barrier = hazard.follow(&mut made.layout, access);
        (made.hazard, *block) = (hazard, hazard);
        barrier
    }

    /// The buffer made for the graph's `buffer`.
    pub(crate) fn buffer(&self, buffer: BufferId) -> &Buffer {
        &self.buffers[self.buffer_of[buffer.0 as usize]].buffer
    }

    /// The barrier, if needed, on `queue` that lets `access`, a use there of the graph's `buffer`,
    /// follow what the buffer's uses so far on that queue left. The use then leaves its own. A
    /// write to a buffer nothing has used on the queue since the other queue wrote it needs none.
    pub(crate) fn follow_buffer(
        &mut self,
        buffer: BufferId,
        queue: Queue,
        access: Access,
    ) -> Option<Barrier> {
        let made = &mut self.buffers[self.buffer_of[buffer.0 as usize]];
        // Buffers have no layouts: undefined follows undefined without a transition.
        let mut layout = vk::ImageLayout::UNDEFINED;
        let barrier = made.hazards[queue.index()].follow(&mut layout, access);
        if !access.writes.is_empty() {
            made.hazards[queue.other().index()] = Hazard::default();
        }
        barrier.filter(|barrier| !barrier.src_stages.is_empty())
    }

    /// What the graph's `buffer`'s uses on the two queues left, for planning a frame's batches.
    pub(crate) fn crossing(&self, buffer: BufferId) -> Crossing {
        self.buffers[self.buffer_of[buffer.0 as usize]].crossing
    }

    /// Takes what a frame's uses of the graph's buffers leave on the two queues, by buffer index,
    /// once the frame has begun.
    pub(crate) fn cross(&mut self, crossings: &[Crossing]) {
        for (&made, &crossing) in self.buffer_of.iter().zip(crossings) {
            self.buffers[made].crossing = crossing;
        }
    }

    /// The bytes of the blocks of memory the images are in, and of the buffers.
    pub(crate) fn bytes(&self) -> u64 {
        let buffers = self.buffers.iter().map(|made| made.buffer.size());
        self.blocks
            .iter()
            .map(|block| block.size)
            .chain(buffers)
            .sum()
    }

    /// The last pass that uses the graph's `image`, if it's one of the graph's own.
    pub(crate) fn last_use(&self, image: ImageId) -> Option<usize> {
        self.of[image.0 as usize]?;
        self.lifetimes[image.0 as usize].map(|(_, last)| last)
    }

    fn made_for(&self, image: ImageId) -> usize {
        self.of[image.0 as usize].expect("one of the graph's own")
    }
}

impl Drop for Transients {
    fn drop(&mut self) {
        self.retire();
    }
}

/// The blocks of memory that images of these requirements (each used by the passes from its first
/// to its last, if any) go in, and the block each goes in. Each is at its block's start, and never
/// shares a block with an image whose uses overlap its own or whose memory types share none with
/// its own.
///
/// Images are placed in the order of their first uses, each in the smallest block free by then
/// that holds it, else the largest, which grows, else a block of its own.
fn assign(
    images: &[(vk::MemoryRequirements, Option<(usize, usize)>)],
) -> (Vec<vk::MemoryRequirements>, Vec<usize>) {
    let mut order: Vec<usize> = (0..images.len()).collect();
    order.sort_by_key(|&i| images[i].1.map(|(first, _)| first));
    // Each block's requirements, and the last pass that uses any image in it.
    let mut blocks: Vec<(vk::MemoryRequirements, Option<usize>)> = Vec::new();
    let mut block_of = vec![0; images.len()];
    for i in order {
        let (need, lifetime) = images[i];
        let free = |(block, last): &(vk::MemoryRequirements, Option<usize>)| {
            block.memory_type_bits & need.memory_type_bits != 0
                && match (last, lifetime) {
                    (Some(last), Some((first, _))) => *last < first,
                    _ => true,
                }
        };
        let holding = (blocks.iter().enumerate())
            .filter(|(_, block)| free(block) && block.0.size >= need.size)
            .min_by_key(|(_, (block, _))| block.size);
        let largest = || {
            (blocks.iter().enumerate())
                .filter(|(_, block)| free(block))
                .max_by_key(|(_, (block, _))| block.size)
        };
        let chosen = holding.or_else(largest).map(|(index, _)| index);
        let index = chosen.unwrap_or_else(|| {
            blocks.push((
                vk::MemoryRequirements {
                    size: 0,
                    alignment: 1,
                    memory_type_bits: need.memory_type_bits,
                },
                None,
            ));
            blocks.len() - 1
        });
        let (block, last) = &mut blocks[index];
        block.size = block.size.max(need.size);
        block.alignment = block.alignment.max(need.alignment);
        block.memory_type_bits &= need.memory_type_bits;
        if let Some((_, end)) = lifetime {
            *last = Some(last.map_or(end, |last| last.max(end)));
        }
        block_of[i] = index;
    }
    (
        blocks.into_iter().map(|(block, _)| block).collect(),
        block_of,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn need(size: u64, memory_type_bits: u32) -> vk::MemoryRequirements {
        vk::MemoryRequirements {
            size,
            alignment: 256,
            memory_type_bits,
        }
    }

    #[test]
    fn images_used_apart_share_blocks_and_those_used_together_dont() {
        // A view's colors, IDs and depth are used by the first pass, the depth by it alone, so
        // that a larger color image drawn from the second pass on takes the depth's block, which
        // grows; and an image no pass uses, placed first, whose block the next takes.
        let (blocks, block_of) = assign(&[
            (need(400, 0b11), Some((0, 3))),
            (need(800, 0b11), Some((0, 2))),
            (need(400, 0b01), Some((0, 0))),
            (need(600, 0b11), Some((1, 3))),
            (need(100, 0b11), None),
        ]);
        assert_eq!(block_of, [0, 1, 2, 2, 0]);
        let sizes: Vec<u64> = blocks.iter().map(|block| block.size).collect();
        assert_eq!(sizes, [400, 800, 600]);
        assert_eq!(
            blocks[2].memory_type_bits, 0b01,
            "the types both images take"
        );
    }

    #[test]
    fn images_of_memory_types_apart_never_share_a_block() {
        let (blocks, block_of) = assign(&[
            (need(64, 0b01), Some((0, 0))),
            (need(64, 0b10), Some((1, 1))),
        ]);
        assert_eq!((blocks.len(), block_of), (2, vec![0, 1]));
    }
}
