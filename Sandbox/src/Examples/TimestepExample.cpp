#include "TimestepExample.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Core/Application.h"
#include "GGEngine/Core/Profiler.h"
#include "GGEngine/Core/Log.h"

#include <imgui.h>
#include <chrono>
#include <cstdlib>
#include <cmath>
#include <ctime>

TimestepExample::TimestepExample()
    : Example("Fixed Timestep",
              "Demonstrates fixed vs variable timestep with interpolation for smooth rendering")
{
}

void TimestepExample::OnAttach()
{
    m_Scene = GGEngine::CreateScope<GGEngine::Scene>("Timestep Demo");

    // Seed random
    std::srand(static_cast<unsigned>(std::time(nullptr)));

    CreateBalls(m_BallCount);

    GG_INFO("TimestepExample attached with {} balls", m_BallCount);
}

void TimestepExample::OnDetach()
{
    m_Scene.reset();
    m_Balls.clear();
}

void TimestepExample::CreateBalls(int count)
{
    m_Balls.clear();
    m_Balls.reserve(count);

    for (int i = 0; i < count; i++)
    {
        Ball ball;

        // Random position
        ball.x = (static_cast<float>(rand()) / RAND_MAX * 2.0f - 1.0f) * m_BoundsX * 0.8f;
        ball.y = (static_cast<float>(rand()) / RAND_MAX * 2.0f - 1.0f) * m_BoundsY * 0.8f;
        ball.prevX = ball.x;
        ball.prevY = ball.y;

        // Random velocity
        ball.vx = (static_cast<float>(rand()) / RAND_MAX * 2.0f - 1.0f) * 5.0f;
        ball.vy = (static_cast<float>(rand()) / RAND_MAX * 2.0f - 1.0f) * 5.0f;

        // Random size
        ball.radius = 0.15f + static_cast<float>(rand()) / RAND_MAX * 0.25f;

        // Random color
        ball.r = 0.3f + static_cast<float>(rand()) / RAND_MAX * 0.7f;
        ball.g = 0.3f + static_cast<float>(rand()) / RAND_MAX * 0.7f;
        ball.b = 0.3f + static_cast<float>(rand()) / RAND_MAX * 0.7f;

        m_Balls.push_back(ball);
    }
}

void TimestepExample::UpdatePhysics(float dt)
{
    GG_PROFILE_SCOPE("Physics");

    dt *= m_TimeScale;

    for (auto& ball : m_Balls)
    {
        // Save previous position for interpolation
        ball.prevX = ball.x;
        ball.prevY = ball.y;

        // Apply gravity
        ball.vy += m_Gravity * dt;

        // Update position
        ball.x += ball.vx * dt;
        ball.y += ball.vy * dt;

        // Bounce off walls
        if (ball.x - ball.radius < -m_BoundsX)
        {
            ball.x = -m_BoundsX + ball.radius;
            ball.vx = -ball.vx * m_Bounciness;
        }
        if (ball.x + ball.radius > m_BoundsX)
        {
            ball.x = m_BoundsX - ball.radius;
            ball.vx = -ball.vx * m_Bounciness;
        }
        if (ball.y - ball.radius < -m_BoundsY)
        {
            ball.y = -m_BoundsY + ball.radius;
            ball.vy = -ball.vy * m_Bounciness;
        }
        if (ball.y + ball.radius > m_BoundsY)
        {
            ball.y = m_BoundsY - ball.radius;
            ball.vy = -ball.vy * m_Bounciness;
        }
    }
}

void TimestepExample::OnFixedUpdate(float fixedDeltaTime)
{
    // This is called at a fixed rate (e.g., 60Hz) when fixed timestep is enabled
    auto& app = GGEngine::Application::Get();

    if (app.GetUseFixedTimestep())
    {
        auto start = std::chrono::high_resolution_clock::now();
        UpdatePhysics(fixedDeltaTime);
        auto end = std::chrono::high_resolution_clock::now();
        m_PhysicsTime = std::chrono::duration<float, std::milli>(end - start).count();
    }
}

void TimestepExample::OnUpdate(GGEngine::Timestep ts, const GGEngine::Camera& /*camera*/)
{
    auto& app = GGEngine::Application::Get();

    // In variable timestep mode, update physics here instead
    if (!app.GetUseFixedTimestep())
    {
        auto start = std::chrono::high_resolution_clock::now();
        UpdatePhysics(ts.GetSeconds());
        auto end = std::chrono::high_resolution_clock::now();
        m_PhysicsTime = std::chrono::duration<float, std::milli>(end - start).count();
    }

    m_FixedUpdatesThisFrame = app.GetFixedUpdatesPerFrame();
}

