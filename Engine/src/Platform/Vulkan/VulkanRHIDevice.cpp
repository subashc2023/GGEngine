#include "ggpch.h"
#include "VulkanResourceRegistry.h"
#include "VulkanConversions.h"
#include "VulkanContext.h"
#include "VulkanUtils.h"
#include "GGEngine/RHI/RHIDevice.h"

namespace GGEngine {

    // ============================================================================
    // RHIDevice Implementation - Core
    // ============================================================================

    RHIDevice& RHIDevice::Get()
    {
        static RHIDevice instance;
        return instance;
    }

    void RHIDevice::Init(void* windowHandle)
    {
        if (m_Initialized) return;

        GG_CORE_INFO("RHIDevice: Initializing...");

        auto& vkContext = VulkanContext::Get();
        vkContext.Init(static_cast<GLFWwindow*>(windowHandle));

        // Register swapchain render pass
        VkRenderPass swapchainRP = vkContext.GetRenderPass();
        m_SwapchainRenderPassHandle = VulkanResourceRegistry::Get().RegisterRenderPass(swapchainRP);

        m_Initialized = true;
        GG_CORE_INFO("RHIDevice: Initialized");
    }

    void RHIDevice::Shutdown()
    {
        GG_CORE_INFO("RHIDevice: Shutting down...");

        WaitIdle();
        VulkanResourceRegistry::Get().Clear();
        VulkanContext::Get().Shutdown();

        m_Initialized = false;
        GG_CORE_TRACE("RHIDevice: Shutdown complete");
    }

    void RHIDevice::BeginFrame()
    {
        auto& vkContext = VulkanContext::Get();
        vkContext.BeginFrame();

        uint32_t frameIndex = vkContext.GetCurrentFrameIndex();
        VkCommandBuffer cmd = vkContext.GetCurrentCommandBuffer();
        VulkanResourceRegistry::Get().SetCurrentCommandBuffer(frameIndex, cmd);
    }

    void RHIDevice::EndFrame()
    {
        VulkanContext::Get().EndFrame();
    }

    void RHIDevice::BeginSwapchainRenderPass()
    {
        VulkanContext::Get().BeginSwapchainRenderPass();
    }

    RHICommandBufferHandle RHIDevice::GetCurrentCommandBuffer() const
    {
        uint32_t frameIndex = VulkanContext::Get().GetCurrentFrameIndex();
        return VulkanResourceRegistry::Get().GetCurrentCommandBufferHandle(frameIndex);
    }

    RHIRenderPassHandle RHIDevice::GetSwapchainRenderPass() const
    {
        return m_SwapchainRenderPassHandle;
    }

    uint32_t RHIDevice::GetSwapchainWidth() const
    {
        return VulkanContext::Get().GetSwapchainExtent().width;
    }

    uint32_t RHIDevice::GetSwapchainHeight() const
    {
        return VulkanContext::Get().GetSwapchainExtent().height;
    }

    uint32_t RHIDevice::GetCurrentFrameIndex() const
    {
        return VulkanContext::Get().GetCurrentFrameIndex();
    }

    void RHIDevice::ImmediateSubmit(const std::function<void(RHICommandBufferHandle)>& func)
    {
        VulkanContext::Get().ImmediateSubmit([&](VkCommandBuffer cmd) {
            VulkanResourceRegistry::Get().SetImmediateCommandBuffer(cmd);
            auto handle = VulkanResourceRegistry::Get().GetImmediateCommandBufferHandle();
            func(handle);
        });
    }

    void RHIDevice::WaitIdle()
    {
        vkDeviceWaitIdle(VulkanContext::Get().GetDevice());
    }

    void RHIDevice::OnWindowResize(uint32_t width, uint32_t height)
    {
        VulkanContext::Get().OnWindowResize(width, height);
    }

    void RHIDevice::SetVSync(bool enabled)
    {
        VulkanContext::Get().SetVSync(enabled);
    }

    bool RHIDevice::IsVSync() const
    {
        return VulkanContext::Get().IsVSync();
    }

    // ============================================================================
    // Descriptor Set Layout Management
    // ============================================================================

