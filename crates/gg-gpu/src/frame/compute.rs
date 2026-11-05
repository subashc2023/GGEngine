//! Passes that compute: dispatches of compute shaders, which write the storage images a graph's
//! pass declares and sample the images it samples.
//!
//! The SAFETY comments here lean on what the parent module keeps, as pass.rs's do: commands record
//! until the frame is submitted, `Frame::outside_pass` checks that no pass is open, and
//! `Frame::open_pass` has placed the barriers that put each image a pass uses in the layout it uses
//! it in.

use std::ffi::CStr;
use std::ops::Range;

use ash::vk;

use super::graph::{ImageDecl, ImageId, PassId, PassKind, Use};
use super::{Frame, Target};
use crate::ComputePipeline;
use crate::bindless::PUSH_CONSTANTS;

impl<'a> Frame<'a> {
    /// Records `pass`, a pass of the frame's graph that computes, which must be the next of its
    /// passes to record. Its dispatches end when it's dropped.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, `pass` isn't the graph's next, or doesn't compute; or if
    /// it samples a target that no pass has drawn into or written since it was made.
    pub fn compute(&mut self, pass: PassId) -> Compute<'_, 'a> {
        let decl = self.take_pass(pass, PassKind::Compute);
        self.open_pass(pass);
        // A pass is open until it's dropped, whatever it records.
        self.rendering = true;
        Compute {
            frame: self,
            uses: decl.uses.clone(),
            labels: 0,
            pipeline_set: false,
        }
    }
}

/// Dispatching compute shaders in a frame's pass, which ends when it's dropped; a pass forgotten
/// instead stays open, which the frame's next call catches. Shaders read data through addresses
/// pushed as push constants, write the storage images the pass writes through their levels' slots,
/// and sample textures from the bindless set, the images the pass samples among them.
pub struct Compute<'p, 'a> {
    frame: &'p mut Frame<'a>,
    /// Its uses, in its graph's.
    uses: Range<usize>,
    /// How many labels inside it are open, which its drop closes.
    labels: u32,
    /// Whether a pipeline is set, which dispatches need.
    pipeline_set: bool,
}

impl Compute<'_, '_> {
    /// The index in the bindless set that shaders sample `image` through, which the pass samples,
    /// as its graph declared, during the pass alone.
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
    /// `level` of `image` through, a storage image the pass writes, as its graph declared, during
    /// the pass alone: a `RWTexture2DArray` of 16-bit floats of its layers, a cube's six faces in
    /// Vulkan's order, +X, -X, +Y, -Y, +Z, -Z.
    ///
    /// # Panics
    ///
    /// If the pass doesn't write `image`, or it hasn't that level.
    pub fn storage(&self, image: ImageId, level: u32) -> u32 {
        assert!(
            self.declared(image, Use::Storage),
            "a pass writes only the images it was added writing, and not image {}",
            image.0
        );
        let ImageDecl::Target(target) = self.frame.graph.images[image.0 as usize] else {
            unreachable!("only targets are written, as the graph checked");
        };
        let Target::Storage(storage) = &self.frame.targets[target] else {
            unreachable!("only storage images are written, as the frame checked");
        };
        storage.storage(level)
    }

    fn declared(&self, image: ImageId, use_: Use) -> bool {
        self.frame.graph.uses[self.uses.clone()].contains(&(image, use_))
    }

    /// Opens a label inside the pass, named `name`, which captures show the dispatches after it
    /// under, until [`Compute::end_label`] closes it, or the pass ends.
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

    /// Sets the push constants, from offset 0, that the dispatches which follow see.
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
    /// If no pipeline is set, or `groups` passes the device's `maxComputeWorkGroupCount`, which is
    /// at least 65,535 each way.
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
