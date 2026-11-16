//! Pipelines, made from SPIR-V with the bindless set's layout: graphics pipelines, for dynamic
//! rendering, and compute pipelines.

use std::ffi::CStr;
use std::io::Cursor;
use std::iter;
use std::sync::Arc;

use ash::vk;
use gg_core::{Error, Result};
use tracing::debug;

use crate::device::Retired;
use crate::frame::graph::ImageKind;
use crate::{Device, vk_error, vk_failed};

/// The pixel formats of what a pass renders into, which pipelines are made for: its color
/// target's, its IDs' and motion's beside it if it has them, and its depth's if it has it, as a
/// [`Graph`]'s passes declare them; or its depth's alone.
///
/// [`Graph`]: crate::Graph
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// `UNDEFINED` for a pass of depth alone.
    pub(crate) color: vk::Format,
    /// `UNDEFINED` for a pass without IDs.
    pub(crate) ids: vk::Format,
    /// `UNDEFINED` for a pass without motion.
    pub(crate) motion: vk::Format,
    /// `UNDEFINED` for a pass without depth.
    pub(crate) depth: vk::Format,
}

impl Format {
    /// The format of a [`Graph`]'s high-dynamic-range images ([`Graph::hdr`]), 16-bit floats,
    /// without IDs or depth.
    ///
    /// [`Graph`]: crate::Graph
    /// [`Graph::hdr`]: crate::Graph::hdr
    pub const HDR: Self = Self::new(ImageKind::Hdr.format());

    /// The format of a visibility buffer ([`Graph::visibility`]), two 32-bit unsigned integers a
    /// pixel, without IDs or depth. A pipeline into one writes them at location 0 and blends only
    /// as [`Blend::Opaque`], as integers don't blend.
    ///
    /// [`Graph::visibility`]: crate::Graph::visibility
    pub const VISIBILITY: Self = Self::new(ImageKind::Visibility.format());

    /// The format of a pass drawing depth alone, as into a shadow map ([`Graph::depth`]).
    ///
    /// [`Graph::depth`]: crate::Graph::depth
    pub const DEPTH: Self = Self::new(vk::Format::UNDEFINED).with_depth();

    /// A target's format, `color`, without IDs or depth.
    pub(crate) const fn new(color: vk::Format) -> Self {
        Self {
            color,
            ids: vk::Format::UNDEFINED,
            motion: vk::Format::UNDEFINED,
            depth: vk::Format::UNDEFINED,
        }
    }

    /// This format with IDs' ([`RenderPass::ids`](crate::RenderPass::ids)), a second color target.
    pub const fn with_ids(self) -> Self {
        Self {
            ids: ImageKind::Ids.format(),
            ..self
        }
    }

    /// This format with motion's ([`RenderPass::motion`](crate::RenderPass::motion)), a third
    /// color target, which a pipeline that writes depth writes at location 2, as 16-bit floats in
    /// RGBA.
    pub const fn with_motion(self) -> Self {
        Self {
            motion: ImageKind::Motion.format(),
            ..self
        }
    }

    /// This format with depth's ([`RenderPass::depth`](crate::RenderPass::depth)).
    pub const fn with_depth(self) -> Self {
        Self {
            depth: ImageKind::Depth.format(),
            ..self
        }
    }

    fn has_color(self) -> bool {
        self.color != vk::Format::UNDEFINED
    }

    fn has_ids(self) -> bool {
        self.ids != vk::Format::UNDEFINED
    }

    fn has_motion(self) -> bool {
        self.motion != vk::Format::UNDEFINED
    }

    fn has_depth(self) -> bool {
        self.depth != vk::Format::UNDEFINED
    }
}

/// How a pipeline's output combines with what's already in the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// The output replaces it.
    Opaque,
    /// The output goes over it by its alpha, which its color isn't multiplied by: straight alpha,
    /// as images store it.
    Alpha,
    /// The output, its color already multiplied by its alpha, goes over it.
    PremultipliedAlpha,
}

