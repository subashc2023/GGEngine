#pragma once

#include "Entity.h"
#include "GUID.h"
#include "ComponentStorage.h"
#include "Components.h"
#include "GGEngine/Core/Core.h"
#include "GGEngine/Core/Timestep.h"
#include "GGEngine/Renderer/Camera.h"

#include <vulkan/vulkan.h>

#include <string>
#include <vector>
#include <unordered_map>

namespace GGEngine {

    class GG_API Scene
    {
    public:
        Scene(const std::string& name = "Untitled Scene");
        ~Scene() = default;

        // Entity lifecycle
        EntityID CreateEntity(const std::string& name = "Entity");
        EntityID CreateEntityWithGUID(const std::string& name, const GUID& guid);
        void DestroyEntity(EntityID entity);
        bool IsEntityValid(EntityID entity) const;

        // Scene management
        void Clear();

        // Get EntityID from index (for iteration)
        EntityID GetEntityID(Entity index) const;

        // Component access (templated for type safety)
        template<typename T>
        T& AddComponent(EntityID entity);

        template<typename T>
        T& AddComponent(EntityID entity, const T& component);

        template<typename T>
        void RemoveComponent(EntityID entity);

        template<typename T>
        bool HasComponent(EntityID entity) const;

        template<typename T>
        T* GetComponent(EntityID entity);

        template<typename T>
        const T* GetComponent(EntityID entity) const;

        // Entity lookup
        EntityID FindEntityByName(const std::string& name) const;
        EntityID FindEntityByGUID(const GUID& guid) const;

        // Scene-wide operations
        void OnUpdate(Timestep ts);
        void OnRender(const Camera& camera, VkRenderPass renderPass,
                      VkCommandBuffer cmd, uint32_t width, uint32_t height);

        // Iteration helpers
        const std::vector<Entity>& GetAllEntities() const { return m_Entities; }
        size_t GetEntityCount() const { return m_Entities.size(); }

        // Scene metadata
        const std::string& GetName() const { return m_Name; }
        void SetName(const std::string& name) { m_Name = name; }

    private:
        // Get storage for component type
        template<typename T>
        ComponentStorage<T>& GetStorage();

        template<typename T>
        const ComponentStorage<T>& GetStorage() const;

        std::string m_Name;

        // Entity management
        std::vector<Entity> m_Entities;              // All active entity indices
        std::vector<uint32_t> m_Generations;         // Generation per entity slot
        std::vector<Entity> m_FreeList;              // Recycled entity indices

        // GUID lookup for serialization
        std::unordered_map<GUID, Entity, GUIDHash> m_GUIDToEntity;

        // Component storage (SoA)
        ComponentStorage<TagComponent> m_Tags;
        ComponentStorage<TransformComponent> m_Transforms;
        ComponentStorage<SpriteRendererComponent> m_Sprites;
    };

    // Template implementations

    template<typename T>
    T& Scene::AddComponent(EntityID entity)
    {
        GG_CORE_ASSERT(IsEntityValid(entity), "Invalid entity");
        return GetStorage<T>().Add(entity.Index);
    }

    template<typename T>
    T& Scene::AddComponent(EntityID entity, const T& component)
    {
        GG_CORE_ASSERT(IsEntityValid(entity), "Invalid entity");
        return GetStorage<T>().Add(entity.Index, component);
    }

    template<typename T>
    void Scene::RemoveComponent(EntityID entity)
    {
        if (!IsEntityValid(entity)) return;
        GetStorage<T>().Remove(entity.Index);
    }

    template<typename T>
    bool Scene::HasComponent(EntityID entity) const
    {
        if (!IsEntityValid(entity)) return false;
        return GetStorage<T>().Has(entity.Index);
    }

    template<typename T>
    T* Scene::GetComponent(EntityID entity)
    {
        if (!IsEntityValid(entity)) return nullptr;
        return GetStorage<T>().Get(entity.Index);
    }

    template<typename T>
    const T* Scene::GetComponent(EntityID entity) const
    {
        if (!IsEntityValid(entity)) return nullptr;
        return GetStorage<T>().Get(entity.Index);
    }

    // Storage accessor inline specializations (required for DLL export)
    template<>
    inline ComponentStorage<TagComponent>& Scene::GetStorage<TagComponent>() { return m_Tags; }

    template<>
    inline ComponentStorage<TransformComponent>& Scene::GetStorage<TransformComponent>() { return m_Transforms; }

    template<>
    inline ComponentStorage<SpriteRendererComponent>& Scene::GetStorage<SpriteRendererComponent>() { return m_Sprites; }

    template<>
    inline const ComponentStorage<TagComponent>& Scene::GetStorage<TagComponent>() const { return m_Tags; }

    template<>
    inline const ComponentStorage<TransformComponent>& Scene::GetStorage<TransformComponent>() const { return m_Transforms; }

    template<>
    inline const ComponentStorage<SpriteRendererComponent>& Scene::GetStorage<SpriteRendererComponent>() const { return m_Sprites; }

}
