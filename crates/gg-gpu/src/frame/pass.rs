//! Passes that draw: rendering into the attachments a graph's pass declares, and the draws recorded
//! there.
//!
//! The SAFETY comments here lean on what the parent module keeps: [`Frames::begin`] begins a slot's
//! commands, and resets its arena, only once the GPU has finished the slot's last frame, and they
//! record until the frame is submitted; `Frame::outside_pass` checks that no pass is open, which
//! only a pass's drop closes; and `Frame::open_pass` has placed the barriers that put each image a
//! pass uses in the layout it uses it in.
//!
//! [`Frames::begin`]: crate::Frames::begin

use std::ffi::CStr;
use std::marker::PhantomData;
use std::ops::Range;

use ash::vk;
use gg_core::Result;

use super::graph::{BufferId, BufferUse, ImageDecl, ImageId, Kind, PassId, PassKind, Use};
use super::{Frame, Target, Upload};
use crate::bindless::PUSH_CONSTANTS;
use crate::{ColorImage, Device, Faces, Format, GpuBuffer, Pipeline};

impl<'a> Frame<'a> {
    /// Records `pass`, a pass of the frame's graph that draws, which must be the next of its passes
    /// to record: its attachments cleared or kept as it declared them, its depth cleared to 0, as far
    /// away as depth goes, and its IDs to 0, for none, unless they're kept. Rendering ends when the
    /// pass is dropped.
    /// Returns `None`, recording nothing, for a pass into a swapchain that had no image ready this
    /// frame; the first target always has one.
    ///
    /// # Panics
    ///
    /// If a forgotten pass is still open, `pass` isn't the graph's next, or doesn't draw; if it
    /// draws over or samples a target that no pass has drawn into since it was made, or since its
    /// swapchain's image was acquired; if its attachments aren't all its color target's size, or
    /// its depth's, for a pass of depth alone; or if they're larger than the device's viewports
    /// reach, as [`Pass::set_viewport`] says.
    pub fn render(&mut self, pass: PassId) -> Option<Pass<'_, 'a>> {
        let decl = self.take_pass(pass, PassKind::Render);
        let graph = self.graph;
        let uses = graph.uses(pass);
        let color =
            (uses.iter()).find_map(|&(image, use_)| matches!(use_, Use::Color(_)).then_some(image));
        if let Some(color) = color
            && let ImageDecl::Target(target) = graph.images[color.0 as usize]
            && self.frames.images[target].is_none()
        {
            return None;
        }
        // What the pass's size and format go by: its color target, or its depth, for a pass of depth
        // alone, which is one of the graph's own.
        let first = color.unwrap_or_else(|| {
            (uses.iter())
                .find_map(|&(image, use_)| matches!(use_, Use::Depth { .. }).then_some(image))
                .expect("a pass that draws has a color target or depth, as it was checked to")
        });
        // Before anything is recorded, as a refusal records nothing.
        let (_, _, extent) = self.parts(first);
        for &(image, use_) in uses {
            let (_, _, size) = self.parts(image);
            assert!(
                use_ == Use::Sampled || size == extent,
                "pass {:?} draws into images of one size, {}x{}, but image {} is {}x{}",
                decl.label,
                extent.width,
                extent.height,
                image.0,
                size.width,
                size.height
            );
        }
        self.open_pass(pass);
        let mut format = match color {
            Some(color) => Format::new(self.format_of(color)),
            None => Format::new(vk::Format::UNDEFINED),
        };
        let mut attachments = [vk::RenderingAttachmentInfo::default(); 3];
        let mut depth_attachment = None;
        for &(image, use_) in uses {
            let (_, view, _) = self.parts(image);
            // Kept for later passes unless this is the last use of an image of the graph's own.
            let store = if self.frames.transients.last_use(image) == Some(pass.0 as usize) {
                vk::AttachmentStoreOp::DONT_CARE
            } else {
                vk::AttachmentStoreOp::STORE
            };
            let attachment = vk::RenderingAttachmentInfo::default()
                .image_view(view)
                .image_layout(use_.access().layout)
                .store_op(store);
            let load = |kept| {
                if kept {
                    vk::AttachmentLoadOp::LOAD
                } else {
                    vk::AttachmentLoadOp::CLEAR
                }
            };
            match use_ {
                Use::Color(clear) => {
                    // A visibility buffer's integers clear to 0, as the graph checked it does.
                    let integers = graph.kind(image) == Some(Kind::Visibility);
                    let color = if integers {
                        vk::ClearColorValue { uint32: [0; 4] }
                    } else {
                        vk::ClearColorValue {
                            float32: clear.unwrap_or_default(),
                        }
                    };
                    attachments[0] =
                        attachment
                            .load_op(clear.map_or(vk::AttachmentLoadOp::LOAD, |_| {
                                vk::AttachmentLoadOp::CLEAR
                            }))
                            .clear_value(vk::ClearValue { color });
                }
                Use::Ids { kept } => {
                    format = format.with_ids();
                    attachments[1] = attachment.load_op(load(kept)).clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { uint32: [0; 4] },
                    });
                }
                Use::Motion { kept } => {
                    format = format.with_motion();
                    attachments[2] = attachment.load_op(load(kept)).clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: [0.0; 4] },
                    });
                }
                Use::Depth { kept } => {
                    format = format.with_depth();
                    depth_attachment =
                        Some(attachment.load_op(load(kept)).clear_value(vk::ClearValue {
                            depth_stencil: vk::ClearDepthStencilValue {
                                depth: 0.0,
                                stencil: 0,
                            },
                        }));
                }
                Use::Sampled | Use::ComputeSampled | Use::Storage { .. } | Use::CopySource => {}
            }
        }
        // Motion is only beside IDs, as the graph checked, so the attachments are in order.
        let colors = usize::from(color.is_some())
            + usize::from(format.ids != vk::Format::UNDEFINED)
            + usize::from(format.motion != vk::Format::UNDEFINED);
        let attachments = &attachments[..colors];
        let mut rendering = vk::RenderingInfo::default()
            .render_area(extent.into())
            .layer_count(1)
            .color_attachments(attachments);
        if let Some(depth) = &depth_attachment {
            rendering = rendering.depth_attachment(depth);
        }
        let device = &self.frames.device;
        let viewport = viewport(device, [0, 0], [extent.width, extent.height]);
        let commands = self.commands();
        let raw = device.raw();
        let (cull_mode, front_face) = Faces::Both.state();
        // SAFETY: the commands are recording outside a pass, and the images and views live until
        // the frame has finished: the swapchain waits for its frames before destroying its images,
        // and an offscreen image or one of the graph's own retires until then. Every pipeline takes
        // its faces as dynamic state, which a draw needs set, and binding one doesn't change.
        unsafe {
            raw.cmd_begin_rendering(commands, &rendering);
            raw.cmd_set_viewport(commands, 0, &[viewport]);
            raw.cmd_set_scissor(commands, 0, &[extent.into()]);
            raw.cmd_set_cull_mode(commands, cull_mode);
            raw.cmd_set_front_face(commands, front_face);
        }
        self.rendering = true;
        Some(Pass {
            frame: self,
            extent,
            format,
            uses: decl.uses.clone(),
            buffer_uses: decl.buffer_uses.clone(),
            labels: 0,
            pipeline_set: false,
            mesh_pipeline: false,
        })
    }

    /// The format of `image`, which the frame has.
    fn format_of(&self, image: ImageId) -> vk::Format {
        match self.graph.images[image.0 as usize] {
            ImageDecl::Target(target) => match &self.targets[target] {
                Target::Swapchain(swapchain) => swapchain.format().color,
                Target::Image(_) => ColorImage::FORMAT.color,
                Target::Storage(_) => unreachable!("no pass draws into a storage image"),
            },
            ImageDecl::Transient { kind, .. } => kind.format(),
        }
    }
}

