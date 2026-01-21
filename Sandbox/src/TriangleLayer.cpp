#include "TriangleLayer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Asset/Shader.h"
#include "GGEngine/Renderer/RenderCommand.h"
#include "GGEngine/ImGui/DebugUI.h"
#include "GGEngine/Input.h"
#include "GGEngine/KeyCodes.h"

#include <imgui.h>
#include <vulkan/vulkan.h>
#include <vector>

namespace {
    struct Vertex
    {
        float position[3];
        float color[3];
    };
}

TriangleLayer::TriangleLayer()
    : Layer("TriangleLayer"), m_CameraController(1280.0f / 720.0f, 1.0f)
{
}

void TriangleLayer::OnAttach()
{
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

    // Load shader using factory
    auto shaderHandle = GGEngine::Shader::Create("basic", "assets/shaders/compiled/basic");
    if (!shaderHandle)
    {
        GG_ERROR("Failed to load basic shader!");
        return;
    }

    // Create material with properties
    m_Material = GGEngine::CreateScope<GGEngine::Material>();

    // Register push constant properties (must match shader layout)
    m_Material->RegisterProperty("model", GGEngine::PropertyType::Mat4, VK_SHADER_STAGE_VERTEX_BIT, 0);
    m_Material->RegisterProperty("colorMultiplier", GGEngine::PropertyType::Vec4, VK_SHADER_STAGE_FRAGMENT_BIT, 64);

    // Create material specification
    GGEngine::MaterialSpecification materialSpec;
    materialSpec.shader = shaderHandle.Get();
    materialSpec.renderPass = GGEngine::VulkanContext::Get().GetRenderPass();
    materialSpec.vertexLayout = &m_VertexLayout;
    materialSpec.cullMode = VK_CULL_MODE_NONE;
    materialSpec.descriptorSetLayouts.push_back(m_CameraDescriptorLayout->GetVkLayout());
    materialSpec.name = "sandbox_triangle";

    m_Material->Create(materialSpec);

    GG_INFO("TriangleLayer attached - using Material system");
}

void TriangleLayer::OnDetach()
{
    m_Material.reset();
    m_CameraDescriptorSet.reset();
    m_CameraDescriptorLayout.reset();
    m_CameraUniformBuffer.reset();
    m_VertexBuffer.reset();
    m_IndexBuffer.reset();
    m_QuadVertexBuffer.reset();
    m_QuadIndexBuffer.reset();
    GG_INFO("TriangleLayer detached");
}

void TriangleLayer::OnUpdate(GGEngine::Timestep ts)
{
    // Update camera controller
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

    auto& vkContext = GGEngine::VulkanContext::Get();
    VkCommandBuffer cmd = vkContext.GetCurrentCommandBuffer();

    if (cmd == VK_NULL_HANDLE || !m_Material || !m_QuadVertexBuffer || !m_QuadIndexBuffer)
        return;

    // Update camera uniform buffer
    GGEngine::CameraUBO cameraUBO = m_CameraController.GetCamera().GetUBO();
    m_CameraUniformBuffer->SetData(cameraUBO);

    // Bind camera descriptor set (global, shared across materials)
    m_CameraDescriptorSet->Bind(cmd, m_Material->GetPipelineLayout(), 0);

    // Set viewport and scissor
    VkExtent2D extent = vkContext.GetSwapchainExtent();
    GGEngine::RenderCommand::SetViewport(cmd, extent.width, extent.height);
    GGEngine::RenderCommand::SetScissor(cmd, extent.width, extent.height);

    // --- Render Grid First (background) ---
    m_Material->SetMat4("model", GGEngine::Mat4::Identity());
    m_Material->SetVec4("colorMultiplier", 1.0f, 1.0f, 1.0f, 1.0f);
    m_Material->Bind(cmd);  // Binds pipeline and pushes all constants

    m_QuadVertexBuffer->Bind(cmd);
    m_QuadIndexBuffer->Bind(cmd);
    GGEngine::RenderCommand::DrawIndexed(cmd, m_QuadIndexBuffer->GetCount());

    // --- Render Triangle On Top ---
    GGEngine::Mat4 triangleModelMatrix = GGEngine::Mat4::Translate(m_Position[0], m_Position[1], m_Position[2]);
    m_Material->SetMat4("model", triangleModelMatrix);
    m_Material->SetVec4("colorMultiplier", m_ColorMultiplier);
    m_Material->Bind(cmd);  // Binds pipeline and pushes all constants

    m_VertexBuffer->Bind(cmd);
    m_IndexBuffer->Bind(cmd);
    GGEngine::RenderCommand::DrawIndexed(cmd, m_IndexBuffer->GetCount());

    // Debug panel
    ImGui::Begin("Debug");
    ImGui::Text("Camera: WASD + RMB drag + Scroll");
    ImGui::Text("Triangle: IJKL");
    ImGui::Separator();
    ImGui::DragFloat3("Position", m_Position, 0.01f);
    ImGui::ColorEdit4("Color Tint", m_ColorMultiplier);
    ImGui::Separator();
    GGEngine::DebugUI::ShowStatsContent(ts);
    ImGui::End();
}

void TriangleLayer::OnEvent(GGEngine::Event& event)
{
    m_CameraController.OnEvent(event);
}
