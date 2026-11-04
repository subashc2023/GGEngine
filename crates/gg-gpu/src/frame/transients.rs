//! A graph's own images, which last for a frame: made from the kinds and sizes its passes declare,
//! each at the start of a block of memory that images whose uses in a frame don't overlap share,
//! and kept for the frames after, which declare them alike, while their uses still don't overlap
//! within a block. Each block remembers what the last use of any image in it left, so that the
//! first use of another, in the frame or the next, waits for it; and each image what its own last
//! use left, which its next waits for too.

use std::sync::Arc;

use ash::vk;
use gg_core::Result;
use tracing::trace;

use super::graph::{Access, Barrier, Graph, Hazard, ImageDecl, ImageId, Kind};
use crate::Device;
use crate::device::Retired;
use crate::image::{image_info, image_view};
use crate::vma::Allocation;

/// The images made for a graph's own, the blocks of memory they're in, and which of the graph being
/// recorded's images each is.
pub(crate) struct Transients {
    device: Arc<Device>,
    made: Vec<Made>,
    blocks: Vec<Block>,
    /// For each of the graph's images, by its index, the one made for it, if it's one of the
    /// graph's own.
    of: Vec<Option<usize>>,
    /// The first and last passes that use each of the graph's images, if any does.
    lifetimes: Vec<Option<(usize, usize)>>,
    /// Which of `made` the graph's images have matched so far, as it's prepared.
    taken: Vec<bool>,
}

/// An image made for one of a graph's own.
pub(crate) struct Made {
    name: &'static str,
    pub(crate) kind: Kind,
    pub(crate) size: [u32; 2],
    pub(crate) raw: vk::Image,
    pub(crate) view: vk::ImageView,
    /// Its slot in the bindless set, if shaders sample its kind.
    pub(crate) index: Option<u32>,
    /// The block it's in, in `Transients::blocks`.
    block: usize,
    /// The layout the frame's commands so far leave it in: undefined as each frame begins.
    pub(crate) layout: vk::ImageLayout,
    /// What its own last use left, which its next waits for as well as for its block's: a barrier
    /// on another image that used the block since is that image's, and leaves this one's own
    /// writes, as a depth image's at the fragment tests, unwaited for, as synchronization
    /// validation finds.
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
        }
    }

    /// Readies the images for `graph`'s own, for a frame to record: those made for an earlier
    /// frame, if every one of them was, by its name, kind and size, and the uses of those that
    /// share a block don't overlap; otherwise new ones, in new blocks, the old retiring until the
    /// frames that use them have finished. Each is undefined until a pass of the frame writes it.
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

    /// Makes the images for `graph`'s own and the blocks they're in, in place of those made
    /// before, which retire.
    fn make(&mut self, graph: &Graph) -> Result<()> {
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
            let view = match image_view(&self.device, image, kind.format(), kind.range(), name) {
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
            self.made.push(Made {
                name,
                kind,
                size,
                raw: image,
                view,
                index,
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

    /// Retires every image made, and its block, until the frames that use them have finished.
    fn retire(&mut self) {
        for made in self.made.drain(..) {
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

    /// The barrier that lets `access`, a use of the graph's `image`, follow what the uses so far of
    /// the image and of its block left, if it needs one, which the use then leaves for both, with
    /// the layout it leaves the image in.
    ///
    /// # Panics
    ///
    /// If `image` isn't one of the graph's own.
    pub(crate) fn follow(&mut self, image: ImageId, access: Access) -> Option<Barrier> {
        let made = &mut self.made[self.of[image.0 as usize].expect("one of the graph's own")];
        let block = &mut self.blocks[made.block].hazard;
        // Within a frame they're the same once the image's first use has left them: no other image
        // uses the block between its uses.
        let mut hazard = made.hazard.and(*block);
        let barrier = hazard.follow(&mut made.layout, access);
        (made.hazard, *block) = (hazard, hazard);
        barrier
    }

    /// The bytes of the blocks of memory the images are in.
    pub(crate) fn bytes(&self) -> u64 {
        self.blocks.iter().map(|block| block.size).sum()
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

/// The blocks of memory that images of these requirements, each used by the passes from its first
/// to its last, if any, go in, and the block each goes in: each at its block's start, never in
/// one with an image whose uses overlap its own, or whose memory types share none with its own.
/// Images are placed in the order of their first uses, each in the smallest block free by then
/// that holds it, else in the largest, which grows; else in a block of its own.
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
        // A view's colors, its IDs and its depth, used by the first pass, the depth by it alone,
        // so that a color image drawn from the second pass on, larger, takes the depth's block,
        // which grows; and an image no pass uses, placed first, whose block the next takes.
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
