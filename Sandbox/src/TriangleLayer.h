#pragma once

#include "GGEngine/Layer.h"
#include "GGEngine/Log.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Events/MouseEvent.h"
#include "GGEngine/Renderer/Pipeline.h"
#include "GGEngine/Renderer/VertexBuffer.h"
#include "GGEngine/Renderer/IndexBuffer.h"
#include "GGEngine/Renderer/VertexLayout.h"
#include "GGEngine/Renderer/UniformBuffer.h"
#include "GGEngine/Renderer/DescriptorSet.h"
#include "GGEngine/Renderer/OrthographicCameraController.h"
#include "GGEngine/Renderer/Camera.h"

#include <memory>

class TriangleLayer : public GGEngine::Layer
{
public:
    TriangleLayer();
    virtual ~TriangleLayer() = default;

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate(GGEngine::Timestep ts) override;
    void OnEvent(GGEngine::Event& event) override;

private:
    std::unique_ptr<GGEngine::Pipeline> m_Pipeline;
    std::unique_ptr<GGEngine::VertexBuffer> m_VertexBuffer;
    std::unique_ptr<GGEngine::IndexBuffer> m_IndexBuffer;
    GGEngine::VertexLayout m_VertexLayout;

    // Camera system
    GGEngine::OrthographicCameraController m_CameraController;
    std::unique_ptr<GGEngine::DescriptorSetLayout> m_CameraDescriptorLayout;
    std::unique_ptr<GGEngine::DescriptorSet> m_CameraDescriptorSet;
    std::unique_ptr<GGEngine::UniformBuffer> m_CameraUniformBuffer;

    // Color multiplier
    float m_ColorMultiplier[4] = { 1.0f, 1.0f, 1.0f, 1.0f };
};
