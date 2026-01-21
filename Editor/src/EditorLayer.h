#pragma once

#include "GGEngine/Core/Layer.h"
#include "GGEngine/Core/Log.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Renderer/Framebuffer.h"
#include "GGEngine/Renderer/OrthographicCameraController.h"
#include "GGEngine/Renderer/Camera.h"

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
    void OnWindowResize(uint32_t width, uint32_t height) override;

private:
    GGEngine::Scope<GGEngine::Framebuffer> m_ViewportFramebuffer;

    // Camera system
    GGEngine::OrthographicCameraController m_CameraController;

    float m_ViewportWidth = 0.0f;
    float m_ViewportHeight = 0.0f;
    float m_PendingViewportWidth = 0.0f;
    float m_PendingViewportHeight = 0.0f;
    bool m_ViewportFocused = false;
    bool m_ViewportHovered = false;
    bool m_NeedsResize = false;

    // Object transform
    float m_Position[3] = { 0.0f, 0.0f, 0.0f };
    float m_Rotation = 0.0f;
    float m_Scale[2] = { 1.0f, 1.0f };
    float m_MoveSpeed = 2.0f;

    // Color
    float m_Color[4] = { 1.0f, 1.0f, 1.0f, 1.0f };
};
