#include "ggpch.h"
#include "Renderer2D.h"
#include "Renderer2DBase.h"
#include "SubTexture2D.h"
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

#include <glm/glm.hpp>
#include <cmath>
#include <array>

namespace GGEngine {

    // Vertex structure for batched quads (bindless rendering)
    struct QuadVertex
    {
        float position[3];     // World-space position (CPU-transformed)
        float texCoord[2];     // UV coordinates
        float color[4];        // RGBA color
        float tilingFactor;    // Texture tiling multiplier
        uint32_t texIndex;     // Bindless texture index (0 = white texture)
    };

    // Implementation class inheriting from base
    class Renderer2DImpl : public Renderer2DBase
    {
    public:
        static constexpr uint32_t InitialMaxQuads = 100000;   // Start with 100k
        static constexpr uint32_t AbsoluteMaxQuads = 1000000; // Cap at 1M

        // Dynamic capacity (can grow at runtime)
        uint32_t MaxQuads = InitialMaxQuads;
        uint32_t MaxVertices = InitialMaxQuads * 4;
        uint32_t MaxIndices = InitialMaxQuads * 6;

        // Vertex data (per-frame vertex buffers to avoid GPU/CPU conflicts)
        Scope<VertexBuffer> QuadVertexBuffers[MaxFramesInFlight];
        Scope<IndexBuffer> QuadIndexBuffer;
        VertexLayout QuadVertexLayout;

        // CPU-side vertex staging buffer
        std::unique_ptr<QuadVertex[]> QuadVertexBufferBase;
        QuadVertex* QuadVertexBufferPtr = nullptr;
        uint32_t QuadIndexCount = 0;

        // GPU buffer offset tracking
        uint32_t QuadVertexOffset = 0;

        // Shader
        AssetHandle<Shader> QuadShader;

        // Statistics
        Renderer2D::Statistics Stats;

        // Track frame for vertex offset reset
        uint32_t LastResetFrameIndex = UINT32_MAX;

        // Unit quad vertex positions (centered at origin)
        static constexpr std::array<float[3], 4> QuadPositions = {{
            { -0.5f, -0.5f, 0.0f },
            {  0.5f, -0.5f, 0.0f },
            {  0.5f,  0.5f, 0.0f },
            { -0.5f,  0.5f, 0.0f }
        }};

        // Quad texture coordinates
        static constexpr std::array<float[2], 4> QuadTexCoords = {{
            { 0.0f, 0.0f },
            { 1.0f, 0.0f },
            { 1.0f, 1.0f },
            { 0.0f, 1.0f }
        }};

        void Init();
        void Shutdown();
        void Flush();

    protected:
        void OnBeginScene() override;
        void RecreatePipeline(RHIRenderPassHandle renderPass) override;
        void GrowBuffers() override;
        uint32_t GetCurrentCapacity() const override { return MaxQuads; }
        uint32_t GetAbsoluteMaxCapacity() const override { return AbsoluteMaxQuads; }
    };

    static Renderer2DImpl s_Impl;

    // Helper declarations
    static bool PrepareForQuad(const Texture* texture, uint32_t& outTextureIndex);
    static void WriteQuadVertices(const float positions[4][3], const float* texCoords,
                                   float r, float g, float b, float a,
                                   float tilingFactor, uint32_t textureIndex);

    // ============================================================================
    // Renderer2DImpl Implementation
    // ============================================================================

    void Renderer2DImpl::Init()
    {
        GG_PROFILE_FUNCTION();
        GG_CORE_INFO("Renderer2D: Initializing (bindless mode)...");

        // Create vertex layout
        QuadVertexLayout
            .Push("aPosition", VertexAttributeType::Float3)
            .Push("aTexCoord", VertexAttributeType::Float2)
            .Push("aColor", VertexAttributeType::Float4)
            .Push("aTilingFactor", VertexAttributeType::Float)
            .Push("aTexIndex", VertexAttributeType::UInt);

        // Allocate CPU-side vertex buffer
        QuadVertexBufferBase = std::make_unique<QuadVertex[]>(MaxVertices);
        QuadVertexBufferPtr = QuadVertexBufferBase.get();

        // Create GPU vertex buffers (one per frame in flight)
        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            QuadVertexBuffers[i] = CreateScope<VertexBuffer>(
                static_cast<uint64_t>(MaxVertices) * sizeof(QuadVertex),
                QuadVertexLayout
            );
            QuadVertexBuffers[i]->SetData(QuadVertexBufferBase.get(),
                MaxVertices * sizeof(QuadVertex));
        }