/// Rendering into a frame's target, which ends when the pass is dropped; a pass forgotten
/// instead stays open, which the frame's next call catches. Draws read geometry and data through
/// addresses pushed as push constants, of [`Upload`]s, [`GpuBuffer`]s or the buffers the pass
/// reads, take indices from either of the first two, and textures from the bindless set, the
/// images the pass samples among them; indirect draws take their commands from a buffer the pass
/// takes them from.
pub struct Pass<'p, 'a> {
    frame: &'p mut Frame<'a>,
    extent: vk::Extent2D,
    format: Format,
    /// Its uses, in its graph's.
    uses: Range<usize>,
    /// Its uses of buffers, in its graph's.
    buffer_uses: Range<usize>,
    /// How many labels inside it are open, which its drop closes.
    labels: u32,
    /// Whether a pipeline is set, which draws need.
    pipeline_set: bool,
    /// Whether the pipeline set last draws through a mesh shader.
    mesh_pipeline: bool,
}

impl<'a> Pass<'_, 'a> {
    /// The target's width and height in pixels.
    pub fn size(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
    }

    /// The formats of its color target, its IDs, its motion and its depth, of those it has, which
    /// the pipelines it draws with must be made for.
    pub fn format(&self) -> Format {
        self.format
    }

    /// The index in the bindless set that shaders sample `image` through, which the pass samples,
    /// as its graph declared, during the pass alone.
    ///
    /// # Panics
    ///
    /// If the pass doesn't sample `image`.
    pub fn sampled(&self, image: ImageId) -> u32 {
        let graph = self.frame.graph;
        assert!(
            graph.uses[self.uses.clone()].contains(&(image, Use::Sampled)),
            "a pass samples only the images it was added sampling, and not image {}",
            image.0
        );
        self.frame.sampled_index(image)
    }

    /// The address that shaders read `buffer` through, which the pass reads, or takes commands
    /// from, as its graph declared, during the pass alone.
    ///
    /// # Panics
    ///
    /// If the pass neither reads `buffer` nor takes commands from it.
    pub fn address(&self, buffer: BufferId) -> u64 {
        assert!(
            self.declared(buffer, BufferUse::Read) || self.declared(buffer, BufferUse::Indirect),
            "a pass reads only the buffers it was added reading, and not buffer {}",
            buffer.0
        );
        self.frame.buffer_parts(buffer).0
    }

    fn declared(&self, buffer: BufferId, use_: BufferUse) -> bool {
        self.frame.graph.buffer_uses[self.buffer_uses.clone()].contains(&(buffer, use_))
    }

    /// As [`Frame::upload`].
    pub fn upload(&mut self, bytes: &[u8]) -> Result<Upload<'a>> {
        self.frame.upload(bytes)
    }

    /// Opens a label inside the pass, named `name`, which captures show the draws after it under,
    /// until [`Pass::end_label`] closes it, or the pass ends.
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
            "a pipeline is made for the formats of the pass it draws in: its target's, its IDs', \
             its motion's and its depth image's"
        );
        self.pipeline_set = true;
        self.mesh_pipeline = pipeline.mesh();
        // SAFETY: the commands are recording inside a pass whose formats, its target's, its IDs',
        // its motion's and its depth image's, are the pipeline's, and the pipeline retires until
        // the frame has finished.
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
        self.check_vertices();
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
        self.check_vertices();
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

    /// Draws `indices` as the command at byte `offset` of `commands` says, a buffer the pass takes
    /// commands from, as an earlier pass wrote it, with the pipeline set last: a
    /// `VkDrawIndexedIndirectCommand`, five 32-bit words, how many indices, how many instances,
    /// the first index, what each index is offset by and the first instance, whose index the
    /// vertex shader sees as `SV_VulkanInstanceID`, counted from it. What wrote the command is
    /// trusted to keep the indices it draws within `indices`, as shaders are trusted with
    /// addresses.
    ///
    /// # Panics
    ///
    /// If no pipeline is set; the pass doesn't take commands from `commands`; the command isn't
    /// 4-byte aligned, or runs past the buffer's end; or the indices are a buffer no frame has
    /// filled or another device made.
    pub fn draw_indexed_indirect<'i>(
        &mut self,
        indices: impl Into<Indices<'i>>,
        commands: BufferId,
        offset: u64,
    ) {
        /// A `VkDrawIndexedIndirectCommand`'s bytes.
        const COMMAND: u64 = 20;
        let indices = indices.into();
        self.frame
            .frames
            .device
            .check_made("the indices' buffer", indices.device);
        self.check_vertices();
        let buffer = self.command_buffer(commands, offset, COMMAND);
        // SAFETY: the commands are recording inside a pass with a pipeline set, after the barrier
        // that lets draws read the command, which lies within its buffer; the buffer and the
        // indices live until the frame has finished, as the graph's own buffers retire until then.
        unsafe {
            let raw = self.frame.frames.device.raw();
            raw.cmd_bind_index_buffer(
                self.commands(),
                indices.buffer,
                indices.offset,
                vk::IndexType::UINT32,
            );
            raw.cmd_draw_indexed_indirect(self.commands(), buffer, offset, 1, COMMAND as u32);
        }
    }

    /// Draws as the command at byte `offset` of `commands` says, a buffer the pass takes commands
    /// from, as an earlier pass wrote it, with the pipeline set last: a `VkDrawIndirectCommand`,
    /// four 32-bit words, how many vertices, how many instances, the first vertex and the first
    /// instance, each vertex's index seen by the vertex shader as `SV_VulkanVertexID` and each
    /// instance's as `SV_VulkanInstanceID`, counted from them.
    ///
    /// # Panics
    ///
    /// If no pipeline is set, or the one set draws through a mesh shader; the pass doesn't take
    /// commands from `commands`; or the command isn't 4-byte aligned, or runs past the buffer's
    /// end.
    pub fn draw_indirect(&mut self, commands: BufferId, offset: u64) {
        /// A `VkDrawIndirectCommand`'s bytes.
        const COMMAND: u64 = 16;
        self.check_vertices();
        let buffer = self.command_buffer(commands, offset, COMMAND);
        // SAFETY: the commands are recording inside a pass with a pipeline set, after the barrier
        // that lets draws read the command, which lies within its buffer, which lives until the
        // frame has finished, as the graph's own buffers retire until then.
        unsafe {
            let raw = self.frame.frames.device.raw();
            raw.cmd_draw_indirect(self.commands(), buffer, offset, 1, COMMAND as u32);
        }
    }

    /// Draws through the mesh shader of the pipeline set last as the command at byte `offset` of
    /// `commands` says, a buffer the pass takes commands from, as an earlier pass wrote it: a
    /// `VkDrawMeshTasksIndirectCommandEXT`, three 32-bit words, how many workgroups across, down
    /// and deep, each of which the mesh shader sees as `SV_GroupID`. What wrote the command is
    /// trusted to keep them within the device's limits, as shaders are trusted with addresses.
    ///
    /// # Panics
    ///
    /// If no pipeline is set, or the one set doesn't draw through a mesh shader; the pass doesn't
    /// take commands from `commands`; or the command isn't 4-byte aligned, or runs past the buffer's
    /// end.
    pub fn draw_mesh_tasks_indirect(&mut self, commands: BufferId, offset: u64) {
        /// A `VkDrawMeshTasksIndirectCommandEXT`'s bytes.
        const COMMAND: u64 = 12;
        assert!(
            self.pipeline_set && self.mesh_pipeline,
            "a draw through a mesh shader needs a pipeline of one set first"
        );
        let buffer = self.command_buffer(commands, offset, COMMAND);
        let device = &self.frame.frames.device;
        let mesh_shader = (device.mesh_shader_fn()).expect("a mesh shader's pipeline's device");
        // SAFETY: as for `draw_indirect`; the pipeline set draws through a mesh shader, which only
        // a device whose mesh shaders are enabled makes, as `Pipeline::graphics` asserts.
        unsafe {
            mesh_shader.cmd_draw_mesh_tasks_indirect(
                self.commands(),
                buffer,
                offset,
                1,
                COMMAND as u32,
            );
        }
    }

    /// Checks that a pipeline is set for a draw of vertices, as the draws but
    /// [`draw_mesh_tasks_indirect`](Self::draw_mesh_tasks_indirect) are.
    fn check_vertices(&self) {
        assert!(self.pipeline_set, "a draw needs a pipeline set first");
        assert!(
            !self.mesh_pipeline,
            "a pipeline that draws through a mesh shader draws through draw_mesh_tasks_indirect"
        );
    }

    /// The buffer of the graph's own `commands`, whose `size` bytes at `offset` are a command the
    /// pass's indirect draws take.
    ///
    /// # Panics
    ///
    /// If the pass doesn't take commands from `commands`, or the command isn't 4-byte aligned, or
    /// runs past the buffer's end.
    fn command_buffer(&self, commands: BufferId, offset: u64, size: u64) -> vk::Buffer {
        assert!(
            self.declared(commands, BufferUse::Indirect),
            "a pass takes commands only from the buffers it was added taking them from, and not \
             buffer {}",
            commands.0
        );
        let held = self.frame.buffer_parts(commands).1;
        assert!(
            offset.is_multiple_of(4) && offset.checked_add(size).is_some_and(|end| end <= held),
            "a command at byte {offset} of a buffer of {held}: a command is {size} bytes, 4-byte \
             aligned, within its buffer"
        );
        self.frame.frames.transients.buffer(commands).raw()
    }

    fn commands(&self) -> vk::CommandBuffer {
        self.frame.commands()
    }
}

impl Drop for Pass<'_, '_> {
    fn drop(&mut self) {
        let device = &self.frame.frames.device;
        let commands = self.frame.commands();
        // SAFETY: the commands are recording inside the pass this ends, with its labels open.
        unsafe {
            for _ in 0..self.labels {
                device.end_label(commands);
            }
            device.raw().cmd_end_rendering(commands);
        }
        self.frame.rendering = false;
        self.frame.close_pass();
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
