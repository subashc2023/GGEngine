#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Asset/Texture.h"

namespace GGEngine {

    // Represents a sub-region of a texture (for sprite sheets/texture atlases)
    // Stores a reference to the parent texture and UV coordinates for the region
    class GG_API SubTexture2D
    {
    public:
        // Create from texture with explicit UV coordinates (0.0 - 1.0 range)
        SubTexture2D(const Texture* texture, float minU, float minV, float maxU, float maxV);

        // Create from texture with pixel coordinates
        static Ref<SubTexture2D> CreateFromCoords(const Texture* texture,
                                                   float spriteX, float spriteY,
                                                   float spriteWidth, float spriteHeight);

        // Create from texture using grid-based indexing (for uniform sprite sheets)
        // cellX, cellY: Grid position (0-indexed from bottom-left)
        // cellWidth, cellHeight: Size of each cell in pixels
        // spriteSize: Optional multiplier for sprites spanning multiple cells (default 1x1)
        static Ref<SubTexture2D> CreateFromGrid(const Texture* texture,
                                                 uint32_t cellX, uint32_t cellY,
                                                 float cellWidth, float cellHeight,
                                                 float spriteSizeX = 1.0f, float spriteSizeY = 1.0f);

        // Calculate UV coordinates from grid position without allocating (for per-frame rendering)
        // Writes 4 UV pairs to outTexCoords: [BL, BR, TR, TL]
        static void CalculateGridUVs(const Texture* texture,
                                     uint32_t cellX, uint32_t cellY,
                                     float cellWidth, float cellHeight,
                                     float spriteSizeX, float spriteSizeY,
                                     float outTexCoords[4][2]);

        // Accessors
        const Texture* GetTexture() const { return m_Texture; }

        // Get UV coordinates (bottom-left, bottom-right, top-right, top-left)
        const float* GetTexCoords() const { return m_TexCoords[0]; }

        // Individual UV accessors
        float GetMinU() const { return m_TexCoords[0][0]; }
        float GetMinV() const { return m_TexCoords[0][1]; }
        float GetMaxU() const { return m_TexCoords[2][0]; }
        float GetMaxV() const { return m_TexCoords[2][1]; }

    private:
        const Texture* m_Texture;

        // UV coordinates for each vertex (same order as QuadTexCoords in Renderer2D)
        // [0] = Bottom-left, [1] = Bottom-right, [2] = Top-right, [3] = Top-left
        float m_TexCoords[4][2];
    };

}