    RHIDescriptorSetLayoutHandle RHIDevice::CreateDescriptorSetLayout(const std::vector<RHIDescriptorBinding>& bindings)
    {
        auto device = VulkanContext::Get().GetDevice();

        std::vector<VkDescriptorSetLayoutBinding> vkBindings;
        vkBindings.reserve(bindings.size());

        for (const auto& binding : bindings)
        {
            VkDescriptorSetLayoutBinding vkBinding{};
            vkBinding.binding = binding.binding;
            vkBinding.descriptorType = ToVulkan(binding.type);
            vkBinding.descriptorCount = binding.count;
            vkBinding.stageFlags = ToVulkan(binding.stages);
            vkBinding.pImmutableSamplers = nullptr;
            vkBindings.push_back(vkBinding);
        }

        VkDescriptorSetLayoutCreateInfo layoutInfo{};
        layoutInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO;
        layoutInfo.bindingCount = static_cast<uint32_t>(vkBindings.size());
        layoutInfo.pBindings = vkBindings.data();

        VkDescriptorSetLayout layout = VK_NULL_HANDLE;
        if (vkCreateDescriptorSetLayout(device, &layoutInfo, nullptr, &layout) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateDescriptorSetLayout: vkCreateDescriptorSetLayout failed");
            return NullDescriptorSetLayout;
        }

        return VulkanResourceRegistry::Get().RegisterDescriptorSetLayout(layout);
    }

    void RHIDevice::DestroyDescriptorSetLayout(RHIDescriptorSetLayoutHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSetLayout layout = registry.GetDescriptorSetLayout(handle);
        auto device = VulkanContext::Get().GetDevice();

        if (layout != VK_NULL_HANDLE)
            vkDestroyDescriptorSetLayout(device, layout, nullptr);

        registry.UnregisterDescriptorSetLayout(handle);
    }

    // ============================================================================
    // Descriptor Set Management
    // ============================================================================

    RHIDescriptorSetHandle RHIDevice::AllocateDescriptorSet(RHIDescriptorSetLayoutHandle layoutHandle)
    {
        if (!layoutHandle.IsValid()) return NullDescriptorSet;

        auto& vkContext = VulkanContext::Get();
        auto device = vkContext.GetDevice();
        auto pool = vkContext.GetDescriptorPool();
        auto& registry = VulkanResourceRegistry::Get();

        VkDescriptorSetLayout layout = registry.GetDescriptorSetLayout(layoutHandle);

        VkDescriptorSetAllocateInfo allocInfo{};
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO;
        allocInfo.descriptorPool = pool;
        allocInfo.descriptorSetCount = 1;
        allocInfo.pSetLayouts = &layout;

        VkDescriptorSet descriptorSet = VK_NULL_HANDLE;
        if (vkAllocateDescriptorSets(device, &allocInfo, &descriptorSet) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::AllocateDescriptorSet: vkAllocateDescriptorSets failed");
            return NullDescriptorSet;
        }

        return registry.RegisterDescriptorSet(descriptorSet, layoutHandle);
    }

    void RHIDevice::FreeDescriptorSet(RHIDescriptorSetHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& vkContext = VulkanContext::Get();
        auto device = vkContext.GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        auto data = registry.GetDescriptorSetData(handle);

        // If the descriptor set has an owning pool, destroy the pool
        if (data.owningPool != VK_NULL_HANDLE)
        {
            vkDestroyDescriptorPool(device, data.owningPool, nullptr);
        }
        else if (data.descriptorSet != VK_NULL_HANDLE)
        {
            // Otherwise just free from the shared pool
            auto pool = vkContext.GetDescriptorPool();
            vkFreeDescriptorSets(device, pool, 1, &data.descriptorSet);
        }

        registry.UnregisterDescriptorSet(handle);
    }

