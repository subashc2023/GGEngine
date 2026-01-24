#include "ggpch.h"
#include "VulkanResourceRegistry.h"
#include "VulkanConversions.h"
#include "VulkanContext.h"
#include "GGEngine/RHI/RHIDevice.h"

namespace GGEngine {

    // ============================================================================
    // Render Pass Management
    // ============================================================================

    RHIRenderPassHandle RHIDevice::CreateRenderPass(const RHIRenderPassSpecification& spec)
    {
        auto device = VulkanContext::Get().GetDevice();

        std::vector<VkAttachmentDescription> attachments;
        std::vector<VkAttachmentReference> colorRefs;

        for (const auto& colorAttach : spec.colorAttachments)
        {
            VkAttachmentDescription attachment{};
            attachment.format = ToVulkan(colorAttach.format);
            attachment.samples = ToVulkan(colorAttach.samples);
            attachment.loadOp = ToVulkan(colorAttach.loadOp);
            attachment.storeOp = ToVulkan(colorAttach.storeOp);
            attachment.stencilLoadOp = ToVulkan(colorAttach.stencilLoadOp);
            attachment.stencilStoreOp = ToVulkan(colorAttach.stencilStoreOp);
            attachment.initialLayout = ToVulkan(colorAttach.initialLayout);
            attachment.finalLayout = ToVulkan(colorAttach.finalLayout);
            attachments.push_back(attachment);

            VkAttachmentReference ref{};
            ref.attachment = static_cast<uint32_t>(colorRefs.size());
            ref.layout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
            colorRefs.push_back(ref);
        }

        VkAttachmentReference depthRef{};
        bool hasDepth = spec.depthStencilAttachment.has_value();
        if (hasDepth)
        {
            const auto& depthAttach = spec.depthStencilAttachment.value();
            VkAttachmentDescription attachment{};
            attachment.format = ToVulkan(depthAttach.format);
            attachment.samples = ToVulkan(depthAttach.samples);
            attachment.loadOp = ToVulkan(depthAttach.loadOp);
            attachment.storeOp = ToVulkan(depthAttach.storeOp);
            attachment.stencilLoadOp = ToVulkan(depthAttach.stencilLoadOp);
            attachment.stencilStoreOp = ToVulkan(depthAttach.stencilStoreOp);
            attachment.initialLayout = ToVulkan(depthAttach.initialLayout);
            attachment.finalLayout = ToVulkan(depthAttach.finalLayout);
            attachments.push_back(attachment);

            depthRef.attachment = static_cast<uint32_t>(attachments.size() - 1);
            depthRef.layout = VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
        }

        VkSubpassDescription subpass{};
        subpass.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS;
        subpass.colorAttachmentCount = static_cast<uint32_t>(colorRefs.size());
        subpass.pColorAttachments = colorRefs.data();
        subpass.pDepthStencilAttachment = hasDepth ? &depthRef : nullptr;

        VkSubpassDependency dependency{};
        dependency.srcSubpass = VK_SUBPASS_EXTERNAL;
        dependency.dstSubpass = 0;
        dependency.srcStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.dstStageMask = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.srcAccessMask = 0;
        dependency.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;

        VkRenderPassCreateInfo renderPassInfo{};
        renderPassInfo.sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO;
        renderPassInfo.attachmentCount = static_cast<uint32_t>(attachments.size());
        renderPassInfo.pAttachments = attachments.data();
        renderPassInfo.subpassCount = 1;
        renderPassInfo.pSubpasses = &subpass;
        renderPassInfo.dependencyCount = 1;
        renderPassInfo.pDependencies = &dependency;

        VkRenderPass renderPass = VK_NULL_HANDLE;
        if (vkCreateRenderPass(device, &renderPassInfo, nullptr, &renderPass) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateRenderPass: vkCreateRenderPass failed");
            return NullRenderPass;
        }

        return VulkanResourceRegistry::Get().RegisterRenderPass(renderPass);
    }

    void RHIDevice::DestroyRenderPass(RHIRenderPassHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto rpData = registry.GetRenderPassData(handle);
        auto device = VulkanContext::Get().GetDevice();

        if (rpData.renderPass != VK_NULL_HANDLE)
            vkDestroyRenderPass(device, rpData.renderPass, nullptr);

        registry.UnregisterRenderPass(handle);
    }

    // ============================================================================
    // Framebuffer Management
    // ============================================================================

    RHIFramebufferHandle RHIDevice::CreateFramebuffer(const RHIFramebufferSpecification& spec)
    {
        auto device = VulkanContext::Get().GetDevice();
        auto& registry = VulkanResourceRegistry::Get();

        std::vector<VkImageView> attachmentViews;
        for (const auto& texHandle : spec.attachments)
        {
            auto texData = registry.GetTextureData(texHandle);
            if (texData.imageView == VK_NULL_HANDLE)
            {
                GG_CORE_ERROR("RHIDevice::CreateFramebuffer: Invalid texture attachment (handle.id={})", texHandle.id);
                return NullFramebuffer;
            }
            attachmentViews.push_back(texData.imageView);
        }

        VkRenderPass vkRenderPass = registry.GetRenderPass(spec.renderPass);
        if (vkRenderPass == VK_NULL_HANDLE)
        {
            GG_CORE_ERROR("RHIDevice::CreateFramebuffer: Invalid render pass handle (id={})", spec.renderPass.id);
            return NullFramebuffer;
        }

        VkFramebufferCreateInfo framebufferInfo{};
        framebufferInfo.sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO;
        framebufferInfo.renderPass = vkRenderPass;
        framebufferInfo.attachmentCount = static_cast<uint32_t>(attachmentViews.size());
        framebufferInfo.pAttachments = attachmentViews.data();
        framebufferInfo.width = spec.width;
        framebufferInfo.height = spec.height;
        framebufferInfo.layers = spec.layers;

        VkFramebuffer framebuffer = VK_NULL_HANDLE;
        if (vkCreateFramebuffer(device, &framebufferInfo, nullptr, &framebuffer) != VK_SUCCESS)
        {
            GG_CORE_ERROR("RHIDevice::CreateFramebuffer: vkCreateFramebuffer failed");
            return NullFramebuffer;
        }

        // Register framebuffer - we'll need to add this to registry
        // For now, return handle with VkFramebuffer as ID
        return RHIFramebufferHandle{ reinterpret_cast<uint64_t>(framebuffer) };
    }

    void RHIDevice::DestroyFramebuffer(RHIFramebufferHandle handle)
    {
        if (!handle.IsValid()) return;

        auto device = VulkanContext::Get().GetDevice();
        VkFramebuffer framebuffer = reinterpret_cast<VkFramebuffer>(handle.id);
        vkDestroyFramebuffer(device, framebuffer, nullptr);
    }

}
