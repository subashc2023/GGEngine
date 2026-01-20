#include "TriangleLayer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Asset/ShaderLibrary.h"
#include "GGEngine/Renderer/RenderCommand.h"
#include "GGEngine/ImGui/DebugUI.h"

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

    // Load shader
    auto shaderHandle = GGEngine::ShaderLibrary::Get().Load("basic", "assets/shaders/compiled/basic");
    if (!shaderHandle)
    {
        GG_ERROR("Failed to load basic shader!");
        return;
    }

    // Create pipeline
    GGEngine::PipelineSpecification pipelineSpec;
    pipelineSpec.shader = shaderHandle.Get();
    pipelineSpec.renderPass = GGEngine::VulkanContext::Get().GetRenderPass();
    pipelineSpec.vertexLayout = &m_VertexLayout;
    pipelineSpec.topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
    pipelineSpec.cullMode = VK_CULL_MODE_NONE;
    pipelineSpec.debugName = "sandbox_triangle";

    // Push constant for model matrix (vertex shader)
    GGEngine::PushConstantRange vertexPushConstant;
    vertexPushConstant.stageFlags = VK_SHADER_STAGE_VERTEX_BIT;
    vertexPushConstant.offset = 0;
    vertexPushConstant.size = sizeof(GGEngine::Mat4);
    pipelineSpec.pushConstantRanges.push_back(vertexPushConstant);

    // Push constant for color multiplier (fragment shader)
    GGEngine::PushConstantRange fragmentPushConstant;
    fragmentPushConstant.stageFlags = VK_SHADER_STAGE_FRAGMENT_BIT;
    fragmentPushConstant.offset = sizeof(GGEngine::Mat4);  // offset 64
    fragmentPushConstant.size = sizeof(float) * 4;
    pipelineSpec.pushConstantRanges.push_back(fragmentPushConstant);

    // Descriptor set layout for camera UBO
    pipelineSpec.descriptorSetLayouts.push_back(m_CameraDescriptorLayout->GetVkLayout());

    m_Pipeline = std::make_unique<GGEngine::Pipeline>(pipelineSpec);

    GG_INFO("TriangleLayer attached - using new rendering system");
}

void TriangleLayer::OnDetach()
{
    m_Pipeline.reset();
    m_CameraDescriptorSet.reset();
    m_CameraDescriptorLayout.reset();
    m_CameraUniformBuffer.reset();
    m_VertexBuffer.reset();
    m_IndexBuffer.reset();
    GG_INFO("TriangleLayer detached");
}

void TriangleLayer::OnUpdate(GGEngine::Timestep ts)
{
    // Update camera controller
    m_CameraController.OnUpdate(ts);

    auto& vkContext = GGEngine::VulkanContext::Get();
    VkCommandBuffer cmd = vkContext.GetCurrentCommandBuffer();

    if (cmd == VK_NULL_HANDLE || !m_Pipeline)
        return;

    // Update camera uniform buffer
    GGEngine::CameraUBO cameraUBO = m_CameraController.GetCamera().GetUBO();
    m_CameraUniformBuffer->SetData(cameraUBO);

    // Bind pipeline
    m_Pipeline->Bind(cmd);

    // Bind camera descriptor set
    m_CameraDescriptorSet->Bind(cmd, m_Pipeline->GetLayout(), 0);

    // Push identity model matrix (no transform)
    GGEngine::Mat4 modelMatrix = GGEngine::Mat4::Identity();
    GGEngine::RenderCommand::PushConstants(
        cmd,
        m_Pipeline->GetLayout(),
        VK_SHADER_STAGE_VERTEX_BIT,
        modelMatrix);

    // Push color multiplier (offset 64, after model matrix)
    GGEngine::RenderCommand::PushConstants(
        cmd,
        m_Pipeline->GetLayout(),
        VK_SHADER_STAGE_FRAGMENT_BIT,
        m_ColorMultiplier,
        sizeof(GGEngine::Mat4));  // offset = 64

    // Set viewport and scissor
    VkExtent2D extent = vkContext.GetSwapchainExtent();
    GGEngine::RenderCommand::SetViewport(cmd, extent.width, extent.height);
    GGEngine::RenderCommand::SetScissor(cmd, extent.width, extent.height);

    // Bind vertex and index buffers
    m_VertexBuffer->Bind(cmd);
    m_IndexBuffer->Bind(cmd);

    // Draw
    GGEngine::RenderCommand::DrawIndexed(cmd, m_IndexBuffer->GetCount());

    // Debug panel with color picker and stats
    ImGui::Begin("Debug");
    ImGui::ColorEdit4("Color Tint", m_ColorMultiplier);
    ImGui::Separator();
    GGEngine::DebugUI::ShowStatsContent(ts);
    ImGui::End();
}

void TriangleLayer::OnEvent(GGEngine::Event& event)
{
    m_CameraController.OnEvent(event);
}
