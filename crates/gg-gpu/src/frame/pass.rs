//! Passes: rendering into a frame's targets, and the draws recorded there.
//!
//! The SAFETY comments here lean on what the parent module keeps: [`Frames::begin`] begins a slot's
//! commands, and resets its arena, only once the GPU has finished the slot's last frame, and they
//! record until the frame is submitted; and `Frame::outside_pass` checks that no pass is open,
//! which only a pass's drop closes.
//!
//! [`Frames::begin`]: crate::Frames::begin

use std::ffi::CStr;
use std::marker::PhantomData;

use ash::vk;
use gg_core::Result;

use super::{Frame, Target, Upload};
use crate::bindless::PUSH_CONSTANTS;
use crate::{
    COLOR_RANGE, ColorImage, DEPTH_RANGE, DepthImage, Device, Faces, Format, GpuBuffer, IdImage,
    Pipeline,
};

impl<'a> Frame<'a> {
    /// Starts rendering into the `target`th of the targets the frame began with, cleared to
    /// `color`, in linear RGBA; with `depth`, if given, which its draws test and write as their
    /// pipelines say, cleared to 0, as far away as depth goes; and with `ids`, if given, a second
    /// target that its draws write their IDs into, cleared to 0, for none, which this frame can
    /// read pixels of back once the pass has ended ([`Frame::read_ids`]). `label` names the pass
    /// in captures. Rendering ends when the pass is dropped. Returns `None` for a swapchain that
    /// had no image ready this frame; the first target always has one.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, the frame has no `target`th target, `depth` or `ids`
    /// isn't the target's size or another device made it, or the target is larger than the
    /// device's viewports reach, as [`Pass::set_viewport`] says.
    pub fn begin_pass(
        &mut self,
        target: usize,
        label: &CStr,
        color: [f32; 4],
        depth: Option<&DepthImage>,
        ids: Option<&mut IdImage>,
    ) -> Option<Pass<'_, 'a>> {
        self.open_pass(target, label, Some(color), depth, ids)
    }

    /// Starts rendering into the `target`th of the targets the frame began with, over what the
    /// last pass into it rendered, which it keeps for its draws to blend over; without IDs or a
    /// depth image, so that its draws can sample the IDs an earlier pass wrote beside it, as an
    /// outline round some of what that pass drew does. `label` names the pass in captures.
    /// Rendering ends when the pass is dropped. Returns `None` for a swapchain that had no image
    /// ready this frame.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, the frame has no `target`th target, or no pass has
    /// rendered into the target since it was made, sampled or read back, or since the frame
    /// acquired its swapchain's image.
    pub fn begin_pass_over(&mut self, target: usize, label: &CStr) -> Option<Pass<'_, 'a>> {
        self.open_pass(target, label, None, None, None)
    }

