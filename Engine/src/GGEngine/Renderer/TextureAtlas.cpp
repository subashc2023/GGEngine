#include "ggpch.h"
#include "TextureAtlas.h"

namespace GGEngine {

    TextureAtlas::TextureAtlas(const Texture* texture, float cellWidth, float cellHeight)
        : m_Texture(texture)
        , m_CellWidth(cellWidth)
        , m_CellHeight(cellHeight)
    {
        m_GridWidth = static_cast<uint32_t>(texture->GetWidth() / cellWidth);
        m_GridHeight = static_cast<uint32_t>(texture->GetHeight() / cellHeight);
    }

    TextureAtlas::TextureAtlas(AssetHandle<Texture> textureHandle, float cellWidth, float cellHeight)
        : m_TextureHandle(textureHandle)
        , m_Texture(textureHandle.Get())
        , m_CellWidth(cellWidth)
        , m_CellHeight(cellHeight)
    {
        m_GridWidth = static_cast<uint32_t>(m_Texture->GetWidth() / cellWidth);
        m_GridHeight = static_cast<uint32_t>(m_Texture->GetHeight() / cellHeight);
    }

    Ref<SubTexture2D> TextureAtlas::GetSprite(uint32_t cellX, uint32_t cellY,
                                               float spriteSizeX, float spriteSizeY)
    {
        // Check cache first
        uint64_t key = MakeCacheKey(cellX, cellY, spriteSizeX, spriteSizeY);
        auto it = m_SpriteCache.find(key);
        if (it != m_SpriteCache.end())
        {
            return it->second;
        }

        // Create new sub-texture
        auto sprite = SubTexture2D::CreateFromGrid(m_Texture, cellX, cellY,
                                                    m_CellWidth, m_CellHeight,
                                                    spriteSizeX, spriteSizeY);
        m_SpriteCache[key] = sprite;
        return sprite;
    }

    void TextureAtlas::RegisterSprite(const std::string& name, uint32_t cellX, uint32_t cellY,
                                      float spriteSizeX, float spriteSizeY)
    {
        m_NamedSprites[name] = GetSprite(cellX, cellY, spriteSizeX, spriteSizeY);
    }

    Ref<SubTexture2D> TextureAtlas::GetSprite(const std::string& name)
    {
        auto it = m_NamedSprites.find(name);
        if (it != m_NamedSprites.end())
        {
            return it->second;
        }
        return nullptr;
    }

    bool TextureAtlas::HasSprite(const std::string& name) const
    {
        return m_NamedSprites.find(name) != m_NamedSprites.end();
    }

    uint64_t TextureAtlas::MakeCacheKey(uint32_t x, uint32_t y, float sizeX, float sizeY)
    {
        // Pack coordinates and size into a 64-bit key
        // Use 16 bits each for x, y, and integer parts of sizeX/sizeY
        uint64_t key = static_cast<uint64_t>(x) |
                       (static_cast<uint64_t>(y) << 16) |
                       (static_cast<uint64_t>(static_cast<uint32_t>(sizeX * 100)) << 32) |
                       (static_cast<uint64_t>(static_cast<uint32_t>(sizeY * 100)) << 48);
        return key;
    }

}
