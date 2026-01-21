#include "EditorLayer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/ImGui/DebugUI.h"
#include "GGEngine/Input.h"
#include "GGEngine/KeyCodes.h"
#include "GGEngine/Asset/Texture.h"

#include <imgui.h>

EditorLayer::EditorLayer()
    : Layer("EditorLayer"), m_CameraController(1280.0f / 720.0f, 1.0f, true)
{
}

void EditorLayer::OnAttach()
{
    GGEngine::FramebufferSpecification spec;
    spec.Width = 1280;
    spec.Height = 720;
    m_ViewportFramebuffer = GGEngine::CreateScope<GGEngine::Framebuffer>(spec);

    GG_INFO("EditorLayer attached - using Renderer2D");
}

void EditorLayer::OnDetach()
{
    m_ViewportFramebuffer.reset();
    GG_INFO("EditorLayer detached");
}

void EditorLayer::OnRenderOffscreen(GGEngine::Timestep ts)
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

        // Update camera aspect ratio
        m_CameraController.SetAspectRatio(m_ViewportWidth / m_ViewportHeight);
    }

    auto& vkContext = GGEngine::VulkanContext::Get();
    VkCommandBuffer cmd = vkContext.GetCurrentCommandBuffer();

    if (cmd == VK_NULL_HANDLE)
        return;

    // Begin offscreen render pass
    m_ViewportFramebuffer->BeginRenderPass(cmd);

    // Begin Renderer2D scene with framebuffer's render pass
    GGEngine::Renderer2D::ResetStats();
    GGEngine::Renderer2D::BeginScene(
        m_CameraController.GetCamera(),
        m_ViewportFramebuffer->GetRenderPass(),
        cmd,
        m_ViewportFramebuffer->GetWidth(),
        m_ViewportFramebuffer->GetHeight());

    // Draw 10x10 grid of colored quads
    const int gridSize = 10;
    const float quadSize = 0.1f;
    const float spacing = 0.11f;
    const float offset = (gridSize - 1) * spacing * 0.5f;

    for (int y = 0; y < gridSize; y++)
    {
        for (int x = 0; x < gridSize; x++)
        {
            float posX = x * spacing - offset;
            float posY = y * spacing - offset;

            // Gradient color
            float r = static_cast<float>(x) / (gridSize - 1);
            float g = static_cast<float>(y) / (gridSize - 1);
            float b = 0.5f;

            GGEngine::Renderer2D::DrawQuad(posX, posY, quadSize, quadSize, r, g, b);
        }
    }

    // Draw movable/scalable quad (z=0.0f specified to avoid overload ambiguity)
    float rotationRadians = m_Rotation * 3.14159265359f / 180.0f;
    GGEngine::Renderer2D::DrawRotatedQuad(
        m_Position[0], m_Position[1], 0.0f,
        m_Scale[0], m_Scale[1],
        rotationRadians,
        m_Color[0], m_Color[1], m_Color[2], m_Color[3]);

    // Draw textured quad
    GGEngine::Renderer2D::DrawQuad(1.5f, 0.0f, 1.0f, 1.0f,
        GGEngine::Texture::GetFallbackPtr());

    GGEngine::Renderer2D::EndScene();

    m_ViewportFramebuffer->EndRenderPass(cmd);
}

void EditorLayer::OnUpdate(GGEngine::Timestep ts)
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

    // Display framebuffer texture
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

    // Update camera controller when viewport is hovered
    if (m_ViewportHovered)
    {
        m_CameraController.OnUpdate(ts);

        // IJKL to move the quad
        float velocity = m_MoveSpeed * ts;
        if (GGEngine::Input::IsKeyPressed(GG_KEY_I))
            m_Position[1] += velocity;
        if (GGEngine::Input::IsKeyPressed(GG_KEY_K))
            m_Position[1] -= velocity;
        if (GGEngine::Input::IsKeyPressed(GG_KEY_J))
            m_Position[0] -= velocity;
        if (GGEngine::Input::IsKeyPressed(GG_KEY_L))
            m_Position[0] += velocity;

        // U/O to rotate the quad
        float rotationSpeed = 90.0f * ts;  // Degrees per second
        if (GGEngine::Input::IsKeyPressed(GG_KEY_U))
            m_Rotation += rotationSpeed;
        if (GGEngine::Input::IsKeyPressed(GG_KEY_O))
            m_Rotation -= rotationSpeed;
    }

    // Properties panel
    ImGui::Begin("Properties");

    if (ImGui::CollapsingHeader("Camera", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::Text("Orthographic 2D Camera");
        ImGui::Text("WASD: Move, Q/E: Rotate");
        ImGui::Text("RMB drag: Pan, Scroll: Zoom");
    }

    if (ImGui::CollapsingHeader("Controls", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::Text("IJKL: Move quad, U/O: Rotate");
    }

    if (ImGui::CollapsingHeader("Transform", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::DragFloat3("Position", m_Position, 0.01f);
        ImGui::DragFloat("Rotation", &m_Rotation, 1.0f, -360.0f, 360.0f, "%.1f deg");
        ImGui::DragFloat2("Scale", m_Scale, 0.01f, 0.01f, 10.0f);
    }

    if (ImGui::CollapsingHeader("Rendering", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::ColorEdit4("Color", m_Color);
    }

    if (ImGui::CollapsingHeader("Stats", ImGuiTreeNodeFlags_DefaultOpen))
    {
        auto stats = GGEngine::Renderer2D::GetStats();
        ImGui::Text("Renderer2D Stats:");
        ImGui::Text("  Draw Calls: %d", stats.DrawCalls);
        ImGui::Text("  Quads: %d", stats.QuadCount);
        ImGui::Separator();
        GGEngine::DebugUI::ShowStatsContent(ts);
    }

    ImGui::End();

    ImGui::End(); // DockSpace
}

void EditorLayer::OnEvent(GGEngine::Event& event)
{
    if (m_ViewportHovered)
    {
        m_CameraController.OnEvent(event);
    }
}

void EditorLayer::OnWindowResize(uint32_t width, uint32_t height)
{
    // Editor uses ImGui viewport sizing for the framebuffer camera,
    // so we don't need to update aspect ratio here.
    // The viewport resize is handled in OnRenderOffscreen via m_NeedsResize.
}
