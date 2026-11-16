//! Passes that compute: dispatches of compute shaders that write the storage images a graph's pass
//! declares and sample the images it samples.
//!
//! The SAFETY comments lean on what the parent module keeps, as pass.rs's do: commands record until
//! the frame is submitted, `Frame::outside_pass` checks that no pass is open, and
//! `Frame::open_pass` has placed the barriers that put each image a pass uses in its layout.

use std::ffi::CStr;
use std::ops::Range;

use ash::vk;

use gg_core::Result;

use super::graph::{BufferId, BufferUse, ImageDecl, ImageId, PassId, PassKind, Use};
use super::{Frame, Target, Upload};
use crate::ComputePipeline;
use crate::bindless::PUSH_CONSTANTS;

impl<'a> Frame<'a> {
    /// Records `pass`, which must be the graph's next pass to record and compute. Its dispatches
    /// end when it's dropped.
    ///
    /// # Panics
    ///
    /// - A forgotten pass is still open.
    /// - `pass` isn't the graph's next, or doesn't compute.
    /// - It samples a target no pass has drawn into or written since it was made.
    pub fn compute(&mut self, pass: PassId) -> Compute<'_, 'a> {
        let decl = self.take_pass(pass, PassKind::Compute);
        self.open_pass(pass);
        // A pass is open until it's dropped, whatever it records.
        self.rendering = true;
        Compute {
            frame: self,
            uses: decl.uses.clone(),
            buffer_uses: decl.buffer_uses.clone(),
            labels: 0,
            pipeline_set: false,
        }
    }
}

/// Dispatching compute shaders in a frame's pass, which ends when it's dropped. A forgotten pass
/// stays open, which the frame's next call catches.
///
/// Shaders read [`Upload`]s and the buffers the pass uses through addresses pushed as push
/// constants, write storage images through their levels' slots and buffers through their
/// addresses, and sample the bindless set's textures, among them the images the pass samples.
pub struct Compute<'p, 'a> {
    frame: &'p mut Frame<'a>,
    /// Its uses, in its graph's.
    uses: Range<usize>,
    /// Its buffer uses, in its graph's.
    buffer_uses: Range<usize>,
    /// How many labels inside it are open, which its drop closes.
    labels: u32,
    /// Whether a pipeline is set, which dispatches need.
    pipeline_set: bool,
}

