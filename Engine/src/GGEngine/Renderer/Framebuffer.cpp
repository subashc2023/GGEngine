#include "ggpch.h"
#include "Framebuffer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "Platform/Vulkan/VulkanUtils.h"
#include "GGEngine/Core/Log.h"

#include <imgui.h>
#include <backends/imgui_impl_vulkan.h>

namespace GGEngine {

    Framebuffer::Framebuffer(const FramebufferSpecification& spec)
        : m_Specification(spec)
    {
        CreateRenderPass();
        CreateResources();
    }

    Framebuffer::~Framebuffer()
    {
        auto device = VulkanContext::Get().GetDevice();
        vkDeviceWaitIdle(device);

        DestroyResources();
        DestroyRenderPass();
    }

    void Framebuffer::Resize(uint32_t width, uint32_t height)
    {
        if (width == 0 || height == 0 || width > 8192 || height > 8192)
        {
            GG_CORE_WARN("Invalid framebuffer resize: {}x{}", width, height);
            return;
        }

        if (width == m_Specification.Width && height == m_Specification.Height)
            return;

        auto device = VulkanContext::Get().GetDevice();
        vkDeviceWaitIdle(device);

        m_Specification.Width = width;
        m_Specification.Height = height;

        DestroyResources();
        CreateResources();
    }

    void Framebuffer::BeginRenderPass(VkCommandBuffer cmd)
    {
        VkRenderPassBeginInfo renderPassInfo{};
        renderPassInfo.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO;
        renderPassInfo.renderPass = m_RenderPass;
        renderPassInfo.framebuffer = m_Framebuffer;
        renderPassInfo.renderArea.offset = { 0, 0 };
        renderPassInfo.renderArea.extent = { m_Specification.Width, m_Specification.Height };

        VkClearValue clearColor = { {{0.1f, 0.1f, 0.1f, 1.0f}} };
        renderPassInfo.clearValueCount = 1;
        renderPassInfo.pClearValues = &clearColor;

        vkCmdBeginRenderPass(cmd, &renderPassInfo, VK_SUBPASS_CONTENTS_INLINE);
    }

    void Framebuffer::EndRenderPass(VkCommandBuffer cmd)
    {
        vkCmdEndRenderPass(cmd);
    }

    void Framebuffer::CreateRenderPass()
    {
        auto device = VulkanContext::Get().GetDevice();

        VkAttachmentDescription colorAttachment{};
        colorAttachment.format = m_Specification.Format;
        colorAttachment.samples = VK_SAMPLE_COUNT_1_BIT;
        colorAttachment.loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR;
        colorAttachment.storeOp = VK_ATTACHMENT_STORE_OP_STORE;
        colorAttachment.stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE;
        colorAttachment.stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE;
        colorAttachment.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        colorAttachment.finalLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;

        VkAttachmentReference colorAttachmentRef{};
        colorAttachmentRef.attachment = 0;
        colorAttachmentRef.layout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;

        VkSubpassDescription subpass{};
        subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
        subpass.colorAttachmentCount = 1;
        subpass.pColorAttachments = &colorAttachmentRef;

        VkSubpassDependency dependency{};
        dependency.srcSubpass = VK_SUBPASS_EXTERNAL;
        dependency.dstSubpass = 0;
        dependency.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.srcAccessMask = 0;
        dependency.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;

        VkRenderPassCreateInfo renderPassInfo{};
        renderPassInfo.sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO;
        renderPassInfo.attachmentCount = 1;
        renderPassInfo.pAttachments = &colorAttachment;
        renderPassInfo.subpassCount = 1;
        renderPassInfo.pSubpasses = &subpass;
        renderPassInfo.dependencyCount = 1;
        renderPassInfo.pDependencies = &dependency;

        if (vkCreateRenderPass(device, &renderPassInfo, nullptr, &m_RenderPass) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create offscreen render pass!");
        }
    }

    void Framebuffer::DestroyRenderPass()
    {
        auto device = VulkanContext::Get().GetDevice();
        if (m_RenderPass != VK_NULL_HANDLE)
        {
            vkDestroyRenderPass(device, m_RenderPass, nullptr);
            m_RenderPass = VK_NULL_HANDLE;
        }
    }

