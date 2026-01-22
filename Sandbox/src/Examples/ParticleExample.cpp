#include "ParticleExample.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Core/Input.h"
#include "GGEngine/Core/KeyCodes.h"
#include "GGEngine/Core/MouseButtonCodes.h"
#include "GGEngine/Core/Application.h"
#include "GGEngine/ParticleSystem/Random.h"

#include <imgui.h>

ParticleExample::ParticleExample()
    : Example("Particle System",
              "Demonstrates particle emission, properties, and presets")
{
}

void ParticleExample::OnAttach()
{
    GGEngine::Random::Init();
    SetPreset(0);  // Fire preset
}

void ParticleExample::SetPreset(int preset)
{
    m_CurrentPreset = preset;

    switch (preset)
    {
        case 0:  // Fire
            m_ParticleProps.ColorBegin[0] = 1.0f;
            m_ParticleProps.ColorBegin[1] = 0.8f;
            m_ParticleProps.ColorBegin[2] = 0.0f;
            m_ParticleProps.ColorBegin[3] = 1.0f;
            m_ParticleProps.ColorEnd[0] = 1.0f;
            m_ParticleProps.ColorEnd[1] = 0.0f;
            m_ParticleProps.ColorEnd[2] = 0.0f;
            m_ParticleProps.ColorEnd[3] = 0.0f;
            m_ParticleProps.SizeBegin = 0.5f;
            m_ParticleProps.SizeEnd = 0.0f;
            m_ParticleProps.SizeVariation = 0.3f;
            m_ParticleProps.Velocity[0] = 0.0f;
            m_ParticleProps.Velocity[1] = 1.0f;
            m_ParticleProps.VelocityVariation[0] = 1.5f;
            m_ParticleProps.VelocityVariation[1] = 0.5f;
            m_ParticleProps.LifeTime = 1.0f;
            m_EmitRate = 5;
            break;

        case 1:  // Smoke
            m_ParticleProps.ColorBegin[0] = 0.5f;
            m_ParticleProps.ColorBegin[1] = 0.5f;
            m_ParticleProps.ColorBegin[2] = 0.5f;
            m_ParticleProps.ColorBegin[3] = 0.8f;
            m_ParticleProps.ColorEnd[0] = 0.2f;
            m_ParticleProps.ColorEnd[1] = 0.2f;
            m_ParticleProps.ColorEnd[2] = 0.2f;
            m_ParticleProps.ColorEnd[3] = 0.0f;
            m_ParticleProps.SizeBegin = 0.3f;
            m_ParticleProps.SizeEnd = 1.5f;
            m_ParticleProps.SizeVariation = 0.2f;
            m_ParticleProps.Velocity[0] = 0.0f;
            m_ParticleProps.Velocity[1] = 0.5f;
            m_ParticleProps.VelocityVariation[0] = 0.3f;
            m_ParticleProps.VelocityVariation[1] = 0.2f;
            m_ParticleProps.LifeTime = 2.5f;
            m_EmitRate = 3;
            break;

        case 2:  // Sparkles
            m_ParticleProps.ColorBegin[0] = 1.0f;
            m_ParticleProps.ColorBegin[1] = 1.0f;
            m_ParticleProps.ColorBegin[2] = 0.5f;
            m_ParticleProps.ColorBegin[3] = 1.0f;
            m_ParticleProps.ColorEnd[0] = 1.0f;
            m_ParticleProps.ColorEnd[1] = 1.0f;
            m_ParticleProps.ColorEnd[2] = 1.0f;
            m_ParticleProps.ColorEnd[3] = 0.0f;
            m_ParticleProps.SizeBegin = 0.15f;
            m_ParticleProps.SizeEnd = 0.0f;
            m_ParticleProps.SizeVariation = 0.1f;
            m_ParticleProps.Velocity[0] = 0.0f;
            m_ParticleProps.Velocity[1] = 0.0f;
            m_ParticleProps.VelocityVariation[0] = 3.0f;
            m_ParticleProps.VelocityVariation[1] = 3.0f;
            m_ParticleProps.LifeTime = 0.5f;
            m_EmitRate = 10;
            break;

        case 3:  // Fountain
            m_ParticleProps.ColorBegin[0] = 0.2f;
            m_ParticleProps.ColorBegin[1] = 0.5f;
            m_ParticleProps.ColorBegin[2] = 1.0f;
            m_ParticleProps.ColorBegin[3] = 1.0f;
            m_ParticleProps.ColorEnd[0] = 0.0f;
            m_ParticleProps.ColorEnd[1] = 0.2f;
            m_ParticleProps.ColorEnd[2] = 0.8f;
            m_ParticleProps.ColorEnd[3] = 0.0f;
            m_ParticleProps.SizeBegin = 0.2f;
            m_ParticleProps.SizeEnd = 0.1f;
            m_ParticleProps.SizeVariation = 0.05f;
            m_ParticleProps.Velocity[0] = 0.0f;
            m_ParticleProps.Velocity[1] = 4.0f;
            m_ParticleProps.VelocityVariation[0] = 1.0f;
            m_ParticleProps.VelocityVariation[1] = 1.0f;
            m_ParticleProps.LifeTime = 1.5f;
            m_EmitRate = 8;
            break;
    }
}

