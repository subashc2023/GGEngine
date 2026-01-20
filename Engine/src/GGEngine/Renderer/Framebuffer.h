#pragma once

#include "GGEngine/Core.h"
#include <vulkan/vulkan.h>
#include <vk_mem_alloc.h>
#include <cstdint>

namespace GGEngine {

    struct FramebufferSpecification {
        uint32_t Width = 1280;
        uint32_t Height = 720;
        VkFormat Format = VK_FORMAT_B8G8R8A8_UNORM;
    };

    class GG_API Framebuffer {
    public:
        Framebuffer(const FramebufferSpecification& spec);
        ~Framebuffer();

        void Resize(uint32_t width, uint32_t height);

        void BeginRenderPass(VkCommandBuffer cmd);
        void EndRenderPass(VkCommandBuffer cmd);

        VkDescriptorSet GetImGuiTextureID() const { return m_ImGuiDescriptorSet; }
        uint32_t GetWidth() const { return m_Specification.Width; }
        uint32_t GetHeight() const { return m_Specification.Height; }
        VkRenderPass GetRenderPass() const { return m_RenderPass; }

    private:
        void CreateResources();
        void DestroyResources();
        void CreateRenderPass();
        void DestroyRenderPass();
        void TransitionImageLayout(VkImageLayout oldLayout, VkImageLayout newLayout);

        FramebufferSpecification m_Specification;

        VkImage m_Image = VK_NULL_HANDLE;
        VmaAllocation m_ImageAllocation = VK_NULL_HANDLE;
        VkImageView m_ImageView = VK_NULL_HANDLE;
        VkSampler m_Sampler = VK_NULL_HANDLE;
        VkFramebuffer m_Framebuffer = VK_NULL_HANDLE;
        VkRenderPass m_RenderPass = VK_NULL_HANDLE;
        VkDescriptorSet m_ImGuiDescriptorSet = VK_NULL_HANDLE;
    };
}
