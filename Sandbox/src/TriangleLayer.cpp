#include "TriangleLayer.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/ImGui/DebugUI.h"
#include "GGEngine/Core/Input.h"
#include "GGEngine/Core/KeyCodes.h"
#include "GGEngine/Core/MouseButtonCodes.h"
#include "GGEngine/Core/Application.h"
#include "GGEngine/Core/Profiler.h"
#include "GGEngine/ParticleSystem/Random.h"

#include <imgui.h>

TriangleLayer::TriangleLayer()
    : Layer("TriangleLayer"), m_CameraController(1280.0f / 720.0f, 1.0f, true)
{
}

void TriangleLayer::OnAttach()
{
    GG_INFO("TriangleLayer attached - using Renderer2D");

    // Initialize random number generator
    GGEngine::Random::Init();

    // Load UI spritesheet (Kenney UI pack - 5x6 grid of 256x256 tiles)
    m_UISpritesheet = GGEngine::Texture::Create("game/thick_default.png");
    if (m_UISpritesheet.IsValid())
    {
        m_UIAtlas = GGEngine::CreateScope<GGEngine::TextureAtlas>(m_UISpritesheet.Get(), 256.0f, 256.0f);
        GG_INFO("Loaded UI spritesheet: {}x{}, {}x{} grid",
                m_UISpritesheet->GetWidth(), m_UISpritesheet->GetHeight(),
                m_UIAtlas->GetGridWidth(), m_UIAtlas->GetGridHeight());
    }

    // Configure fire-like particle properties
    m_ParticleProps.ColorBegin[0] = 1.0f;   // Red
    m_ParticleProps.ColorBegin[1] = 0.8f;   // Orange-ish
    m_ParticleProps.ColorBegin[2] = 0.0f;
    m_ParticleProps.ColorBegin[3] = 1.0f;

    m_ParticleProps.ColorEnd[0] = 1.0f;
    m_ParticleProps.ColorEnd[1] = 0.0f;     // Deep red
    m_ParticleProps.ColorEnd[2] = 0.0f;
    m_ParticleProps.ColorEnd[3] = 0.0f;     // Fade out

    m_ParticleProps.SizeBegin = 0.5f;
    m_ParticleProps.SizeEnd = 0.0f;
    m_ParticleProps.SizeVariation = 0.3f;

    m_ParticleProps.Velocity[0] = 0.0f;
    m_ParticleProps.Velocity[1] = 0.5f;     // Rise up
    m_ParticleProps.VelocityVariation[0] = 2.0f;
    m_ParticleProps.VelocityVariation[1] = 1.0f;

    m_ParticleProps.LifeTime = 1.0f;
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
    if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::I))
        m_Position[1] += velocity;
    if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::K))
        m_Position[1] -= velocity;
    if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::J))
        m_Position[0] -= velocity;
    if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::L))
        m_Position[0] += velocity;

    // U/O to rotate the quad
    float rotationSpeed = 2.0f * ts;  // Radians per second
    if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::U))
        m_Rotation += rotationSpeed;
    if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::O))
        m_Rotation -= rotationSpeed;

    // Emit particles on left mouse button
    if (GGEngine::Input::IsMouseButtonPressed(GGEngine::MouseCode::Left))
    {
        auto [mouseX, mouseY] = GGEngine::Input::GetMousePosition();
        auto& window = GGEngine::Application::Get().GetWindow();
        auto& bounds = m_CameraController.GetBounds();
        auto& camera = m_CameraController.GetCamera();

        // Convert screen coordinates to world coordinates
        float worldX = (mouseX / window.GetWidth()) * bounds.GetWidth() - bounds.GetWidth() * 0.5f;
        float worldY = bounds.GetHeight() * 0.5f - (mouseY / window.GetHeight()) * bounds.GetHeight();

        // Offset by camera position
        worldX += camera.GetPositionX();
        worldY += camera.GetPositionY();

        m_ParticleProps.Position[0] = worldX;
        m_ParticleProps.Position[1] = worldY;

        // Emit multiple particles per frame for better effect
        for (int i = 0; i < 5; i++)
            m_ParticleSystem.Emit(m_ParticleProps);
    }

    // Update particles
    m_ParticleSystem.OnUpdate(ts);

    {
        GG_PROFILE_SCOPE("Renderer2D::Draw");

        // Begin 2D rendering
        GGEngine::Renderer2D::ResetStats();
        GGEngine::Renderer2D::BeginScene(m_CameraController.GetCamera());

        // Draw 10x10 grid of colored quads
        const int gridSize = 100;
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

                GGEngine::Renderer2D::DrawQuad(GGEngine::QuadSpec()
                    .SetPosition(posX, posY)
                    .SetSize(quadSize, quadSize)
                    .SetColor(r, g, b));
            }
        }

        // Draw movable/rotatable quad on top
        GGEngine::Renderer2D::DrawQuad(GGEngine::QuadSpec()
            .SetPosition(m_Position[0], m_Position[1], 0.0f)
            .SetSize(0.5f, 0.5f)
            .SetRotation(m_Rotation)
            .SetColor(m_Color[0], m_Color[1], m_Color[2], m_Color[3]));

        // Draw textured quad using fallback texture (magenta/black checkerboard)
        GGEngine::Renderer2D::DrawQuad(GGEngine::QuadSpec()
            .SetPosition(1.5f, 0.0f)
            .SetSize(1.0f, 1.0f)
            .SetTexture(GGEngine::Texture::GetFallbackPtr()));

        // Draw UI sprites from atlas (white sprites, tinted with colors)
        if (m_UIAtlas)
        {
            // Draw a row of different UI tiles with color tints
            // Grid is 5x6, cells are 256x256 - draw from bottom row (y=0)
            float startX = -3.0f;
            float y = 2.0f;
            float size = 1.0f;

            // Draw tiles from bottom row with different color tints
            for (uint32_t i = 0; i < 5; i++)
            {
                auto sprite = m_UIAtlas->GetSprite(i, 0);  // Bottom row
                float r = (i == 0 || i == 3) ? 1.0f : 0.3f;
                float g = (i == 1 || i == 3) ? 1.0f : 0.3f;
                float b = (i == 2 || i == 4) ? 1.0f : 0.3f;
                GGEngine::Renderer2D::DrawQuad(GGEngine::QuadSpec()
                    .SetPosition(startX + i * 1.2f, y)
                    .SetSize(size, size)
                    .SetSubTexture(sprite.get())
                    .SetColor(r, g, b));
            }

            // Draw second row of tiles
            for (uint32_t i = 0; i < 5; i++)
            {
                auto sprite = m_UIAtlas->GetSprite(i, 1);  // Second row from bottom
                GGEngine::Renderer2D::DrawQuad(GGEngine::QuadSpec()
                    .SetPosition(startX + i * 1.2f, y - 1.2f)
                    .SetSize(size, size)
                    .SetSubTexture(sprite.get())
                    .SetColor(0.2f, 0.6f, 1.0f));
            }
        }

        GGEngine::Renderer2D::EndScene();
    }

    // Render particles
    {
        GG_PROFILE_SCOPE("ParticleSystem::OnRender");
        m_ParticleSystem.OnRender(m_CameraController.GetCamera());
    }

    // Debug panel
    ImGui::Begin("Debug");
    ImGui::Text("Camera: WASD + Q/E rotate + RMB drag + Scroll");
    ImGui::Text("Quad: IJKL move, U/O rotate");
    ImGui::Text("Particles: Hold LMB to emit");
    ImGui::Separator();
    ImGui::DragFloat3("Position", m_Position, 0.01f);
    ImGui::ColorEdit4("Color", m_Color);
    ImGui::Separator();

    // Particle properties
    if (ImGui::CollapsingHeader("Particle Settings"))
    {
        ImGui::ColorEdit4("Color Begin", m_ParticleProps.ColorBegin);
        ImGui::ColorEdit4("Color End", m_ParticleProps.ColorEnd);
        ImGui::DragFloat("Size Begin", &m_ParticleProps.SizeBegin, 0.01f, 0.0f, 5.0f);
        ImGui::DragFloat("Size End", &m_ParticleProps.SizeEnd, 0.01f, 0.0f, 5.0f);
        ImGui::DragFloat("Size Variation", &m_ParticleProps.SizeVariation, 0.01f, 0.0f, 1.0f);
        ImGui::DragFloat2("Velocity", m_ParticleProps.Velocity, 0.01f);
        ImGui::DragFloat2("Velocity Variation", m_ParticleProps.VelocityVariation, 0.01f);
        ImGui::DragFloat("Life Time", &m_ParticleProps.LifeTime, 0.01f, 0.1f, 10.0f);
    }
    ImGui::Separator();

    auto stats = GGEngine::Renderer2D::GetStats();
    ImGui::Text("Renderer2D Stats:");
    ImGui::Text("  Draw Calls: %d", stats.DrawCalls);
    ImGui::Text("  Quads: %d / %d", stats.QuadCount, stats.MaxQuadCapacity);
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
