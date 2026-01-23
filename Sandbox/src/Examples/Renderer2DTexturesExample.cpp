#include "Renderer2DTexturesExample.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Core/Log.h"

#include <imgui.h>
#include <cmath>

Renderer2DTexturesExample::Renderer2DTexturesExample()
    : Example("Renderer2D Textures",
              "Demonstrates textures, tinting, sprite atlases, and sub-textures")
{
}

void Renderer2DTexturesExample::OnAttach()
{
    // Load spritesheet (UI pack - 5x6 grid of 256x256 tiles)
    m_Spritesheet = GGEngine::Texture::Create("game/thick_default.png");
    if (m_Spritesheet.IsValid())
    {
        m_Atlas = GGEngine::CreateScope<GGEngine::TextureAtlas>(
            m_Spritesheet.Get(), 256.0f, 256.0f);
        GG_INFO("Loaded spritesheet: {}x{} ({} x {} grid)",
                m_Spritesheet->GetWidth(), m_Spritesheet->GetHeight(),
                m_Atlas->GetGridWidth(), m_Atlas->GetGridHeight());
    }

    m_AnimTime = 0.0f;
}

void Renderer2DTexturesExample::OnDetach()
{
    m_Atlas.reset();
    m_Spritesheet = GGEngine::AssetHandle<GGEngine::Texture>();
    m_Texture = GGEngine::AssetHandle<GGEngine::Texture>();
}

