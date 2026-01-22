#pragma once

#include "GGEngine/Core/Layer.h"
#include "GGEngine/Renderer/OrthographicCameraController.h"
#include "Examples/Example.h"

#include <vector>
#include <memory>

// Main layer that manages and switches between examples
class ExamplesLayer : public GGEngine::Layer
{
public:
    ExamplesLayer();
    virtual ~ExamplesLayer() = default;

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate(GGEngine::Timestep ts) override;
    void OnEvent(GGEngine::Event& event) override;
    void OnWindowResize(uint32_t width, uint32_t height) override;

private:
    void SwitchExample(int index);

    // Camera for all examples
    GGEngine::OrthographicCameraController m_CameraController;

    // Example management
    std::vector<std::unique_ptr<Example>> m_Examples;
    int m_CurrentExampleIndex = 0;
    Example* m_CurrentExample = nullptr;
};
