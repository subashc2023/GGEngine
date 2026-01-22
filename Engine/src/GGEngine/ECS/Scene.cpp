#include "ggpch.h"
#include "Scene.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Renderer/SubTexture2D.h"
#include "GGEngine/Renderer/SceneCamera.h"
#include "GGEngine/Asset/TextureLibrary.h"
#include "GGEngine/Core/Math.h"

#include <algorithm>

namespace GGEngine {

    Scene::Scene(const std::string& name)
        : m_Name(name)
    {
    }

    std::pair<Entity, uint32_t> Scene::AllocateEntitySlot()
    {
        Entity index;
        uint32_t generation;

        if (!m_FreeList.empty())
        {
            // Reuse recycled slot
            index = m_FreeList.back();
            m_FreeList.pop_back();
            generation = m_Generations[index];
        }
        else
        {
            // Allocate new slot
            index = static_cast<Entity>(m_Generations.size());
            m_Generations.push_back(1);
            generation = 1;
        }

        m_Entities.push_back(index);
        return { index, generation };
    }

    EntityID Scene::CreateEntity(const std::string& name)
    {
        auto [index, generation] = AllocateEntitySlot();

        // Add required TagComponent with auto-generated GUID
        TagComponent tag(name);
        m_Tags.Add(index, tag);
        m_GUIDToEntity[tag.ID] = index;

        // Add default TransformComponent
        m_Transforms.Add(index);

        GG_CORE_TRACE("Created entity '{}' (index={}, gen={})", name, index, generation);
        return EntityID{ index, generation };
    }

    EntityID Scene::CreateEntityWithGUID(const std::string& name, const GUID& guid)
    {
        auto [index, generation] = AllocateEntitySlot();

        // Add TagComponent with provided GUID
        TagComponent tag;
        tag.Name = name;
        tag.ID = guid;
        m_Tags.Add(index, tag);
        m_GUIDToEntity[tag.ID] = index;

        // Add default TransformComponent
        m_Transforms.Add(index);

        GG_CORE_TRACE("Created entity '{}' with GUID (index={}, gen={})", name, index, generation);
        return EntityID{ index, generation };
    }

    void Scene::Clear()
    {
        // Destroy all entities by clearing all storage
        m_Entities.clear();
        m_Generations.clear();
        m_FreeList.clear();
        m_GUIDToEntity.clear();
        m_Tags.Clear();
        m_Transforms.Clear();
        m_Sprites.Clear();
        m_Tilemaps.Clear();
        m_Cameras.Clear();

        GG_CORE_TRACE("Scene '{}' cleared", m_Name);
    }

    void Scene::DestroyEntity(EntityID entity)
    {
        if (!IsEntityValid(entity)) return;

        Entity index = entity.Index;

        // Remove from GUID lookup
        if (auto* tag = m_Tags.Get(index))
        {
            m_GUIDToEntity.erase(tag->ID);
        }

        // Remove all components
        m_Tags.Remove(index);
        m_Transforms.Remove(index);
        m_Sprites.Remove(index);
        m_Tilemaps.Remove(index);
        m_Cameras.Remove(index);

        // Remove from active entities list using swap-and-pop (O(1) removal instead of O(n))
        auto it = std::find(m_Entities.begin(), m_Entities.end(), index);
        if (it != m_Entities.end())
        {
            *it = m_Entities.back();
            m_Entities.pop_back();
        }

        // Increment generation and add to free list
        m_Generations[index]++;
        m_FreeList.push_back(index);

        GG_CORE_TRACE("Destroyed entity index={}", index);
    }

    bool Scene::IsEntityValid(EntityID entity) const
    {
        if (entity.Index == InvalidEntity) return false;
        if (entity.Index >= m_Generations.size()) return false;
        return m_Generations[entity.Index] == entity.Generation;
    }

    EntityID Scene::GetEntityID(Entity index) const
    {
        if (index >= m_Generations.size()) return InvalidEntityID;
        return EntityID{ index, m_Generations[index] };
    }

    EntityID Scene::FindEntityByName(const std::string& name) const
    {
        for (size_t i = 0; i < m_Tags.Size(); i++)
        {
            if (m_Tags.Data()[i].Name == name)
            {
                Entity index = m_Tags.GetEntity(i);
                return EntityID{ index, m_Generations[index] };
            }
        }
        return InvalidEntityID;
    }

    EntityID Scene::FindEntityByGUID(const GUID& guid) const
    {
        auto it = m_GUIDToEntity.find(guid);
        if (it != m_GUIDToEntity.end())
        {
            Entity index = it->second;
            return EntityID{ index, m_Generations[index] };
        }
        return InvalidEntityID;
    }

    void Scene::OnUpdate(Timestep ts)
    {
        // Reserved for future systems (physics, scripting, etc.)
    }

