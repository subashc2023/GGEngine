//! Graphics pipelines, made from SPIR-V with the bindless set's layout, for dynamic rendering.

use std::ffi::CStr;
use std::io::Cursor;
use std::sync::Arc;

use ash::vk;
use gg_core::{Error, Result};
use tracing::debug;

use crate::device::Retired;
use crate::{DepthImage, Device, IdImage, vk_error};

/// The pixel formats of what a pass renders into, which pipelines are made for: its target's, its
/// [`IdImage`]'s, if it has one, and its depth image's, if it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub(crate) color: vk::Format,
    /// `UNDEFINED` for a pass without IDs.
    pub(crate) ids: vk::Format,
    /// `UNDEFINED` for a pass without depth.
    pub(crate) depth: vk::Format,
}

impl Format {
    /// A target's format, `color`, without IDs or depth.
    pub(crate) const fn new(color: vk::Format) -> Self {
        Self {
            color,
            ids: vk::Format::UNDEFINED,
            depth: vk::Format::UNDEFINED,
        }
    }

    /// This format with an [`IdImage`]'s: what a pass into a target of this format with IDs
    /// renders into, as a second color target.
    pub const fn with_ids(self) -> Self {
        Self {
            ids: IdImage::FORMAT,
            ..self
        }
    }

    /// This format with a [`DepthImage`]'s: what a pass into a target of this format with a depth
    /// image renders into.
    pub const fn with_depth(self) -> Self {
        Self {
            depth: DepthImage::FORMAT,
            ..self
        }
    }

    fn has_ids(self) -> bool {
        self.ids != vk::Format::UNDEFINED
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
    /// The output, whose color is already multiplied by its alpha, goes over it.
    PremultipliedAlpha,
}

impl Blend {
    /// The color target's blend state, which combines the output with it this way.
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

/// Which of a mesh's triangles draw, by the way their corners wind as they show on screen.
/// Vulkan tells it in framebuffer coordinates, whose Y points down, and flips the sign of the
/// area it measures there, so its counterclockwise is as seen; the renderer's cameras show the
/// world upright.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Faces {
    /// All of them: what a pass starts drawing.
    Both,
    /// Those whose corners wind counterclockwise: the fronts of a mesh whose fronts wind so seen
    /// from in front, as glTF's do, drawn with a transform that doesn't mirror it.
    CounterClockwise,
    /// Those whose corners wind clockwise: such a mesh's fronts drawn with a transform that
    /// mirrors it.
    Clockwise,
}

impl Faces {
    /// The cull mode and front face that draw them.
    pub(crate) fn state(self) -> (vk::CullModeFlags, vk::FrontFace) {
        match self {
            Self::Both => (vk::CullModeFlags::NONE, vk::FrontFace::COUNTER_CLOCKWISE),
            Self::CounterClockwise => (vk::CullModeFlags::BACK, vk::FrontFace::COUNTER_CLOCKWISE),
            Self::Clockwise => (vk::CullModeFlags::BACK, vk::FrontFace::CLOCKWISE),
        }
    }
}

/// What a graphics pipeline is made from. It draws triangle lists, and its viewport, scissor and
/// which [`Faces`] draw are set as it draws, a pass starting with both. Where its targets have
/// depth, it tests it, drawing only where it's as near as what was drawn there before or nearer:
/// depth runs from 1 at the near plane to 0 far away, as the renderer's cameras project it.
pub struct GraphicsDesc<'a> {
    /// The debug name.
    pub name: &'a str,
    /// A SPIR-V module holding both entry points, which slangc compiled. It's trusted to be valid
    /// and to keep to the pipeline layout: to read only the push constants set before a draw,
    /// to use only the device's enabled features, and to bind nothing but the bindless set's
    /// textures and samplers. Vulkan checks none of it; validation does in debug builds.
    pub spirv: &'a [u8],
    pub vertex: &'a CStr,
    pub fragment: &'a CStr,
    /// The formats of the targets it draws into. Where they have IDs, its fragment shader writes
    /// them at location 1, as two 32-bit unsigned integers, which replace what's there wherever it
    /// draws, however its color blends.
    pub format: Format,
    pub blend: Blend,
    /// Whether it writes its depth where it draws, for later draws to test against. Only a
    /// pipeline whose targets have depth can.
    pub depth_write: bool,
}