impl Blend {
    /// The color target's blend state for this blend.
    fn attachment(self) -> vk::PipelineColorBlendAttachmentState {
        match self {
            Self::Opaque => vk::PipelineColorBlendAttachmentState::default(),
            Self::Alpha => vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD),
            Self::PremultipliedAlpha => vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_DST_ALPHA)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                .alpha_blend_op(vk::BlendOp::ADD),
        }
        .color_write_mask(vk::ColorComponentFlags::RGBA)
    }
}

/// Which of a mesh's triangles draw, by how their corners wind on screen. Vulkan measures in
/// framebuffer coordinates, whose Y points down, and flips the area's sign, so its
/// counterclockwise is as seen; the renderer's cameras show the world upright. Whichever draw, a
/// fragment shader's front face, `SV_IsFrontFace`, is a triangle that winds counterclockwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Faces {
    /// All of them: what a pass starts drawing.
    Both,
    /// Triangles winding counterclockwise: the fronts of a mesh whose fronts wind so seen from in
    /// front, as glTF's do, drawn with a transform that doesn't mirror it.
    CounterClockwise,
    /// Triangles winding clockwise: such a mesh's fronts drawn with a mirroring transform.
    Clockwise,
}

impl Faces {
    /// The cull mode and front face that draw them.
    pub(crate) fn state(self) -> (vk::CullModeFlags, vk::FrontFace) {
        match self {
            Self::Both => (vk::CullModeFlags::NONE, vk::FrontFace::COUNTER_CLOCKWISE),
            Self::CounterClockwise => (vk::CullModeFlags::BACK, vk::FrontFace::COUNTER_CLOCKWISE),
            Self::Clockwise => (vk::CullModeFlags::FRONT, vk::FrontFace::COUNTER_CLOCKWISE),
        }
    }
}

/// What a graphics pipeline is made from. It draws triangle lists; its viewport, scissor and
/// [`Faces`] are set as it draws, a pass starting with both. Where its targets have depth, it
/// tests it, drawing only where it's as near as what was drawn there or nearer: depth runs from 1
/// at the near plane to 0 far away, as the renderer's cameras project it.
pub struct GraphicsDesc<'a> {
    /// The debug name.
    pub name: &'a str,
    /// A SPIR-V module holding its entry points, but a mesh shader's, which slangc compiled. It's
    /// trusted to be valid and keep to the pipeline layout: read only the push constants set
    /// before a draw, use only the device's enabled features, and bind nothing but the bindless
    /// set's textures and samplers. Vulkan checks none of it; validation does in debug builds.
    pub spirv: &'a [u8],
    pub vertex: &'a CStr,
    /// A mesh shader's module, holding `vertex` alone, in place of a vertex shader; only a device
    /// that draws through mesh shaders takes it ([`Device::mesh_shaders`]). The pipeline draws
    /// through [`Pass::draw_mesh_tasks_indirect`](crate::Pass::draw_mesh_tasks_indirect). Trusted
    /// as `spirv` is. A module of its own, as validation before Vulkan SDK 1.4.328's, as Debian
    /// 13's, refuses a mesh shader beside a fragment shader in one module: it checked the mesh
    /// shader's outputs against every entry point of the module (SPIRV-Tools #6313).
    pub mesh: Option<&'a [u8]>,
    /// `None` for a pipeline that draws depth alone, all of each triangle, as a shadow map's
    /// opaque casters are drawn.
    pub fragment: Option<&'a CStr>,
    /// The formats of the targets it draws into. Where they have IDs, its fragment shader writes
    /// them at location 1, as two 32-bit unsigned integers, replacing what's there wherever it
    /// draws, however its color blends, if [`ids_write`](Self::ids_write). Where they have
    /// motion, a pipeline that writes depth writes it at location 2, a float4 replacing what's
    /// there, as the motion belongs to what owns the depth; one that doesn't leaves it as it was.
    ///
    /// lavapipe, once a command buffer has bound a pipeline without color targets, writes every
    /// target of the pipelines it binds after as it writes the color target, by its blending and
    /// write mask (Mesa's `lvp_execute.c` clears `independent_blend_enable` then, and only a new
    /// command buffer sets it again, through Mesa 25.3 at least). So there, a fragment shader that
    /// returns nothing at a location its targets have, even one the pipeline doesn't write, leaves
    /// undefined values in that target, differing from run to run; zeros returned there leave a
    /// target that blends as it was.
    pub format: Format,
    pub blend: Blend,
    /// Whether it writes its depth where it draws, for later draws to test against. Only a
    /// pipeline whose targets have depth can.
    pub depth_write: bool,
    /// Whether it writes its IDs where it draws. Only a pipeline whose targets have IDs can. One
    /// that doesn't leaves the IDs under what it draws, as a draw for seeing by does, such as the
    /// engine's grid, so a pick there finds what's under it.
    pub ids_write: bool,
    /// Slope-scaled depth bias: how far each fragment's depth goes back from the near plane, in
    /// multiples of the most its triangle's depth changes across a pixel. It keeps a shadow map's
    /// depths behind surfaces sloping away from the light, so a surface doesn't shadow itself. 0
    /// for none. Depth runs from 1 at the near plane, so going back lowers it: Vulkan's slope
    /// factor is this negated.
    pub depth_bias: f32,
    /// Whether depth nearer than the near plane or farther than the far is clamped to it, not
    /// clipped: a shadow map's casters between the light and its near plane still cast shadows.
    pub depth_clamp: bool,
}

