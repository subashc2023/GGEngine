#include "TriangleLayer.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/ImGui/DebugUI.h"
#include "GGEngine/Core/Input.h"
#include "GGEngine/Core/KeyCodes.h"
#include "GGEngine/Core/Profiler.h"

#include <imgui.h>

TriangleLayer::TriangleLayer()
    : Layer("TriangleLayer"), m_CameraController(1280.0f / 720.0f, 1.0f, true)
{
}

void TriangleLayer::OnAttach()
{
    GG_INFO("TriangleLayer attached - using Renderer2D");
}

void TriangleLayer::OnDetach()
{
    m_Texture = GGEngine::AssetHandle<GGEngine::Texture>();
    GG_INFO("TriangleLayer detached");
}

void TriangleLayer::OnUpdate(GGEngine::Timestep ts)
{
    GG_PROFILE_FUNCTION();

    {
        GG_PROFILE_SCOPE("CameraController::OnUpdate");
        m_CameraController.OnUpdate(ts);
    }

    // IJKL to move the quad
    float velocity = m_TriangleMoveSpeed * ts;
    if (GGEngine::Input::IsKeyPressed(GG_KEY_I))
        m_Position[1] += velocity;
    if (GGEngine::Input::IsKeyPressed(GG_KEY_K))
        m_Position[1] -= velocity;
    if (GGEngine::Input::IsKeyPressed(GG_KEY_J))
        m_Position[0] -= velocity;
    if (GGEngine::Input::IsKeyPressed(GG_KEY_L))
        m_Position[0] += velocity;

    // U/O to rotate the quad
    float rotationSpeed = 2.0f * ts;  // Radians per second
    if (GGEngine::Input::IsKeyPressed(GG_KEY_U))
        m_Rotation += rotationSpeed;
    if (GGEngine::Input::IsKeyPressed(GG_KEY_O))
        m_Rotation -= rotationSpeed;

    {
        GG_PROFILE_SCOPE("Renderer2D::Draw");

        // Begin 2D rendering
        GGEngine::Renderer2D::ResetStats();
        GGEngine::Renderer2D::BeginScene(m_CameraController.GetCamera());

        // Draw 10x10 grid of colored quads
        const int gridSize = 10;
        const float quadSize = 0.1f;
        const float spacing = 0.11f;
        const float offset = (gridSize - 1) * spacing * 0.5f;

        for (int y = 0; y < gridSize; y++)
        {
            for (int x = 0; x < gridSize; x++)
            {
                float posX = x * spacing - offset;
                float posY = y * spacing - offset;

                // Gradient color: red from left to right, green from bottom to top
                float r = static_cast<float>(x) / (gridSize - 1);
                float g = static_cast<float>(y) / (gridSize - 1);
                float b = 0.5f;

                GGEngine::Renderer2D::DrawQuad(posX, posY, quadSize, quadSize, r, g, b);
            }
        }

        // Draw movable/rotatable quad on top (z=0.0f specified to avoid overload ambiguity)
        GGEngine::Renderer2D::DrawRotatedQuad(
            m_Position[0], m_Position[1], 0.0f, 0.5f, 0.5f,
            m_Rotation,
            m_Color[0], m_Color[1], m_Color[2], m_Color[3]);

        // Draw textured quad using fallback texture (magenta/black checkerboard)
        GGEngine::Renderer2D::DrawQuad(1.5f, 0.0f, 1.0f, 1.0f,
            GGEngine::Texture::GetFallbackPtr());

        GGEngine::Renderer2D::EndScene();
    }

    // Debug panel
    ImGui::Begin("Debug");
    ImGui::Text("Camera: WASD + Q/E rotate + RMB drag + Scroll");
    ImGui::Text("Quad: IJKL move, U/O rotate");
    ImGui::Separator();
    ImGui::DragFloat3("Position", m_Position, 0.01f);
    ImGui::ColorEdit4("Color", m_Color);
    ImGui::Separator();

    auto stats = GGEngine::Renderer2D::GetStats();
    ImGui::Text("Renderer2D Stats:");
    ImGui::Text("  Draw Calls: %d", stats.DrawCalls);
    ImGui::Text("  Quads: %d", stats.QuadCount);
    ImGui::Separator();

    GGEngine::DebugUI::ShowStatsContent(ts);

    ImGui::Separator();
    GGEngine::DebugUI::ShowProfilerContent();

    ImGui::End();
}

void TriangleLayer::OnEvent(GGEngine::Event& event)
{
    m_CameraController.OnEvent(event);
}

void TriangleLayer::OnWindowResize(uint32_t width, uint32_t height)
{
    if (width > 0 && height > 0)
    {
        float aspectRatio = static_cast<float>(width) / static_cast<float>(height);
        m_CameraController.SetAspectRatio(aspectRatio);
    }
}
