#include "ggpch.h"
#include "Renderer2DBase.h"
#include "SceneCamera.h"
#include "GGEngine/Core/Profiler.h"
#include "RenderCommand.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "GGEngine/RHI/RHICommandBuffer.h"

namespace GGEngine {

    void Renderer2DBase::InitBase()
    {
        // Create 1x1 white texture for solid color rendering
        uint32_t whitePixel = 0xFFFFFFFF;  // RGBA: white with full alpha
        m_WhiteTexture = Texture::Create(1, 1, &whitePixel);
        m_WhiteTextureIndex = m_WhiteTexture->GetBindlessIndex();

        // Create descriptor set layout for camera UBO (Set 0)
        std::vector<DescriptorBinding> cameraBindings = {
            { 0, DescriptorType::UniformBuffer, ShaderStage::Vertex, 1 }
        };
        m_CameraDescriptorLayout = CreateScope<DescriptorSetLayout>(cameraBindings);

        // Create per-frame camera UBOs and descriptor sets
        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            m_CameraUniformBuffers[i] = CreateScope<UniformBuffer>(sizeof(CameraUBO));
            m_CameraDescriptorSets[i] = CreateScope<DescriptorSet>(*m_CameraDescriptorLayout);
            m_CameraDescriptorSets[i]->SetUniformBuffer(0, *m_CameraUniformBuffers[i]);
        }
    }

    void Renderer2DBase::ShutdownBase()
    {
        m_Pipeline.reset();

        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            m_CameraDescriptorSets[i].reset();
            m_CameraUniformBuffers[i].reset();
        }
        m_CameraDescriptorLayout.reset();

        m_WhiteTexture.reset();
    }

    bool Renderer2DBase::BeginSceneInternal(const CameraUBO& cameraUBO,
                                             RHIRenderPassHandle renderPass,
                                             RHICommandBufferHandle cmd,
                                             uint32_t viewportWidth,
                                             uint32_t viewportHeight)
    {
        GG_PROFILE_FUNCTION();

        // Check if we need to grow buffers from previous frame
        if (m_NeedsBufferGrowth && GetCurrentCapacity() < GetAbsoluteMaxCapacity())
        {
            GrowBuffers();
            m_NeedsBufferGrowth = false;
        }

        // Get current frame index for per-frame resources
        m_CurrentFrameIndex = RHIDevice::Get().GetCurrentFrameIndex();

        // Update camera UBO for current frame
        m_CameraUniformBuffers[m_CurrentFrameIndex]->SetData(cameraUBO);

        // Create or recreate pipeline if render pass changed
        if (!m_Pipeline || m_CurrentRenderPass != renderPass)
        {
            RecreatePipeline(renderPass);
            m_CurrentRenderPass = renderPass;
        }

        // Store current render state
        m_CurrentCommandBuffer = cmd;
        m_ViewportWidth = viewportWidth;
        m_ViewportHeight = viewportHeight;

        m_SceneStarted = true;

        // Let derived class do its own BeginScene setup
        OnBeginScene();

        return true;
    }

    void Renderer2DBase::SetViewportAndScissor()
    {
        RHICmd::SetViewport(m_CurrentCommandBuffer, m_ViewportWidth, m_ViewportHeight);
        RHICmd::SetScissor(m_CurrentCommandBuffer, m_ViewportWidth, m_ViewportHeight);
    }

    void Renderer2DBase::BindCameraDescriptorSet(RHIPipelineLayoutHandle pipelineLayout)
    {
        m_CameraDescriptorSets[m_CurrentFrameIndex]->Bind(m_CurrentCommandBuffer, pipelineLayout, 0);
    }

    void Renderer2DBase::BindBindlessDescriptorSet(RHIPipelineLayoutHandle pipelineLayout)
    {
        RHICmd::BindDescriptorSetRaw(m_CurrentCommandBuffer, pipelineLayout,
                                      BindlessTextureManager::Get().GetDescriptorSet(), 1);
    }

}