/// A graphics pipeline. Dropping it retires it until the GPU is done with it.
pub struct Pipeline {
    device: Arc<Device>,
    raw: vk::Pipeline,
    format: Format,
    /// Whether its geometry is a mesh shader.
    mesh: bool,
}

impl Pipeline {
    /// Makes a pipeline from `desc`, through the device's pipeline cache if it has one. Fails
    /// with `gpu.bad_shader` if `desc.spirv` isn't SPIR-V, and `gpu.vulkan` or `gpu.out_of_memory`
    /// if Vulkan can't make it.
    ///
    /// # Panics
    ///
    /// - It writes, biases or clamps depth for targets without it.
    /// - It writes IDs for targets without them.
    /// - It has no fragment shader but its targets have colors.
    /// - It blends into a visibility buffer.
    /// - Its targets have IDs but no color, or motion but no IDs.
    /// - Its geometry is a mesh shader and the device draws through none.
    pub fn graphics(device: &Arc<Device>, desc: &GraphicsDesc<'_>) -> Result<Self> {
        let depth = desc.format.has_depth();
        // Locations below follow the color's: without it, IDs would be at location 0, but shaders
        // write them at 1. No pass draws IDs without color.
        assert!(
            desc.format.has_color() || !desc.format.has_ids(),
            "a pipeline draws IDs only beside a color target, its first"
        );
        assert!(
            desc.format.has_ids() || !desc.format.has_motion(),
            "a pipeline draws motion only beside IDs, its third color target"
        );
        // Integers don't blend (VUID-VkGraphicsPipelineCreateInfo-renderPass-06062).
        assert!(
            desc.blend == Blend::Opaque || desc.format.color != Format::VISIBILITY.color,
            "a pipeline into a visibility buffer doesn't blend"
        );
        assert!(
            desc.mesh.is_none() || device.mesh_shaders(),
            "a pipeline of a mesh shader is made only on a device that draws through them"
        );
        assert!(
            depth || !(desc.depth_write || desc.depth_bias != 0.0 || desc.depth_clamp),
            "a pipeline writes, biases and clamps depth only for targets that have it"
        );
        assert!(
            desc.format.has_ids() || !desc.ids_write,
            "a pipeline writes IDs only for targets that have them"
        );
        assert!(
            desc.fragment.is_some() || !desc.format.has_color(),
            "a pipeline without a fragment shader draws depth alone"
        );
        let raw_device = device.raw();
        let module = shader_module(device, desc.name, desc.spirv)?;
        let mesh = match desc.mesh {
            Some(spirv) => match shader_module(device, desc.name, spirv) {
                Ok(mesh) => Some(mesh),
                Err(error) => {
                    // SAFETY: the module is live, and nothing has used it.
                    unsafe { raw_device.destroy_shader_module(module, None) };
                    return Err(error);
                }
            },
            None => None,
        };

        let geometry = if mesh.is_some() {
            vk::ShaderStageFlags::MESH_EXT
        } else {
            vk::ShaderStageFlags::VERTEX
        };
        let vertex = vk::PipelineShaderStageCreateInfo::default()
            .stage(geometry)
            .module(mesh.unwrap_or(module))
            .name(desc.vertex);
        let fragment = desc.fragment.map(|entry| {
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(module)
                .name(entry)
        });
        let stages: Vec<_> = iter::once(vertex).chain(fragment).collect();
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0)
            .depth_clamp_enable(desc.depth_clamp)
            .depth_bias_enable(desc.depth_bias != 0.0)
            .depth_bias_slope_factor(-desc.depth_bias);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let attachment = desc.blend.attachment();
        // Integers don't blend (VUID-VkGraphicsPipelineCreateInfo-renderPass-06062), so the IDs'
        // state differs from a blended color's: independent blending.
        let ids =
            vk::PipelineColorBlendAttachmentState::default().color_write_mask(if desc.ids_write {
                vk::ColorComponentFlags::R | vk::ColorComponentFlags::G
            } else {
                vk::ColorComponentFlags::empty()
            });
        // Written where depth is, unblended, and left alone by a pipeline that writes none.
        let motion = vk::PipelineColorBlendAttachmentState::default().color_write_mask(
            if desc.depth_write {
                vk::ColorComponentFlags::RGBA
            } else {
                vk::ColorComponentFlags::empty()
            },
        );
        let attachments = [attachment, ids, motion];
        // The color target's, then the IDs', then the motion's, of those it has.
        let first = usize::from(!desc.format.has_color());
        let targets =
            1 + usize::from(desc.format.has_ids()) + usize::from(desc.format.has_motion());
        let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(&attachments[first..targets]);
        // Or equal, so later draws at one depth go over earlier ones, as 2D games stack at one z.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(depth)
            .depth_write_enable(desc.depth_write)
            .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL);
        let dynamic_states = [
            vk::DynamicState::VIEWPORT,
            vk::DynamicState::SCISSOR,
            vk::DynamicState::CULL_MODE,
            vk::DynamicState::FRONT_FACE,
        ];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = [desc.format.color, desc.format.ids, desc.format.motion];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&formats[first..targets])
            .depth_attachment_format(desc.format.depth);
        let mut feedback = vk::PipelineCreationFeedback::default();
        let mut feedback_info = vk::PipelineCreationFeedbackCreateInfo::default()
            .pipeline_creation_feedback(&mut feedback);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic)
            .layout(device.bindless().pipeline_layout())
            .push_next(&mut rendering)
            .push_next(&mut feedback_info);
        // SAFETY: the modules, the layout and the pipeline cache, if any, are live, and the infos
        // outlive the call; the modules can go once the pipeline is made. The modules are trusted
        // to have their entry points and keep to the layout, as `GraphicsDesc::spirv` says. The
        // device has depth clamp, which `Candidate::new` checked, and mesh shaders, for a mesh
        // shader's pipeline, as asserted; Vulkan ignores the vertex input and input assembly of a
        // pipeline with a mesh shader.
        let made = unsafe {
            let cache = device.raw_pipeline_cache();
            let made = raw_device.create_graphics_pipelines(cache, &[info], None);
            for module in iter::once(module).chain(mesh) {
                raw_device.destroy_shader_module(module, None);
            }
            made
        };
        let raw = made.map_err(|(_, e)| {
            let pipeline = format_args!(
                "pipeline {:?} for {:?}, blending {:?}",
                desc.name, desc.format, desc.blend
            );
            vk_error("vkCreateGraphicsPipelines", pipeline, e)
        })?[0];
        // SAFETY: the pipeline is live.
        unsafe { device.set_name(raw, desc.name) };
        made_log(desc.name, feedback);
        Ok(Self {
            device: device.clone(),
            raw,
            format: desc.format,
            mesh: desc.mesh.is_some(),
        })
    }

    pub(crate) fn raw(&self) -> vk::Pipeline {
        self.raw
    }

    /// The handle of the device that made it.
    pub(crate) fn device(&self) -> vk::Device {
        self.device.handle()
    }

    /// The formats of the targets it draws into.
    pub(crate) fn format(&self) -> Format {
        self.format
    }

    /// Whether its geometry is a mesh shader, which draws only through
    /// [`Pass::draw_mesh_tasks_indirect`](crate::Pass::draw_mesh_tasks_indirect).
    pub(crate) fn mesh(&self) -> bool {
        self.mesh
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.device.retire(Retired::Pipeline(self.raw));
    }
}

