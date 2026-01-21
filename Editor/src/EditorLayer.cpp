#include "EditorLayer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Asset/AssetManager.h"
#include "GGEngine/Asset/Shader.h"
#include "GGEngine/Renderer/RenderCommand.h"
#include "GGEngine/ImGui/DebugUI.h"
#include "GGEngine/Input.h"
#include "GGEngine/KeyCodes.h"

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
    m_ViewportFramebuffer = GGEngine::CreateScope<GGEngine::Framebuffer>(spec);

    // Create camera uniform buffer
    m_CameraUniformBuffer = GGEngine::CreateScope<GGEngine::UniformBuffer>(sizeof(GGEngine::CameraUBO));

    // Create descriptor set layout for camera UBO
    std::vector<GGEngine::DescriptorBinding> bindings = {
        { 0, VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER, VK_SHADER_STAGE_VERTEX_BIT, 1 }
    };
    m_CameraDescriptorLayout = GGEngine::CreateScope<GGEngine::DescriptorSetLayout>(bindings);

    // Create descriptor set and bind the uniform buffer
    m_CameraDescriptorSet = GGEngine::CreateScope<GGEngine::DescriptorSet>(*m_CameraDescriptorLayout);
    m_CameraDescriptorSet->SetUniformBuffer(0, *m_CameraUniformBuffer);

    // Define vertex layout: position (vec3) + color (vec3)
    m_VertexLayout.Push("aPosition", GGEngine::VertexAttributeType::Float3)
                  .Push("aColor", GGEngine::VertexAttributeType::Float3);

    // Create triangle vertex data (pointing up, white base color)
    std::vector<Vertex> vertices = {
        {{ 0.0f,  0.5f, 0.0f}, {1.0f, 1.0f, 1.0f}},  // Top
        {{ 0.5f, -0.5f, 0.0f}, {1.0f, 1.0f, 1.0f}},  // Bottom right
        {{-0.5f, -0.5f, 0.0f}, {1.0f, 1.0f, 1.0f}}   // Bottom left
    };

    std::vector<uint32_t> indices = { 0, 1, 2 };

    // Create vertex and index buffers
    m_VertexBuffer = GGEngine::VertexBuffer::Create(vertices, m_VertexLayout);
    m_IndexBuffer = GGEngine::IndexBuffer::Create(indices);

    // Create 10x10 grid of quads with gradient colors
    {
        const int gridSize = 10;
        const float quadSize = 0.1f;
        const float spacing = 0.11f;  // Slight gap between quads
        const float offset = (gridSize - 1) * spacing * 0.5f;  // Center the grid

        std::vector<Vertex> quadVertices;
        std::vector<uint32_t> quadIndices;
        quadVertices.reserve(gridSize * gridSize * 4);  // 4 vertices per quad
        quadIndices.reserve(gridSize * gridSize * 6);   // 6 indices per quad (2 triangles)

        for (int y = 0; y < gridSize; y++)
        {
            for (int x = 0; x < gridSize; x++)
            {
                // Calculate quad center position
                float posX = x * spacing - offset;
                float posY = y * spacing - offset;

                // Gradient color: red from left to right, green from bottom to top
                float r = static_cast<float>(x) / (gridSize - 1);
                float g = static_cast<float>(y) / (gridSize - 1);
                float b = 0.5f;  // Fixed blue for visual interest

                float halfSize = quadSize * 0.5f;
                uint32_t baseIndex = static_cast<uint32_t>(quadVertices.size());

                // 4 vertices for this quad (all same color)
                quadVertices.push_back({{ posX - halfSize, posY - halfSize, 0.0f }, { r, g, b }});  // Bottom-left
                quadVertices.push_back({{ posX + halfSize, posY - halfSize, 0.0f }, { r, g, b }});  // Bottom-right
                quadVertices.push_back({{ posX + halfSize, posY + halfSize, 0.0f }, { r, g, b }});  // Top-right
                quadVertices.push_back({{ posX - halfSize, posY + halfSize, 0.0f }, { r, g, b }});  // Top-left

                // 6 indices for 2 triangles (counter-clockwise winding)
                quadIndices.push_back(baseIndex + 0);
                quadIndices.push_back(baseIndex + 1);
                quadIndices.push_back(baseIndex + 2);
                quadIndices.push_back(baseIndex + 2);
                quadIndices.push_back(baseIndex + 3);
                quadIndices.push_back(baseIndex + 0);
            }
        }

        m_QuadVertexBuffer = GGEngine::VertexBuffer::Create(quadVertices, m_VertexLayout);
        m_QuadIndexBuffer = GGEngine::IndexBuffer::Create(quadIndices);
    }

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
    m_QuadVertexBuffer.reset();
    m_QuadIndexBuffer.reset();
    m_ViewportFramebuffer.reset();
    GG_INFO("EditorLayer detached");
}