    void RHIDevice::UpdateDescriptorSet(RHIDescriptorSetHandle handle, const std::vector<RHIDescriptorWrite>& writes)
    {
        if (!handle.IsValid() || writes.empty()) return;

        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();
        VkDescriptorSet vkSet = registry.GetDescriptorSet(handle);

        std::vector<VkWriteDescriptorSet> vkWrites;
        std::vector<VkDescriptorBufferInfo> bufferInfos;
        std::vector<VkDescriptorImageInfo> imageInfos;

        // Pre-allocate to avoid reallocation invalidating pointers
        bufferInfos.reserve(writes.size());
        imageInfos.reserve(writes.size());

        for (const auto& write : writes)
        {
            VkWriteDescriptorSet vkWrite{};
            vkWrite.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET;
            vkWrite.dstSet = vkSet;
            vkWrite.dstBinding = write.binding;
            vkWrite.dstArrayElement = write.arrayElement;
            vkWrite.descriptorType = ToVulkan(write.type);
            vkWrite.descriptorCount = 1;

            // Handle buffer resources
            if (std::holds_alternative<RHIDescriptorBufferInfo>(write.resource))
            {
                const auto& bufInfo = std::get<RHIDescriptorBufferInfo>(write.resource);
                auto bufferData = registry.GetBufferData(bufInfo.buffer);
                VkDescriptorBufferInfo vkBufInfo{};
                vkBufInfo.buffer = bufferData.buffer;
                vkBufInfo.offset = bufInfo.offset;
                vkBufInfo.range = bufInfo.range == 0 ? bufferData.size : bufInfo.range;
                bufferInfos.push_back(vkBufInfo);
                vkWrite.pBufferInfo = &bufferInfos.back();
            }
            // Handle image resources
            else if (std::holds_alternative<RHIDescriptorImageInfo>(write.resource))
            {
                const auto& imgInfo = std::get<RHIDescriptorImageInfo>(write.resource);
                auto texData = registry.GetTextureData(imgInfo.texture);
                VkDescriptorImageInfo vkImgInfo{};
                vkImgInfo.sampler = imgInfo.sampler.IsValid()
                    ? reinterpret_cast<VkSampler>(imgInfo.sampler.id)
                    : texData.sampler;
                vkImgInfo.imageView = texData.imageView;
                vkImgInfo.imageLayout = ToVulkan(imgInfo.layout);
                imageInfos.push_back(vkImgInfo);
                vkWrite.pImageInfo = &imageInfos.back();
            }

            vkWrites.push_back(vkWrite);
        }

        vkUpdateDescriptorSets(device, static_cast<uint32_t>(vkWrites.size()), vkWrites.data(), 0, nullptr);
    }

    // ============================================================================
    // Buffer Management
    // ============================================================================

    Result<RHIBufferHandle> RHIDevice::TryCreateBuffer(const RHIBufferSpecification& spec)
    {
        auto& vkContext = VulkanContext::Get();
        auto allocator = vkContext.GetAllocator();

        VkBufferCreateInfo bufferInfo{};
        bufferInfo.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
        bufferInfo.size = spec.size;
        bufferInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        bufferInfo.usage = ToVulkanBufferUsage(spec.usage, spec.cpuVisible);

        VmaAllocationCreateInfo allocInfo{};
        allocInfo.usage = VMA_MEMORY_USAGE_AUTO;
        if (spec.cpuVisible)
        {
            allocInfo.flags = VMA_ALLOCATION_CREATE_HOST_ACCESS_SEQUENTIAL_WRITE_BIT |
                              VMA_ALLOCATION_CREATE_MAPPED_BIT;
        }

        VkBuffer buffer = VK_NULL_HANDLE;
        VmaAllocation allocation = VK_NULL_HANDLE;

        VkResult vkResult = vmaCreateBuffer(allocator, &bufferInfo, &allocInfo, &buffer, &allocation, nullptr);
        if (vkResult != VK_SUCCESS)
        {
            return Result<RHIBufferHandle>::Err(
                std::string("vmaCreateBuffer failed: ") + std::string(VkResultToString(vkResult)));
        }

        return Result<RHIBufferHandle>::Ok(
            VulkanResourceRegistry::Get().RegisterBuffer(buffer, allocation, spec.size, spec.cpuVisible));
    }

    RHIBufferHandle RHIDevice::CreateBuffer(const RHIBufferSpecification& spec)
    {
        auto result = TryCreateBuffer(spec);
        if (result.IsErr())
        {
            GG_CORE_ERROR("RHIDevice::CreateBuffer: {}", result.Error());
            return NullBuffer;
        }
        return result.Value();
    }

