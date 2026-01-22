#include "ggpch.h"
#include "Renderer2D.h"
#include "VertexBuffer.h"
#include "IndexBuffer.h"
#include "VertexLayout.h"
#include "UniformBuffer.h"
#include "DescriptorSet.h"
#include "Pipeline.h"
#include "RenderCommand.h"
#include "GGEngine/Asset/Shader.h"
#include "GGEngine/Asset/ShaderLibrary.h"
#include "GGEngine/Asset/Texture.h"
#include "Platform/Vulkan/VulkanContext.h"

#include <cmath>
#include <array>

namespace GGEngine {

    // Vertex structure for batched quads
    struct QuadVertex
    {
        float position[3];     // World-space position (CPU-transformed)
        float texCoord[2];     // UV coordinates
        float color[4];        // RGBA color
        float tilingFactor;    // Texture tiling multiplier
    };

    // Internal renderer data
    struct Renderer2DData
    {
        static constexpr uint32_t MaxQuads = 10000;
        static constexpr uint32_t MaxVertices = MaxQuads * 4;
        static constexpr uint32_t MaxIndices = MaxQuads * 6;

        // Vertex data
        Scope<VertexBuffer> QuadVertexBuffer;
        Scope<IndexBuffer> QuadIndexBuffer;
        VertexLayout QuadVertexLayout;

        // CPU-side vertex staging buffer
        QuadVertex* QuadVertexBufferBase = nullptr;
        QuadVertex* QuadVertexBufferPtr = nullptr;
        uint32_t QuadIndexCount = 0;

        // GPU buffer offset tracking - persists across flushes within a frame
        // to prevent later batches from overwriting earlier batches' vertex data
        uint32_t QuadVertexOffset = 0;  // Offset in vertices (not bytes)

        // Current texture being batched
        const Texture* CurrentTexture = nullptr;

        // White pixel texture for solid colors
        Scope<Texture> WhiteTexture;

        // Shader and pipeline
        AssetHandle<Shader> QuadShader;
        Scope<Pipeline> QuadPipeline;
        VkRenderPass CurrentRenderPass = VK_NULL_HANDLE;

        // Camera UBO and descriptors (per-frame to avoid updating in-flight descriptors)
        static constexpr uint32_t MaxFramesInFlight = 2;
        static constexpr uint32_t DescriptorSetsPerFrame = 16;  // Pool size for texture switches per frame
        Scope<UniformBuffer> CameraUniformBuffers[MaxFramesInFlight];
        Scope<DescriptorSetLayout> DescriptorLayout;
        // Pool of descriptor sets per frame - each texture switch uses the next one
        Scope<DescriptorSet> DescriptorSets[MaxFramesInFlight][DescriptorSetsPerFrame];
        uint32_t CurrentFrameIndex = 0;
        uint32_t CurrentDescriptorIndex = 0;  // Which descriptor set in the pool we're on

        // Current render state
        VkCommandBuffer CurrentCommandBuffer = VK_NULL_HANDLE;
        uint32_t ViewportWidth = 0;
        uint32_t ViewportHeight = 0;
        bool SceneStarted = false;

        // Statistics
        Renderer2D::Statistics Stats;

        // Unit quad vertex positions (centered at origin)
        static constexpr std::array<float[3], 4> QuadPositions = {{
            { -0.5f, -0.5f, 0.0f },  // Bottom-left
            {  0.5f, -0.5f, 0.0f },  // Bottom-right
            {  0.5f,  0.5f, 0.0f },  // Top-right
            { -0.5f,  0.5f, 0.0f }   // Top-left
        }};

        // Quad texture coordinates
        static constexpr std::array<float[2], 4> QuadTexCoords = {{
            { 0.0f, 0.0f },  // Bottom-left
            { 1.0f, 0.0f },  // Bottom-right
            { 1.0f, 1.0f },  // Top-right
            { 0.0f, 1.0f }   // Top-left
        }};
    };

    static Renderer2DData s_Data;