void EditorLayer::CreatePipeline()
{
    // Load shader using factory
    auto shaderHandle = GGEngine::Shader::Create("basic", "assets/shaders/compiled/basic");
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

    // Push constant for model matrix (mat4 = 64 bytes) in vertex shader
    GGEngine::PushConstantRange vertexPushConstant;
    vertexPushConstant.stageFlags = VK_SHADER_STAGE_VERTEX_BIT;
    vertexPushConstant.offset = 0;
    vertexPushConstant.size = sizeof(GGEngine::Mat4);
    pipelineSpec.pushConstantRanges.push_back(vertexPushConstant);

    // Push constant for color multiplier (vec4 = 16 bytes) in fragment shader
    GGEngine::PushConstantRange fragmentPushConstant;
    fragmentPushConstant.stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    fragmentPushConstant.offset = sizeof(GGEngine::Mat4);  // offset 64
    fragmentPushConstant.size = sizeof(float) * 4;
    pipelineSpec.pushConstantRanges.push_back(fragmentPushConstant);

    // Descriptor set layout for camera UBO
    pipelineSpec.descriptorSetLayouts.push_back(m_CameraDescriptorLayout->GetVkLayout());

    m_Pipeline = GGEngine::CreateScope<GGEngine::Pipeline>(pipelineSpec);
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

    // Render scene
    if (m_Pipeline && m_VertexBuffer && m_IndexBuffer && m_QuadVertexBuffer && m_QuadIndexBuffer)
    {
        m_Pipeline->Bind(cmd);

        // Bind camera descriptor set
        m_CameraDescriptorSet->Bind(cmd, m_Pipeline->GetLayout(), 0);

        GGEngine::RenderCommand::SetViewport(cmd, m_ViewportFramebuffer->GetWidth(), m_ViewportFramebuffer->GetHeight());
        GGEngine::RenderCommand::SetScissor(cmd, m_ViewportFramebuffer->GetWidth(), m_ViewportFramebuffer->GetHeight());

        // --- Render Grid First (background) ---
        // Grid positions are baked into vertices, so use identity transform
        GGEngine::Mat4 gridModelMatrix = GGEngine::Mat4::Identity();
        GGEngine::RenderCommand::PushConstants(
            cmd,
            m_Pipeline->GetLayout(),
            VK_SHADER_STAGE_VERTEX_BIT,
            gridModelMatrix);

        // White color multiplier for grid (show vertex colors as-is)
        float gridColor[4] = { 1.0f, 1.0f, 1.0f, 1.0f };
        GGEngine::RenderCommand::PushConstants(
            cmd,
            m_Pipeline->GetLayout(),
            VK_SHADER_STAGE_FRAGMENT_BIT,
            gridColor,
            sizeof(GGEngine::Mat4));  // offset = 64

        m_QuadVertexBuffer->Bind(cmd);
        m_QuadIndexBuffer->Bind(cmd);
        GGEngine::RenderCommand::DrawIndexed(cmd, m_QuadIndexBuffer->GetCount());

        // --- Render Triangle On Top ---
        // Build and push model matrix (Scale * Rotate * Translate order for TRS)
        float rotationRadians = m_Rotation * 3.14159265359f / 180.0f;
        GGEngine::Mat4 triangleModelMatrix = GGEngine::Mat4::Translate(m_Position[0], m_Position[1], m_Position[2])
                                           * GGEngine::Mat4::RotateZ(rotationRadians)
                                           * GGEngine::Mat4::Scale(m_Scale[0], m_Scale[1], m_Scale[2]);

        GGEngine::RenderCommand::PushConstants(
            cmd,
            m_Pipeline->GetLayout(),
            VK_SHADER_STAGE_VERTEX_BIT,
            triangleModelMatrix);

        // Push triangle color multiplier (offset 64, after model matrix)
        GGEngine::RenderCommand::PushConstants(
            cmd,
            m_Pipeline->GetLayout(),
            VK_SHADER_STAGE_FRAGMENT_BIT,
            m_ColorMultiplier,
            sizeof(GGEngine::Mat4));  // offset = 64

        m_VertexBuffer->Bind(cmd);
        m_IndexBuffer->Bind(cmd);
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

        // IJKL to move the triangle independently
        float velocity = m_TriangleMoveSpeed * ts;
        if (GGEngine::Input::IsKeyPressed(GG_KEY_I))
            m_Position[1] += velocity;
        if (GGEngine::Input::IsKeyPressed(GG_KEY_K))
            m_Position[1] -= velocity;
        if (GGEngine::Input::IsKeyPressed(GG_KEY_J))
            m_Position[0] -= velocity;
        if (GGEngine::Input::IsKeyPressed(GG_KEY_L))
            m_Position[0] += velocity;
    }

    // Properties panel
    ImGui::Begin("Properties");

    if (ImGui::CollapsingHeader("Camera", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::Text("Orthographic 2D Camera");
        ImGui::Text("WASD: Move camera");
        ImGui::Text("RMB drag: Pan, Scroll: Zoom");
    }

    if (ImGui::CollapsingHeader("Controls", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::Text("IJKL: Move triangle");
        ImGui::Text("(I=up, K=down, J=left, L=right)");
    }

    if (ImGui::CollapsingHeader("Transform", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::DragFloat3("Position", m_Position, 0.01f);
        ImGui::DragFloat("Rotation", &m_Rotation, 1.0f, -360.0f, 360.0f, "%.1f deg");
        ImGui::DragFloat3("Scale", m_Scale, 0.01f, 0.01f, 10.0f);
    }

    if (ImGui::CollapsingHeader("Rendering", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::ColorEdit4("Color Tint", m_ColorMultiplier);
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
