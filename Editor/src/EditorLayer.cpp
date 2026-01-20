#include "EditorLayer.h"
#include "Platform/Vulkan/VulkanContext.h"

#include <imgui.h>
#include <vulkan/vulkan.h>

EditorLayer::EditorLayer()
    : Layer("EditorLayer")
{
}

void EditorLayer::OnAttach()
{
    GGEngine::FramebufferSpecification spec;
    spec.Width = 1280;
    spec.Height = 720;
    m_ViewportFramebuffer = std::make_unique<GGEngine::Framebuffer>(spec);

    GG_INFO("EditorLayer attached");
}

void EditorLayer::OnDetach()
{
    m_ViewportFramebuffer.reset();
    GG_INFO("EditorLayer detached");
}

void EditorLayer::OnRenderOffscreen()
{
    if (!m_ViewportFramebuffer)
        return;

    // Handle pending resize before rendering
    if (m_NeedsResize && m_PendingViewportWidth > 0 && m_PendingViewportHeight > 0)
    {
        m_ViewportFramebuffer->Resize(
            static_cast<uint32_t>(m_PendingViewportWidth),
            static_cast<uint32_t>(m_PendingViewportHeight));
        m_ViewportWidth = m_PendingViewportWidth;
        m_ViewportHeight = m_PendingViewportHeight;
        m_NeedsResize = false;
    }

    auto& vkContext = GGEngine::VulkanContext::Get();
    VkCommandBuffer cmd = vkContext.GetCurrentCommandBuffer();

    if (cmd == VK_NULL_HANDLE)
        return;

    // Begin offscreen render pass
    m_ViewportFramebuffer->BeginRenderPass(cmd);

    // Render triangle to framebuffer
    VkPipeline pipeline = vkContext.GetTrianglePipeline();
    if (pipeline != VK_NULL_HANDLE)
    {
        vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);

        VkViewport viewport{};
        viewport.x = 0.0f;
        viewport.y = 0.0f;
        viewport.width = static_cast<float>(m_ViewportFramebuffer->GetWidth());
        viewport.height = static_cast<float>(m_ViewportFramebuffer->GetHeight());
        viewport.minDepth = 0.0f;
        viewport.maxDepth = 1.0f;
        vkCmdSetViewport(cmd, 0, 1, &viewport);

        VkRect2D scissor{};
        scissor.offset = {0, 0};
        scissor.extent = { m_ViewportFramebuffer->GetWidth(), m_ViewportFramebuffer->GetHeight() };
        vkCmdSetScissor(cmd, 0, 1, &scissor);

        vkCmdDraw(cmd, 3, 1, 0, 0);
    }

    m_ViewportFramebuffer->EndRenderPass(cmd);
}

void EditorLayer::OnUpdate()
{
    // Dockspace setup
    static bool dockspaceOpen = true;
    static ImGuiDockNodeFlags dockspaceFlags = ImGuiDockNodeFlags_None;

    ImGuiWindowFlags windowFlags = ImGuiWindowFlags_MenuBar | ImGuiWindowFlags_NoDocking;
    ImGuiViewport* viewport = ImGui::GetMainViewport();
    ImGui::SetNextWindowPos(viewport->Pos);
    ImGui::SetNextWindowSize(viewport->Size);
    ImGui::SetNextWindowViewport(viewport->ID);
    ImGui::PushStyleVar(ImGuiStyleVar_WindowRounding, 0.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_WindowBorderSize, 0.0f);
    windowFlags |= ImGuiWindowFlags_NoTitleBar | ImGuiWindowFlags_NoCollapse | ImGuiWindowFlags_NoResize | ImGuiWindowFlags_NoMove;
    windowFlags |= ImGuiWindowFlags_NoBringToFrontOnFocus | ImGuiWindowFlags_NoNavFocus;

    if (dockspaceFlags & ImGuiDockNodeFlags_PassthruCentralNode)
        windowFlags |= ImGuiWindowFlags_NoBackground;

    ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, ImVec2(0.0f, 0.0f));
    ImGui::Begin("DockSpace Demo", &dockspaceOpen, windowFlags);
    ImGui::PopStyleVar(3);

    // DockSpace
    ImGuiIO& io = ImGui::GetIO();
    if (io.ConfigFlags & ImGuiConfigFlags_DockingEnable)
    {
        ImGuiID dockspaceId = ImGui::GetID("MyDockSpace");
        ImGui::DockSpace(dockspaceId, ImVec2(0.0f, 0.0f), dockspaceFlags);
    }

    // Menu bar
    if (ImGui::BeginMenuBar())
    {
        if (ImGui::BeginMenu("File"))
        {
            if (ImGui::MenuItem("Exit"))
            {
                // Could dispatch a window close event here
            }
            ImGui::EndMenu();
        }
        ImGui::EndMenuBar();
    }

    // Viewport window
    ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, ImVec2(0, 0));
    ImGui::Begin("Viewport");

    m_ViewportFocused = ImGui::IsWindowFocused();
    m_ViewportHovered = ImGui::IsWindowHovered();

    ImVec2 viewportPanelSize = ImGui::GetContentRegionAvail();
    if (m_ViewportWidth != viewportPanelSize.x || m_ViewportHeight != viewportPanelSize.y)
    {
        // Defer resize to next frame's OnRenderOffscreen
        m_PendingViewportWidth = viewportPanelSize.x;
        m_PendingViewportHeight = viewportPanelSize.y;
        m_NeedsResize = true;
    }

    // Display framebuffer texture using framebuffer's actual size
    if (m_ViewportFramebuffer && m_ViewportFramebuffer->GetImGuiTextureID())
    {
        float displayWidth = m_ViewportWidth > 0 ? m_ViewportWidth : static_cast<float>(m_ViewportFramebuffer->GetWidth());
        float displayHeight = m_ViewportHeight > 0 ? m_ViewportHeight : static_cast<float>(m_ViewportFramebuffer->GetHeight());
        ImGui::Image(
            m_ViewportFramebuffer->GetImGuiTextureID(),
            ImVec2(displayWidth, displayHeight));
    }

    ImGui::End();
    ImGui::PopStyleVar();

    ImGui::End(); // DockSpace
}

void EditorLayer::OnEvent(GGEngine::Event& event)
{
    // Handle events if needed
}
