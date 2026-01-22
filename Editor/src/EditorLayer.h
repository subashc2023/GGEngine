#pragma once

#include "GGEngine/Core/Layer.h"
#include "GGEngine/Core/Log.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Renderer/Framebuffer.h"
#include "GGEngine/Renderer/OrthographicCameraController.h"
#include "GGEngine/Renderer/Camera.h"
#include "GGEngine/ECS/Scene.h"
#include "GGEngine/ECS/Entity.h"

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
    // UI Panels
    void DrawSceneHierarchyPanel();
    void DrawPropertiesPanel(GGEngine::Timestep ts);

    // Scene management
    void CreateDefaultScene();
    void NewScene();
    void OpenScene();
    void SaveScene();
    void SaveSceneAs();

    GGEngine::Scope<GGEngine::Framebuffer> m_ViewportFramebuffer;

    // Camera system
    GGEngine::OrthographicCameraController m_CameraController;

    // Viewport state
    float m_ViewportWidth = 0.0f;
    float m_ViewportHeight = 0.0f;
    float m_PendingViewportWidth = 0.0f;
    float m_PendingViewportHeight = 0.0f;
    bool m_ViewportFocused = false;
    bool m_ViewportHovered = false;
    bool m_NeedsResize = false;

    // Scene
    GGEngine::Scope<GGEngine::Scene> m_ActiveScene;
    std::string m_CurrentScenePath;

    // Selection state
    GGEngine::EntityID m_SelectedEntity = GGEngine::InvalidEntityID;
};
