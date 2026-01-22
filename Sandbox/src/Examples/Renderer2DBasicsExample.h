#pragma once

#include "Example.h"
#include <glm/glm.hpp>

// Demonstrates basic Renderer2D API:
// - Colored quads
// - Rotated quads
// - Matrix-based rendering (TransformComponent::GetMatrix())
class Renderer2DBasicsExample : public Example
{
public:
    Renderer2DBasicsExample();

    void OnAttach() override;
    void OnUpdate(GGEngine::Timestep ts, const GGEngine::Camera& camera) override;
    void OnRender(const GGEngine::Camera& camera) override;
    void OnImGuiRender() override;

private:
    // Animated quad properties
    float m_QuadPosition[3] = { 0.0f, 0.0f, 0.0f };
    float m_QuadRotation = 0.0f;
    float m_QuadScale[2] = { 1.0f, 1.0f };
    float m_QuadColor[4] = { 0.2f, 0.8f, 0.3f, 1.0f };

    // Animation state
    float m_Time = 0.0f;
    bool m_AnimatePosition = true;
    bool m_AnimateRotation = true;
    bool m_AnimateScale = false;

    // Demo mode
    int m_DemoMode = 0;  // 0=basic, 1=grid, 2=matrix
};