    void Renderer2D::Init()
    {
        GG_CORE_INFO("Renderer2D: Initializing...");

        // Create vertex layout: position (vec3) + texCoord (vec2) + color (vec4) + tilingFactor (float)
        s_Data.QuadVertexLayout
            .Push("aPosition", VertexAttributeType::Float3)
            .Push("aTexCoord", VertexAttributeType::Float2)
            .Push("aColor", VertexAttributeType::Float4)
            .Push("aTilingFactor", VertexAttributeType::Float);

        // Allocate CPU-side vertex buffer (zero-initialized to avoid any uninitialized memory issues)
        s_Data.QuadVertexBufferBase = new QuadVertex[Renderer2DData::MaxVertices]();
        s_Data.QuadVertexBufferPtr = s_Data.QuadVertexBufferBase;

        // Create GPU vertex buffer (dynamic)
        s_Data.QuadVertexBuffer = CreateScope<VertexBuffer>(
            static_cast<uint64_t>(Renderer2DData::MaxVertices) * sizeof(QuadVertex),
            s_Data.QuadVertexLayout
        );
        // Initialize GPU buffer with zeros to avoid potential first-upload issues
        s_Data.QuadVertexBuffer->SetData(s_Data.QuadVertexBufferBase,
            Renderer2DData::MaxVertices * sizeof(QuadVertex));

        // Generate indices for all quads (0,1,2,2,3,0 pattern) - standard indexing
        std::vector<uint32_t> indices(Renderer2DData::MaxIndices);
        uint32_t offset = 0;
        for (uint32_t i = 0; i < Renderer2DData::MaxIndices; i += 6)
        {
            indices[i + 0] = offset + 0;
            indices[i + 1] = offset + 1;
            indices[i + 2] = offset + 2;
            indices[i + 3] = offset + 2;
            indices[i + 4] = offset + 3;
            indices[i + 5] = offset + 0;
            offset += 4;
        }
        s_Data.QuadIndexBuffer = IndexBuffer::Create(indices);

        // Create 1x1 white texture for solid color rendering
        uint32_t whitePixel = 0xFFFFFFFF;  // RGBA: white with full alpha
        s_Data.WhiteTexture = Texture::Create(1, 1, &whitePixel);

        // Get quad2d shader from library (should be pre-loaded)
        s_Data.QuadShader = ShaderLibrary::Get().Get("quad2d");
        if (!s_Data.QuadShader.IsValid())
        {
            GG_CORE_ERROR("Renderer2D: Failed to get 'quad2d' shader from library!");
            return;
        }

        // Create descriptor set layout (camera UBO + texture sampler)
        std::vector<DescriptorBinding> bindings = {
            { 0, VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER, VK_SHADER_STAGE_VERTEX_BIT, 1 },
            { 1, VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, VK_SHADER_STAGE_FRAGMENT_BIT, 1 }
        };
        s_Data.DescriptorLayout = CreateScope<DescriptorSetLayout>(bindings);

        // Create per-frame camera UBOs and descriptor set pools
        for (uint32_t i = 0; i < Renderer2DData::MaxFramesInFlight; i++)
        {
            s_Data.CameraUniformBuffers[i] = CreateScope<UniformBuffer>(sizeof(CameraUBO));
            // Create a pool of descriptor sets per frame for texture switches
            for (uint32_t j = 0; j < Renderer2DData::DescriptorSetsPerFrame; j++)
            {
                s_Data.DescriptorSets[i][j] = CreateScope<DescriptorSet>(*s_Data.DescriptorLayout);
                s_Data.DescriptorSets[i][j]->SetUniformBuffer(0, *s_Data.CameraUniformBuffers[i]);
                // Set initial texture to white (for solid color rendering)
                s_Data.DescriptorSets[i][j]->SetTexture(1, *s_Data.WhiteTexture);
            }
        }

        GG_CORE_INFO("Renderer2D: Initialized (max {} quads per batch, {} frames in flight, {} descriptor sets per frame)",
                     Renderer2DData::MaxQuads, Renderer2DData::MaxFramesInFlight, Renderer2DData::DescriptorSetsPerFrame);
    }

