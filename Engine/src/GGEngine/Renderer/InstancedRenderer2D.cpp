#include "ggpch.h"
#include "InstancedRenderer2D.h"
#include "SceneCamera.h"
#include "GGEngine/Core/Profiler.h"
#include "VertexBuffer.h"
#include "IndexBuffer.h"
#include "VertexLayout.h"
#include "UniformBuffer.h"
#include "DescriptorSet.h"
#include "Pipeline.h"
#include "RenderCommand.h"
#include "BindlessTextureManager.h"
#include "GGEngine/Asset/Shader.h"
#include "GGEngine/Asset/ShaderLibrary.h"
#include "GGEngine/Asset/Texture.h"
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

    // Internal renderer data
    struct InstancedRenderer2DData
    {
        static constexpr uint32_t MaxFramesInFlight = 2;
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

        // CPU-side instance staging buffer (unique_ptr for RAII)
        std::unique_ptr<QuadInstanceData[]> InstanceBufferBase;
        std::atomic<uint32_t> InstanceCount{0};

        // White pixel texture for solid colors
        Scope<Texture> WhiteTexture;
        BindlessTextureIndex WhiteTextureIndex = InvalidBindlessIndex;

        // Shader and pipeline
        AssetHandle<Shader> InstancedShader;
        Scope<Pipeline> InstancedPipeline;
        RHIRenderPassHandle CurrentRenderPass;

        // Camera UBO and descriptors (per-frame)
        Scope<UniformBuffer> CameraUniformBuffers[MaxFramesInFlight];
        Scope<DescriptorSetLayout> CameraDescriptorLayout;
        Scope<DescriptorSet> CameraDescriptorSets[MaxFramesInFlight];
        uint32_t CurrentFrameIndex = 0;

        // Current render state
        RHICommandBufferHandle CurrentCommandBuffer;
        uint32_t ViewportWidth = 0;
        uint32_t ViewportHeight = 0;
        bool SceneStarted = false;

        // Statistics
        InstancedRenderer2D::Statistics Stats;

        // Flag for buffer growth
        bool NeedsBufferGrowth = false;

        // Unit quad vertices (centered at origin)
        static constexpr std::array<StaticQuadVertex, 4> QuadVertices = {{
            { { -0.5f, -0.5f }, { 0.0f, 0.0f } },  // Bottom-left
            { {  0.5f, -0.5f }, { 1.0f, 0.0f } },  // Bottom-right
            { {  0.5f,  0.5f }, { 1.0f, 1.0f } },  // Top-right
            { { -0.5f,  0.5f }, { 0.0f, 1.0f } }   // Top-left
        }};
    };

    static InstancedRenderer2DData s_Data;

    // Forward declarations
    static void BeginSceneShared(const CameraUBO& cameraUBO,
                                  RHIRenderPassHandle renderPass,
                                  RHICommandBufferHandle cmd,
                                  uint32_t viewportWidth,
                                  uint32_t viewportHeight);
    static void Flush();
    static void GrowBuffers();

    static void GrowBuffers()
    {
        uint32_t newMaxInstances = std::min(s_Data.MaxInstances * 2, InstancedRenderer2DData::AbsoluteMaxInstances);
        if (newMaxInstances == s_Data.MaxInstances)
        {
            GG_CORE_WARN("InstancedRenderer2D: Cannot grow buffers - already at maximum capacity ({} instances)", s_Data.MaxInstances);
            return;
        }

        GG_CORE_INFO("InstancedRenderer2D: Growing buffers {} -> {} instances", s_Data.MaxInstances, newMaxInstances);

        RHIDevice::Get().WaitIdle();

        // Reallocate CPU staging buffer (using unique_ptr for exception safety)
        s_Data.InstanceBufferBase = std::make_unique<QuadInstanceData[]>(newMaxInstances);

        // Reallocate GPU instance buffers
        for (uint32_t i = 0; i < InstancedRenderer2DData::MaxFramesInFlight; i++)
        {
            s_Data.InstanceBuffers[i].reset();
            s_Data.InstanceBuffers[i] = CreateScope<VertexBuffer>(
                static_cast<uint64_t>(newMaxInstances) * sizeof(QuadInstanceData),
                s_Data.InstanceLayout
            );
        }

        s_Data.MaxInstances = newMaxInstances;
        GG_CORE_INFO("InstancedRenderer2D: Buffer growth complete (now {} instances, ~{} MB per buffer)",
                     s_Data.MaxInstances, (s_Data.MaxInstances * sizeof(QuadInstanceData)) / (1024 * 1024));
    }

    void InstancedRenderer2D::Init(uint32_t initialMaxInstances)
    {
        GG_PROFILE_FUNCTION();
        GG_CORE_INFO("InstancedRenderer2D: Initializing...");

        s_Data.MaxInstances = std::min(initialMaxInstances, InstancedRenderer2DData::AbsoluteMaxInstances);

        // Create static quad vertex layout (binding 0)
        s_Data.StaticVertexLayout
            .Push("aLocalPosition", VertexAttributeType::Float2)
            .Push("aBaseUV", VertexAttributeType::Float2);

        // Create static quad vertex buffer (shared across all frames and instances)
        s_Data.StaticQuadBuffer = VertexBuffer::Create(
            InstancedRenderer2DData::QuadVertices.data(),
            sizeof(InstancedRenderer2DData::QuadVertices),
            s_Data.StaticVertexLayout
        );

        // Create index buffer (same quad indices)
        std::vector<uint32_t> indices = { 0, 1, 2, 2, 3, 0 };
        s_Data.QuadIndexBuffer = IndexBuffer::Create(indices);

        // Create instance data layout (binding 1)
        // Must match shader locations 2-9
        s_Data.InstanceLayout
            .Push("aPosition", VertexAttributeType::Float3)     // location 2
            .Push("aRotation", VertexAttributeType::Float)      // location 3
            .Push("aScale", VertexAttributeType::Float2)        // location 4
            .Push("_pad1", VertexAttributeType::Float2)         // location 5 (padding)
            .Push("aColor", VertexAttributeType::Float4)        // location 6
            .Push("aTexCoords", VertexAttributeType::Float4)    // location 7
            .Push("aTexIndex", VertexAttributeType::UInt)       // location 8
            .Push("aTilingFactor", VertexAttributeType::Float)  // location 9
            .Push("_pad2", VertexAttributeType::Float2);        // location 10 (padding)

        // Allocate CPU-side instance buffer (zero-initialized via value initialization)
        s_Data.InstanceBufferBase = std::make_unique<QuadInstanceData[]>(s_Data.MaxInstances);

        // Create GPU instance buffers (per-frame)
        for (uint32_t i = 0; i < InstancedRenderer2DData::MaxFramesInFlight; i++)
        {
            s_Data.InstanceBuffers[i] = CreateScope<VertexBuffer>(
                static_cast<uint64_t>(s_Data.MaxInstances) * sizeof(QuadInstanceData),
                s_Data.InstanceLayout
            );
        }

        // Create white texture for solid colors
        uint32_t whitePixel = 0xFFFFFFFF;
        s_Data.WhiteTexture = Texture::Create(1, 1, &whitePixel);
        s_Data.WhiteTextureIndex = s_Data.WhiteTexture->GetBindlessIndex();

        // Get instanced shader from library
        s_Data.InstancedShader = ShaderLibrary::Get().Get("quad2d_instanced");
        if (!s_Data.InstancedShader.IsValid())
        {
            GG_CORE_ERROR("InstancedRenderer2D: Failed to get 'quad2d_instanced' shader from library!");
            return;
        }

        // Create descriptor set layout for camera UBO (Set 0)
        std::vector<DescriptorBinding> cameraBindings = {
            { 0, DescriptorType::UniformBuffer, ShaderStage::Vertex, 1 }
        };
        s_Data.CameraDescriptorLayout = CreateScope<DescriptorSetLayout>(cameraBindings);

        // Create per-frame camera UBOs and descriptor sets
        for (uint32_t i = 0; i < InstancedRenderer2DData::MaxFramesInFlight; i++)
        {
            s_Data.CameraUniformBuffers[i] = CreateScope<UniformBuffer>(sizeof(CameraUBO));
            s_Data.CameraDescriptorSets[i] = CreateScope<DescriptorSet>(*s_Data.CameraDescriptorLayout);
            s_Data.CameraDescriptorSets[i]->SetUniformBuffer(0, *s_Data.CameraUniformBuffers[i]);
        }

        GG_CORE_INFO("InstancedRenderer2D: Initialized ({} max instances, {} max textures, {} frames in flight)",
                     s_Data.MaxInstances,
                     BindlessTextureManager::Get().GetMaxTextures(),
                     InstancedRenderer2DData::MaxFramesInFlight);
    }

    void InstancedRenderer2D::Shutdown()
    {
        GG_PROFILE_FUNCTION();
        GG_CORE_INFO("InstancedRenderer2D: Shutting down...");

        s_Data.InstanceBufferBase.reset();

        s_Data.InstancedPipeline.reset();
        for (uint32_t i = 0; i < InstancedRenderer2DData::MaxFramesInFlight; i++)
        {
            s_Data.CameraDescriptorSets[i].reset();
            s_Data.CameraUniformBuffers[i].reset();
            s_Data.InstanceBuffers[i].reset();
        }
        s_Data.CameraDescriptorLayout.reset();
        s_Data.QuadIndexBuffer.reset();
        s_Data.StaticQuadBuffer.reset();
        s_Data.WhiteTexture.reset();
        s_Data.InstancedShader = AssetHandle<Shader>();

        GG_CORE_TRACE("InstancedRenderer2D: Shutdown complete");
    }

    static void BeginSceneShared(const CameraUBO& cameraUBO,
                                  RHIRenderPassHandle renderPass,
                                  RHICommandBufferHandle cmd,
                                  uint32_t viewportWidth,
                                  uint32_t viewportHeight)
    {
        GG_PROFILE_FUNCTION();

        // Check for buffer growth
        if (s_Data.NeedsBufferGrowth && s_Data.MaxInstances < InstancedRenderer2DData::AbsoluteMaxInstances)
        {
            GrowBuffers();
            s_Data.NeedsBufferGrowth = false;
        }

        s_Data.CurrentFrameIndex = RHIDevice::Get().GetCurrentFrameIndex();

        // Update camera UBO
        s_Data.CameraUniformBuffers[s_Data.CurrentFrameIndex]->SetData(cameraUBO);

        // Create or recreate pipeline if render pass changed
        if (!s_Data.InstancedPipeline || s_Data.CurrentRenderPass != renderPass)
        {
            s_Data.InstancedPipeline.reset();

            PipelineSpecification spec;
            spec.shader = s_Data.InstancedShader.Get();
            spec.renderPass = renderPass;
            spec.vertexLayout = &s_Data.StaticVertexLayout;  // Binding 0: static quad vertices
            spec.cullMode = CullMode::None;
            spec.blendMode = BlendMode::Alpha;
            spec.depthTestEnable = false;
            spec.depthWriteEnable = false;

            // Binding 1: instance data (per-instance rate)
            VertexBindingInfo instanceBinding;
            instanceBinding.layout = &s_Data.InstanceLayout;
            instanceBinding.binding = 1;
            instanceBinding.startLocation = 2;  // Shader locations 2-10
            instanceBinding.inputRate = VertexInputRate::Instance;
            spec.additionalVertexBindings.push_back(instanceBinding);

            // Descriptor sets
            spec.descriptorSetLayouts.push_back(s_Data.CameraDescriptorLayout->GetHandle());
            spec.descriptorSetLayouts.push_back(BindlessTextureManager::Get().GetLayoutHandle());
            spec.debugName = "InstancedRenderer2D_Quad";

            s_Data.InstancedPipeline = CreateScope<Pipeline>(spec);
            s_Data.CurrentRenderPass = renderPass;
        }

        // Store render state
        s_Data.CurrentCommandBuffer = cmd;
        s_Data.ViewportWidth = viewportWidth;
        s_Data.ViewportHeight = viewportHeight;

        // Reset instance count (atomic for thread safety)
        s_Data.InstanceCount.store(0, std::memory_order_relaxed);

        s_Data.SceneStarted = true;
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
        BeginSceneShared(cameraUBO, renderPass, cmd, viewportWidth, viewportHeight);
    }

    void InstancedRenderer2D::BeginScene(const SceneCamera& camera, const Mat4& transform)
    {
        auto& device = RHIDevice::Get();
        BeginScene(camera, transform, device.GetSwapchainRenderPass(), device.GetCurrentCommandBuffer(),
                   device.GetSwapchainWidth(), device.GetSwapchainHeight());
    }

    void InstancedRenderer2D::BeginScene(const SceneCamera& camera, const Mat4& transform,
                                          RHIRenderPassHandle renderPass, RHICommandBufferHandle cmd,
                                          uint32_t viewportWidth, uint32_t viewportHeight)
    {
        Mat4 viewMatrix = Mat4::Inverse(transform);
        Mat4 projectionMatrix = camera.GetProjection();
        Mat4 viewProjectionMatrix = projectionMatrix * viewMatrix;

        CameraUBO cameraUBO;
        cameraUBO.view = viewMatrix;
        cameraUBO.projection = projectionMatrix;
        cameraUBO.viewProjection = viewProjectionMatrix;

        BeginSceneShared(cameraUBO, renderPass, cmd, viewportWidth, viewportHeight);
    }

    void InstancedRenderer2D::EndScene()
    {
        GG_PROFILE_FUNCTION();
        Flush();
        s_Data.SceneStarted = false;
        s_Data.CurrentCommandBuffer = RHICommandBufferHandle{};
    }

    static void Flush()
    {
        GG_PROFILE_FUNCTION();

        uint32_t instanceCount = s_Data.InstanceCount.load(std::memory_order_relaxed);
        if (instanceCount == 0)
            return;

        // Upload instance data to GPU
        uint64_t dataSize = static_cast<uint64_t>(instanceCount) * sizeof(QuadInstanceData);
        s_Data.InstanceBuffers[s_Data.CurrentFrameIndex]->SetData(s_Data.InstanceBufferBase.get(), dataSize);

        RHICommandBufferHandle cmd = s_Data.CurrentCommandBuffer;

        // Set viewport and scissor
        RHICmd::SetViewport(cmd, s_Data.ViewportWidth, s_Data.ViewportHeight);
        RHICmd::SetScissor(cmd, s_Data.ViewportWidth, s_Data.ViewportHeight);

        // Bind pipeline
        s_Data.InstancedPipeline->Bind(cmd);

        // Bind Set 0: Camera UBO
        s_Data.CameraDescriptorSets[s_Data.CurrentFrameIndex]->Bind(cmd, s_Data.InstancedPipeline->GetLayoutHandle(), 0);

        // Bind Set 1: Bindless textures
        RHICmd::BindDescriptorSetRaw(cmd, s_Data.InstancedPipeline->GetLayoutHandle(),
                                      BindlessTextureManager::Get().GetDescriptorSet(), 1);

        // Bind vertex buffers
        s_Data.StaticQuadBuffer->Bind(cmd, 0);       // Binding 0: static quad
        s_Data.InstanceBuffers[s_Data.CurrentFrameIndex]->Bind(cmd, 1);  // Binding 1: instance data

        // Bind index buffer
        s_Data.QuadIndexBuffer->Bind(cmd);

        // Draw instanced: 6 indices per quad, instanceCount instances
        RHICmd::DrawIndexed(cmd, 6, instanceCount, 0, 0, 0);

        // Update stats
        s_Data.Stats.DrawCalls++;
        s_Data.Stats.InstanceCount = instanceCount;
    }

    QuadInstanceData* InstancedRenderer2D::AllocateInstances(uint32_t count)
    {
        if (!s_Data.SceneStarted)
        {
            GG_CORE_WARN("InstancedRenderer2D::AllocateInstances called outside BeginScene/EndScene");
            return nullptr;
        }

        // Atomic allocation for thread safety
        uint32_t offset = s_Data.InstanceCount.fetch_add(count, std::memory_order_relaxed);

        if (offset + count > s_Data.MaxInstances)
        {
            // Request growth for next frame
            if (!s_Data.NeedsBufferGrowth && s_Data.MaxInstances < InstancedRenderer2DData::AbsoluteMaxInstances)
            {
                s_Data.NeedsBufferGrowth = true;
                GG_CORE_INFO("InstancedRenderer2D: Buffer capacity exceeded - will grow on next frame");
            }
            return nullptr;
        }

        return s_Data.InstanceBufferBase.get() + offset;
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
        return s_Data.WhiteTextureIndex;
    }

    void InstancedRenderer2D::ResetStats()
    {
        s_Data.Stats = {};
        s_Data.Stats.MaxInstanceCapacity = s_Data.MaxInstances;
    }

    InstancedRenderer2D::Statistics InstancedRenderer2D::GetStats()
    {
        s_Data.Stats.MaxInstanceCapacity = s_Data.MaxInstances;
        return s_Data.Stats;
    }

}
