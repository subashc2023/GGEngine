#pragma once

#include <string>
#include <cstdint>

namespace GGEngine {

    // Sprite renderer - color tint and optional texture/spritesheet
    struct SpriteRendererComponent
    {
        float Color[4] = { 1.0f, 1.0f, 1.0f, 1.0f };  // RGBA tint
        std::string TextureName;                       // Texture name from TextureLibrary (empty = no texture)
        float TilingFactor = 1.0f;                     // Texture tiling (ignored when using atlas)

        // Spritesheet/Atlas support
        bool UseAtlas = false;                         // If true, treat texture as a spritesheet
        uint32_t AtlasCellX = 0;                       // Grid X position (0-indexed)
        uint32_t AtlasCellY = 0;                       // Grid Y position (0-indexed)
        float AtlasCellWidth = 64.0f;                  // Cell width in pixels
        float AtlasCellHeight = 64.0f;                 // Cell height in pixels
        float AtlasSpriteWidth = 1.0f;                 // Sprite size in cells (for multi-cell sprites)
        float AtlasSpriteHeight = 1.0f;
    };

}