        // Generate indices for all quads
        std::vector<uint32_t> indices(MaxIndices);
        uint32_t offset = 0;
        for (uint32_t i = 0; i < MaxIndices; i += 6)
        {
            indices[i + 0] = offset + 0;
            indices[i + 1] = offset + 1;
            indices[i + 2] = offset + 2;
            indices[i + 3] = offset + 2;
            indices[i + 4] = offset + 3;
            indices[i + 5] = offset + 0;
            offset += 4;
        }
        QuadIndexBuffer = IndexBuffer::Create(indices);

        // Initialize base class resources (white texture, camera descriptors)
        InitBase();

        // Get quad shader from library
        QuadShader = ShaderLibrary::Get().Get("quad2d");
        if (!QuadShader.IsValid())
        {
            GG_CORE_ERROR("Renderer2D: Failed to get 'quad2d' shader from library!");
            return;
        }

        GG_CORE_INFO("Renderer2D: Initialized (bindless mode, initial {} quads (max {}), {} max textures, {} frames in flight)",
                     MaxQuads, AbsoluteMaxQuads,
                     BindlessTextureManager::Get().GetMaxTextures(),
                     MaxFramesInFlight);
    }

    void Renderer2DImpl::Shutdown()
    {
        GG_PROFILE_FUNCTION();
        GG_CORE_INFO("Renderer2D: Shutting down...");

        QuadVertexBufferBase.reset();
        QuadVertexBufferPtr = nullptr;

        QuadIndexBuffer.reset();
        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            QuadVertexBuffers[i].reset();
        }

        QuadShader = AssetHandle<Shader>();

        // Shutdown base class resources
        ShutdownBase();

        GG_CORE_TRACE("Renderer2D: Shutdown complete");
    }

    void Renderer2DImpl::OnBeginScene()
    {
        // Reset batch state
        QuadIndexCount = 0;
        QuadVertexBufferPtr = QuadVertexBufferBase.get();

        // Only reset GPU buffer offset when moving to a new frame
        if (m_CurrentFrameIndex != LastResetFrameIndex)
        {
            QuadVertexOffset = 0;
            LastResetFrameIndex = m_CurrentFrameIndex;
        }
    }

    void Renderer2DImpl::RecreatePipeline(RHIRenderPassHandle renderPass)
    {
        m_Pipeline.reset();

        PipelineSpecification spec;
        spec.shader = QuadShader.Get();
        spec.renderPass = renderPass;
        spec.vertexLayout = &QuadVertexLayout;
        spec.cullMode = CullMode::None;
        spec.blendMode = BlendMode::Alpha;
        spec.depthTestEnable = false;
        spec.depthWriteEnable = false;
        spec.descriptorSetLayouts.push_back(m_CameraDescriptorLayout->GetHandle());
        spec.descriptorSetLayouts.push_back(BindlessTextureManager::Get().GetLayoutHandle());
        spec.debugName = "Renderer2D_Quad_Bindless";

        m_Pipeline = CreateScope<Pipeline>(spec);
    }

    void Renderer2DImpl::GrowBuffers()
    {
        uint32_t newMaxQuads = std::min(MaxQuads * 2, AbsoluteMaxQuads);
        if (newMaxQuads == MaxQuads)
        {
            GG_CORE_WARN("Renderer2D: Cannot grow buffers - already at maximum capacity ({} quads)", MaxQuads);
            return;
        }

        uint32_t newMaxVertices = newMaxQuads * 4;
        uint32_t newMaxIndices = newMaxQuads * 6;

        GG_CORE_INFO("Renderer2D: Growing buffers {} -> {} quads", MaxQuads, newMaxQuads);

        RHIDevice::Get().WaitIdle();

        // Reallocate CPU staging buffer
        QuadVertexBufferBase = std::make_unique<QuadVertex[]>(newMaxVertices);
        QuadVertexBufferPtr = QuadVertexBufferBase.get();

        // Reallocate GPU vertex buffers
        for (uint32_t i = 0; i < MaxFramesInFlight; i++)
        {
            QuadVertexBuffers[i].reset();
            QuadVertexBuffers[i] = CreateScope<VertexBuffer>(
                static_cast<uint64_t>(newMaxVertices) * sizeof(QuadVertex),
                QuadVertexLayout
            );
        }

        // Reallocate index buffer
        std::vector<uint32_t> indices(newMaxIndices);
        uint32_t offset = 0;
        for (uint32_t i = 0; i < newMaxIndices; i += 6)
        {
            indices[i + 0] = offset + 0;
            indices[i + 1] = offset + 1;
            indices[i + 2] = offset + 2;
            indices[i + 3] = offset + 2;
            indices[i + 4] = offset + 3;
            indices[i + 5] = offset + 0;
            offset += 4;
        }
        QuadIndexBuffer.reset();
        QuadIndexBuffer = IndexBuffer::Create(indices);

        MaxQuads = newMaxQuads;
        MaxVertices = newMaxVertices;
        MaxIndices = newMaxIndices;

        GG_CORE_INFO("Renderer2D: Buffer growth complete (now {} quads, ~{} MB per vertex buffer)",
                     MaxQuads, (MaxVertices * sizeof(QuadVertex)) / (1024 * 1024));
    }

    void Renderer2DImpl::Flush()
    {
        GG_PROFILE_FUNCTION();
        if (QuadIndexCount == 0)
            return;

        // Calculate data size and vertex count
        uint32_t dataSize = static_cast<uint32_t>(
            reinterpret_cast<uint8_t*>(QuadVertexBufferPtr) -
            reinterpret_cast<uint8_t*>(QuadVertexBufferBase.get())
        );
        uint32_t vertexCount = dataSize / sizeof(QuadVertex);

        // Upload vertex data to current frame's GPU buffer
        uint64_t gpuOffset = static_cast<uint64_t>(QuadVertexOffset) * sizeof(QuadVertex);
        QuadVertexBuffers[m_CurrentFrameIndex]->SetData(QuadVertexBufferBase.get(), dataSize, gpuOffset);

        // Set viewport and scissor
        SetViewportAndScissor();

        // Bind pipeline
        m_Pipeline->Bind(m_CurrentCommandBuffer);

        // Bind descriptors
        BindCameraDescriptorSet(m_Pipeline->GetLayoutHandle());
        BindBindlessDescriptorSet(m_Pipeline->GetLayoutHandle());

        // Bind vertex and index buffers
        QuadVertexBuffers[m_CurrentFrameIndex]->Bind(m_CurrentCommandBuffer);
        QuadIndexBuffer->Bind(m_CurrentCommandBuffer);

        // Draw with vertex offset
        RHICmd::DrawIndexed(m_CurrentCommandBuffer, QuadIndexCount, 1, 0,
                            static_cast<int32_t>(QuadVertexOffset), 0);

        // Update GPU buffer offset for next batch
        QuadVertexOffset += vertexCount;

        Stats.DrawCalls++;

        // Reset batch state
        QuadIndexCount = 0;
        QuadVertexBufferPtr = QuadVertexBufferBase.get();
    }

    // ============================================================================
    // Static API Implementation (delegates to s_Impl)
    // ============================================================================

    void Renderer2D::Init()
    {
        s_Impl.Init();
    }

    void Renderer2D::Shutdown()
    {
        s_Impl.Shutdown();
    }

    void Renderer2D::BeginScene(const Camera& camera)
    {
        auto& device = RHIDevice::Get();
        BeginScene(camera, device.GetSwapchainRenderPass(), device.GetCurrentCommandBuffer(),
                   device.GetSwapchainWidth(), device.GetSwapchainHeight());
    }

    void Renderer2D::BeginScene(const Camera& camera, RHIRenderPassHandle renderPass,
                                RHICommandBufferHandle cmd, uint32_t viewportWidth, uint32_t viewportHeight)
    {
        CameraUBO cameraUBO = camera.GetUBO();
        s_Impl.BeginSceneInternal(cameraUBO, renderPass, cmd, viewportWidth, viewportHeight);
    }

    void Renderer2D::BeginScene(const SceneCamera& camera, const glm::mat4& transform)
    {
        auto& device = RHIDevice::Get();
        BeginScene(camera, transform, device.GetSwapchainRenderPass(), device.GetCurrentCommandBuffer(),
                   device.GetSwapchainWidth(), device.GetSwapchainHeight());
    }

    void Renderer2D::BeginScene(const SceneCamera& camera, const glm::mat4& transform,
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

    void Renderer2D::EndScene()
    {
        GG_PROFILE_FUNCTION();
        Flush();
        s_Impl.SetSceneStarted(false);
        s_Impl.ClearCommandBuffer();
    }

    void Renderer2D::Flush()
    {
        s_Impl.Flush();
    }

    // ============================================================================
    // Quad Drawing Helpers
    // ============================================================================

    static bool PrepareForQuad(const Texture* texture, uint32_t& outTextureIndex)
    {
        if (!s_Impl.IsSceneStarted())
        {
            GG_CORE_WARN("Renderer2D::DrawQuad called outside BeginScene/EndScene");
            return false;
        }

        // Get bindless texture index
        outTextureIndex = s_Impl.GetWhiteTextureIndex();
        if (texture != nullptr)
        {
            BindlessTextureIndex bindlessIdx = texture->GetBindlessIndex();
            if (bindlessIdx != InvalidBindlessIndex)
            {
                outTextureIndex = bindlessIdx;
            }
        }

        // Flush if batch is full
        if (s_Impl.QuadIndexCount >= s_Impl.MaxIndices)
        {
            Renderer2D::Flush();
        }

        // Check capacity
        uint32_t currentBatchVertices = static_cast<uint32_t>(s_Impl.QuadVertexBufferPtr - s_Impl.QuadVertexBufferBase.get());
        uint32_t totalVerticesAfterThisQuad = s_Impl.QuadVertexOffset + currentBatchVertices + 4;
        if (totalVerticesAfterThisQuad > s_Impl.MaxVertices)
        {
            if (!s_Impl.NeedsBufferGrowth() && s_Impl.MaxQuads < Renderer2DImpl::AbsoluteMaxQuads)
            {
                s_Impl.RequestBufferGrowth();
                GG_CORE_INFO("Renderer2D: Buffer capacity exceeded - will grow on next frame");
            }
            return false;
        }

        return true;
    }

    static void WriteQuadVertices(const float positions[4][3],
                                   const float* texCoords,
                                   float r, float g, float b, float a,
                                   float tilingFactor,
                                   uint32_t textureIndex)
    {
        for (int i = 0; i < 4; i++)
        {
            s_Impl.QuadVertexBufferPtr->position[0] = positions[i][0];
            s_Impl.QuadVertexBufferPtr->position[1] = positions[i][1];
            s_Impl.QuadVertexBufferPtr->position[2] = positions[i][2];

            if (texCoords)
            {
                s_Impl.QuadVertexBufferPtr->texCoord[0] = texCoords[i * 2 + 0];
                s_Impl.QuadVertexBufferPtr->texCoord[1] = texCoords[i * 2 + 1];
            }
            else
            {
                s_Impl.QuadVertexBufferPtr->texCoord[0] = Renderer2DImpl::QuadTexCoords[i][0];
                s_Impl.QuadVertexBufferPtr->texCoord[1] = Renderer2DImpl::QuadTexCoords[i][1];
            }

            s_Impl.QuadVertexBufferPtr->color[0] = r;
            s_Impl.QuadVertexBufferPtr->color[1] = g;
            s_Impl.QuadVertexBufferPtr->color[2] = b;
            s_Impl.QuadVertexBufferPtr->color[3] = a;

            s_Impl.QuadVertexBufferPtr->tilingFactor = tilingFactor;
            s_Impl.QuadVertexBufferPtr->texIndex = textureIndex;

            s_Impl.QuadVertexBufferPtr++;
        }

        s_Impl.QuadIndexCount += 6;
        s_Impl.Stats.QuadCount++;
    }

    static void DrawQuadInternal(float x, float y, float z, float width, float height,
                                 const Texture* texture, float r, float g, float b, float a,
                                 float rotation = 0.0f, float tilingFactor = 1.0f,
                                 const float* texCoords = nullptr)
    {
        uint32_t textureIndex;
        if (!PrepareForQuad(texture, textureIndex))
            return;

        float cosR = 1.0f, sinR = 0.0f;
        if (rotation != 0.0f)
        {
            cosR = std::cos(rotation);
            sinR = std::sin(rotation);
        }

        float positions[4][3];
        for (int i = 0; i < 4; i++)
        {
            float localX = Renderer2DImpl::QuadPositions[i][0] * width;
            float localY = Renderer2DImpl::QuadPositions[i][1] * height;

            positions[i][0] = x + (localX * cosR - localY * sinR);
            positions[i][1] = y + (localX * sinR + localY * cosR);
            positions[i][2] = z;
        }

        WriteQuadVertices(positions, texCoords, r, g, b, a, tilingFactor, textureIndex);
    }

    static void DrawQuadWithMatrix(const glm::mat4& transform,
                                   const Texture* texture, float r, float g, float b, float a,
                                   float tilingFactor = 1.0f,
                                   const float* texCoords = nullptr)
    {
        uint32_t textureIndex;
        if (!PrepareForQuad(texture, textureIndex))
            return;

        static constexpr glm::vec4 unitQuadPositions[4] = {
            { -0.5f, -0.5f, 0.0f, 1.0f },
            {  0.5f, -0.5f, 0.0f, 1.0f },
            {  0.5f,  0.5f, 0.0f, 1.0f },
            { -0.5f,  0.5f, 0.0f, 1.0f }
        };

        float positions[4][3];
        for (int i = 0; i < 4; i++)
        {
            glm::vec4 worldPos = transform * unitQuadPositions[i];
            positions[i][0] = worldPos.x;
            positions[i][1] = worldPos.y;
            positions[i][2] = worldPos.z;
        }

        WriteQuadVertices(positions, texCoords, r, g, b, a, tilingFactor, textureIndex);
    }

    void Renderer2D::DrawQuad(const QuadSpec& spec)
    {
        const Texture* texture = spec.texture;
        const float* texCoordsPtr = spec.texCoords ? &spec.texCoords[0][0] : nullptr;
        float tilingFactor = spec.tilingFactor;

        if (spec.subTexture)
        {
            texture = spec.subTexture->GetTexture();
            texCoordsPtr = spec.subTexture->GetTexCoords();
            tilingFactor = 1.0f;
        }

        if (spec.transform)
        {
            DrawQuadWithMatrix(
                *spec.transform,
                texture,
                spec.color[0], spec.color[1], spec.color[2], spec.color[3],
                tilingFactor,
                texCoordsPtr
            );
        }
        else
        {
            DrawQuadInternal(
                spec.x, spec.y, spec.z,
                spec.width, spec.height,
                texture,
                spec.color[0], spec.color[1], spec.color[2], spec.color[3],
                spec.rotation,
                tilingFactor,
                texCoordsPtr
            );
        }
    }

    void Renderer2D::ResetStats()
    {
        s_Impl.Stats.DrawCalls = 0;
        s_Impl.Stats.QuadCount = 0;
    }

    Renderer2D::Statistics Renderer2D::GetStats()
    {
        Renderer2D::Statistics stats = s_Impl.Stats;
        stats.MaxQuadCapacity = s_Impl.MaxQuads;
        return stats;
    }

}
