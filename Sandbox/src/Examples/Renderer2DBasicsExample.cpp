#include "Renderer2DBasicsExample.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/ECS/Components/TransformComponent.h"
#include "GGEngine/Core/Math.h"

#include <imgui.h>
#include <cmath>

Renderer2DBasicsExample::Renderer2DBasicsExample()
    : Example("Renderer2D Basics",
              "Demonstrates colored quads, rotated quads, and matrix-based rendering")
{
}

void Renderer2DBasicsExample::OnAttach()
{
    m_Time = 0.0f;
}

void Renderer2DBasicsExample::OnUpdate(GGEngine::Timestep ts, const GGEngine::Camera& /*camera*/)
{
    m_Time += ts;

    // Animate based on settings
    if (m_AnimatePosition)
    {
        m_QuadPosition[0] = std::sin(m_Time) * 2.0f;
        m_QuadPosition[1] = std::cos(m_Time * 0.7f) * 1.5f;
    }

    if (m_AnimateRotation)
    {
        m_QuadRotation = m_Time * 45.0f;  // 45 degrees per second
    }

    if (m_AnimateScale)
    {
        float scale = 1.0f + std::sin(m_Time * 2.0f) * 0.3f;
        m_QuadScale[0] = scale;
        m_QuadScale[1] = scale;
    }
}

void Renderer2DBasicsExample::OnRender(const GGEngine::Camera& camera)
{
    using namespace GGEngine;

    Renderer2D::ResetStats();
    Renderer2D::BeginScene(camera);

    switch (m_DemoMode)
    {
        case 0:  // Basic quads demo
        {
            // Static colored quad
            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(-3.0f, 0.0f)
                .SetSize(1.0f, 1.0f)
                .SetColor(0.8f, 0.2f, 0.2f));

            // Animated colored quad
            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(m_QuadPosition[0], m_QuadPosition[1], 0.0f)
                .SetSize(m_QuadScale[0], m_QuadScale[1])
                .SetColor(m_QuadColor[0], m_QuadColor[1], m_QuadColor[2], m_QuadColor[3]));

            // Semi-transparent quad
            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(3.0f, 0.0f, 0.0f)
                .SetSize(1.5f, 1.5f)
                .SetColor(0.2f, 0.2f, 0.8f, 0.5f));

            // Rotated quad
            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(0.0f, -2.0f)
                .SetSize(1.0f, 1.0f)
                .SetRotation(GGEngine::Math::ToRadians(m_QuadRotation))
                .SetColor(0.8f, 0.8f, 0.2f));

            break;
        }

        case 1:  // Color gradient grid
        {
            const int gridSize = 20;
            const float quadSize = 0.18f;
            const float spacing = 0.2f;
            const float offset = (gridSize - 1) * spacing * 0.5f;

            for (int y = 0; y < gridSize; y++)
            {
                for (int x = 0; x < gridSize; x++)
                {
                    float posX = x * spacing - offset;
                    float posY = y * spacing - offset;

                    // Gradient based on position
                    float r = static_cast<float>(x) / (gridSize - 1);
                    float g = static_cast<float>(y) / (gridSize - 1);
                    float b = 0.5f + 0.5f * std::sin(m_Time + x * 0.3f + y * 0.3f);

                    Renderer2D::DrawQuad(QuadSpec()
                        .SetPosition(posX, posY)
                        .SetSize(quadSize, quadSize)
                        .SetColor(r, g, b));
                }
            }
            break;
        }

        case 2:  // Matrix-based rendering demo
        {
            // Create a TransformComponent and use GetMatrix()
            TransformComponent transform;
            transform.Position[0] = m_QuadPosition[0];
            transform.Position[1] = m_QuadPosition[1];
            transform.Position[2] = 0.0f;
            transform.Rotation = m_QuadRotation;
            transform.Scale[0] = m_QuadScale[0];
            transform.Scale[1] = m_QuadScale[1];

            // Render using the matrix API via QuadSpec
            auto mat = transform.GetMatrix();
            Renderer2D::DrawQuad(QuadSpec()
                .SetTransform(&mat)
                .SetColor(m_QuadColor[0], m_QuadColor[1], m_QuadColor[2], m_QuadColor[3]));

            // Show multiple transforms with different rotations
            for (int i = 0; i < 8; i++)
            {
                TransformComponent t;
                float angle = (360.0f / 8.0f) * i + m_Time * 30.0f;
                float radius = 2.5f;
                t.Position[0] = std::cos(GGEngine::Math::ToRadians(angle)) * radius;
                t.Position[1] = std::sin(GGEngine::Math::ToRadians(angle)) * radius;
                t.Rotation = angle;
                t.Scale[0] = 0.5f;
                t.Scale[1] = 0.5f;

                float hue = static_cast<float>(i) / 8.0f;
                // Simple HSV to RGB (hue only, full saturation/value)
                float r = std::abs(hue * 6.0f - 3.0f) - 1.0f;
                float g = 2.0f - std::abs(hue * 6.0f - 2.0f);
                float b = 2.0f - std::abs(hue * 6.0f - 4.0f);
                r = std::max(0.0f, std::min(1.0f, r));
                g = std::max(0.0f, std::min(1.0f, g));
                b = std::max(0.0f, std::min(1.0f, b));

                auto tMat = t.GetMatrix();
                Renderer2D::DrawQuad(QuadSpec()
                    .SetTransform(&tMat)
                    .SetColor(r, g, b, 1.0f));
            }
            break;
        }
    }

    Renderer2D::EndScene();
}

void Renderer2DBasicsExample::OnImGuiRender()
{
    ImGui::Text("Demo Mode:");
    ImGui::RadioButton("Basic Quads", &m_DemoMode, 0);
    ImGui::RadioButton("Color Grid", &m_DemoMode, 1);
    ImGui::RadioButton("Matrix Transform", &m_DemoMode, 2);

    ImGui::Separator();
    ImGui::Text("Animation:");
    ImGui::Checkbox("Animate Position", &m_AnimatePosition);
    ImGui::Checkbox("Animate Rotation", &m_AnimateRotation);
    ImGui::Checkbox("Animate Scale", &m_AnimateScale);

    ImGui::Separator();
    ImGui::Text("Quad Properties:");
    ImGui::DragFloat3("Position", m_QuadPosition, 0.1f);
    ImGui::DragFloat("Rotation", &m_QuadRotation, 1.0f);
    ImGui::DragFloat2("Scale", m_QuadScale, 0.1f, 0.1f, 5.0f);
    ImGui::ColorEdit4("Color", m_QuadColor);

    ImGui::Separator();
    auto stats = GGEngine::Renderer2D::GetStats();
    ImGui::Text("Draw Calls: %d", stats.DrawCalls);
    ImGui::Text("Quads: %d", stats.QuadCount);
}