    /// Starts a pass as [`Frame::begin_pass`] does, its target cleared to `clear`; or without it,
    /// as [`Frame::begin_pass_over`] does.
    fn open_pass(
        &mut self,
        target: usize,
        label: &CStr,
        clear: Option<[f32; 4]>,
        depth: Option<&DepthImage>,
        ids: Option<&mut IdImage>,
    ) -> Option<Pass<'_, 'a>> {
        self.outside_pass();
        let (index, layout) = self.frames.images[target]?;
        assert!(
            clear.is_some() || layout == vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            "a pass over target {target} keeps what the last pass into it rendered, but none has \
             since it was made, sampled or read back, or its swapchain's image acquired"
        );
        let number = self.number();
        let device = &self.frames.device;
        let commands = self.frames.slots[self.slot].commands;
        let ((image, view, extent), mut format) = match &self.targets[target] {
            Target::Swapchain(swapchain) => (swapchain.image(index), swapchain.format()),
            Target::Image(image) => (image.parts(), ColorImage::FORMAT),
        };
        let depth = depth.map(|depth| {
            device.check_made("the pass's depth image", depth.device());
            let (image, view, size) = depth.parts();
            assert_eq!(size, extent, "a pass's depth image is its target's size");
            (image, view)
        });
        let id_parts = ids.as_deref().map(|ids| {
            device.check_made("the pass's ID image", ids.device());
            let (image, view, size) = ids.parts();
            assert_eq!(size, extent, "a pass's IDs are its target's size");
            (image, view)
        });
        if id_parts.is_some() {
            format = format.with_ids();
        }
        if depth.is_some() {
            format = format.with_depth();
        }
        // Cleared, the old contents are discarded. A swapchain image is ready once its acquire
        // semaphore is signaled, which the submission waits for at the color-attachment stage, and
        // a later pass into it this frame waits there for the writes of the one before; an
        // offscreen image must wait for whatever used it before. Kept, the last pass into it wrote
        // it, and nothing used it since: its load and its draws' blending read what that wrote.
        let (src_stage, src_access) = match (clear, &self.targets[target]) {
            (None, _) | (Some(_), Target::Swapchain(_)) => (
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            ),
            (Some(_), Target::Image(_)) => (
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::AccessFlags2::MEMORY_WRITE,
            ),
        };
        let (old_layout, dst_access, load_op) = match clear {
            Some(_) => (
                vk::ImageLayout::UNDEFINED,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                vk::AttachmentLoadOp::CLEAR,
            ),
            None => (
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                vk::AttachmentLoadOp::LOAD,
            ),
        };
        let barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(src_stage)
            .src_access_mask(src_access)
            .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(dst_access)
            .old_layout(old_layout)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .image(image)
            .subresource_range(COLOR_RANGE);
        // The IDs' old contents are discarded too, once whatever used them before is done with
        // them, as for an offscreen image: the copies that read them back and the passes that
        // sampled them among it.
        let ids_barrier = id_parts.map(|(image, _)| {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .image(image)
                .subresource_range(COLOR_RANGE)
        });
        // The depth's old contents are discarded too, once the last pass that had it is done with
        // it: its clear wrote it at the early fragment tests, and its store at the late ones.
        let tests = vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
        let depth_barrier = depth.map(|(image, _)| {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(tests)
                .src_access_mask(vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .dst_stage_mask(tests)
                .dst_access_mask(
                    vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                        | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                )
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                .image(image)
                .subresource_range(DEPTH_RANGE)
        });
        let mut barriers = [vk::ImageMemoryBarrier2::default(); 3];
        let mut count = 0;
        for barrier in [Some(barrier), ids_barrier, depth_barrier]
            .into_iter()
            .flatten()
        {
            barriers[count] = barrier;
            count += 1;
        }
        let barriers = &barriers[..count];
        let attachment = vk::RenderingAttachmentInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .load_op(load_op)
            .store_op(vk::AttachmentStoreOp::STORE)
            .clear_value(vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: clear.unwrap_or_default(),
                },
            });
        // Stored, for the frame to read back.
        let ids_attachment = id_parts.map(|(_, view)| {
            vk::RenderingAttachmentInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .clear_value(vk::ClearValue {
                    color: vk::ClearColorValue { uint32: [0; 4] },
                })
        });
        let attachments = [attachment, ids_attachment.unwrap_or_default()];
        let attachments = &attachments[..1 + usize::from(ids_attachment.is_some())];
        // Nothing reads depth after the pass.
        let depth_attachment = depth.map(|(_, view)| {
            vk::RenderingAttachmentInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .clear_value(vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue {
                        depth: 0.0,
                        stencil: 0,
                    },
                })
        });
        let mut rendering = vk::RenderingInfo::default()
            .render_area(extent.into())
            .layer_count(1)
            .color_attachments(attachments);
        if let Some(depth) = &depth_attachment {
            rendering = rendering.depth_attachment(depth);
        }
        let viewport = viewport(device, [0, 0], [extent.width, extent.height]);
        let raw = device.raw();
        let (cull_mode, front_face) = Faces::Both.state();
        // SAFETY: the commands are recording outside a pass, and the images and views live until
        // the frame has finished on the GPU: the swapchain waits for its frames before
        // destroying its images, and an offscreen image, an ID image and a depth image retire
        // until then, dropped while the frame records. Every pipeline takes its faces as dynamic
        // state, which a draw needs set, and binding one doesn't change.
        unsafe {
            device.begin_label(commands, label);
            raw.cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().image_memory_barriers(barriers),
            );
            raw.cmd_begin_rendering(commands, &rendering);
            raw.cmd_set_viewport(commands, 0, &[viewport]);
            raw.cmd_set_scissor(commands, 0, &[extent.into()]);
            raw.cmd_set_cull_mode(commands, cull_mode);
            raw.cmd_set_front_face(commands, front_face);
        }
        self.frames.images[target] = Some((index, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL));
        if let Some(ids) = ids {
            ids.set_rendered(number);
        }
        self.rendering = true;
        Some(Pass {
            frame: self,
            extent,
            format,
            pipeline_set: false,
        })
    }
}

/// Rendering into a frame's target, which ends when the pass is dropped; a pass forgotten
/// instead stays open, which the frame's next call catches. Draws read geometry and data through
/// addresses pushed as push constants, of [`Upload`]s or [`GpuBuffer`]s, take indices from either,
/// and textures from the bindless set.
pub struct Pass<'p, 'a> {
    frame: &'p mut Frame<'a>,
    extent: vk::Extent2D,
    format: Format,
    /// Whether a pipeline is set, which draws need.
    pipeline_set: bool,
}

