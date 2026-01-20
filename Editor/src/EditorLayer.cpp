#include "EditorLayer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Asset/AssetManager.h"
#include "GGEngine/Asset/ShaderLibrary.h"
#include "GGEngine/Renderer/RenderCommand.h"
#include "GGEngine/ImGui/DebugUI.h"

#include <imgui.h>
#include <vulkan/vulkan.h>

namespace {
    struct Vertex
    {
        float position[3];
        float color[3];
    };
}

EditorLayer::EditorLayer()
    : Layer("EditorLayer"), m_CameraController(1280.0f / 720.0f, 1.0f)
{
}

void EditorLayer::OnAttach()
{
    GGEngine::FramebufferSpecification spec;
    spec.Width = 1280;
    spec.Height = 720;
    m_ViewportFramebuffer = std::make_unique<GGEngine::Framebuffer>(spec);

    // Create camera uniform buffer
    m_CameraUniformBuffer = std::make_unique<GGEngine::UniformBuffer>(sizeof(GGEngine::CameraUBO));

    // Create descriptor set layout for camera UBO
    std::vector<GGEngine::DescriptorBinding> bindings = {
        { 0, VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER, VK_SHADER_STAGE_VERTEX_BIT, 1 }
    };
    m_CameraDescriptorLayout = std::make_unique<GGEngine::DescriptorSetLayout>(bindings);

    // Create descriptor set and bind the uniform buffer
    m_CameraDescriptorSet = std::make_unique<GGEngine::DescriptorSet>(*m_CameraDescriptorLayout);
    m_CameraDescriptorSet->SetUniformBuffer(0, *m_CameraUniformBuffer);

    // Define vertex layout: position (vec3) + color (vec3)
    m_VertexLayout.Push("aPosition", GGEngine::VertexAttributeType::Float3)
                  .Push("aColor", GGEngine::VertexAttributeType::Float3);

    // Create triangle vertex data (pointing up)
    std::vector<Vertex> vertices = {
        {{ 0.0f,  0.5f, 0.0f}, {1.0f, 0.0f, 0.0f}},  // Top (point) - Red
        {{ 0.5f, -0.5f, 0.0f}, {0.0f, 1.0f, 0.0f}},  // Bottom right - Green
        {{-0.5f, -0.5f, 0.0f}, {0.0f, 0.0f, 1.0f}}   // Bottom left - Blue
    };

    std::vector<uint32_t> indices = { 0, 1, 2 };

    // Create vertex and index buffers
    m_VertexBuffer = GGEngine::VertexBuffer::Create(vertices, m_VertexLayout);
    m_IndexBuffer = GGEngine::IndexBuffer::Create(indices);

    // Create pipeline with vertex layout
    CreatePipeline();

    GG_INFO("EditorLayer attached - with camera system");
}

void EditorLayer::OnDetach()
{
    m_Pipeline.reset();
    m_CameraDescriptorSet.reset();
    m_CameraDescriptorLayout.reset();
    m_CameraUniformBuffer.reset();
    m_VertexBuffer.reset();
    m_IndexBuffer.reset();
    m_ViewportFramebuffer.reset();
    GG_INFO("EditorLayer detached");
}

void EditorLayer::CreatePipeline()
{
    // Load shader that accepts vertex input
    auto shaderHandle = GGEngine::ShaderLibrary::Get().Load("basic", "assets/shaders/compiled/basic");
    if (!shaderHandle)
    {
        GG_ERROR("Failed to load basic shader!");
        return;
    }

    GGEngine::PipelineSpecification pipelineSpec;
    pipelineSpec.shader = shaderHandle.Get();
    pipelineSpec.renderPass = m_ViewportFramebuffer->GetRenderPass();
    pipelineSpec.vertexLayout = &m_VertexLayout;
    pipelineSpec.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
    pipelineSpec.cullMode = VK_CULL_MODE_NONE;
    pipelineSpec.debugName = "basic_triangle";

    // Push constant for color multiplier (vec4 = 16 bytes)
    GGEngine::PushConstantRange pushConstantRange;
    pushConstantRange.stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    pushConstantRange.offset = 0;
    pushConstantRange.size = sizeof(float) * 4;
    pipelineSpec.pushConstantRanges.push_back(pushConstantRange);

    // Descriptor set layout for camera UBO
    pipelineSpec.descriptorSetLayouts.push_back(m_CameraDescriptorLayout->GetVkLayout());

    m_Pipeline = std::make_unique<GGEngine::Pipeline>(pipelineSpec);
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

    // Update camera uniform buffer
    GGEngine::CameraUBO cameraUBO = m_CameraController.GetCamera().GetUBO();
    m_CameraUniformBuffer->SetData(cameraUBO);

    // Render triangle using vertex buffers
    if (m_Pipeline && m_VertexBuffer && m_IndexBuffer)
    {
        m_Pipeline->Bind(cmd);

        // Bind camera descriptor set
        m_CameraDescriptorSet->Bind(cmd, m_Pipeline->GetLayout(), 0);

        // Push color multiplier constant
        GGEngine::RenderCommand::PushConstants(
            cmd,
            m_Pipeline->GetLayout(),
            VK_SHADER_STAGE_FRAGMENT_BIT,
            m_ColorMultiplier);

        GGEngine::RenderCommand::SetViewport(cmd, m_ViewportFramebuffer->GetWidth(), m_ViewportFramebuffer->GetHeight());
        GGEngine::RenderCommand::SetScissor(cmd, m_ViewportFramebuffer->GetWidth(), m_ViewportFramebuffer->GetHeight());

        // Bind vertex and index buffers
        m_VertexBuffer->Bind(cmd);
        m_IndexBuffer->Bind(cmd);

        // Draw indexed
        GGEngine::RenderCommand::DrawIndexed(cmd, m_IndexBuffer->GetCount());
    }

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

    // Update camera controller when viewport is hovered
    if (m_ViewportHovered)
    {
        m_CameraController.OnUpdate(ts);
    }

    // Properties panel
    ImGui::Begin("Properties");

    if (ImGui::CollapsingHeader("Camera", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::Text("Orthographic 2D Camera");
        ImGui::Text("Controls: RMB drag to pan, scroll to zoom");
    }

    if (ImGui::CollapsingHeader("Rendering", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::ColorEdit4("Color", m_ColorMultiplier);
    }

    if (ImGui::CollapsingHeader("Stats", ImGuiTreeNodeFlags_DefaultOpen))
    {
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