/// A graphics pipeline. Dropping it retires it until the GPU is done with it.
pub struct Pipeline {
    device: Arc<Device>,
    raw: vk::Pipeline,
    format: Format,
}

impl Pipeline {
    /// Makes a pipeline from `desc`, through the device's pipeline cache if it has one. Fails
    /// with `gpu.bad_shader` if `desc.spirv` isn't SPIR-V, and `gpu.vulkan` or `gpu.out_of_memory`
    /// if Vulkan can't make it.
    ///
    /// # Panics
    ///
    /// If it writes depth into targets without it.
    pub fn graphics(device: &Arc<Device>, desc: &GraphicsDesc<'_>) -> Result<Self> {
        let depth = desc.format.has_depth();
        assert!(
            depth || !desc.depth_write,
            "a pipeline writes depth only into targets that have it"
        );
        let code = ash::util::read_spv(&mut Cursor::new(desc.spirv)).map_err(|error| {
            Error::new(
                "gpu.bad_shader",
                format!("the shaders of pipeline {:?} aren't SPIR-V", desc.name),
            )
            .with_source(error)
        })?;
        let raw_device = device.raw();
        let module_info = vk::ShaderModuleCreateInfo::default().code(&code);
        // SAFETY: the device is live, and the code is a whole number of words that starts with
        // SPIR-V's magic number, which `read_spv` checked. The module itself is trusted to be
        // valid SPIR-V, as `GraphicsDesc::spirv` says.
        let module =
            unsafe { raw_device.create_shader_module(&module_info, None) }.map_err(|e| {
                let shaders = format_args!("the shaders of pipeline {:?}", desc.name);
                vk_error("vkCreateShaderModule", shaders, e)
            })?;
        // SAFETY: the module is live.
        unsafe { device.set_name(module, desc.name) };

        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(module)
                .name(desc.vertex),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(module)
                .name(desc.fragment),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let attachment = desc.blend.attachment();
        // Integers don't blend (VUID-VkGraphicsPipelineCreateInfo-renderPass-06062), so the IDs'
        // state differs from a blended color's, which takes independent blending.
        let ids = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::R | vk::ColorComponentFlags::G);
        let targets = 1 + usize::from(desc.format.has_ids());
        let attachments = [attachment, ids];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments[..targets]);
        // Or equal, so that what draws later at one depth goes over what drew there before, as 2D
        // games stack what they draw at one z.
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
        let formats = [desc.format.color, desc.format.ids];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&formats[..targets])
            .depth_attachment_format(desc.format.depth);
        // How long the driver took, and whether the device's pipeline cache held the pipeline.
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
        // SAFETY: the module, the layout and the pipeline cache, if the device has one, are live,
        // and the infos outlive the call; the module can go once the pipeline is made. The module
        // is trusted to have both entry points and to keep to the layout, as `GraphicsDesc::spirv`
        // says.
        let made = unsafe {
            let cache = device.raw_pipeline_cache();
            let made = raw_device.create_graphics_pipelines(cache, &[info], None);
            raw_device.destroy_shader_module(module, None);
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
        let flags = feedback.flags;
        debug!(
            pipeline = desc.name,
            us = flags
                .contains(vk::PipelineCreationFeedbackFlags::VALID)
                .then_some(feedback.duration / 1000),
            cache_hit =
                flags.contains(vk::PipelineCreationFeedbackFlags::APPLICATION_PIPELINE_CACHE_HIT),
            "Pipeline made"
        );
        Ok(Self {
            device: device.clone(),
            raw,
            format: desc.format,
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
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.device.retire(Retired::Pipeline(self.raw));
    }
}
