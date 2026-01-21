#pragma once

#include "GGEngine/Core/Layer.h"
#include "GGEngine/Core/Log.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Events/MouseEvent.h"
#include "GGEngine/Renderer/Framebuffer.h"
#include "GGEngine/Renderer/Pipeline.h"
#include "GGEngine/Renderer/VertexBuffer.h"
#include "GGEngine/Renderer/IndexBuffer.h"
#include "GGEngine/Renderer/VertexLayout.h"
#include "GGEngine/Renderer/UniformBuffer.h"
#include "GGEngine/Renderer/DescriptorSet.h"
#include "GGEngine/Renderer/OrthographicCameraController.h"
#include "GGEngine/Renderer/Camera.h"

#include <memory>

class EditorLayer : public GGEngine::Layer
{
public:
    EditorLayer();
    virtual ~EditorLayer() = default;

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate(GGEngine::Timestep ts) override;
    void OnRenderOffscreen(GGEngine::Timestep ts) override;
    void OnEvent(GGEngine::Event& event) override;

private:
    void CreatePipeline();

    std::unique_ptr<GGEngine::Framebuffer> m_ViewportFramebuffer;
    std::unique_ptr<GGEngine::Pipeline> m_Pipeline;
    std::unique_ptr<GGEngine::VertexBuffer> m_VertexBuffer;
    std::unique_ptr<GGEngine::IndexBuffer> m_IndexBuffer;
    GGEngine::VertexLayout m_VertexLayout;

    // Camera system
    GGEngine::OrthographicCameraController m_CameraController;
    std::unique_ptr<GGEngine::DescriptorSetLayout> m_CameraDescriptorLayout;
    std::unique_ptr<GGEngine::DescriptorSet> m_CameraDescriptorSet;
    std::unique_ptr<GGEngine::UniformBuffer> m_CameraUniformBuffer;

    float m_ViewportWidth = 0.0f;
    float m_ViewportHeight = 0.0f;
    float m_PendingViewportWidth = 0.0f;
    float m_PendingViewportHeight = 0.0f;
    bool m_ViewportFocused = false;
    bool m_ViewportHovered = false;
    bool m_NeedsResize = false;

    // Triangle transform
    float m_Position[3] = { 0.0f, 0.0f, 0.0f };
    float m_Rotation = 0.0f;  // Rotation around Z in degrees
    float m_Scale[3] = { 1.0f, 1.0f, 1.0f };

    // Color multiplier
    float m_ColorMultiplier[4] = { 1.0f, 1.0f, 1.0f, 1.0f };
};
