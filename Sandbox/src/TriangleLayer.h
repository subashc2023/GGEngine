#pragma once

#include "GGEngine/Layer.h"
#include "GGEngine/Log.h"
#include "GGEngine/Events/Event.h"

class TriangleLayer : public GGEngine::Layer
{
public:
    TriangleLayer();
    virtual ~TriangleLayer() = default;

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate() override;
    void OnEvent(GGEngine::Event& event) override;
};