    void Renderer2D::Shutdown()
    {
        GG_CORE_INFO("Renderer2D: Shutting down...");

        delete[] s_Data.QuadVertexBufferBase;
        s_Data.QuadVertexBufferBase = nullptr;
        s_Data.QuadVertexBufferPtr = nullptr;

        s_Data.QuadPipeline.reset();
        for (uint32_t i = 0; i < Renderer2DData::MaxFramesInFlight; i++)
        {
            for (uint32_t j = 0; j < Renderer2DData::DescriptorSetsPerFrame; j++)
            {
                s_Data.DescriptorSets[i][j].reset();
            }
            s_Data.CameraUniformBuffers[i].reset();
        }
        s_Data.DescriptorLayout.reset();
        s_Data.QuadIndexBuffer.reset();
        s_Data.QuadVertexBuffer.reset();
        s_Data.WhiteTexture.reset();
        s_Data.QuadShader = AssetHandle<Shader>();  // Release shader reference

        GG_CORE_TRACE("Renderer2D: Shutdown complete");
    }

    void Renderer2D::BeginScene(const Camera& camera)
    {
        auto& vkContext = VulkanContext::Get();
        VkExtent2D extent = vkContext.GetSwapchainExtent();
        BeginScene(camera, vkContext.GetRenderPass(), vkContext.GetCurrentCommandBuffer(),
                   extent.width, extent.height);
    }

    void Renderer2D::BeginScene(const Camera& camera, VkRenderPass renderPass, VkCommandBuffer cmd,
                                uint32_t viewportWidth, uint32_t viewportHeight)
    {
        // Get current frame index for per-frame resources
        s_Data.CurrentFrameIndex = VulkanContext::Get().GetCurrentFrameIndex();

        // Update camera UBO for current frame
        CameraUBO cameraUBO = camera.GetUBO();
        s_Data.CameraUniformBuffers[s_Data.CurrentFrameIndex]->SetData(cameraUBO);

        // Create or recreate pipeline if render pass changed
        if (!s_Data.QuadPipeline || s_Data.CurrentRenderPass != renderPass)
        {
            s_Data.QuadPipeline.reset();

            PipelineSpecification spec;
            spec.shader = s_Data.QuadShader.Get();
            spec.renderPass = renderPass;
            spec.vertexLayout = &s_Data.QuadVertexLayout;
            spec.cullMode = VK_CULL_MODE_NONE;
            spec.blendMode = BlendMode::Alpha;
            spec.depthTestEnable = false;
            spec.depthWriteEnable = false;
            spec.descriptorSetLayouts.push_back(s_Data.DescriptorLayout->GetVkLayout());
            spec.debugName = "Renderer2D_Quad";

            s_Data.QuadPipeline = CreateScope<Pipeline>(spec);
            s_Data.CurrentRenderPass = renderPass;
        }

        // Store current render state
        s_Data.CurrentCommandBuffer = cmd;
        s_Data.ViewportWidth = viewportWidth;
        s_Data.ViewportHeight = viewportHeight;

        // Reset batch state
        s_Data.QuadIndexCount = 0;
        s_Data.QuadVertexBufferPtr = s_Data.QuadVertexBufferBase;
        s_Data.QuadVertexOffset = 0;  // Reset GPU buffer offset at start of scene
        s_Data.CurrentTexture = s_Data.WhiteTexture.get();
        s_Data.CurrentDescriptorIndex = 0;
        s_Data.SceneStarted = true;

    }

    void Renderer2D::EndScene()
    {
        Flush();
        s_Data.SceneStarted = false;
        s_Data.CurrentCommandBuffer = VK_NULL_HANDLE;
    }