void ParticleExample::OnUpdate(GGEngine::Timestep ts, const GGEngine::Camera& camera)
{
    using namespace GGEngine;

    // Move emitter with mouse if dragging
    if (Input::IsMouseButtonPressed(MouseCode::Left))
    {
        auto [mouseX, mouseY] = Input::GetMousePosition();
        auto& window = Application::Get().GetWindow();

        // Get camera view bounds and position
        float orthoWidth = camera.GetOrthoWidth();
        float orthoHeight = camera.GetOrthoHeight();
        float cameraX = camera.GetPositionX();
        float cameraY = camera.GetPositionY();

        // Convert screen coordinates to world coordinates
        // Screen coords: (0,0) = top-left, (width, height) = bottom-right
        // World coords: centered at camera position
        float normalizedX = mouseX / window.GetWidth();
        float normalizedY = mouseY / window.GetHeight();

        m_EmitterPosition[0] = (normalizedX - 0.5f) * orthoWidth + cameraX;
        m_EmitterPosition[1] = (0.5f - normalizedY) * orthoHeight + cameraY;
    }

    // Emit particles
    if (m_AutoEmit)
    {
        m_ParticleProps.Position[0] = m_EmitterPosition[0];
        m_ParticleProps.Position[1] = m_EmitterPosition[1];

        for (int i = 0; i < m_EmitRate; i++)
        {
            m_ParticleSystem.Emit(m_ParticleProps);
        }
    }

    // Manual emit with space
    if (Input::IsKeyPressed(KeyCode::Space))
    {
        m_ParticleProps.Position[0] = m_EmitterPosition[0];
        m_ParticleProps.Position[1] = m_EmitterPosition[1];

        for (int i = 0; i < m_EmitRate * 2; i++)
        {
            m_ParticleSystem.Emit(m_ParticleProps);
        }
    }

    m_ParticleSystem.OnUpdate(ts);
}

void ParticleExample::OnRender(const GGEngine::Camera& camera)
{
    using namespace GGEngine;

    // Draw emitter indicator
    Renderer2D::ResetStats();
    Renderer2D::BeginScene(camera);
    Renderer2D::DrawQuad(m_EmitterPosition[0], m_EmitterPosition[1], 0.1f, 0.1f, 1.0f, 1.0f, 1.0f);
    Renderer2D::EndScene();

    // Draw particles
    m_ParticleSystem.OnRender(camera);
}

void ParticleExample::OnImGuiRender()
{
    ImGui::Text("Controls:");
    ImGui::BulletText("Left Click + Drag: Move emitter");
    ImGui::BulletText("Space: Burst emit");

    ImGui::Separator();
    ImGui::Text("Presets:");
    if (ImGui::RadioButton("Fire", m_CurrentPreset == 0)) SetPreset(0);
    ImGui::SameLine();
    if (ImGui::RadioButton("Smoke", m_CurrentPreset == 1)) SetPreset(1);
    if (ImGui::RadioButton("Sparkles", m_CurrentPreset == 2)) SetPreset(2);
    ImGui::SameLine();
    if (ImGui::RadioButton("Fountain", m_CurrentPreset == 3)) SetPreset(3);

    ImGui::Separator();
    ImGui::Checkbox("Auto Emit", &m_AutoEmit);
    ImGui::SliderInt("Emit Rate", &m_EmitRate, 1, 20);
    ImGui::DragFloat2("Emitter Position", m_EmitterPosition, 0.1f);

    ImGui::Separator();
    if (ImGui::CollapsingHeader("Particle Properties", ImGuiTreeNodeFlags_DefaultOpen))
    {
        ImGui::ColorEdit4("Color Begin", m_ParticleProps.ColorBegin);
        ImGui::ColorEdit4("Color End", m_ParticleProps.ColorEnd);
        ImGui::DragFloat("Size Begin", &m_ParticleProps.SizeBegin, 0.01f, 0.0f, 5.0f);
        ImGui::DragFloat("Size End", &m_ParticleProps.SizeEnd, 0.01f, 0.0f, 5.0f);
        ImGui::DragFloat("Size Variation", &m_ParticleProps.SizeVariation, 0.01f, 0.0f, 1.0f);
        ImGui::DragFloat2("Velocity", m_ParticleProps.Velocity, 0.1f);
        ImGui::DragFloat2("Velocity Variation", m_ParticleProps.VelocityVariation, 0.1f);
        ImGui::DragFloat("Life Time", &m_ParticleProps.LifeTime, 0.1f, 0.1f, 10.0f);
    }

    ImGui::Separator();
    auto stats = GGEngine::Renderer2D::GetStats();
    ImGui::Text("Draw Calls: %d", stats.DrawCalls);
    ImGui::Text("Quads: %d", stats.QuadCount);
}
