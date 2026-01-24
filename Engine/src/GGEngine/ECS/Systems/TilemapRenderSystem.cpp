#include "ggpch.h"
#include "TilemapRenderSystem.h"
#include "GGEngine/ECS/Scene.h"
#include "GGEngine/ECS/Components.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Renderer/SubTexture2D.h"
#include "GGEngine/Renderer/SceneCamera.h"
#include "GGEngine/Asset/TextureLibrary.h"

namespace GGEngine {

    std::vector<ComponentRequirement> TilemapRenderSystem::GetRequirements() const
    {
        return {
            Require<TilemapComponent>(AccessMode::Read),
            Require<TransformComponent>(AccessMode::Read)
        };
    }

    void TilemapRenderSystem::Execute(Scene& scene, float deltaTime)
    {
        (void)deltaTime;  // Not used for rendering

        if (!m_RenderContext.IsValid())
        {
            GG_CORE_WARN("TilemapRenderSystem::Execute - Invalid render context");
            return;
        }

        // Begin scene with appropriate camera
        Renderer2D::ResetStats();

        if (m_RenderContext.UsesRuntimeCamera())
        {
            Renderer2D::BeginScene(
                *m_RenderContext.RuntimeCamera,
                *m_RenderContext.CameraTransform,
                m_RenderContext.RenderPass,
                m_RenderContext.CommandBuffer,
                m_RenderContext.ViewportWidth,
                m_RenderContext.ViewportHeight
            );
        }
        else
        {
            Renderer2D::BeginScene(
                *m_RenderContext.ExternalCamera,
                m_RenderContext.RenderPass,
                m_RenderContext.CommandBuffer,
                m_RenderContext.ViewportWidth,
                m_RenderContext.ViewportHeight
            );
        }

        RenderTilemaps(scene);

        Renderer2D::EndScene();
    }

    void TilemapRenderSystem::RenderTilemaps(Scene& scene)
    {
        auto& textureLib = TextureLibrary::Get();
        auto& tilemaps = scene.GetStorage<TilemapComponent>();
        auto& transforms = scene.GetStorage<TransformComponent>();

        for (size_t i = 0; i < tilemaps.Size(); i++)
        {
            Entity entity = tilemaps.GetEntity(i);

            const TransformComponent* transform = transforms.Get(entity);
            if (!transform) continue;

            const TilemapComponent& tilemap = tilemaps.Data()[i];

            // Skip if no texture assigned
            if (tilemap.TextureName.empty()) continue;

            Texture* texture = textureLib.GetTexturePtr(tilemap.TextureName);
            if (!texture) continue;

            // Calculate base position (tilemap is centered on entity position)
            float baseX = transform->Position[0] - (tilemap.Width * tilemap.TileWidth * 0.5f);
            float baseY = transform->Position[1] - (tilemap.Height * tilemap.TileHeight * 0.5f);
            float baseZ = transform->Position[2] + tilemap.ZOffset;

            // Render each tile
            for (uint32_t ty = 0; ty < tilemap.Height; ty++)
            {
                for (uint32_t tx = 0; tx < tilemap.Width; tx++)
                {
                    int32_t tileIndex = tilemap.GetTile(tx, ty);
                    if (tileIndex < 0) continue;  // Skip empty tiles

                    // Convert linear atlas index to cell coordinates
                    uint32_t cellX, cellY;
                    tilemap.IndexToCell(tileIndex, cellX, cellY);

                    // Calculate UV coordinates on stack (no heap allocation)
                    float texCoords[4][2];
                    SubTexture2D::CalculateGridUVs(
                        texture,
                        cellX, cellY,
                        tilemap.AtlasCellWidth, tilemap.AtlasCellHeight,
                        1.0f, 1.0f,
                        texCoords
                    );

                    // Calculate world position for this tile (center of tile)
                    float worldX = baseX + tx * tilemap.TileWidth + tilemap.TileWidth * 0.5f;
                    float worldY = baseY + ty * tilemap.TileHeight + tilemap.TileHeight * 0.5f;

                    QuadSpec spec;
                    spec.x = worldX;
                    spec.y = worldY;
                    spec.z = baseZ;
                    spec.width = tilemap.TileWidth;
                    spec.height = tilemap.TileHeight;
                    spec.texture = texture;
                    spec.texCoords = texCoords;
                    spec.color[0] = tilemap.Color[0];
                    spec.color[1] = tilemap.Color[1];
                    spec.color[2] = tilemap.Color[2];
                    spec.color[3] = tilemap.Color[3];
                    Renderer2D::DrawQuad(spec);
                }
            }
        }
    }

}