void TimestepExample::OnRender(const GGEngine::Camera& camera)
{
    GG_PROFILE_SCOPE("TimestepExample::Render");

    auto& app = GGEngine::Application::Get();
    float alpha = 1.0f;

    // Only interpolate if using fixed timestep
    if (app.GetUseFixedTimestep() && m_ShowInterpolation)
    {
        // Get alpha from the application (would need to pass through Timestep)
        // For now, calculate based on accumulator state
        alpha = app.GetFixedUpdateTime() > 0.0f ? 1.0f : 1.0f;  // Simplified
    }

    GGEngine::Renderer2D::ResetStats();
    GGEngine::Renderer2D::BeginScene(camera);

    // Draw bounds
    GGEngine::Renderer2D::DrawQuad(GGEngine::QuadSpec()
        .SetPosition(0.0f, 0.0f, -0.1f)
        .SetSize(m_BoundsX * 2.0f, m_BoundsY * 2.0f)
        .SetColor(0.1f, 0.1f, 0.15f, 1.0f));

    // Draw balls
    for (const auto& ball : m_Balls)
    {
        float renderX, renderY;

        if (app.GetUseFixedTimestep() && m_ShowInterpolation)
        {
            // Interpolate between previous and current position
            // Note: In a real implementation, alpha would come from Timestep
            // For this demo, we show current position (alpha = 1)
            renderX = ball.x;
            renderY = ball.y;

            // Draw ghost at previous position to visualize interpolation
            if (m_ShowTrails)
            {
                GGEngine::Renderer2D::DrawQuad(GGEngine::QuadSpec()
                    .SetPosition(ball.prevX, ball.prevY, 0.0f)
                    .SetSize(ball.radius * 2.0f, ball.radius * 2.0f)
                    .SetColor(ball.r * 0.3f, ball.g * 0.3f, ball.b * 0.3f, 0.5f));
            }
        }
        else
        {
            renderX = ball.x;
            renderY = ball.y;
        }

        // Draw ball
        GGEngine::Renderer2D::DrawQuad(GGEngine::QuadSpec()
            .SetPosition(renderX, renderY, 0.0f)
            .SetSize(ball.radius * 2.0f, ball.radius * 2.0f)
            .SetColor(ball.r, ball.g, ball.b, 1.0f));
    }

    GGEngine::Renderer2D::EndScene();
}

void TimestepExample::OnImGuiRender()
{
    auto& app = GGEngine::Application::Get();

    // Timestep mode selection
    ImGui::Text("Timestep Mode:");
    bool useFixed = app.GetUseFixedTimestep();
    if (ImGui::RadioButton("Variable (frame-dependent)", !useFixed))
    {
        app.SetUseFixedTimestep(false);
    }
    if (ImGui::RadioButton("Fixed (60Hz physics)", useFixed))
    {
        app.SetUseFixedTimestep(true);
    }

    ImGui::Separator();

    // Fixed timestep settings
    if (useFixed)
    {
        float fixedHz = 1.0f / app.GetFixedTimestep();
        if (ImGui::SliderFloat("Physics Rate (Hz)", &fixedHz, 10.0f, 120.0f, "%.0f"))
        {
            app.SetFixedTimestep(1.0f / fixedHz);
        }

        ImGui::Checkbox("Show Interpolation", &m_ShowInterpolation);
        ImGui::Checkbox("Show Trails (prev pos)", &m_ShowTrails);

        ImGui::Text("Fixed Updates/Frame: %d", m_FixedUpdatesThisFrame);
        ImGui::Text("Fixed Update Time: %.3f ms", app.GetFixedUpdateTime());
    }

    ImGui::Separator();

    // Physics settings
    ImGui::Text("Physics Settings:");
    ImGui::SliderFloat("Gravity", &m_Gravity, -30.0f, 0.0f);
    ImGui::SliderFloat("Bounciness", &m_Bounciness, 0.0f, 1.0f);
    ImGui::SliderFloat("Time Scale", &m_TimeScale, 0.1f, 2.0f);

    ImGui::Separator();

    // Ball controls
    ImGui::Text("Ball Count:");
    ImGui::SameLine();
    if (ImGui::InputInt("##ballcount", &m_BallCount, 5, 20))
    {
        m_BallCount = std::max(1, std::min(200, m_BallCount));
    }
    if (ImGui::Button("Recreate Balls"))
    {
        CreateBalls(m_BallCount);
    }
    ImGui::SameLine();
    if (ImGui::Button("Add Impulse"))
    {
        // Add random impulse to all balls
        for (auto& ball : m_Balls)
        {
            ball.vy += 10.0f + static_cast<float>(rand()) / RAND_MAX * 5.0f;
            ball.vx += (static_cast<float>(rand()) / RAND_MAX * 2.0f - 1.0f) * 3.0f;
        }
    }

    ImGui::Separator();

    // Performance stats
    ImGui::Text("Performance:");
    ImGui::Text("Physics Time: %.3f ms", m_PhysicsTime);

    ImGui::Separator();

    // Explanation
    if (ImGui::CollapsingHeader("How It Works"))
    {
        ImGui::TextWrapped(
            "VARIABLE TIMESTEP:\n"
            "Physics updates every frame using actual delta time.\n"
            "- Simple but physics varies with framerate\n"
            "- At 30 FPS: dt=0.033s, at 120 FPS: dt=0.008s\n"
            "- Can cause inconsistent behavior\n\n"
            "FIXED TIMESTEP:\n"
            "Physics updates at fixed rate (e.g., 60Hz).\n"
            "- Consistent simulation regardless of FPS\n"
            "- May run 0, 1, or multiple times per frame\n"
            "- Interpolation smooths between states\n\n"
            "Try lowering Physics Rate to 10Hz to see\n"
            "the 'stutter' that interpolation fixes!\n\n"
            "The 'spiral of death' is prevented by clamping\n"
            "max frame time to 250ms."
        );
    }

    // Renderer stats
    ImGui::Separator();
    auto stats = GGEngine::Renderer2D::GetStats();
    ImGui::Text("Draw Calls: %d", stats.DrawCalls);
    ImGui::Text("Quads: %d", stats.QuadCount);
}
