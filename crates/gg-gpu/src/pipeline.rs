//! Graphics pipelines, made from SPIR-V with the bindless set's layout, for dynamic rendering.

use std::ffi::CStr;
use std::io::Cursor;
use std::sync::Arc;

use ash::vk;
use gg_core::{Error, Result};

use crate::device::Retired;
use crate::{Device, vk_error};

/// The pixel format of what a frame renders into, which pipelines are made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format(pub(crate) vk::Format);

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

/// What a graphics pipeline is made from. It draws triangle lists without culling or depth, and
/// its viewport and scissor are set as it draws.
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
    /// The format of the targets it draws into.
    pub format: Format,
    pub blend: Blend,
}

/// A graphics pipeline. Dropping it retires it until the GPU is done with it.
pub struct Pipeline {
    device: Arc<Device>,
    raw: vk::Pipeline,
    format: Format,
}

impl Pipeline {
    pub fn graphics(device: &Arc<Device>, desc: &GraphicsDesc<'_>) -> Result<Self> {
        let code = ash::util::read_spv(&mut Cursor::new(desc.spirv)).map_err(|error| {
            Error::new(
                "gpu.bad_shader",
                format!("the shaders of {} aren't SPIR-V", desc.name),
            )
            .with_source(error)
        })?;
        let raw_device = device.raw();
        let module_info = vk::ShaderModuleCreateInfo::default().code(&code);
        // SAFETY: the device is live, and the code is a whole number of words that starts with
        // SPIR-V's magic number, which `read_spv` checked. The module itself is trusted to be
        // valid SPIR-V, as `GraphicsDesc::spirv` says.
        let module = unsafe { raw_device.create_shader_module(&module_info, None) }
            .map_err(|e| vk_error("vkCreateShaderModule", e))?;
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
        let attachment = match desc.blend {
            Blend::Opaque => vk::PipelineColorBlendAttachmentState::default(),
            Blend::Alpha => vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD),
            Blend::PremultipliedAlpha => vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_DST_ALPHA)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                .alpha_blend_op(vk::BlendOp::ADD),
        }
        .color_write_mask(vk::ColorComponentFlags::RGBA);
        let attachments = [attachment];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = [desc.format.0];
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic)
            .layout(device.bindless().pipeline_layout())
            .push_next(&mut rendering);
        // SAFETY: the module and layout are live, and the infos outlive the call; the module
        // can go once the pipeline is made. The module is trusted to have both entry points and
        // to keep to the layout, as `GraphicsDesc::spirv` says.
        let made = unsafe {
            let made =
                raw_device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None);
            raw_device.destroy_shader_module(module, None);
            made
        };
        let raw = made.map_err(|(_, e)| vk_error("vkCreateGraphicsPipelines", e))?[0];
        // SAFETY: the pipeline is live.
        unsafe { device.set_name(raw, desc.name) };
        Ok(Self {
            device: device.clone(),
            raw,
            format: desc.format,
        })
    }

    pub(crate) fn raw(&self) -> vk::Pipeline {
        self.raw
    }

    /// The format of the targets it draws into.
    pub(crate) fn format(&self) -> Format {
        self.format
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.device.retire(Retired::Pipeline(self.raw));
    }
}
