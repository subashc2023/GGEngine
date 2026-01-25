#include "ggpch.h"
#include "InstancedRenderer2D.h"
#include "Renderer2DBase.h"
#include "Camera.h"
#include "SceneCamera.h"
#include "GGEngine/Core/Profiler.h"
#include "VertexBuffer.h"
#include "IndexBuffer.h"
#include "VertexLayout.h"
#include "RenderCommand.h"
#include "GGEngine/Asset/Shader.h"
#include "GGEngine/Asset/ShaderLibrary.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "GGEngine/RHI/RHICommandBuffer.h"

#include <cmath>
#include <array>

namespace GGEngine {

    // Static quad vertex (shared across all instances)
    struct StaticQuadVertex
    {
        float LocalPosition[2];  // Unit quad corners
        float BaseUV[2];         // Base UV (0-1)
    };

    // Implementation class inheriting from base
    class InstancedRenderer2DImpl : public Renderer2DBase
    {
    public:
        static constexpr uint32_t InitialMaxInstances = 100000;
        static constexpr uint32_t AbsoluteMaxInstances = 1000000;

        // Dynamic capacity
        uint32_t MaxInstances = InitialMaxInstances;

        // Static quad vertex buffer (binding 0, shared across all instances)
        Scope<VertexBuffer> StaticQuadBuffer;
        Scope<IndexBuffer> QuadIndexBuffer;
        VertexLayout StaticVertexLayout;

        // Instance data buffers (binding 1, per-frame to avoid GPU/CPU conflicts)
        Scope<VertexBuffer> InstanceBuffers[MaxFramesInFlight];
        VertexLayout InstanceLayout;

        // CPU-side instance staging buffer
        std::unique_ptr<QuadInstanceData[]> InstanceBufferBase;
        std::atomic<uint32_t> InstanceCount{0};

        // Shader
        AssetHandle<Shader> InstancedShader;

        // Statistics
        InstancedRenderer2D::Statistics Stats;

        // Unit quad vertices (centered at origin)
        static constexpr std::array<StaticQuadVertex, 4> QuadVertices = {{
            { { -0.5f, -0.5f }, { 0.0f, 0.0f } },
            { {  0.5f, -0.5f }, { 1.0f, 0.0f } },
            { {  0.5f,  0.5f }, { 1.0f, 1.0f } },
            { { -0.5f,  0.5f }, { 0.0f, 1.0f } }
        }};

        void Init(uint32_t initialMaxInstances);
        void Shutdown();
        void Flush();

    protected:
        void OnBeginScene() override;
        void RecreatePipeline(RHIRenderPassHandle renderPass) override;
        void GrowBuffers() override;
        uint32_t GetCurrentCapacity() const override { return MaxInstances; }
        uint32_t GetAbsoluteMaxCapacity() const override { return AbsoluteMaxInstances; }
    };

    static InstancedRenderer2DImpl s_Impl;

    // ============================================================================
    // InstancedRenderer2DImpl Implementation
    // ============================================================================

    void InstancedRenderer2DImpl::Init(uint32_t initialMaxInstances)
    {
        GG_PROFILE_FUNCTION();
        GG_CORE_INFO("InstancedRenderer2D: Initializing...");

        MaxInstances = std::min(initialMaxInstances, AbsoluteMaxInstances);

        // Create static quad vertex layout (binding 0)
        StaticVertexLayout
            .Push("aLocalPosition", VertexAttributeType::Float2)
            .Push("aBaseUV", VertexAttributeType::Float2);

        // Create static quad vertex buffer
        StaticQuadBuffer = VertexBuffer::Create(
            QuadVertices.data(),
            sizeof(QuadVertices),
            StaticVertexLayout
        );

        // Create index buffer
        std::vector<uint32_t> indices = { 0, 1, 2, 2, 3, 0 };
        QuadIndexBuffer = IndexBuffer::Create(indices);

        // Create instance data layout (binding 1)
        InstanceLayout
            .Push("aPosition", VertexAttributeType::Float3)
            .Push("aRotation", VertexAttributeType::Float)
            .Push("aScale", VertexAttributeType::Float2)
            .Push("_pad1", VertexAttributeType::Float2)
            .Push("aColor", VertexAttributeType::Float4)
            .Push("aTexCoords", VertexAttributeType::Float4)
            .Push("aTexIndex", VertexAttributeType::UInt)
            .Push("aTilingFactor", VertexAttributeType::Float)
            .Push("_pad2", VertexAttributeType::Float2);

        // Allocate CPU-side instance buffer
        InstanceBufferBase = std::make_unique<QuadInstanceData[]>(MaxInstances);

        // Create GPU instance buffers (per-frame)
        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            InstanceBuffers[i] = CreateScope<VertexBuffer>(
                static_cast<uint64_t>(MaxInstances) * sizeof(QuadInstanceData),
                InstanceLayout
            );
        }

