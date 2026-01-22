#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"
#include "GGEngine/RHI/RHIEnums.h"
#include <cstdint>

namespace GGEngine {

    struct FramebufferSpecification {
        uint32_t Width = 1280;
        uint32_t Height = 720;
        TextureFormat Format = TextureFormat::B8G8R8A8_UNORM;
    };

    class GG_API Framebuffer {
    public:
        Framebuffer(const FramebufferSpecification& spec);
        ~Framebuffer();

        void Resize(uint32_t width, uint32_t height);

        // Render pass management (RHI handle)
        void BeginRenderPass(RHICommandBufferHandle cmd);
        void EndRenderPass(RHICommandBufferHandle cmd);

        // Backward compatibility (Vulkan)
        void BeginRenderPassVk(void* vkCmd);
        void EndRenderPassVk(void* vkCmd);

        // ImGui integration - returns opaque handle for ImGui texture binding
        void* GetImGuiTextureID() const { return m_ImGuiDescriptorSet; }

        uint32_t GetWidth() const { return m_Specification.Width; }
        uint32_t GetHeight() const { return m_Specification.Height; }

        // RHI handle for render pass
        RHIRenderPassHandle GetRenderPassHandle() const { return m_RenderPassHandle; }

    private:
        void CreateResources();
        void DestroyResources();
        void CreateRenderPass();
        void DestroyRenderPass();
        void TransitionImageLayout(int oldLayout, int newLayout);

        FramebufferSpecification m_Specification;
        RHIRenderPassHandle m_RenderPassHandle;

        // Opaque pointer for ImGui descriptor set (cast to VkDescriptorSet in impl)
        void* m_ImGuiDescriptorSet = nullptr;
    };
}
