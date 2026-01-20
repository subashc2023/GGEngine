#pragma once

#include "GGEngine/Layer.h"
#include "GGEngine/Log.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Renderer/Framebuffer.h"

#include <memory>

class EditorLayer : public GGEngine::Layer
{
public:
    EditorLayer();
    virtual ~EditorLayer() = default;

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate() override;
    void OnRenderOffscreen() override;
    void OnEvent(GGEngine::Event& event) override;

private:
    std::unique_ptr<GGEngine::Framebuffer> m_ViewportFramebuffer;
    float m_ViewportWidth = 0.0f;
    float m_ViewportHeight = 0.0f;
    float m_PendingViewportWidth = 0.0f;
    float m_PendingViewportHeight = 0.0f;
    bool m_ViewportFocused = false;
    bool m_ViewportHovered = false;
    bool m_NeedsResize = false;
};