    void Renderer2D::Flush()
    {
        if (s_Data.QuadIndexCount == 0)
            return;

        // Calculate data size and vertex count for current batch
        uint32_t dataSize = static_cast<uint32_t>(
            reinterpret_cast<uint8_t*>(s_Data.QuadVertexBufferPtr) -
            reinterpret_cast<uint8_t*>(s_Data.QuadVertexBufferBase)
        );
        uint32_t vertexCount = dataSize / sizeof(QuadVertex);

        // Upload vertex data to GPU at current offset (not at 0!)
        // This prevents later batches from overwriting earlier batches' data
        uint64_t gpuOffset = static_cast<uint64_t>(s_Data.QuadVertexOffset) * sizeof(QuadVertex);
        s_Data.QuadVertexBuffer->SetData(s_Data.QuadVertexBufferBase, dataSize, gpuOffset);

        VkCommandBuffer cmd = s_Data.CurrentCommandBuffer;

        // Set viewport and scissor using stored dimensions
        RenderCommand::SetViewport(cmd, s_Data.ViewportWidth, s_Data.ViewportHeight);
        RenderCommand::SetScissor(cmd, s_Data.ViewportWidth, s_Data.ViewportHeight);

        // Bind pipeline
        s_Data.QuadPipeline->Bind(cmd);

        // Get a fresh descriptor set from the pool to avoid updating one that's already bound
        if (s_Data.CurrentDescriptorIndex >= Renderer2DData::DescriptorSetsPerFrame)
        {
            GG_CORE_WARN("Renderer2D: Exceeded descriptor set pool size ({} texture switches per frame). Reusing first set.",
                        Renderer2DData::DescriptorSetsPerFrame);
            s_Data.CurrentDescriptorIndex = 0;
        }

        auto& descriptorSet = s_Data.DescriptorSets[s_Data.CurrentFrameIndex][s_Data.CurrentDescriptorIndex];
        s_Data.CurrentDescriptorIndex++;

        // Update texture in this fresh descriptor set BEFORE binding
        const Texture* texToBind = s_Data.CurrentTexture ? s_Data.CurrentTexture : s_Data.WhiteTexture.get();
        descriptorSet->SetTexture(1, *texToBind);

        // Bind descriptor set (camera UBO + current texture) for current frame
        descriptorSet->Bind(cmd, s_Data.QuadPipeline->GetLayout(), 0);

        // Bind vertex and index buffers
        s_Data.QuadVertexBuffer->Bind(cmd);
        s_Data.QuadIndexBuffer->Bind(cmd);

        // Draw with vertex offset to read from correct GPU buffer location
        // The indices (0,1,2,2,3,0 etc.) are relative, so we need vertexOffset
        // to tell the GPU where this batch's vertices actually start
        RenderCommand::DrawIndexed(cmd, s_Data.QuadIndexCount, 1, 0,
                                   static_cast<int32_t>(s_Data.QuadVertexOffset), 0);

        // Update GPU buffer offset for next batch (persists across flushes)
        s_Data.QuadVertexOffset += vertexCount;

        // Update stats
        s_Data.Stats.DrawCalls++;

        // Reset batch state for next batch (but NOT QuadVertexOffset - that persists!)
        s_Data.QuadIndexCount = 0;
        s_Data.QuadVertexBufferPtr = s_Data.QuadVertexBufferBase;
    }

    // Internal helper to add a quad to the batch
    static void DrawQuadInternal(float x, float y, float z, float width, float height,
                                 const Texture* texture, float r, float g, float b, float a,
                                 float rotation = 0.0f, float tilingFactor = 1.0f)
    {
        if (!s_Data.SceneStarted)
        {
            GG_CORE_WARN("Renderer2D::DrawQuad called outside BeginScene/EndScene");
            return;
        }

        // Use white texture for solid colors when no texture provided
        const Texture* tex = texture ? texture : s_Data.WhiteTexture.get();

        // Flush if texture changed (descriptor set update happens in Flush before binding)
        if (s_Data.CurrentTexture != tex)
        {
            Renderer2D::Flush();
            s_Data.CurrentTexture = tex;
        }

        // Flush if batch is full
        if (s_Data.QuadIndexCount >= Renderer2DData::MaxIndices)
        {
            Renderer2D::Flush();
        }

        // Check if adding this quad would exceed total GPU buffer capacity for the frame
        // (This accounts for all batches already flushed + current batch + this new quad)
        uint32_t currentBatchVertices = static_cast<uint32_t>(s_Data.QuadVertexBufferPtr - s_Data.QuadVertexBufferBase);
        uint32_t totalVerticesAfterThisQuad = s_Data.QuadVertexOffset + currentBatchVertices + 4;
        if (totalVerticesAfterThisQuad > Renderer2DData::MaxVertices)
        {
            GG_CORE_WARN("Renderer2D: Frame vertex limit exceeded ({} > {}). Quad skipped.",
                        totalVerticesAfterThisQuad, Renderer2DData::MaxVertices);
            return;
        }

        // Compute rotation if needed
        float cosR = 1.0f, sinR = 0.0f;
        if (rotation != 0.0f)
        {
            cosR = std::cos(rotation);
            sinR = std::sin(rotation);
        }

        // Write 4 vertices
        for (int i = 0; i < 4; i++)
        {
            // Scale unit quad by size
            float localX = Renderer2DData::QuadPositions[i][0] * width;
            float localY = Renderer2DData::QuadPositions[i][1] * height;

            // Apply rotation (around center)
            float rotatedX = localX * cosR - localY * sinR;
            float rotatedY = localX * sinR + localY * cosR;

            // Translate to world position
            s_Data.QuadVertexBufferPtr->position[0] = x + rotatedX;
            s_Data.QuadVertexBufferPtr->position[1] = y + rotatedY;
            s_Data.QuadVertexBufferPtr->position[2] = z;

            // Texture coordinates
            s_Data.QuadVertexBufferPtr->texCoord[0] = Renderer2DData::QuadTexCoords[i][0];
            s_Data.QuadVertexBufferPtr->texCoord[1] = Renderer2DData::QuadTexCoords[i][1];

            // Color
            s_Data.QuadVertexBufferPtr->color[0] = r;
            s_Data.QuadVertexBufferPtr->color[1] = g;
            s_Data.QuadVertexBufferPtr->color[2] = b;
            s_Data.QuadVertexBufferPtr->color[3] = a;

            // Tiling factor
            s_Data.QuadVertexBufferPtr->tilingFactor = tilingFactor;

            s_Data.QuadVertexBufferPtr++;
        }

        s_Data.QuadIndexCount += 6;
        s_Data.Stats.QuadCount++;
    }

