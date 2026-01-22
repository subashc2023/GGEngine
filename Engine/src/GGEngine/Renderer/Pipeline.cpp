#include "ggpch.h"
#include "Pipeline.h"
#include "VertexLayout.h"
#include "DescriptorSet.h"
#include "GGEngine/Asset/Shader.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "GGEngine/RHI/RHICommandBuffer.h"

namespace GGEngine {

    Pipeline::Pipeline(const PipelineSpecification& spec)
        : m_Specification(spec)
    {
        Create();
    }

    Pipeline::~Pipeline()
    {
        Destroy();
    }

    void Pipeline::Bind(RHICommandBufferHandle cmd)
    {
        RHICmd::BindPipeline(cmd, m_Handle);
    }

    void Pipeline::Create()
    {
        auto& device = RHIDevice::Get();

        if (!m_Specification.shader || !m_Specification.shader->IsLoaded())
        {
            GG_CORE_ERROR("Pipeline creation failed: Invalid or unloaded shader");
            return;
        }

        if (!m_Specification.renderPass.IsValid())
        {
            GG_CORE_ERROR("Pipeline creation failed: No render pass specified");
            return;
        }

        // Build RHI graphics pipeline specification
        RHIGraphicsPipelineSpecification rhiSpec;
        rhiSpec.renderPass = m_Specification.renderPass;
        rhiSpec.subpass = m_Specification.subpass;

        // Collect shader module handles
        const auto& stages = m_Specification.shader->GetStages();
        if (stages.empty())
        {
            GG_CORE_ERROR("Pipeline creation failed: Shader has no stages");
            return;
        }
        for (const auto& stageInfo : stages)
        {
            rhiSpec.shaderModules.push_back(stageInfo.handle);
        }

        // Vertex input from layout
        if (m_Specification.vertexLayout && !m_Specification.vertexLayout->IsEmpty())
        {
            rhiSpec.vertexBindings.push_back(m_Specification.vertexLayout->GetBindingDescription());
            rhiSpec.vertexAttributes = m_Specification.vertexLayout->GetAttributeDescriptions();
        }

        // Input assembly
        rhiSpec.topology = m_Specification.topology;

        // Rasterization
        rhiSpec.polygonMode = m_Specification.polygonMode;
        rhiSpec.cullMode = m_Specification.cullMode;
        rhiSpec.frontFace = m_Specification.frontFace;
        rhiSpec.lineWidth = m_Specification.lineWidth;

        // Multisampling
        rhiSpec.samples = m_Specification.samples;

        // Depth
        rhiSpec.depthTestEnable = m_Specification.depthTestEnable;
        rhiSpec.depthWriteEnable = m_Specification.depthWriteEnable;
        rhiSpec.depthCompareOp = m_Specification.depthCompareOp;

        // Blending
        RHIBlendState blendState;
        switch (m_Specification.blendMode)
        {
            case BlendMode::None:
                blendState = RHIBlendState::Opaque();
                break;
            case BlendMode::Alpha:
                blendState = RHIBlendState::Alpha();
                break;
            case BlendMode::Additive:
                blendState = RHIBlendState::Additive();
                break;
        }
        rhiSpec.colorBlendStates.push_back(blendState);

        // Descriptor set layouts
        rhiSpec.descriptorSetLayouts = m_Specification.descriptorSetLayouts;

        // Push constant ranges
        for (const auto& range : m_Specification.pushConstantRanges)
        {
            RHIPushConstantRange rhiRange;
            rhiRange.stages = range.stageFlags;
            rhiRange.offset = range.offset;
            rhiRange.size = range.size;
            rhiSpec.pushConstantRanges.push_back(rhiRange);
        }

        rhiSpec.debugName = m_Specification.debugName;

        // Create pipeline through RHI device
        auto result = device.CreateGraphicsPipeline(rhiSpec);
        if (!result.IsValid())
        {
            GG_CORE_ERROR("Pipeline creation failed!");
            return;
        }

        m_Handle = result.pipeline;
        m_LayoutHandle = result.layout;

        if (!m_Specification.debugName.empty())
        {
            GG_CORE_INFO("Pipeline '{}' created successfully", m_Specification.debugName);
        }
    }

    void Pipeline::Destroy()
    {
        auto& device = RHIDevice::Get();
        device.WaitIdle();

        if (m_Handle.IsValid())
        {
            device.DestroyPipeline(m_Handle);
            m_Handle = NullPipeline;
        }

        if (m_LayoutHandle.IsValid())
        {
            device.DestroyPipelineLayout(m_LayoutHandle);
            m_LayoutHandle = NullPipelineLayout;
        }
    }

}
