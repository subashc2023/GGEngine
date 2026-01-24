#include "ggpch.h"
#include "Scene.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Renderer/InstancedRenderer2D.h"
#include "GGEngine/Renderer/SubTexture2D.h"
#include "GGEngine/Renderer/SceneCamera.h"
#include "GGEngine/Asset/TextureLibrary.h"
#include "GGEngine/Core/Math.h"
#include "GGEngine/Core/TaskGraph.h"

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
        GetStorage<TagComponent>().Add(index, tag);
        m_GUIDToEntity[tag.ID] = index;

        // Add default TransformComponent
        GetStorage<TransformComponent>().Add(index);

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
        GetStorage<TagComponent>().Add(index, tag);
        m_GUIDToEntity[tag.ID] = index;

        // Add default TransformComponent
        GetStorage<TransformComponent>().Add(index);

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

        // Clear all registered component storages
        for (auto& [typeIndex, storage] : m_ComponentRegistry)
        {
            storage->Clear();
        }

        GG_CORE_TRACE("Scene '{}' cleared", m_Name);
    }

    void Scene::DestroyEntity(EntityID entity)
    {
        if (!IsEntityValid(entity)) return;

        Entity index = entity.Index;

        // Remove from GUID lookup
        if (auto* tag = GetStorage<TagComponent>().Get(index))
        {
            m_GUIDToEntity.erase(tag->ID);
        }

        // Remove all components from all registered storages
        for (auto& [typeIndex, storage] : m_ComponentRegistry)
        {
            storage->Remove(index);
        }

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
        auto& tags = GetStorage<TagComponent>();
        for (size_t i = 0; i < tags.Size(); i++)
        {
            if (tags.Data()[i].Name == name)
            {
                Entity index = tags.GetEntity(i);
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
        auto& tilemaps = GetStorage<TilemapComponent>();
        auto& transforms = GetStorage<TransformComponent>();

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

    void Scene::RenderSprites()
    {
        auto& textureLib = TextureLibrary::Get();
        auto& sprites = GetStorage<SpriteRendererComponent>();
        auto& transforms = GetStorage<TransformComponent>();

        for (size_t i = 0; i < sprites.Size(); i++)
        {
            Entity entity = sprites.GetEntity(i);

            // Get transform (should always exist)
            const TransformComponent* transform = transforms.Get(entity);
            if (!transform) continue;

            const SpriteRendererComponent& sprite = sprites.Data()[i];

            float rotationRadians = Math::ToRadians(transform->Rotation);

            // Resolve texture from library
            Texture* texture = nullptr;
            if (!sprite.TextureName.empty())
            {
                texture = textureLib.GetTexturePtr(sprite.TextureName);
            }

            // Build QuadSpec for this sprite
            QuadSpec spec;
            spec.x = transform->Position[0];
            spec.y = transform->Position[1];
            spec.z = transform->Position[2];
            spec.width = transform->Scale[0];
            spec.height = transform->Scale[1];
            spec.rotation = rotationRadians;
            spec.color[0] = sprite.Color[0];
            spec.color[1] = sprite.Color[1];
            spec.color[2] = sprite.Color[2];
            spec.color[3] = sprite.Color[3];

            if (texture)
            {
                spec.texture = texture;

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
                    spec.texCoords = texCoords;
                }
                else
                {
                    spec.tilingFactor = sprite.TilingFactor;
                }
            }

            Renderer2D::DrawQuad(spec);
        }
    }

    void Scene::RenderSpritesInstanced()
    {
        auto& textureLib = TextureLibrary::Get();
        auto& taskGraph = TaskGraph::Get();
        auto& sprites = GetStorage<SpriteRendererComponent>();
        auto& transforms = GetStorage<TransformComponent>();

        const size_t spriteCount = sprites.Size();
        if (spriteCount == 0)
            return;

        // Allocate instance buffer space (thread-safe)
        QuadInstanceData* instances = InstancedRenderer2D::AllocateInstances(static_cast<uint32_t>(spriteCount));
        if (!instances)
        {
            GG_CORE_WARN("Scene::RenderSpritesInstanced - Failed to allocate instance buffer");
            return;
        }

        // Get raw data pointers for parallel access
        const SpriteRendererComponent* spriteData = sprites.Data();
        const uint32_t whiteTexIndex = InstancedRenderer2D::GetWhiteTextureIndex();

        // Determine chunk size based on worker count
        const uint32_t workerCount = taskGraph.GetWorkerCount();
        const size_t minChunkSize = 256;
        const size_t chunkSize = std::max(minChunkSize, (spriteCount + workerCount) / (workerCount + 1));

        // Create parallel tasks for instance buffer preparation
        std::vector<TaskID> tasks;
        tasks.reserve((spriteCount + chunkSize - 1) / chunkSize);

        for (size_t start = 0; start < spriteCount; start += chunkSize)
        {
            size_t end = std::min(start + chunkSize, spriteCount);

            TaskID taskId = taskGraph.CreateTask("PrepareInstances",
                [this, &textureLib, &sprites, &transforms, spriteData, instances, whiteTexIndex, start, end]() -> TaskResult
                {
                    for (size_t i = start; i < end; ++i)
                    {
                        Entity entity = sprites.GetEntity(i);

                        // Get transform
                        const TransformComponent* transform = transforms.Get(entity);
                        if (!transform)
                            continue;

                        const SpriteRendererComponent& sprite = spriteData[i];
                        QuadInstanceData& inst = instances[i];

                        // Set transform (position, rotation, scale)
                        inst.SetTransform(
                            transform->Position[0],
                            transform->Position[1],
                            transform->Position[2],
                            Math::ToRadians(transform->Rotation),
                            transform->Scale[0],
                            transform->Scale[1]
                        );

                        // Set color
                        inst.SetColor(
                            sprite.Color[0],
                            sprite.Color[1],
                            sprite.Color[2],
                            sprite.Color[3]
                        );

                        // Resolve texture and UVs
                        uint32_t texIndex = whiteTexIndex;
                        float minU = 0.0f, minV = 0.0f, maxU = 1.0f, maxV = 1.0f;
                        float tiling = sprite.TilingFactor;

                        if (!sprite.TextureName.empty())
                        {
                            Texture* texture = textureLib.GetTexturePtr(sprite.TextureName);
                            if (texture)
                            {
                                texIndex = texture->GetBindlessIndex();

                                if (sprite.UseAtlas && texture->GetWidth() > 0 && texture->GetHeight() > 0)
                                {
                                    // Calculate atlas UVs
                                    float texWidth = static_cast<float>(texture->GetWidth());
                                    float texHeight = static_cast<float>(texture->GetHeight());

                                    minU = (sprite.AtlasCellX * sprite.AtlasCellWidth) / texWidth;
                                    minV = (sprite.AtlasCellY * sprite.AtlasCellHeight) / texHeight;
                                    maxU = minU + (sprite.AtlasSpriteWidth * sprite.AtlasCellWidth) / texWidth;
                                    maxV = minV + (sprite.AtlasSpriteHeight * sprite.AtlasCellHeight) / texHeight;
                                }
                            }
                        }

                        inst.SetTexCoords(minU, minV, maxU, maxV, texIndex, tiling);
                    }

                    return TaskResult::Success();
                },
                JobPriority::High
            );

            tasks.push_back(taskId);
        }

        // Wait for all preparation tasks to complete
        taskGraph.WaitAll(tasks);
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
        auto& cameras = GetStorage<CameraComponent>();
        for (size_t i = 0; i < cameras.Size(); i++)
        {
            if (cameras.Data()[i].Primary)
            {
                Entity entity = cameras.GetEntity(i);
                return EntityID{ entity, m_Generations[entity] };
            }
        }
        return InvalidEntityID;
    }

    void Scene::OnViewportResize(uint32_t width, uint32_t height)
    {
        // Update all cameras that don't have fixed aspect ratio
        auto& cameras = GetStorage<CameraComponent>();
        for (size_t i = 0; i < cameras.Size(); i++)
        {
            CameraComponent& cameraComp = cameras.Data()[i];
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

        auto& cameras = GetStorage<CameraComponent>();
        auto& transforms = GetStorage<TransformComponent>();

        for (size_t i = 0; i < cameras.Size(); i++)
        {
            CameraComponent& cameraComp = cameras.Data()[i];
            if (cameraComp.Primary)
            {
                Entity entity = cameras.GetEntity(i);

                // Get transform for this camera entity
                const TransformComponent* transform = transforms.Get(entity);
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

    void Scene::OnRenderInstanced(const Camera& camera, RHIRenderPassHandle renderPass,
                                   RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        InstancedRenderer2D::ResetStats();
        InstancedRenderer2D::BeginScene(camera, renderPass, cmd, width, height);

        // Tilemaps still use batched renderer for now (could be instanced in future)
        Renderer2D::ResetStats();
        Renderer2D::BeginScene(camera, renderPass, cmd, width, height);
        RenderTilemaps();
        Renderer2D::EndScene();

        // Sprites use instanced renderer with parallel preparation
        RenderSpritesInstanced();

        InstancedRenderer2D::EndScene();
    }

    void Scene::OnRenderRuntimeInstanced(RHIRenderPassHandle renderPass,
                                          RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        // Find primary camera
        SceneCamera* mainCamera = nullptr;
        Mat4 cameraTransform;

        auto& cameras = GetStorage<CameraComponent>();
        auto& transforms = GetStorage<TransformComponent>();

        for (size_t i = 0; i < cameras.Size(); i++)
        {
            CameraComponent& cameraComp = cameras.Data()[i];
            if (cameraComp.Primary)
            {
                Entity entity = cameras.GetEntity(i);
                const TransformComponent* transform = transforms.Get(entity);
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
            GG_CORE_WARN("Scene::OnRenderRuntimeInstanced - No primary camera found!");
            return;
        }

        InstancedRenderer2D::ResetStats();
        InstancedRenderer2D::BeginScene(*mainCamera, cameraTransform, renderPass, cmd, width, height);

        // Tilemaps still use batched renderer
        Renderer2D::ResetStats();
        Renderer2D::BeginScene(*mainCamera, cameraTransform, renderPass, cmd, width, height);
        RenderTilemaps();
        Renderer2D::EndScene();

        // Sprites use instanced renderer with parallel preparation
        RenderSpritesInstanced();

        InstancedRenderer2D::EndScene();
    }

}