        // Initialize base class resources (white texture, camera descriptors)
        InitBase();

        // Get instanced shader from library
        InstancedShader = ShaderLibrary::Get().Get("quad2d_instanced");
        if (!InstancedShader.IsValid())
        {
            GG_CORE_ERROR("InstancedRenderer2D: Failed to get 'quad2d_instanced' shader from library!");
            return;
        }

        GG_CORE_INFO("InstancedRenderer2D: Initialized ({} max instances, {} max textures, {} frames in flight)",
                     MaxInstances,
                     BindlessTextureManager::Get().GetMaxTextures(),
                     MaxFramesInFlight);
    }

    void InstancedRenderer2DImpl::Shutdown()
    {
        GG_PROFILE_FUNCTION();
        GG_CORE_INFO("InstancedRenderer2D: Shutting down...");

        InstanceBufferBase.reset();

        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            InstanceBuffers[i].reset();
        }
        QuadIndexBuffer.reset();
        StaticQuadBuffer.reset();

        InstancedShader = AssetHandle<Shader>();

        // Shutdown base class resources
        ShutdownBase();

        GG_CORE_TRACE("InstancedRenderer2D: Shutdown complete");
    }

    void InstancedRenderer2DImpl::OnBeginScene()
    {
        // Reset instance count (atomic for thread safety)
        InstanceCount.store(0, std::memory_order_relaxed);
    }

    void InstancedRenderer2DImpl::RecreatePipeline(RHIRenderPassHandle renderPass)
    {
        m_Pipeline.reset();

        PipelineSpecification spec;
        spec.shader = InstancedShader.Get();
        spec.renderPass = renderPass;
        spec.vertexLayout = &StaticVertexLayout;  // Binding 0: static quad vertices
        spec.cullMode = CullMode::None;
        spec.blendMode = BlendMode::Alpha;
        spec.depthTestEnable = false;
        spec.depthWriteEnable = false;

        // Binding 1: instance data (per-instance rate)
        VertexBindingInfo instanceBinding;
        instanceBinding.layout = &InstanceLayout;
        instanceBinding.binding = 1;
        instanceBinding.startLocation = 2;  // Shader locations 2-10
        instanceBinding.inputRate = VertexInputRate::Instance;
        spec.additionalVertexBindings.push_back(instanceBinding);

        // Descriptor sets
        spec.descriptorSetLayouts.push_back(m_CameraDescriptorLayout->GetHandle());
        spec.descriptorSetLayouts.push_back(BindlessTextureManager::Get().GetLayoutHandle());
        spec.debugName = "InstancedRenderer2D_Quad";

        m_Pipeline = CreateScope<Pipeline>(spec);
    }

    void InstancedRenderer2DImpl::GrowBuffers()
    {
        uint32_t newMaxInstances = std::min(MaxInstances * 2, AbsoluteMaxInstances);
        if (newMaxInstances == MaxInstances)
        {
            GG_CORE_WARN("InstancedRenderer2D: Cannot grow buffers - already at maximum capacity ({} instances)", MaxInstances);
            return;
        }

        GG_CORE_INFO("InstancedRenderer2D: Growing buffers {} -> {} instances", MaxInstances, newMaxInstances);

        RHIDevice::Get().WaitIdle();

        // Reallocate CPU staging buffer
        InstanceBufferBase = std::make_unique<QuadInstanceData[]>(newMaxInstances);

        // Reallocate GPU instance buffers
        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            InstanceBuffers[i].reset();
            InstanceBuffers[i] = CreateScope<VertexBuffer>(
                static_cast<uint64_t>(newMaxInstances) * sizeof(QuadInstanceData),
                InstanceLayout
            );
        }

        MaxInstances = newMaxInstances;
        GG_CORE_INFO("InstancedRenderer2D: Buffer growth complete (now {} instances, ~{} MB per buffer)",
                     MaxInstances, (MaxInstances * sizeof(QuadInstanceData)) / (1024 * 1024));
    }

    void InstancedRenderer2DImpl::Flush()
    {
        GG_PROFILE_FUNCTION();

        uint32_t instanceCount = InstanceCount.load(std::memory_order_relaxed);
        if (instanceCount == 0)
            return;

        // Upload instance data to GPU
        uint64_t dataSize = static_cast<uint64_t>(instanceCount) * sizeof(QuadInstanceData);
        InstanceBuffers[m_CurrentFrameIndex]->SetData(InstanceBufferBase.get(), dataSize);

        // Set viewport and scissor
        SetViewportAndScissor();

        // Bind pipeline
        m_Pipeline->Bind(m_CurrentCommandBuffer);

        // Bind descriptors
        BindCameraDescriptorSet(m_Pipeline->GetLayoutHandle());
        BindBindlessDescriptorSet(m_Pipeline->GetLayoutHandle());

        // Bind vertex buffers
        StaticQuadBuffer->Bind(m_CurrentCommandBuffer, 0);       // Binding 0: static quad
        InstanceBuffers[m_CurrentFrameIndex]->Bind(m_CurrentCommandBuffer, 1);  // Binding 1: instance data

        // Bind index buffer
        QuadIndexBuffer->Bind(m_CurrentCommandBuffer);

        // Draw instanced: 6 indices per quad, instanceCount instances
        RHICmd::DrawIndexed(m_CurrentCommandBuffer, 6, instanceCount, 0, 0, 0);

        // Update stats
        Stats.DrawCalls++;
        Stats.InstanceCount = instanceCount;
    }

    // ============================================================================
    // Static API Implementation (delegates to s_Impl)
    // ============================================================================

    void InstancedRenderer2D::Init(uint32_t initialMaxInstances)
    {
        s_Impl.Init(initialMaxInstances);
    }

    void InstancedRenderer2D::Shutdown()
    {
        s_Impl.Shutdown();
    }

    void InstancedRenderer2D::BeginScene(const Camera& camera)
    {
        auto& device = RHIDevice::Get();
        BeginScene(camera, device.GetSwapchainRenderPass(), device.GetCurrentCommandBuffer(),
                   device.GetSwapchainWidth(), device.GetSwapchainHeight());
    }

    void InstancedRenderer2D::BeginScene(const Camera& camera, RHIRenderPassHandle renderPass,
                                          RHICommandBufferHandle cmd, uint32_t viewportWidth, uint32_t viewportHeight)
    {
        CameraUBO cameraUBO = camera.GetUBO();
        s_Impl.BeginSceneInternal(cameraUBO, renderPass, cmd, viewportWidth, viewportHeight);
    }

    void InstancedRenderer2D::BeginScene(const SceneCamera& camera, const glm::mat4& transform)
    {
        auto& device = RHIDevice::Get();
        BeginScene(camera, transform, device.GetSwapchainRenderPass(), device.GetCurrentCommandBuffer(),
                   device.GetSwapchainWidth(), device.GetSwapchainHeight());
    }

    void InstancedRenderer2D::BeginScene(const SceneCamera& camera, const glm::mat4& transform,
                                          RHIRenderPassHandle renderPass, RHICommandBufferHandle cmd,
                                          uint32_t viewportWidth, uint32_t viewportHeight)
    {
        glm::mat4 viewMatrix = glm::inverse(transform);
        glm::mat4 projectionMatrix = camera.GetProjection();
        glm::mat4 viewProjectionMatrix = projectionMatrix * viewMatrix;

        CameraUBO cameraUBO;
        cameraUBO.view = viewMatrix;
        cameraUBO.projection = projectionMatrix;
        cameraUBO.viewProjection = viewProjectionMatrix;

        s_Impl.BeginSceneInternal(cameraUBO, renderPass, cmd, viewportWidth, viewportHeight);
    }

    void InstancedRenderer2D::EndScene()
    {
        GG_PROFILE_FUNCTION();
        s_Impl.Flush();
        s_Impl.SetSceneStarted(false);
        s_Impl.ClearCommandBuffer();
    }

    QuadInstanceData* InstancedRenderer2D::AllocateInstances(uint32_t count)
    {
        if (!s_Impl.IsSceneStarted())
        {
            GG_CORE_WARN("InstancedRenderer2D::AllocateInstances called outside BeginScene/EndScene");
            return nullptr;
        }

        // Atomic allocation for thread safety
        uint32_t offset = s_Impl.InstanceCount.fetch_add(count, std::memory_order_relaxed);

        if (offset + count > s_Impl.MaxInstances)
        {
            // Request growth for next frame
            if (!s_Impl.NeedsBufferGrowth() && s_Impl.MaxInstances < InstancedRenderer2DImpl::AbsoluteMaxInstances)
            {
                s_Impl.RequestBufferGrowth();
                GG_CORE_INFO("InstancedRenderer2D: Buffer capacity exceeded - will grow on next frame");
            }
            return nullptr;
        }

        return s_Impl.InstanceBufferBase.get() + offset;
    }

    void InstancedRenderer2D::SubmitInstance(const QuadInstanceData& instance)
    {
        QuadInstanceData* ptr = AllocateInstances(1);
        if (ptr)
        {
            *ptr = instance;
        }
    }

    uint32_t InstancedRenderer2D::GetWhiteTextureIndex()
    {
        return s_Impl.GetWhiteTextureIndex();
    }

    void InstancedRenderer2D::ResetStats()
    {
        s_Impl.Stats = {};
        s_Impl.Stats.MaxInstanceCapacity = s_Impl.MaxInstances;
    }

    InstancedRenderer2D::Statistics InstancedRenderer2D::GetStats()
    {
        s_Impl.Stats.MaxInstanceCapacity = s_Impl.MaxInstances;
        return s_Impl.Stats;
    }

}