    void Scene::RenderTilemaps()
    {
        auto& textureLib = TextureLibrary::Get();

        for (size_t i = 0; i < m_Tilemaps.Size(); i++)
        {
            Entity entity = m_Tilemaps.GetEntity(i);

            const TransformComponent* transform = m_Transforms.Get(entity);
            if (!transform) continue;

            const TilemapComponent& tilemap = m_Tilemaps.Data()[i];

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

                    Renderer2D::DrawQuad(
                        worldX, worldY, baseZ,
                        tilemap.TileWidth, tilemap.TileHeight,
                        texture,
                        texCoords,
                        tilemap.Color[0], tilemap.Color[1], tilemap.Color[2], tilemap.Color[3]
                    );
                }
            }
        }
    }

    void Scene::RenderSprites()
    {
        auto& textureLib = TextureLibrary::Get();

        for (size_t i = 0; i < m_Sprites.Size(); i++)
        {
            Entity entity = m_Sprites.GetEntity(i);

            // Get transform (should always exist)
            const TransformComponent* transform = m_Transforms.Get(entity);
            if (!transform) continue;

            const SpriteRendererComponent& sprite = m_Sprites.Data()[i];

            float rotationRadians = Math::ToRadians(transform->Rotation);

            // Resolve texture from library
            Texture* texture = nullptr;
            if (!sprite.TextureName.empty())
            {
                texture = textureLib.GetTexturePtr(sprite.TextureName);
            }

            if (texture)
            {
                if (sprite.UseAtlas)
                {
                    // Spritesheet/Atlas rendering - calculate UVs on stack (no heap allocation)
                    float texCoords[4][2];
                    SubTexture2D::CalculateGridUVs(
                        texture,
                        sprite.AtlasCellX, sprite.AtlasCellY,
                        sprite.AtlasCellWidth, sprite.AtlasCellHeight,
                        sprite.AtlasSpriteWidth, sprite.AtlasSpriteHeight,
                        texCoords
                    );

                    Renderer2D::DrawRotatedQuad(
                        transform->Position[0], transform->Position[1], transform->Position[2],
                        transform->Scale[0], transform->Scale[1],
                        rotationRadians,
                        texture,
                        texCoords,
                        sprite.Color[0], sprite.Color[1], sprite.Color[2], sprite.Color[3]
                    );
                }
                else
                {
                    // Full texture rendering
                    Renderer2D::DrawRotatedQuad(
                        transform->Position[0], transform->Position[1], transform->Position[2],
                        transform->Scale[0], transform->Scale[1],
                        rotationRadians,
                        texture,
                        sprite.TilingFactor,
                        sprite.Color[0], sprite.Color[1], sprite.Color[2], sprite.Color[3]
                    );
                }
            }
            else
            {
                // Colored quad (no texture)
                Renderer2D::DrawRotatedQuad(
                    transform->Position[0], transform->Position[1], transform->Position[2],
                    transform->Scale[0], transform->Scale[1],
                    rotationRadians,
                    sprite.Color[0], sprite.Color[1], sprite.Color[2], sprite.Color[3]
                );
            }
        }
    }

    void Scene::OnRender(const Camera& camera, RHIRenderPassHandle renderPass,
                         RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        Renderer2D::ResetStats();
        Renderer2D::BeginScene(camera, renderPass, cmd, width, height);

        RenderTilemaps();
        RenderSprites();

        Renderer2D::EndScene();
    }

    EntityID Scene::GetPrimaryCameraEntity()
    {
        for (size_t i = 0; i < m_Cameras.Size(); i++)
        {
            if (m_Cameras.Data()[i].Primary)
            {
                Entity entity = m_Cameras.GetEntity(i);
                return EntityID{ entity, m_Generations[entity] };
            }
        }
        return InvalidEntityID;
    }

    void Scene::OnViewportResize(uint32_t width, uint32_t height)
    {
        // Update all cameras that don't have fixed aspect ratio
        for (size_t i = 0; i < m_Cameras.Size(); i++)
        {
            CameraComponent& cameraComp = m_Cameras.Data()[i];
            if (!cameraComp.FixedAspectRatio)
            {
                cameraComp.Camera.SetViewportSize(width, height);
            }
        }
    }

    void Scene::OnRenderRuntime(RHIRenderPassHandle renderPass,
                                 RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        // Find primary camera
        SceneCamera* mainCamera = nullptr;
        Mat4 cameraTransform;

        for (size_t i = 0; i < m_Cameras.Size(); i++)
        {
            CameraComponent& cameraComp = m_Cameras.Data()[i];
            if (cameraComp.Primary)
            {
                Entity entity = m_Cameras.GetEntity(i);

                // Get transform for this camera entity
                const TransformComponent* transform = m_Transforms.Get(entity);
                if (transform)
                {
                    mainCamera = &cameraComp.Camera;
                    cameraTransform = transform->GetMat4();
                    break;
                }
            }
        }

        if (!mainCamera)
        {
            GG_CORE_WARN("Scene::OnRenderRuntime - No primary camera found!");
            return;
        }

        Renderer2D::ResetStats();
        Renderer2D::BeginScene(*mainCamera, cameraTransform, renderPass, cmd, width, height);

        RenderTilemaps();
        RenderSprites();

        Renderer2D::EndScene();
    }

}