/// A compute pipeline for a [`Graph`]'s compute passes. Dropping it retires it until the GPU is
/// done with it.
///
/// [`Graph`]: crate::Graph
pub struct ComputePipeline {
    device: Arc<Device>,
    raw: vk::Pipeline,
}

impl ComputePipeline {
    /// Makes a pipeline `name` of the entry point `entry` of `spirv`, a SPIR-V module slangc
    /// compiled, through the device's pipeline cache if it has one. The module is trusted as
    /// [`GraphicsDesc::spirv`] says, and to write only the bindless set's storage images. Fails
    /// as [`Pipeline::graphics`] does.
    pub fn new(device: &Arc<Device>, name: &str, spirv: &[u8], entry: &CStr) -> Result<Self> {
        let module = shader_module(device, name, spirv)?;
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(entry);
        let mut feedback = vk::PipelineCreationFeedback::default();
        let mut feedback_info = vk::PipelineCreationFeedbackCreateInfo::default()
            .pipeline_creation_feedback(&mut feedback);
        let info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(device.bindless().pipeline_layout())
            .push_next(&mut feedback_info);
        let raw_device = device.raw();
        // SAFETY: as for a graphics pipeline's, but for the one entry point.
        let made = unsafe {
            let cache = device.raw_pipeline_cache();
            let made = raw_device.create_compute_pipelines(cache, &[info], None);
            raw_device.destroy_shader_module(module, None);
            made
        };
        let raw = made.map_err(|(_, e)| {
            vk_error(
                "vkCreateComputePipelines",
                format_args!("compute pipeline {name:?} of {entry:?}"),
                e,
            )
        })?[0];
        // SAFETY: the pipeline is live.
        unsafe { device.set_name(raw, name) };
        made_log(name, feedback);
        Ok(Self {
            device: device.clone(),
            raw,
        })
    }

