#include "ggpch.h"
#include "SubTexture2D.h"

namespace GGEngine {

    SubTexture2D::SubTexture2D(const Texture* texture, float minU, float minV, float maxU, float maxV)
        : m_Texture(texture)
    {
        // Set UV coordinates for each vertex (matches QuadTexCoords order)
        // Bottom-left
        m_TexCoords[0][0] = minU;
        m_TexCoords[0][1] = minV;
        // Bottom-right
        m_TexCoords[1][0] = maxU;
        m_TexCoords[1][1] = minV;
        // Top-right
        m_TexCoords[2][0] = maxU;
        m_TexCoords[2][1] = maxV;
        // Top-left
        m_TexCoords[3][0] = minU;
        m_TexCoords[3][1] = maxV;
    }

    Ref<SubTexture2D> SubTexture2D::CreateFromCoords(const Texture* texture,
                                                      float spriteX, float spriteY,
                                                      float spriteWidth, float spriteHeight)
    {
        float texWidth = static_cast<float>(texture->GetWidth());
        float texHeight = static_cast<float>(texture->GetHeight());

        float minU = spriteX / texWidth;
        float minV = spriteY / texHeight;
        float maxU = (spriteX + spriteWidth) / texWidth;
        float maxV = (spriteY + spriteHeight) / texHeight;

        return CreateRef<SubTexture2D>(texture, minU, minV, maxU, maxV);
    }

    Ref<SubTexture2D> SubTexture2D::CreateFromGrid(const Texture* texture,
                                                    uint32_t cellX, uint32_t cellY,
                                                    float cellWidth, float cellHeight,
                                                    float spriteSizeX, float spriteSizeY)
    {
        float texWidth = static_cast<float>(texture->GetWidth());
        float texHeight = static_cast<float>(texture->GetHeight());

        // Calculate pixel coordinates from grid position
        float spriteX = cellX * cellWidth;
        float spriteY = cellY * cellHeight;
        float spriteWidth = cellWidth * spriteSizeX;
        float spriteHeight = cellHeight * spriteSizeY;

        // Convert to UV coordinates (0.0 - 1.0)
        float minU = spriteX / texWidth;
        float minV = spriteY / texHeight;
        float maxU = (spriteX + spriteWidth) / texWidth;
        float maxV = (spriteY + spriteHeight) / texHeight;

        return CreateRef<SubTexture2D>(texture, minU, minV, maxU, maxV);
    }

    void SubTexture2D::CalculateGridUVs(const Texture* texture,
                                        uint32_t cellX, uint32_t cellY,
                                        float cellWidth, float cellHeight,
                                        float spriteSizeX, float spriteSizeY,
                                        float outTexCoords[4][2])
    {
        float texWidth = static_cast<float>(texture->GetWidth());
        float texHeight = static_cast<float>(texture->GetHeight());

        // Calculate pixel coordinates from grid position
        float spriteX = cellX * cellWidth;
        float spriteY = cellY * cellHeight;
        float spriteWidth = cellWidth * spriteSizeX;
        float spriteHeight = cellHeight * spriteSizeY;

        // Convert to UV coordinates (0.0 - 1.0)
        float minU = spriteX / texWidth;
        float minV = spriteY / texHeight;
        float maxU = (spriteX + spriteWidth) / texWidth;
        float maxV = (spriteY + spriteHeight) / texHeight;

        // Write to output array (same order as QuadTexCoords)
        // Bottom-left
        outTexCoords[0][0] = minU;
        outTexCoords[0][1] = minV;
        // Bottom-right
        outTexCoords[1][0] = maxU;
        outTexCoords[1][1] = minV;
        // Top-right
        outTexCoords[2][0] = maxU;
        outTexCoords[2][1] = maxV;
        // Top-left
        outTexCoords[3][0] = minU;
        outTexCoords[3][1] = maxV;
    }

}
