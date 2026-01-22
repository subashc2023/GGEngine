#include "ggpch.h"
#include "Scene.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Asset/TextureLibrary.h"

#include <algorithm>

namespace GGEngine {

    Scene::Scene(const std::string& name)
        : m_Name(name)
    {
    }

    EntityID Scene::CreateEntity(const std::string& name)
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

        // Add required TagComponent
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

        // Remove from active entities list
        auto it = std::find(m_Entities.begin(), m_Entities.end(), index);
        if (it != m_Entities.end())
        {
            m_Entities.erase(it);
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

    void Scene::OnRender(const Camera& camera, RHIRenderPassHandle renderPass,
                         RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        Renderer2D::ResetStats();
        Renderer2D::BeginScene(camera, renderPass, cmd, width, height);

        auto& textureLib = TextureLibrary::Get();

        // Render all entities with SpriteRenderer component
        for (size_t i = 0; i < m_Sprites.Size(); i++)
        {
            Entity entity = m_Sprites.GetEntity(i);

            // Get transform (should always exist)
            const TransformComponent* transform = m_Transforms.Get(entity);
            if (!transform) continue;

            const SpriteRendererComponent& sprite = m_Sprites.Data()[i];

            float rotationRadians = transform->Rotation * 3.14159265359f / 180.0f;

            // Resolve texture from library
            Texture* texture = nullptr;
            if (!sprite.TextureName.empty())
            {
                texture = textureLib.GetTexturePtr(sprite.TextureName);
            }

            if (texture)
            {
                // Textured quad
                Renderer2D::DrawRotatedQuad(
                    transform->Position[0], transform->Position[1], transform->Position[2],
                    transform->Scale[0], transform->Scale[1],
                    rotationRadians,
                    texture,
                    sprite.TilingFactor,
                    sprite.Color[0], sprite.Color[1], sprite.Color[2], sprite.Color[3]
                );
            }
            else
            {
                // Colored quad
                Renderer2D::DrawRotatedQuad(
                    transform->Position[0], transform->Position[1], transform->Position[2],
                    transform->Scale[0], transform->Scale[1],
                    rotationRadians,
                    sprite.Color[0], sprite.Color[1], sprite.Color[2], sprite.Color[3]
                );
            }
        }

        Renderer2D::EndScene();
    }

}