void Renderer2DTexturesExample::OnRender(const GGEngine::Camera& camera)
{
    using namespace GGEngine;

    m_AnimTime += 0.016f;  // Approximate 60fps

    Renderer2D::ResetStats();
    Renderer2D::BeginScene(camera);

    switch (m_DemoMode)
    {
        case 0:  // Single texture with fallback
        {
            // Draw fallback texture (magenta/black checkerboard)
            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(-2.0f, 0.0f)
                .SetSize(2.0f, 2.0f)
                .SetTexture(Texture::GetFallbackPtr()));

            // Draw with tint
            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(2.0f, 0.0f)
                .SetSize(2.0f, 2.0f)
                .SetTexture(Texture::GetFallbackPtr())
                .SetColor(m_TintColor[0], m_TintColor[1], m_TintColor[2], m_TintColor[3]));

            ImGui::TextWrapped("Left: Fallback texture (no texture loaded)");
            ImGui::TextWrapped("Right: Fallback with color tint applied");
            break;
        }

        case 1:  // Tiling demo
        {
            // Draw with different tiling factors
            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(-3.0f, 0.0f)
                .SetSize(2.0f, 2.0f)
                .SetTexture(Texture::GetFallbackPtr(), 1.0f));

            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(0.0f, 0.0f)
                .SetSize(2.0f, 2.0f)
                .SetTexture(Texture::GetFallbackPtr(), 2.0f));

            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(3.0f, 0.0f)
                .SetSize(2.0f, 2.0f)
                .SetTexture(Texture::GetFallbackPtr(), m_TilingFactor));

            // Labels (using colored quads as indicators)
            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(-3.0f, -1.5f)
                .SetSize(0.3f, 0.1f)
                .SetColor(1.0f, 1.0f, 1.0f));

            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(0.0f, -1.5f)
                .SetSize(0.6f, 0.1f)
                .SetColor(1.0f, 1.0f, 1.0f));

            Renderer2D::DrawQuad(QuadSpec()
                .SetPosition(3.0f, -1.5f)
                .SetSize(0.9f, 0.1f)
                .SetColor(1.0f, 1.0f, 1.0f));

            break;
        }

        case 2:  // Atlas grid display
        {
            if (m_Atlas)
            {
                uint32_t gridW = m_Atlas->GetGridWidth();
                uint32_t gridH = m_Atlas->GetGridHeight();
                float size = 1.0f;
                float spacing = 1.1f;
                float startX = -(gridW - 1) * spacing * 0.5f;
                float startY = (gridH - 1) * spacing * 0.5f;

                for (uint32_t y = 0; y < gridH; y++)
                {
                    for (uint32_t x = 0; x < gridW; x++)
                    {
                        auto sprite = m_Atlas->GetSprite(x, y);
                        float posX = startX + x * spacing;
                        float posY = startY - y * spacing;

                        // Highlight selected sprite
                        if (x == static_cast<uint32_t>(m_SelectedSpriteX) &&
                            y == static_cast<uint32_t>(m_SelectedSpriteY))
                        {
                            Renderer2D::DrawQuad(QuadSpec()
                                .SetPosition(posX, posY, -0.01f)
                                .SetSize(size * 1.1f, size * 1.1f)
                                .SetColor(1.0f, 1.0f, 0.0f, 1.0f));
                        }

                        Renderer2D::DrawQuad(QuadSpec()
                            .SetPosition(posX, posY)
                            .SetSize(size, size)
                            .SetSubTexture(sprite.get())
                            .SetColor(m_TintColor[0], m_TintColor[1], m_TintColor[2], m_TintColor[3]));
                    }
                }
            }
            else
            {
                Renderer2D::DrawQuad(QuadSpec()
                    .SetPosition(0.0f, 0.0f)
                    .SetSize(2.0f, 2.0f)
                    .SetColor(0.5f, 0.5f, 0.5f));
            }
            break;
        }

        case 3:  // Animated sprite demo
        {
            if (m_Atlas)
            {
                // Cycle through sprites in a row
                uint32_t gridW = m_Atlas->GetGridWidth();
                int frame = static_cast<int>(m_AnimTime * 4.0f) % gridW;  // 4 fps animation

                auto sprite = m_Atlas->GetSprite(frame, m_SelectedSpriteY);
                Renderer2D::DrawQuad(QuadSpec()
                    .SetPosition(0.0f, 0.0f)
                    .SetSize(3.0f, 3.0f)
                    .SetSubTexture(sprite.get())
                    .SetColor(m_TintColor[0], m_TintColor[1], m_TintColor[2], m_TintColor[3]));

                // Show all frames below
                float startX = -(gridW - 1) * 0.6f * 0.5f;
                for (uint32_t x = 0; x < gridW; x++)
                {
                    auto frameSprite = m_Atlas->GetSprite(x, m_SelectedSpriteY);
                    float alpha = (x == static_cast<uint32_t>(frame)) ? 1.0f : 0.3f;
                    Renderer2D::DrawQuad(QuadSpec()
                        .SetPosition(startX + x * 0.6f, -2.5f)
                        .SetSize(0.5f, 0.5f)
                        .SetSubTexture(frameSprite.get())
                        .SetColor(alpha, alpha, alpha, 1.0f));
                }
            }
            break;
        }
    }

    Renderer2D::EndScene();
}

void Renderer2DTexturesExample::OnImGuiRender()
{
    ImGui::Text("Demo Mode:");
    ImGui::RadioButton("Single Texture", &m_DemoMode, 0);
    ImGui::RadioButton("Tiling Factor", &m_DemoMode, 1);
    ImGui::RadioButton("Atlas Grid", &m_DemoMode, 2);
    ImGui::RadioButton("Animated Sprite", &m_DemoMode, 3);

    ImGui::Separator();
    ImGui::ColorEdit4("Tint Color", m_TintColor);
    ImGui::SliderFloat("Tiling Factor", &m_TilingFactor, 0.1f, 10.0f);

    if (m_Atlas)
    {
        ImGui::Separator();
        ImGui::Text("Atlas Selection:");
        ImGui::SliderInt("Sprite X", &m_SelectedSpriteX, 0, m_Atlas->GetGridWidth() - 1);
        ImGui::SliderInt("Sprite Y", &m_SelectedSpriteY, 0, m_Atlas->GetGridHeight() - 1);
    }

    ImGui::Separator();
    auto stats = GGEngine::Renderer2D::GetStats();
    ImGui::Text("Draw Calls: %d", stats.DrawCalls);
    ImGui::Text("Quads: %d", stats.QuadCount);
}