impl<'a> Pass<'_, 'a> {
    /// The target's width and height in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
    }

    /// The formats of its target, and of its IDs and its depth image, if it has them, which the
    /// pipelines it draws with must be made for.
    pub fn format(&self) -> Format {
        self.format
    }

    /// As [`Frame::upload`].
    pub fn upload(&mut self, bytes: &[u8]) -> Result<Upload<'a>> {
        self.frame.upload(bytes)
    }

    /// Sets the pipeline the draws that follow use.
    ///
    /// # Panics
    ///
    /// If the pipeline isn't made for the pass's formats, or another device made it.
    pub fn set_pipeline(&mut self, pipeline: &Pipeline) {
        self.frame
            .frames
            .device
            .check_made("the pipeline", pipeline.device());
        assert_eq!(
            pipeline.format(),
            self.format,
            "a pipeline is made for the formats of the pass it draws in: its target's, its IDs' and \
             its depth image's"
        );
        self.pipeline_set = true;
        // SAFETY: the commands are recording inside a pass whose formats, its target's, its IDs'
        // and its depth image's, are the pipeline's, and the pipeline retires until the frame has
        // finished.
        unsafe {
            self.frame.frames.device.raw().cmd_bind_pipeline(
                self.commands(),
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.raw(),
            );
        }
    }

    /// Maps clip space onto the `size` pixels at `origin` for the draws that follow, and keeps
    /// them inside those pixels, clipped to the target.
    ///
    /// # Panics
    ///
    /// If `size` has no area, which a viewport can't have, or the viewport is larger than the
    /// device's `maxViewportDimensions` or reaches past its `viewportBoundsRange`.
    pub fn set_viewport(&mut self, origin: [u32; 2], size: [u32; 2]) {
        let viewport = viewport(&self.frame.frames.device, origin, size);
        // SAFETY: the commands are recording, and the viewport keeps within the device's limits.
        unsafe {
            self.frame
                .frames
                .device
                .raw()
                .cmd_set_viewport(self.commands(), 0, &[viewport]);
        }
        self.set_scissor(origin, size);
    }

    /// Has the draws that follow draw only the triangles `faces` names: a pass starts drawing
    /// both.
    pub fn set_faces(&mut self, faces: Faces) {
        let (cull_mode, front_face) = faces.state();
        let raw = self.frame.frames.device.raw();
        let commands = self.commands();
        // SAFETY: the commands are recording, and every pipeline takes its faces as dynamic state.
        unsafe {
            raw.cmd_set_cull_mode(commands, cull_mode);
            raw.cmd_set_front_face(commands, front_face);
        }
    }

    /// Limits the draws that follow to the `size` pixels at `origin`, clipped to the target.
    pub fn set_scissor(&mut self, origin: [u32; 2], size: [u32; 2]) {
        let x = origin[0].min(self.extent.width);
        let y = origin[1].min(self.extent.height);
        let scissor = vk::Rect2D {
            offset: vk::Offset2D {
                x: x as i32,
                y: y as i32,
            },
            extent: vk::Extent2D {
                width: size[0].min(self.extent.width - x),
                height: size[1].min(self.extent.height - y),
            },
        };
        // SAFETY: the commands are recording, and the scissor lies within the target.
        unsafe {
            self.frame
                .frames
                .device
                .raw()
                .cmd_set_scissor(self.commands(), 0, &[scissor]);
        }
    }

    /// Sets the push constants, from offset 0, that the draws which follow see in every stage.
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

    /// Draws `vertices` vertices, `instances` times, with the pipeline set last: the vertex shader
    /// sees each vertex's index, from 0, as `SV_VulkanVertexID`, and each instance's as
    /// `SV_VulkanInstanceID`.
    ///
    /// # Panics
    ///
    /// If no pipeline is set.
    pub fn draw(&mut self, vertices: u32, instances: u32) {
        assert!(self.pipeline_set, "a draw needs a pipeline set first");
        // SAFETY: the commands are recording inside a pass with a pipeline set.
        unsafe {
            self.frame
                .frames
                .device
                .raw()
                .cmd_draw(self.commands(), vertices, instances, 0, 0);
        }
    }

    /// Draws `count` of `indices`, starting `first` indices in, `instances` times, with the
    /// pipeline set last: the vertex shader sees each instance's index, from 0, as
    /// `SV_VulkanInstanceID`. Each index is offset by `vertex_offset` before the vertex shader
    /// sees it.
    ///
    /// # Panics
    ///
    /// If no pipeline is set, the indices are a buffer no frame has filled or another device
    /// made, or the indices drawn run past the end of `indices`.
    pub fn draw_indexed<'i>(
        &mut self,
        indices: impl Into<Indices<'i>>,
        first: u32,
        count: u32,
        vertex_offset: i32,
        instances: u32,
    ) {
        let indices = indices.into();
        self.frame
            .frames
            .device
            .check_made("the indices' buffer", indices.device);
        assert!(self.pipeline_set, "a draw needs a pipeline set first");
        assert!(
            u64::from(first) + u64::from(count) <= indices.count,
            "{count} indices from index {first} run past the {} there are",
            indices.count
        );
        // SAFETY: the commands are recording inside a pass with a pipeline set, the indices
        // drawn are within their buffer, and it lives until the frame has finished: an upload's
        // arena is reset only after that, and a GPU buffer retires until then.
        unsafe {
            let raw = self.frame.frames.device.raw();
            raw.cmd_bind_index_buffer(
                self.commands(),
                indices.buffer,
                indices.offset,
                vk::IndexType::UINT32,
            );
            raw.cmd_draw_indexed(self.commands(), count, instances, first, vertex_offset, 0);
        }
    }

    fn commands(&self) -> vk::CommandBuffer {
        self.frame.frames.slots[self.frame.slot].commands
    }
}

