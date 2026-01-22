#include "ggpch.h"
#include "Framebuffer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "Platform/Vulkan/VulkanUtils.h"
#include "Platform/Vulkan/VulkanRHI.h"
#include "GGEngine/Core/Log.h"

#include <imgui.h>
#include <backends/imgui_impl_vulkan.h>
#include <unordered_map>

namespace GGEngine {

    // Internal Vulkan-specific data storage
    struct VulkanFramebufferData
    {
        VkImage image = VK_NULL_HANDLE;
        VmaAllocation imageAllocation = VK_NULL_HANDLE;
        VkImageView imageView = VK_NULL_HANDLE;
        VkSampler sampler = VK_NULL_HANDLE;
        VkFramebuffer framebuffer = VK_NULL_HANDLE;
        VkRenderPass renderPass = VK_NULL_HANDLE;
    };

    // Map to store Vulkan data for each framebuffer instance
    static std::unordered_map<const Framebuffer*, VulkanFramebufferData> s_VulkanData;

    Framebuffer::Framebuffer(const FramebufferSpecification& spec)
        : m_Specification(spec)
    {
        s_VulkanData[this] = {};
        CreateRenderPass();
        CreateResources();
    }

    Framebuffer::~Framebuffer()
    {
        auto device = VulkanContext::Get().GetDevice();
        vkDeviceWaitIdle(device);

        DestroyResources();
        DestroyRenderPass();

        // Unregister render pass from registry
        if (m_RenderPassHandle.IsValid())
        {
            VulkanResourceRegistry::Get().UnregisterRenderPass(m_RenderPassHandle);
        }

        s_VulkanData.erase(this);
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

    void Framebuffer::BeginRenderPass(RHICommandBufferHandle cmd)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        BeginRenderPassVk(vkCmd);
    }

    void Framebuffer::EndRenderPass(RHICommandBufferHandle cmd)
    {
        auto& registry = VulkanResourceRegistry::Get();
        VkCommandBuffer vkCmd = registry.GetCommandBuffer(cmd);
        EndRenderPassVk(vkCmd);
    }

    void Framebuffer::BeginRenderPassVk(void* vkCmd)
    {
        auto& data = s_VulkanData[this];

        VkRenderPassBeginInfo renderPassInfo{};
        renderPassInfo.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO;
        renderPassInfo.renderPass = data.renderPass;
        renderPassInfo.framebuffer = data.framebuffer;
        renderPassInfo.renderArea.offset = { 0, 0 };
        renderPassInfo.renderArea.extent = { m_Specification.Width, m_Specification.Height };

        VkClearValue clearColor = { {{0.1f, 0.1f, 0.1f, 1.0f}} };
        renderPassInfo.clearValueCount = 1;
        renderPassInfo.pClearValues = &clearColor;

        vkCmdBeginRenderPass(static_cast<VkCommandBuffer>(vkCmd), &renderPassInfo, VK_SUBPASS_CONTENTS_INLINE);
    }

    void Framebuffer::EndRenderPassVk(void* vkCmd)
    {
        vkCmdEndRenderPass(static_cast<VkCommandBuffer>(vkCmd));
    }

    void Framebuffer::CreateRenderPass()
    {
        auto device = VulkanContext::Get().GetDevice();
        auto& data = s_VulkanData[this];
        VkFormat vkFormat = ToVulkan(m_Specification.Format);

        VkAttachmentDescription colorAttachment{};
        colorAttachment.format = vkFormat;
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

        if (vkCreateRenderPass(device, &renderPassInfo, nullptr, &data.renderPass) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create offscreen render pass!");
            return;
        }

        // Register with VulkanResourceRegistry
        m_RenderPassHandle = VulkanResourceRegistry::Get().RegisterRenderPass(
            data.renderPass, data.framebuffer, m_Specification.Width, m_Specification.Height);
    }

    void Framebuffer::DestroyRenderPass()
    {
        auto device = VulkanContext::Get().GetDevice();
        auto& data = s_VulkanData[this];

        if (data.renderPass != VK_NULL_HANDLE)
        {
            vkDestroyRenderPass(device, data.renderPass, nullptr);
            data.renderPass = VK_NULL_HANDLE;
        }
    }