    pub(crate) fn raw(&self) -> vk::Pipeline {
        self.raw
    }

    /// The handle of the device that made it.
    pub(crate) fn device(&self) -> vk::Device {
        self.device.handle()
    }
}

impl Drop for ComputePipeline {
    fn drop(&mut self) {
        self.device.retire(Retired::Pipeline(self.raw));
    }
}

/// A shader module of `spirv`, named for pipeline `name`.
///
/// Fails with `gpu.bad_shader` if `spirv` isn't SPIR-V.
fn shader_module(device: &Device, name: &str, spirv: &[u8]) -> Result<vk::ShaderModule> {
    let code = ash::util::read_spv(&mut Cursor::new(spirv)).map_err(|error| {
        Error::new(
            "gpu.bad_shader",
            format!("the shaders of pipeline {name:?} aren't SPIR-V"),
        )
        .with_source(error)
    })?;
    let module_info = vk::ShaderModuleCreateInfo::default().code(&code);
    // SAFETY: the device is live, and the code is a whole number of words starting with SPIR-V's
    // magic number, which `read_spv` checked. The module is trusted to be valid SPIR-V, as
    // `GraphicsDesc::spirv` says.
    let module =
        unsafe { device.raw().create_shader_module(&module_info, None) }.map_err(vk_failed(
            "vkCreateShaderModule",
            format_args!("the shaders of pipeline {name:?}"),
        ))?;
    // SAFETY: the module is live.
    unsafe { device.set_name(module, name) };
    Ok(module)
}

/// Logs that pipeline `name` was made: the driver's time, and whether the pipeline cache held it.
fn made_log(name: &str, feedback: vk::PipelineCreationFeedback) {
    let flags = feedback.flags;
    debug!(
        pipeline = name,
        us = flags
            .contains(vk::PipelineCreationFeedbackFlags::VALID)
            .then_some(feedback.duration / 1000),
        cache_hit =
            flags.contains(vk::PipelineCreationFeedbackFlags::APPLICATION_PIPELINE_CACHE_HIT),
        "Pipeline made"
    );
}
