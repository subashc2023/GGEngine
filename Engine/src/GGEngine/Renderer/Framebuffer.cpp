#include "ggpch.h"
#include "Framebuffer.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "GGEngine/RHI/RHICommandBuffer.h"
#include "GGEngine/Core/Log.h"

namespace GGEngine {

    Framebuffer::Framebuffer(const FramebufferSpecification& spec)
        : m_Specification(spec)
    {
        CreateRenderPass();
        CreateResources();
    }

    Framebuffer::~Framebuffer()
    {
        auto& device = RHIDevice::Get();
        device.WaitIdle();

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

        auto& device = RHIDevice::Get();
        device.WaitIdle();

        m_Specification.Width = width;
        m_Specification.Height = height;

        DestroyResources();
        CreateResources();
    }

    void Framebuffer::BeginRenderPass(RHICommandBufferHandle cmd)
    {
        RHICmd::BeginRenderPass(cmd, m_RenderPassHandle, m_FramebufferHandle,
                                m_Specification.Width, m_Specification.Height,
                                0.1f, 0.1f, 0.1f, 1.0f);
    }

    void Framebuffer::EndRenderPass(RHICommandBufferHandle cmd)
    {
        RHICmd::EndRenderPass(cmd);
    }

    void Framebuffer::CreateRenderPass()
    {
        auto& device = RHIDevice::Get();

        // Build render pass specification
        RHIRenderPassSpecification rpSpec;

        RHIAttachmentDescription colorAttachment;
        colorAttachment.format = m_Specification.Format;
        colorAttachment.samples = SampleCount::Count1;
        colorAttachment.loadOp = LoadOp::Clear;
        colorAttachment.storeOp = StoreOp::Store;
        colorAttachment.stencilLoadOp = LoadOp::DontCare;
        colorAttachment.stencilStoreOp = StoreOp::DontCare;
        colorAttachment.initialLayout = ImageLayout::Undefined;
        colorAttachment.finalLayout = ImageLayout::ShaderReadOnly;

        rpSpec.colorAttachments.push_back(colorAttachment);
        rpSpec.debugName = "Framebuffer_RenderPass";

        m_RenderPassHandle = device.CreateRenderPass(rpSpec);
        if (!m_RenderPassHandle.IsValid())
        {
            GG_CORE_ERROR("Failed to create offscreen render pass!");
        }
    }

    void Framebuffer::DestroyRenderPass()
    {
        if (m_RenderPassHandle.IsValid())
        {
            RHIDevice::Get().DestroyRenderPass(m_RenderPassHandle);
            m_RenderPassHandle = NullRenderPass;
        }
    }

    void Framebuffer::CreateResources()
    {
        auto& device = RHIDevice::Get();

        // 1. Create texture (color attachment)
        RHITextureSpecification textureSpec;
        textureSpec.width = m_Specification.Width;
        textureSpec.height = m_Specification.Height;
        textureSpec.depth = 1;
        textureSpec.mipLevels = 1;
        textureSpec.arrayLayers = 1;
        textureSpec.format = m_Specification.Format;
        textureSpec.samples = SampleCount::Count1;
        textureSpec.usage = TextureUsage::ColorAttachment | TextureUsage::Sampled;
        textureSpec.initialLayout = ImageLayout::Undefined;
        textureSpec.debugName = "Framebuffer_ColorAttachment";

        m_TextureHandle = device.CreateTexture(textureSpec);
        if (!m_TextureHandle.IsValid())
        {
            GG_CORE_ERROR("Failed to create framebuffer texture!");
            return;
        }

        // 2. Create sampler (linear filtering, clamp to edge for framebuffer)
        RHISamplerSpecification samplerSpec;
        samplerSpec.minFilter = Filter::Linear;
        samplerSpec.magFilter = Filter::Linear;
        samplerSpec.mipmapMode = MipmapMode::Linear;
        samplerSpec.addressModeU = AddressMode::ClampToEdge;
        samplerSpec.addressModeV = AddressMode::ClampToEdge;
        samplerSpec.addressModeW = AddressMode::ClampToEdge;

        m_SamplerHandle = device.CreateSampler(samplerSpec);
        if (!m_SamplerHandle.IsValid())
        {
            device.DestroyTexture(m_TextureHandle);
            m_TextureHandle = NullTexture;
            GG_CORE_ERROR("Failed to create framebuffer sampler!");
            return;
        }

        // 3. Create framebuffer
        RHIFramebufferSpecification fbSpec;
        fbSpec.renderPass = m_RenderPassHandle;
        fbSpec.attachments.push_back(m_TextureHandle);
        fbSpec.width = m_Specification.Width;
        fbSpec.height = m_Specification.Height;
        fbSpec.layers = 1;
        fbSpec.debugName = "Framebuffer";

        m_FramebufferHandle = device.CreateFramebuffer(fbSpec);
        if (!m_FramebufferHandle.IsValid())
        {
            device.DestroySampler(m_SamplerHandle);
            m_SamplerHandle = NullSampler;
            device.DestroyTexture(m_TextureHandle);
            m_TextureHandle = NullTexture;
            GG_CORE_ERROR("Failed to create framebuffer!");
            return;
        }

        // 4. Transition image to SHADER_READ_ONLY_OPTIMAL so it's ready for ImGui
        device.ImmediateSubmit([this](RHICommandBufferHandle cmd) {
            RHICmd::TransitionImageLayout(cmd, m_TextureHandle,
                                          ImageLayout::Undefined, ImageLayout::ShaderReadOnly);
        });

        // 5. Register with ImGui (abstracted through RHI)
        m_ImGuiDescriptorSet = device.RegisterImGuiTexture(m_TextureHandle, m_SamplerHandle);

        GG_CORE_INFO("Framebuffer created: {}x{}", m_Specification.Width, m_Specification.Height);
    }

    void Framebuffer::DestroyResources()
    {
        auto& device = RHIDevice::Get();

        // Unregister from ImGui
        if (m_ImGuiDescriptorSet != nullptr)
        {
            device.UnregisterImGuiTexture(m_ImGuiDescriptorSet);
            m_ImGuiDescriptorSet = nullptr;
        }

        // Destroy framebuffer
        if (m_FramebufferHandle.IsValid())
        {
            device.DestroyFramebuffer(m_FramebufferHandle);
            m_FramebufferHandle = NullFramebuffer;
        }

        // Destroy sampler
        if (m_SamplerHandle.IsValid())
        {
            device.DestroySampler(m_SamplerHandle);
            m_SamplerHandle = NullSampler;
        }

        // Destroy texture
        if (m_TextureHandle.IsValid())
        {
            device.DestroyTexture(m_TextureHandle);
            m_TextureHandle = NullTexture;
        }
    }

}
