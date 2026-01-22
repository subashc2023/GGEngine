#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Asset/Texture.h"
#include "SubTexture2D.h"

#include <unordered_map>
#include <string>

namespace GGEngine {

    // Manages a sprite sheet / texture atlas with uniform grid-based sprites
    // Provides convenient access to sub-textures by name or grid coordinates
    class GG_API TextureAtlas
    {
    public:
        // Create atlas from texture with uniform cell size
        TextureAtlas(const Texture* texture, float cellWidth, float cellHeight);

        // Create atlas from texture handle with uniform cell size
        TextureAtlas(AssetHandle<Texture> textureHandle, float cellWidth, float cellHeight);

        // Get the underlying texture
        const Texture* GetTexture() const { return m_Texture; }

        // Get cell dimensions
        float GetCellWidth() const { return m_CellWidth; }
        float GetCellHeight() const { return m_CellHeight; }

        // Get grid dimensions (number of cells)
        uint32_t GetGridWidth() const { return m_GridWidth; }
        uint32_t GetGridHeight() const { return m_GridHeight; }

        // Get a sub-texture by grid coordinates (0-indexed from bottom-left)
        // spriteSize allows for sprites spanning multiple cells
        Ref<SubTexture2D> GetSprite(uint32_t cellX, uint32_t cellY,
                                     float spriteSizeX = 1.0f, float spriteSizeY = 1.0f);

        // Register a named sprite at specific grid coordinates
        void RegisterSprite(const std::string& name, uint32_t cellX, uint32_t cellY,
                           float spriteSizeX = 1.0f, float spriteSizeY = 1.0f);

        // Get a previously registered sprite by name
        Ref<SubTexture2D> GetSprite(const std::string& name);

        // Check if a sprite is registered
        bool HasSprite(const std::string& name) const;

    private:
        const Texture* m_Texture;
        AssetHandle<Texture> m_TextureHandle;  // Keep handle alive if created from handle

        float m_CellWidth;
        float m_CellHeight;
        uint32_t m_GridWidth;
        uint32_t m_GridHeight;

        // Cache of created sub-textures (keyed by packed grid coords)
        std::unordered_map<uint64_t, Ref<SubTexture2D>> m_SpriteCache;

        // Named sprites for convenience
        std::unordered_map<std::string, Ref<SubTexture2D>> m_NamedSprites;

        // Helper to create cache key from grid coords and size
        static uint64_t MakeCacheKey(uint32_t x, uint32_t y, float sizeX, float sizeY);
    };

}
