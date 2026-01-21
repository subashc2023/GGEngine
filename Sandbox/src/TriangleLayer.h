#pragma once

#include "GGEngine/Core/Layer.h"
#include "GGEngine/Core/Log.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Renderer/OrthographicCameraController.h"
#include "GGEngine/Renderer/Camera.h"
#include "GGEngine/Asset/Texture.h"

class TriangleLayer : public GGEngine::Layer
{
public:
    TriangleLayer();
    virtual ~TriangleLayer() = default;

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate(GGEngine::Timestep ts) override;
    void OnEvent(GGEngine::Event& event) override;
    void OnWindowResize(uint32_t width, uint32_t height) override;

private:
    // Camera system
    GGEngine::OrthographicCameraController m_CameraController;

    // Triangle transform
    float m_Position[3] = { 0.0f, 0.0f, 0.0f };
    float m_TriangleMoveSpeed = 2.0f;
    float m_Rotation = 0.0f;

    // Color
    float m_Color[4] = { 1.0f, 1.0f, 1.0f, 1.0f };

    // Texture (optional demo)
    GGEngine::AssetHandle<GGEngine::Texture> m_Texture;
};