    void RHIDevice::DestroyBuffer(RHIBufferHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);
        auto allocator = VulkanContext::Get().GetAllocator();

        if (bufferData.buffer != VK_NULL_HANDLE)
            vmaDestroyBuffer(allocator, bufferData.buffer, bufferData.allocation);

        registry.UnregisterBuffer(handle);
    }

    void* RHIDevice::MapBuffer(RHIBufferHandle handle)
    {
        if (!handle.IsValid()) return nullptr;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);

        if (!bufferData.cpuVisible)
        {
            GG_CORE_ERROR("RHIDevice::MapBuffer: Buffer is not CPU-visible");
            return nullptr;
        }

        void* data = nullptr;
        VmaAllocator allocator = VulkanContext::Get().GetAllocator();
        if (vmaMapMemory(allocator, bufferData.allocation, &data) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::MapBuffer: vmaMapMemory failed");
            return nullptr;
        }
        return data;
    }

    void RHIDevice::UnmapBuffer(RHIBufferHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);

        VmaAllocator allocator = VulkanContext::Get().GetAllocator();
        vmaUnmapMemory(allocator, bufferData.allocation);
    }

    void RHIDevice::FlushBuffer(RHIBufferHandle handle, uint64_t offset, uint64_t size)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);

        VmaAllocator allocator = VulkanContext::Get().GetAllocator();
        vmaFlushAllocation(allocator, bufferData.allocation, offset, size == 0 ? bufferData.size : size);
    }

    void RHIDevice::UploadBufferData(RHIBufferHandle handle, const void* data, uint64_t size, uint64_t offset)
    {
        if (!handle.IsValid() || !data || size == 0) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto bufferData = registry.GetBufferData(handle);

        if (bufferData.cpuVisible)
        {
            void* mapped = MapBuffer(handle);
            if (mapped)
            {
                std::memcpy(static_cast<char*>(mapped) + offset, data, size);
                FlushBuffer(handle, offset, size);
                UnmapBuffer(handle);
            }
        }
        else
        {
            // Create staging buffer and copy
            RHIBufferSpecification stagingSpec;
            stagingSpec.size = size;
            stagingSpec.usage = BufferUsage::Staging;
            stagingSpec.cpuVisible = true;

            RHIBufferHandle staging = CreateBuffer(stagingSpec);
            UploadBufferData(staging, data, size, 0);

            // Copy via immediate submit
            VulkanContext::Get().ImmediateSubmit([&](VkCommandBuffer cmd) {
                VkBuffer srcBuffer = registry.GetBuffer(staging);
                VkBuffer dstBuffer = bufferData.buffer;

                VkBufferCopy copyRegion{};
                copyRegion.srcOffset = 0;
                copyRegion.dstOffset = offset;
                copyRegion.size = size;
                vkCmdCopyBuffer(cmd, srcBuffer, dstBuffer, 1, &copyRegion);
            });

            DestroyBuffer(staging);
        }
    }

    // ============================================================================
    // Texture Management
    // ============================================================================

    Result<RHITextureHandle> RHIDevice::TryCreateTexture(const RHITextureSpecification& spec)
    {
        auto& vkContext = VulkanContext::Get();
        auto device = vkContext.GetDevice();
        auto allocator = vkContext.GetAllocator();

        VkImageCreateInfo imageInfo{};
        imageInfo.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
        // Always use 2D for standard textures (even 1x1), only use 3D for volume textures
        imageInfo.imageType = spec.depth > 1 ? VK_IMAGE_TYPE_3D : VK_IMAGE_TYPE_2D;
        imageInfo.format = ToVulkan(spec.format);
        imageInfo.extent.width = spec.width;
        imageInfo.extent.height = spec.height;
        imageInfo.extent.depth = spec.depth;
        imageInfo.mipLevels = spec.mipLevels;
        imageInfo.arrayLayers = spec.arrayLayers;
        imageInfo.samples = ToVulkan(spec.samples);
        imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
        imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;

        // Convert usage flags
        VkImageUsageFlags vkUsage = 0;
        if (HasFlag(spec.usage, TextureUsage::Sampled))
            vkUsage |= VK_IMAGE_USAGE_SAMPLED_BIT;
        if (HasFlag(spec.usage, TextureUsage::Storage))
            vkUsage |= VK_IMAGE_USAGE_STORAGE_BIT;
        if (HasFlag(spec.usage, TextureUsage::ColorAttachment))
            vkUsage |= VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
        if (HasFlag(spec.usage, TextureUsage::DepthStencilAttachment))
            vkUsage |= VK_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT;
        if (HasFlag(spec.usage, TextureUsage::TransferSrc))
            vkUsage |= VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
        if (HasFlag(spec.usage, TextureUsage::TransferDst))
            vkUsage |= VK_IMAGE_USAGE_TRANSFER_DST_BIT;
        if (HasFlag(spec.usage, TextureUsage::InputAttachment))
            vkUsage |= VK_IMAGE_USAGE_INPUT_ATTACHMENT_BIT;
        imageInfo.usage = vkUsage;

        VmaAllocationCreateInfo allocInfo{};
        allocInfo.usage = VMA_MEMORY_USAGE_AUTO;
        allocInfo.flags = VMA_ALLOCATION_CREATE_DEDICATED_MEMORY_BIT;

        VkImage image = VK_NULL_HANDLE;
        VmaAllocation allocation = VK_NULL_HANDLE;

        VkResult vkResult = vmaCreateImage(allocator, &imageInfo, &allocInfo, &image, &allocation, nullptr);
        if (vkResult != VK_SUCCESS)
        {
            return Result<RHITextureHandle>::Err(
                std::string("vmaCreateImage failed (") + std::to_string(spec.width) + "x" +
                std::to_string(spec.height) + "): " + std::string(VkResultToString(vkResult)));
        }

        // Create image view
        VkImageViewCreateInfo viewInfo{};
        viewInfo.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO;
        viewInfo.image = image;
        viewInfo.viewType = spec.arrayLayers > 1 ? VK_IMAGE_VIEW_TYPE_2D_ARRAY : VK_IMAGE_VIEW_TYPE_2D;
        viewInfo.format = ToVulkan(spec.format);
        viewInfo.subresourceRange.aspectMask = IsDepthFormat(spec.format) ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT;
        viewInfo.subresourceRange.baseMipLevel = 0;
        viewInfo.subresourceRange.levelCount = spec.mipLevels;
        viewInfo.subresourceRange.baseArrayLayer = 0;
        viewInfo.subresourceRange.layerCount = spec.arrayLayers;

        VkImageView imageView = VK_NULL_HANDLE;
        vkResult = vkCreateImageView(device, &viewInfo, nullptr, &imageView);
        if (vkResult != VK_SUCCESS)
        {
            vmaDestroyImage(allocator, image, allocation);
            return Result<RHITextureHandle>::Err(
                std::string("vkCreateImageView failed: ") + std::string(VkResultToString(vkResult)));
        }

        return Result<RHITextureHandle>::Ok(
            VulkanResourceRegistry::Get().RegisterTexture(image, imageView, VK_NULL_HANDLE, allocation,
                                                          spec.width, spec.height, spec.format));
    }

    RHITextureHandle RHIDevice::CreateTexture(const RHITextureSpecification& spec)
    {
        auto result = TryCreateTexture(spec);
        if (result.IsErr())
        {
            GG_CORE_ERROR("RHIDevice::CreateTexture: {}", result.Error());
            return NullTexture;
        }
        return result.Value();
    }

    void RHIDevice::DestroyTexture(RHITextureHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto textureData = registry.GetTextureData(handle);
        auto device = VulkanContext::Get().GetDevice();
        auto allocator = VulkanContext::Get().GetAllocator();

        if (textureData.imageView != VK_NULL_HANDLE)
            vkDestroyImageView(device, textureData.imageView, nullptr);
        if (textureData.sampler != VK_NULL_HANDLE)
            vkDestroySampler(device, textureData.sampler, nullptr);
        if (textureData.image != VK_NULL_HANDLE)
            vmaDestroyImage(allocator, textureData.image, textureData.allocation);

        registry.UnregisterTexture(handle);
    }

    void RHIDevice::UploadTextureData(RHITextureHandle handle, const void* pixels, uint64_t size)
    {
        if (!handle.IsValid() || !pixels || size == 0) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto textureData = registry.GetTextureData(handle);

        // Create staging buffer
        RHIBufferSpecification stagingSpec;
        stagingSpec.size = size;
        stagingSpec.usage = BufferUsage::Staging;
        stagingSpec.cpuVisible = true;

        RHIBufferHandle staging = CreateBuffer(stagingSpec);
        UploadBufferData(staging, pixels, size, 0);

        VulkanContext::Get().ImmediateSubmit([&](VkCommandBuffer cmd) {
            // Transition to transfer dst
            VkImageMemoryBarrier barrier{};
            barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
            barrier.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED;
            barrier.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
            barrier.image = textureData.image;
            barrier.subresourceRange.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT;
            barrier.subresourceRange.baseMipLevel = 0;
            barrier.subresourceRange.levelCount = 1;
            barrier.subresourceRange.baseArrayLayer = 0;
            barrier.subresourceRange.layerCount = 1;
            barrier.srcAccessMask = 0;
            barrier.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;

            vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
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
            region.imageExtent = { textureData.width, textureData.height, 1 };

            vkCmdCopyBufferToImage(cmd, registry.GetBuffer(staging), textureData.image,
                                   VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);

            // Transition to shader read
            barrier.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
            barrier.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
            barrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            barrier.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;

            vkCmdPipelineBarrier(cmd, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                                 0, 0, nullptr, 0, nullptr, 1, &barrier);
        });

        DestroyBuffer(staging);
    }

    uint32_t RHIDevice::GetTextureWidth(RHITextureHandle handle) const
    {
        if (!handle.IsValid()) return 0;
        return VulkanResourceRegistry::Get().GetTextureData(handle).width;
    }

    uint32_t RHIDevice::GetTextureHeight(RHITextureHandle handle) const
    {
        if (!handle.IsValid()) return 0;
        return VulkanResourceRegistry::Get().GetTextureData(handle).height;
    }

    // ============================================================================
    // Sampler Management
    // ============================================================================

    RHISamplerHandle RHIDevice::CreateSampler(const RHISamplerSpecification& spec)
    {
        auto device = VulkanContext::Get().GetDevice();

        VkSamplerCreateInfo samplerInfo{};
        samplerInfo.sType = VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO;
        samplerInfo.magFilter = ToVulkan(spec.magFilter);
        samplerInfo.minFilter = ToVulkan(spec.minFilter);
        samplerInfo.mipmapMode = ToVulkan(spec.mipmapMode);
        samplerInfo.addressModeU = ToVulkan(spec.addressModeU);
        samplerInfo.addressModeV = ToVulkan(spec.addressModeV);
        samplerInfo.addressModeW = ToVulkan(spec.addressModeW);
        samplerInfo.mipLodBias = spec.mipLodBias;
        samplerInfo.anisotropyEnable = spec.anisotropyEnable ? VK_TRUE : VK_FALSE;
        samplerInfo.maxAnisotropy = spec.maxAnisotropy;
        samplerInfo.compareEnable = spec.compareEnable ? VK_TRUE : VK_FALSE;
        samplerInfo.compareOp = ToVulkan(spec.compareOp);
        samplerInfo.minLod = spec.minLod;
        samplerInfo.maxLod = spec.maxLod;
        samplerInfo.borderColor = ToVulkan(spec.borderColor);
        samplerInfo.unnormalizedCoordinates = VK_FALSE;

        VkSampler sampler = VK_NULL_HANDLE;
        if (vkCreateSampler(device, &samplerInfo, nullptr, &sampler) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateSampler: vkCreateSampler failed");
            return NullSampler;
        }

        // Register in a simple way - store sampler handle directly as ID
        return RHISamplerHandle{ reinterpret_cast<uint64_t>(sampler) };
    }

    void RHIDevice::DestroySampler(RHISamplerHandle handle)
    {
        if (!handle.IsValid()) return;
        auto device = VulkanContext::Get().GetDevice();
        VkSampler sampler = reinterpret_cast<VkSampler>(handle.id);
        vkDestroySampler(device, sampler, nullptr);
    }

}
