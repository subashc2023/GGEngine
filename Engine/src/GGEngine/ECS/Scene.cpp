#include "ggpch.h"
#include "Scene.h"
#include "GGEngine/Renderer/SceneCamera.h"

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

        // Clear all registered component storages (thread-safe)
        {
            std::shared_lock<std::shared_mutex> lock(m_RegistryMutex);
            for (auto& [typeIndex, storage] : m_ComponentRegistry)
            {
                storage->Clear();
            }
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

        // Remove all components from all registered storages (thread-safe)
        {
            std::shared_lock<std::shared_mutex> lock(m_RegistryMutex);
            for (auto& [typeIndex, storage] : m_ComponentRegistry)
            {
                storage->Remove(index);
            }
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

}