impl Drop for Pass<'_, '_> {
    fn drop(&mut self) {
        let device = &self.frame.frames.device;
        // SAFETY: the commands are recording inside the pass this ends, and its label.
        unsafe {
            device.raw().cmd_end_rendering(self.commands());
            device.end_label(self.commands());
        }
        self.frame.rendering = false;
    }
}

/// The viewport over the `size` pixels at `origin`, which keeps within `device`'s limits.
///
/// # Panics
///
/// If `size` has no area, which a viewport can't have, or the viewport is larger than the device's
/// `maxViewportDimensions` or reaches past its `viewportBoundsRange`, which the driver doesn't
/// check.
fn viewport(device: &Device, origin: [u32; 2], size: [u32; 2]) -> vk::Viewport {
    assert!(
        size[0] > 0 && size[1] > 0,
        "a viewport has an area; got {size:?}"
    );
    let limits = device.limits();
    let [max_width, max_height] = limits.max_viewport_dimensions;
    assert!(
        size[0] <= max_width && size[1] <= max_height,
        "a {}x{} viewport, but {} draws viewports of up to {max_width}x{max_height}",
        size[0],
        size[1],
        device.name()
    );
    let viewport = vk::Viewport {
        x: origin[0] as f32,
        y: origin[1] as f32,
        width: size[0] as f32,
        height: size[1] as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    // The origin is never negative, so only the far bound can be crossed.
    let bound = f64::from(limits.viewport_bounds_range[1]);
    assert!(
        f64::from(viewport.x) + f64::from(viewport.width) <= bound
            && f64::from(viewport.y) + f64::from(viewport.height) <= bound,
        "a {}x{} viewport at {origin:?}, but {}'s viewports reach at most {bound} pixels from \
         the origin",
        size[0],
        size[1],
        device.name()
    );
    viewport
}

/// The 32-bit indices a draw takes: an [`Upload`] of the frame's, or a [`GpuBuffer`] a frame has
/// filled, from its start.
#[derive(Clone, Copy, Debug)]
pub struct Indices<'a> {
    buffer: vk::Buffer,
    offset: u64,
    /// How many indices there are.
    count: u64,
    /// The handle of the device that made the buffer.
    device: vk::Device,
    /// The indices borrow what holds them, so that it outlives the draw's recording.
    holder: PhantomData<&'a ()>,
}

impl<'a> From<Upload<'a>> for Indices<'a> {
    fn from(upload: Upload<'a>) -> Self {
        Self {
            buffer: upload.buffer,
            offset: upload.offset,
            count: upload.size / 4,
            device: upload.device,
            holder: PhantomData,
        }
    }
}

impl<'a> From<&'a GpuBuffer> for Indices<'a> {
    /// # Panics
    ///
    /// If no frame has filled the buffer: its indices are undefined until then, and a fill after
    /// the draw in the same frame would race it.
    fn from(buffer: &'a GpuBuffer) -> Self {
        assert!(
            buffer.written(),
            "a buffer is drawn once a frame has filled it; its indices are undefined until then"
        );
        Self {
            buffer: buffer.raw(),
            offset: 0,
            count: buffer.size() / 4,
            device: buffer.device(),
            holder: PhantomData,
        }
    }
}