    void Framebuffer::CreateResources()
    {
        auto device = VulkanContext::Get().GetDevice();
        auto allocator = VulkanContext::Get().GetAllocator();
        auto& data = s_VulkanData[this];
        VkFormat vkFormat = ToVulkan(m_Specification.Format);

        // Create Image with VMA
        VkImageCreateInfo imageInfo{};
        imageInfo.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
        imageInfo.imageType = VK_IMAGE_TYPE_2D;
        imageInfo.extent.width = m_Specification.Width;
        imageInfo.extent.height = m_Specification.Height;
        imageInfo.extent.depth = 1;
        imageInfo.mipLevels = 1;
        imageInfo.arrayLayers = 1;
        imageInfo.format = vkFormat;
        imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
        imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        imageInfo.usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_SAMPLED_BIT;
        imageInfo.samples = VK_SAMPLE_COUNT_1_BIT;
        imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE;

        VmaAllocationCreateInfo allocCreateInfo{};
        allocCreateInfo.usage = VMA_MEMORY_USAGE_AUTO;
        allocCreateInfo.flags = VMA_ALLOCATION_CREATE_DEDICATED_MEMORY_BIT;

        if (vmaCreateImage(allocator, &imageInfo, &allocCreateInfo, &data.image, &data.imageAllocation, nullptr) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create framebuffer image with VMA!");
            return;
        }

        // Create Image View
        data.imageView = VulkanUtils::CreateImageView2D(device, data.image, vkFormat);
        if (data.imageView == VK_NULL_HANDLE)
            return;

        // Create Sampler (linear filtering, clamp to edge for framebuffer)
        data.sampler = VulkanUtils::CreateSampler(device, VK_FILTER_LINEAR, VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE);
        if (data.sampler == VK_NULL_HANDLE)
            return;

        // Create Framebuffer
        VkFramebufferCreateInfo framebufferInfo{};
        framebufferInfo.sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO;
        framebufferInfo.renderPass = data.renderPass;
        framebufferInfo.attachmentCount = 1;
        framebufferInfo.pAttachments = &data.imageView;
        framebufferInfo.width = m_Specification.Width;
        framebufferInfo.height = m_Specification.Height;
        framebufferInfo.layers = 1;

        if (vkCreateFramebuffer(device, &framebufferInfo, nullptr, &data.framebuffer) != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create framebuffer!");
            return;
        }

        // Transition image to SHADER_READ_ONLY_OPTIMAL so it's ready for ImGui
        TransitionImageLayout(VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL);

        // Register with ImGui
        m_ImGuiDescriptorSet = ImGui_ImplVulkan_AddTexture(
            data.sampler, data.imageView, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL);

        GG_CORE_INFO("Framebuffer created: {}x{}", m_Specification.Width, m_Specification.Height);
    }

    void Framebuffer::TransitionImageLayout(int oldLayout, int newLayout)
    {
        auto& data = s_VulkanData[this];
        VulkanContext::Get().ImmediateSubmit([&](VkCommandBuffer cmd) {
            VulkanUtils::TransitionImageLayout(cmd, data.image,
                static_cast<VkImageLayout>(oldLayout), static_cast<VkImageLayout>(newLayout));
        });
    }

    void Framebuffer::DestroyResources()
    {
        auto device = VulkanContext::Get().GetDevice();
        auto& data = s_VulkanData[this];

        if (m_ImGuiDescriptorSet != nullptr)
        {
            ImGui_ImplVulkan_RemoveTexture(static_cast<VkDescriptorSet>(m_ImGuiDescriptorSet));
            m_ImGuiDescriptorSet = nullptr;
        }

        if (data.framebuffer != VK_NULL_HANDLE)
        {
            vkDestroyFramebuffer(device, data.framebuffer, nullptr);
            data.framebuffer = VK_NULL_HANDLE;
        }

        if (data.sampler != VK_NULL_HANDLE)
        {
            vkDestroySampler(device, data.sampler, nullptr);
            data.sampler = VK_NULL_HANDLE;
        }

        if (data.imageView != VK_NULL_HANDLE)
        {
            vkDestroyImageView(device, data.imageView, nullptr);
            data.imageView = VK_NULL_HANDLE;
        }

        if (data.image != VK_NULL_HANDLE)
        {
            auto allocator = VulkanContext::Get().GetAllocator();
            vmaDestroyImage(allocator, data.image, data.imageAllocation);
            data.image = VK_NULL_HANDLE;
            data.imageAllocation = VK_NULL_HANDLE;
        }
    }
}