impl<'a> Compute<'_, 'a> {
    /// The bindless index shaders sample `image` through, during the pass alone. The pass must
    /// sample it, as its graph declared.
    ///
    /// # Panics
    ///
    /// If the pass doesn't sample `image`.
    pub fn sampled(&self, image: ImageId) -> u32 {
        assert!(
            self.declared(image, Use::ComputeSampled),
            "a pass samples only the images it was added sampling, and not image {}",
            image.0
        );
        self.frame.sampled_index(image)
    }

    /// The index among the bindless set's storage images that shaders write and read level
    /// `level` of `image` through, during the pass alone. The pass must write `image`, as its
    /// graph declared.
    ///
    /// It is a `RWTexture2DArray` of the layers, a cube's six faces in Vulkan's order (+X, -X, +Y,
    /// -Y, +Z, -Z), of 16-bit floats in RGBA, or of a 32-bit float for an image of scalars
    /// ([`Graph::scalar`](crate::Graph::scalar)). A graph's own image has one layer and one level.
    ///
    /// # Panics
    ///
    /// If the pass doesn't write `image`, or it hasn't that level.
    pub fn storage(&self, image: ImageId, level: u32) -> u32 {
        let written = [true, false].map(|kept| self.declared(image, Use::Storage { kept }));
        assert!(
            written.contains(&true),
            "a pass writes only the images it was added writing, and not image {}",
            image.0
        );
        match self.frame.graph.images[image.0 as usize] {
            ImageDecl::Target(target) => {
                let Target::Storage(storage) = &self.frame.targets[target] else {
                    unreachable!("only storage images are written, as the frame checked");
                };
                storage.storage(level)
            }
            ImageDecl::Transient { name, .. } => {
                assert_eq!(
                    level, 0,
                    "image {name:?} has a level alone, not a level {level}"
                );
                let made = self.frame.frames.transients.image(image);
                let (_, slot) = made
                    .storage
                    .expect("only kinds that shaders write are written, as the graph checked");
                slot
            }
        }
    }

    fn declared(&self, image: ImageId, use_: Use) -> bool {
        self.frame.graph.uses[self.uses.clone()].contains(&(image, use_))
    }

    /// The address shaders read or write `buffer` through, during the pass alone. The pass must
    /// read or write it, as its graph declared.
    ///
    /// # Panics
    ///
    /// If the pass neither reads nor writes `buffer`.
    pub fn address(&self, buffer: BufferId) -> u64 {
        let uses = &self.frame.graph.buffer_uses[self.buffer_uses.clone()];
        assert!(
            uses.contains(&(buffer, BufferUse::ComputeRead))
                || uses.contains(&(buffer, BufferUse::ComputeWrite)),
            "a pass reads and writes only the buffers it was added using, and not buffer {}",
            buffer.0
        );
        self.frame.buffer_parts(buffer).0
    }

    /// As [`Frame::upload`].
    pub fn upload(&mut self, bytes: &[u8]) -> Result<Upload<'a>> {
        self.frame.upload(bytes)
    }

    /// Opens a label `name` that captures show the dispatches after it under, until
    /// [`Compute::end_label`] or the pass's end closes it.
    pub fn begin_label(&mut self, name: &CStr) {
        // SAFETY: the commands are recording.
        unsafe { self.frame.frames.device.begin_label(self.commands(), name) };
        self.labels += 1;
    }

    /// Closes the label opened last inside the pass.
    ///
    /// # Panics
    ///
    /// If none is open.
    pub fn end_label(&mut self) {
        assert!(self.labels > 0, "a pass closes only the labels it opened");
        // SAFETY: the commands are recording, with a label of the pass's open.
        unsafe { self.frame.frames.device.end_label(self.commands()) };
        self.labels -= 1;
    }

    /// Sets the pipeline the dispatches that follow use.
    ///
    /// # Panics
    ///
    /// If another device made the pipeline.
    pub fn set_pipeline(&mut self, pipeline: &ComputePipeline) {
        let device = &self.frame.frames.device;
        device.check_made("the pipeline", pipeline.device());
        self.pipeline_set = true;
        // SAFETY: the commands are recording, and the pipeline retires until the frame has
        // finished.
        unsafe {
            device.raw().cmd_bind_pipeline(
                self.commands(),
                vk::PipelineBindPoint::COMPUTE,
                pipeline.raw(),
            );
        }
    }

    /// Sets the push constants, from offset 0, for the dispatches that follow.
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

    /// Dispatches `groups` workgroups across, down and deep with the pipeline set last.
    ///
    /// # Panics
    ///
    /// - No pipeline is set.
    /// - `groups` passes the device's `maxComputeWorkGroupCount`, at least 65,535 each way.
    pub fn dispatch(&mut self, groups: [u32; 3]) {
        assert!(self.pipeline_set, "a dispatch needs a pipeline set first");
        let device = &self.frame.frames.device;
        let most = device.limits().max_compute_work_group_count;
        assert!(
            groups.iter().zip(most).all(|(&count, most)| count <= most),
            "{groups:?} workgroups, but {} dispatches at most {most:?}",
            device.name()
        );
        let [x, y, z] = groups;
        // SAFETY: the commands are recording inside a pass with a pipeline set, and the counts are
        // within the device's limits.
        unsafe { device.raw().cmd_dispatch(self.commands(), x, y, z) };
    }

    fn commands(&self) -> vk::CommandBuffer {
        self.frame.commands()
    }
}

impl Drop for Compute<'_, '_> {
    fn drop(&mut self) {
        let device = &self.frame.frames.device;
        let commands = self.frame.commands();
        // SAFETY: the commands are recording inside the pass this ends, with its labels open.
        unsafe {
            for _ in 0..self.labels {
                device.end_label(commands);
            }
        }
        self.frame.rendering = false;
        self.frame.close_pass();
    }
}
