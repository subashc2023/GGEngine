#pragma once

#include <string>
#include <vector>
#include <cstdint>

namespace GGEngine {

    // Tilemap component - stores a 2D grid of tiles referencing an atlas texture
    // Each tile is a linear index into the atlas: tileIndex = cellY * AtlasColumns + cellX
    // -1 indicates an empty/transparent tile
    struct TilemapComponent
    {
        // Grid dimensions (in tiles)
        uint32_t Width = 10;
        uint32_t Height = 10;

        // Tile size in world units
        float TileWidth = 1.0f;
        float TileHeight = 1.0f;

        // Atlas texture settings
        std::string TextureName;                // Texture name from TextureLibrary
        float AtlasCellWidth = 16.0f;           // Cell width in pixels
        float AtlasCellHeight = 16.0f;          // Cell height in pixels
        uint32_t AtlasColumns = 16;             // Number of columns in atlas (for index->cellX,Y conversion)

        // Tile data: linear index into atlas, -1 = empty/transparent
        // Size should be Width * Height
        std::vector<int32_t> Tiles;

        // Rendering options
        float ZOffset = -0.01f;                 // Render slightly behind entity by default
        float Color[4] = { 1.0f, 1.0f, 1.0f, 1.0f };  // Tint color (RGBA)

        // Resize tile data when dimensions change, preserving existing data where possible
        void ResizeTiles()
        {
            size_t newSize = static_cast<size_t>(Width) * Height;
            Tiles.resize(newSize, -1);  // Fill new tiles with empty (-1)
        }

        // Get tile at grid position (returns -1 if out of bounds or empty)
        int32_t GetTile(uint32_t x, uint32_t y) const
        {
            if (x >= Width || y >= Height) return -1;
            return Tiles[y * Width + x];
        }

        // Set tile at grid position
        void SetTile(uint32_t x, uint32_t y, int32_t tileIndex)
        {
            if (x >= Width || y >= Height) return;
            Tiles[y * Width + x] = tileIndex;
        }

        // Convert linear atlas index to cell coordinates
        void IndexToCell(int32_t index, uint32_t& cellX, uint32_t& cellY) const
        {
            if (AtlasColumns == 0) { cellX = 0; cellY = 0; return; }
            cellX = static_cast<uint32_t>(index) % AtlasColumns;
            cellY = static_cast<uint32_t>(index) / AtlasColumns;
        }

        // Convert cell coordinates to linear index
        int32_t CellToIndex(uint32_t cellX, uint32_t cellY) const
        {
            return static_cast<int32_t>(cellY * AtlasColumns + cellX);
        }
    };

}
