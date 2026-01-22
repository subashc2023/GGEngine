#pragma once

#include <string>

namespace GGEngine {

    // Sprite renderer - color tint and optional texture
    struct SpriteRendererComponent
    {
        float Color[4] = { 1.0f, 1.0f, 1.0f, 1.0f };  // RGBA tint
        std::string TextureName;                       // Texture name from TextureLibrary (empty = no texture)
        float TilingFactor = 1.0f;                     // Texture tiling
    };

}
