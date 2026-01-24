#pragma once

#include "Example.h"
#include "GGEngine/ECS/Scene.h"
#include "GGEngine/ECS/Components/InterpolationComponent.h"

#include <vector>

// Demonstrates fixed vs variable timestep and interpolation
// Shows how fixed timestep provides consistent physics while
// interpolation keeps rendering smooth
class TimestepExample : public Example
{
public:
    TimestepExample();

    void OnAttach() override;
    void OnDetach() override;
    void OnFixedUpdate(float fixedDeltaTime) override;
    void OnUpdate(GGEngine::Timestep ts, const GGEngine::Camera& camera) override;
    void OnRender(const GGEngine::Camera& camera) override;
    void OnImGuiRender() override;

private:
    void CreateBalls(int count);
    void UpdatePhysics(float dt);  // Physics simulation

    GGEngine::Scope<GGEngine::Scene> m_Scene;

    // Ball physics state (simple bouncing balls)
    struct Ball
    {
        float x, y;           // Position
        float prevX, prevY;   // Previous position (for interpolation)
        float vx, vy;         // Velocity
        float radius;
        float r, g, b;        // Color
    };
    std::vector<Ball> m_Balls;

    // Settings
    int m_BallCount = 20;
    float m_Gravity = -15.0f;
    float m_Bounciness = 0.8f;
    float m_BoundsX = 7.5f;
    float m_BoundsY = 4.5f;

    // Timing stats
    float m_PhysicsTime = 0.0f;
    int m_FixedUpdatesThisFrame = 0;

    // Visualization
    bool m_ShowInterpolation = true;
    bool m_ShowTrails = false;
    float m_TimeScale = 1.0f;  // Slow-mo for visualization
};