    void Framebuffer::CreateResources()
    {
        auto device = VulkanContext::Get().GetDevice();
        auto allocator = VulkanContext::Get().GetAllocator();

        // Create Image with VMA
        VkImageCreateInfo imageInfo{};
        imageInfo.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
        imageInfo.imageType = VK_IMAGE_TYPE_2D;
        imageInfo.extent.width = m_Specification.Width;
        imageInfo.extent.height = m_Specification.Height;
        imageInfo.extent.depth = 1;
        imageInfo.mipLevels = 1;
        imageInfo.arrayLayers = 1;
        imageInfo.format = m_Specification.Format;
        imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
        imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        imageInfo.usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_SAMPLED_BIT;
        imageInfo.samples = VK_SAMPLE_COUNT_1_BIT;
        imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;

        VmaAllocationCreateInfo allocCreateInfo{};
        allocCreateInfo.usage = VMA_MEMORY_USAGE_AUTO;
        allocCreateInfo.flags = VMA_ALLOCATION_CREATE_DEDICATED_MEMORY_BIT;

        if (vmaCreateImage(allocator, &imageInfo, &allocCreateInfo, &m_Image, &m_ImageAllocation, nullptr) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create framebuffer image with VMA!");
            return;
        }

        // Create Image View
        m_ImageView = VulkanUtils::CreateImageView2D(device, m_Image, m_Specification.Format);
        if (m_ImageView == VK_NULL_HANDLE)
            return;

        // Create Sampler (linear filtering, clamp to edge for framebuffer)
        m_Sampler = VulkanUtils::CreateSampler(device, VK_FILTER_LINEAR, VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE);
        if (m_Sampler == VK_NULL_HANDLE)
            return;

        // Create Framebuffer
        VkFramebufferCreateInfo framebufferInfo{};
        framebufferInfo.sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO;
        framebufferInfo.renderPass = m_RenderPass;
        framebufferInfo.attachmentCount = 1;
        framebufferInfo.pAttachments = &m_ImageView;
        framebufferInfo.width = m_Specification.Width;
        framebufferInfo.height = m_Specification.Height;
        framebufferInfo.layers = 1;

        if (vkCreateFramebuffer(device, &framebufferInfo, nullptr, &m_Framebuffer) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create framebuffer!");
            return;
        }

        // Transition image to SHADER_READ_ONLY_OPTIMAL so it's ready for ImGui
        TransitionImageLayout(VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL);

        // Register with ImGui
        m_ImGuiDescriptorSet = ImGui_ImplVulkan_AddTexture(
            m_Sampler, m_ImageView, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL);

        GG_CORE_INFO("Framebuffer created: {}x{}", m_Specification.Width, m_Specification.Height);
    }

    void Framebuffer::TransitionImageLayout(VkImageLayout oldLayout, VkImageLayout newLayout)
    {
        VulkanContext::Get().ImmediateSubmit([=](VkCommandBuffer cmd) {
            VulkanUtils::TransitionImageLayout(cmd, m_Image, oldLayout, newLayout);
        });
    }

    void Framebuffer::DestroyResources()
    {
        auto device = VulkanContext::Get().GetDevice();

        if (m_ImGuiDescriptorSet != VK_NULL_HANDLE)
        {
            ImGui_ImplVulkan_RemoveTexture(m_ImGuiDescriptorSet);
            m_ImGuiDescriptorSet = VK_NULL_HANDLE;
        }

        if (m_Framebuffer != VK_NULL_HANDLE)
        {
            vkDestroyFramebuffer(device, m_Framebuffer, nullptr);
            m_Framebuffer = VK_NULL_HANDLE;
        }

        if (m_Sampler != VK_NULL_HANDLE)
        {
            vkDestroySampler(device, m_Sampler, nullptr);
            m_Sampler = VK_NULL_HANDLE;
        }

        if (m_ImageView != VK_NULL_HANDLE)
        {
            vkDestroyImageView(device, m_ImageView, nullptr);
            m_ImageView = VK_NULL_HANDLE;
        }

        if (m_Image != VK_NULL_HANDLE)
        {
            auto allocator = VulkanContext::Get().GetAllocator();
            vmaDestroyImage(allocator, m_Image, m_ImageAllocation);
            m_Image = VK_NULL_HANDLE;
            m_ImageAllocation = VK_NULL_HANDLE;
        }
    }
}
