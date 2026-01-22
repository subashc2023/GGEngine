#pragma once

#include "Example.h"
#include "GGEngine/Asset/Texture.h"
#include "GGEngine/Renderer/TextureAtlas.h"
#include "GGEngine/Renderer/SubTexture2D.h"

// Demonstrates Renderer2D texture features:
// - Loading and rendering textures
// - Texture tinting
// - Sprite atlases / spritesheets
// - SubTexture2D for atlas regions
class Renderer2DTexturesExample : public Example
{
public:
    Renderer2DTexturesExample();

    void OnAttach() override;
    void OnDetach() override;
    void OnRender(const GGEngine::Camera& camera) override;
    void OnImGuiRender() override;

private:
    // Textures
    GGEngine::AssetHandle<GGEngine::Texture> m_Texture;
    GGEngine::AssetHandle<GGEngine::Texture> m_Spritesheet;
    GGEngine::Scope<GGEngine::TextureAtlas> m_Atlas;

    // Settings
    float m_TilingFactor = 1.0f;
    float m_TintColor[4] = { 1.0f, 1.0f, 1.0f, 1.0f };
    int m_SelectedSpriteX = 0;
    int m_SelectedSpriteY = 0;

    // Demo mode
    int m_DemoMode = 0;  // 0=single texture, 1=tiling, 2=atlas grid, 3=animated sprite
    float m_AnimTime = 0.0f;
};
