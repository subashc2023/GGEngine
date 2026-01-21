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
        float position[3];   // World-space position (CPU-transformed)
        float texCoord[2];   // UV coordinates
        float color[4];      // RGBA color
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

        // Current texture being batched
        const Texture* CurrentTexture = nullptr;

        // White pixel texture for solid colors
        Scope<Texture> WhiteTexture;

        // Shader and pipeline
        AssetHandle<Shader> QuadShader;
        Scope<Pipeline> QuadPipeline;
        VkRenderPass CurrentRenderPass = VK_NULL_HANDLE;

        // Camera UBO and descriptors
        Scope<UniformBuffer> CameraUniformBuffer;
        Scope<DescriptorSetLayout> DescriptorLayout;
        Scope<DescriptorSet> DescriptorSet;

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

    // Helper to create 1x1 white texture
    static void CreateWhiteTexture()
    {
        // Create a 1x1 white pixel texture
        uint8_t whitePixel[4] = { 255, 255, 255, 255 };

        auto& ctx = VulkanContext::Get();
        VkDevice device = ctx.GetDevice();
        VmaAllocator allocator = ctx.GetAllocator();

        s_Data.WhiteTexture = CreateScope<Texture>();

        // We need to manually create the texture resources similar to Texture::InitFallback()
        // but for a 1x1 white pixel

        VkDeviceSize imageSize = 4; // 1x1 RGBA

        // Create staging buffer
        BufferSpecification stagingSpec;
        stagingSpec.size = imageSize;
        stagingSpec.usage = BufferUsage::Staging;
        stagingSpec.cpuVisible = true;
        stagingSpec.debugName = "WhiteTextureStagingBuffer";

        Buffer stagingBuffer(stagingSpec);
        stagingBuffer.SetData(whitePixel, imageSize);

        // Create image
        VkImageCreateInfo imageInfo{};
        imageInfo.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
        imageInfo.imageType = VK_IMAGE_TYPE_2D;
        imageInfo.extent.width = 1;
        imageInfo.extent.height = 1;
        imageInfo.extent.depth = 1;
        imageInfo.mipLevels = 1;
        imageInfo.arrayLayers = 1;
        imageInfo.format = VK_FORMAT_R8G8B8A8_UNORM;
        imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
        imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        imageInfo.usage = VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT;
        imageInfo.samples = VK_SAMPLE_COUNT_1_BIT;
        imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;

        VmaAllocationCreateInfo allocCreateInfo{};
        allocCreateInfo.usage = VMA_MEMORY_USAGE_AUTO;
        allocCreateInfo.flags = VMA_ALLOCATION_CREATE_DEDICATED_MEMORY_BIT;

        VkImage image;
        VmaAllocation imageAllocation;
        if (vmaCreateImage(allocator, &imageInfo, &allocCreateInfo, &image, &imageAllocation, nullptr) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Renderer2D: Failed to create white texture image!");
            return;
        }

        VkBuffer stagingVkBuffer = stagingBuffer.GetVkBuffer();

        ctx.ImmediateSubmit([&](VkCommandBuffer cmd) {
            // Transition to TRANSFER_DST
            VkImageMemoryBarrier barrier{};
            barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
            barrier.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
            barrier.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.image = image;
            barrier.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
            barrier.subresourceRange.baseMipLevel = 0;
            barrier.subresourceRange.levelCount = 1;
            barrier.subresourceRange.baseArrayLayer = 0;
            barrier.subresourceRange.layerCount = 1;
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;

            vkCmdPipelineBarrier(cmd,
                VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
                VK_PIPELINE_STAGE_TRANSFER_BIT,
                0, 0, nullptr, 0, nullptr, 1, &barrier);

            // Copy buffer to image
            VkBufferImageCopy region{};
            region.bufferOffset = 0;
            region.bufferRowLength = 0;
            region.bufferImageHeight = 0;
            region.imageSubresource.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
            region.imageSubresource.mipLevel = 0;
            region.imageSubresource.baseArrayLayer = 0;
            region.imageSubresource.layerCount = 1;
            region.imageOffset = { 0, 0, 0 };
            region.imageExtent = { 1, 1, 1 };

            vkCmdCopyBufferToImage(cmd, stagingVkBuffer, image,
                VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);

            // Transition to SHADER_READ_ONLY
            barrier.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
            barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;

            vkCmdPipelineBarrier(cmd,
                VK_PIPELINE_STAGE_TRANSFER_BIT,
                VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                0, 0, nullptr, 0, nullptr, 1, &barrier);
        });

        // Create image view
        VkImageView imageView;
        VkImageViewCreateInfo viewInfo{};
        viewInfo.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO;
        viewInfo.image = image;
        viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D;
        viewInfo.format = VK_FORMAT_R8G8B8A8_UNORM;
        viewInfo.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
        viewInfo.subresourceRange.baseMipLevel = 0;
        viewInfo.subresourceRange.levelCount = 1;
        viewInfo.subresourceRange.baseArrayLayer = 0;
        viewInfo.subresourceRange.layerCount = 1;

        if (vkCreateImageView(device, &viewInfo, nullptr, &imageView) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Renderer2D: Failed to create white texture image view!");
            vmaDestroyImage(allocator, image, imageAllocation);
            return;
        }

        // Create sampler
        VkSampler sampler;
        VkSamplerCreateInfo samplerInfo{};
        samplerInfo.sType = VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO;
        samplerInfo.magFilter = VK_FILTER_NEAREST;
        samplerInfo.minFilter = VK_FILTER_NEAREST;
        samplerInfo.addressModeU = VK_SAMPLER_ADDRESS_MODE_REPEAT;
        samplerInfo.addressModeV = VK_SAMPLER_ADDRESS_MODE_REPEAT;
        samplerInfo.addressModeW = VK_SAMPLER_ADDRESS_MODE_REPEAT;
        samplerInfo.anisotropyEnable = VK_FALSE;
        samplerInfo.maxAnisotropy = 1.0f;
        samplerInfo.borderColor = VK_BORDER_COLOR_INT_OPAQUE_WHITE;
        samplerInfo.unnormalizedCoordinates = VK_FALSE;
        samplerInfo.compareEnable = VK_FALSE;
        samplerInfo.compareOp = VK_COMPARE_OP_ALWAYS;
        samplerInfo.mipmapMode = VK_SAMPLER_MIPMAP_MODE_NEAREST;

        if (vkCreateSampler(device, &samplerInfo, nullptr, &sampler) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Renderer2D: Failed to create white texture sampler!");
            vkDestroyImageView(device, imageView, nullptr);
            vmaDestroyImage(allocator, image, imageAllocation);
            return;
        }

        // Store in a temporary holder - we'll use the fallback texture pattern
        // Since Texture doesn't expose setters, we'll just use the fallback texture for white
        // and create our own minimal texture tracking

        // Clean up - we actually can't easily assign to Texture internals
        // Let's just destroy these and use the fallback texture instead
        vkDestroySampler(device, sampler, nullptr);
        vkDestroyImageView(device, imageView, nullptr);
        vmaDestroyImage(allocator, image, imageAllocation);

        s_Data.WhiteTexture.reset();

        GG_CORE_TRACE("Renderer2D: Using fallback texture as white texture placeholder");
    }

    void Renderer2D::Init()
    {
        GG_CORE_INFO("Renderer2D: Initializing...");

        // Create vertex layout: position (vec3) + texCoord (vec2) + color (vec4)
        s_Data.QuadVertexLayout
            .Push("aPosition", VertexAttributeType::Float3)
            .Push("aTexCoord", VertexAttributeType::Float2)
            .Push("aColor", VertexAttributeType::Float4);

        // Allocate CPU-side vertex buffer
        s_Data.QuadVertexBufferBase = new QuadVertex[Renderer2DData::MaxVertices];
        s_Data.QuadVertexBufferPtr = s_Data.QuadVertexBufferBase;

        // Create GPU vertex buffer (dynamic, no initial data)
        s_Data.QuadVertexBuffer = CreateScope<VertexBuffer>(
            static_cast<uint64_t>(Renderer2DData::MaxVertices) * sizeof(QuadVertex),
            s_Data.QuadVertexLayout
        );

        // Generate indices for all quads (0,1,2,2,3,0 pattern)
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

        // Get quad2d shader from library (should be pre-loaded)
        s_Data.QuadShader = ShaderLibrary::Get().Get("quad2d");
        if (!s_Data.QuadShader.IsValid())
        {
            GG_CORE_ERROR("Renderer2D: Failed to get 'quad2d' shader from library!");
            return;
        }

        // Create camera UBO
        s_Data.CameraUniformBuffer = CreateScope<UniformBuffer>(sizeof(CameraUBO));

        // Create descriptor set layout (camera UBO + texture sampler)
        std::vector<DescriptorBinding> bindings = {
            { 0, VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER, VK_SHADER_STAGE_VERTEX_BIT, 1 },
            { 1, VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, VK_SHADER_STAGE_FRAGMENT_BIT, 1 }
        };
        s_Data.DescriptorLayout = CreateScope<DescriptorSetLayout>(bindings);

        // Create descriptor set
        s_Data.DescriptorSet = CreateScope<DescriptorSet>(*s_Data.DescriptorLayout);
        s_Data.DescriptorSet->SetUniformBuffer(0, *s_Data.CameraUniformBuffer);

        // Set initial texture to fallback (acts as white texture)
        s_Data.DescriptorSet->SetTexture(1, *Texture::GetFallbackPtr());

        GG_CORE_INFO("Renderer2D: Initialized (max {} quads per batch)", Renderer2DData::MaxQuads);
    }

    void Renderer2D::Shutdown()
    {
        GG_CORE_INFO("Renderer2D: Shutting down...");

        delete[] s_Data.QuadVertexBufferBase;
        s_Data.QuadVertexBufferBase = nullptr;
        s_Data.QuadVertexBufferPtr = nullptr;

        s_Data.QuadPipeline.reset();
        s_Data.DescriptorSet.reset();
        s_Data.DescriptorLayout.reset();
        s_Data.CameraUniformBuffer.reset();
        s_Data.QuadIndexBuffer.reset();
        s_Data.QuadVertexBuffer.reset();
        s_Data.WhiteTexture.reset();

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
        // Update camera UBO
        CameraUBO cameraUBO = camera.GetUBO();
        s_Data.CameraUniformBuffer->SetData(cameraUBO);

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
        s_Data.CurrentTexture = nullptr;
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

        // Calculate data size
        uint32_t dataSize = static_cast<uint32_t>(
            reinterpret_cast<uint8_t*>(s_Data.QuadVertexBufferPtr) -
            reinterpret_cast<uint8_t*>(s_Data.QuadVertexBufferBase)
        );

        // Upload vertex data to GPU
        s_Data.QuadVertexBuffer->SetData(s_Data.QuadVertexBufferBase, dataSize);

        VkCommandBuffer cmd = s_Data.CurrentCommandBuffer;

        // Set viewport and scissor using stored dimensions
        RenderCommand::SetViewport(cmd, s_Data.ViewportWidth, s_Data.ViewportHeight);
        RenderCommand::SetScissor(cmd, s_Data.ViewportWidth, s_Data.ViewportHeight);

        // Bind pipeline
        s_Data.QuadPipeline->Bind(cmd);

        // Bind descriptor set (camera UBO + current texture)
        s_Data.DescriptorSet->Bind(cmd, s_Data.QuadPipeline->GetLayout(), 0);

        // Bind vertex and index buffers
        s_Data.QuadVertexBuffer->Bind(cmd);
        s_Data.QuadIndexBuffer->Bind(cmd);

        // Draw
        RenderCommand::DrawIndexed(cmd, s_Data.QuadIndexCount);

        // Update stats
        s_Data.Stats.DrawCalls++;

        // Reset batch
        s_Data.QuadIndexCount = 0;
        s_Data.QuadVertexBufferPtr = s_Data.QuadVertexBufferBase;
    }

    // Internal helper to add a quad to the batch
    static void DrawQuadInternal(float x, float y, float z, float width, float height,
                                 const Texture* texture, float r, float g, float b, float a,
                                 float rotation = 0.0f)
    {
        if (!s_Data.SceneStarted)
        {
            GG_CORE_WARN("Renderer2D::DrawQuad called outside BeginScene/EndScene");
            return;
        }

        // Use fallback texture if null (acts as white)
        const Texture* tex = texture ? texture : Texture::GetFallbackPtr();

        // Flush if texture changed
        if (s_Data.CurrentTexture != tex)
        {
            Renderer2D::Flush();
            s_Data.CurrentTexture = tex;
            s_Data.DescriptorSet->SetTexture(1, *tex);
        }

        // Flush if batch is full
        if (s_Data.QuadIndexCount >= Renderer2DData::MaxIndices)
        {
            Renderer2D::Flush();
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
                             float tintR, float tintG, float tintB, float tintA)
    {
        DrawQuadInternal(x, y, 0.0f, width, height, texture, tintR, tintG, tintB, tintA);
    }

    void Renderer2D::DrawQuad(float x, float y, float z, float width, float height,
                             const Texture* texture,
                             float tintR, float tintG, float tintB, float tintA)
    {
        DrawQuadInternal(x, y, z, width, height, texture, tintR, tintG, tintB, tintA);
    }

    // Rotated textured quads
    void Renderer2D::DrawRotatedQuad(float x, float y, float width, float height,
                                    float rotationRadians,
                                    const Texture* texture,
                                    float tintR, float tintG, float tintB, float tintA)
    {
        DrawQuadInternal(x, y, 0.0f, width, height, texture, tintR, tintG, tintB, tintA, rotationRadians);
    }

    void Renderer2D::DrawRotatedQuad(float x, float y, float z, float width, float height,
                                    float rotationRadians,
                                    const Texture* texture,
                                    float tintR, float tintG, float tintB, float tintA)
    {
        DrawQuadInternal(x, y, z, width, height, texture, tintR, tintG, tintB, tintA, rotationRadians);
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