    // Colored quads
    void Renderer2D::DrawQuad(float x, float y, float width, float height,
                             float r, float g, float b, float a)
    {
        DrawQuadInternal(x, y, 0.0f, width, height, nullptr, r, g, b, a);
    }

    void Renderer2D::DrawQuad(float x, float y, float z, float width, float height,
                             float r, float g, float b, float a)
    {
        DrawQuadInternal(x, y, z, width, height, nullptr, r, g, b, a);
    }

    // Rotated colored quads
    void Renderer2D::DrawRotatedQuad(float x, float y, float width, float height,
                                    float rotationRadians,
                                    float r, float g, float b, float a)
    {
        DrawQuadInternal(x, y, 0.0f, width, height, nullptr, r, g, b, a, rotationRadians);
    }

    void Renderer2D::DrawRotatedQuad(float x, float y, float z, float width, float height,
                                    float rotationRadians,
                                    float r, float g, float b, float a)
    {
        DrawQuadInternal(x, y, z, width, height, nullptr, r, g, b, a, rotationRadians);
    }

    // Textured quads
    void Renderer2D::DrawQuad(float x, float y, float width, float height,
                             const Texture* texture,
                             float tilingFactor,
                             float tintR, float tintG, float tintB, float tintA)
    {
        DrawQuadInternal(x, y, 0.0f, width, height, texture, tintR, tintG, tintB, tintA, 0.0f, tilingFactor);
    }

    void Renderer2D::DrawQuad(float x, float y, float z, float width, float height,
                             const Texture* texture,
                             float tilingFactor,
                             float tintR, float tintG, float tintB, float tintA)
    {
        DrawQuadInternal(x, y, z, width, height, texture, tintR, tintG, tintB, tintA, 0.0f, tilingFactor);
    }

    // Rotated textured quads
    void Renderer2D::DrawRotatedQuad(float x, float y, float width, float height,
                                    float rotationRadians,
                                    const Texture* texture,
                                    float tilingFactor,
                                    float tintR, float tintG, float tintB, float tintA)
    {
        DrawQuadInternal(x, y, 0.0f, width, height, texture, tintR, tintG, tintB, tintA, rotationRadians, tilingFactor);
    }

    void Renderer2D::DrawRotatedQuad(float x, float y, float z, float width, float height,
                                    float rotationRadians,
                                    const Texture* texture,
                                    float tilingFactor,
                                    float tintR, float tintG, float tintB, float tintA)
    {
        DrawQuadInternal(x, y, z, width, height, texture, tintR, tintG, tintB, tintA, rotationRadians, tilingFactor);
    }

    // Statistics
    void Renderer2D::ResetStats()
    {
        s_Data.Stats.DrawCalls = 0;
        s_Data.Stats.QuadCount = 0;
    }

    Renderer2D::Statistics Renderer2D::GetStats()
    {
        return s_Data.Stats;
    }

}
