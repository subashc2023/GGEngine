#pragma once

#include "Entity.h"
#include "GUID.h"
#include "ComponentStorage.h"
#include "Components.h"
#include "GGEngine/Core/Core.h"
#include "GGEngine/Core/Timestep.h"
#include "GGEngine/Renderer/Camera.h"
#include "GGEngine/RHI/RHITypes.h"

#include <string>
#include <vector>
#include <unordered_map>
#include <typeindex>
#include <memory>
#include <shared_mutex>

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

        // Viewport resize (updates camera aspect ratios)
        void OnViewportResize(uint32_t width, uint32_t height);

        // Get primary camera entity
        EntityID GetPrimaryCameraEntity();

        // Iteration helpers
        const std::vector<Entity>& GetAllEntities() const { return m_Entities; }
        size_t GetEntityCount() const { return m_Entities.size(); }

        // Scene metadata
        const std::string& GetName() const { return m_Name; }
        void SetName(const std::string& name) { m_Name = name; }

        // Get storage for component type (for bulk iteration in systems)
        template<typename T>
        ComponentStorage<T>& GetStorage();

        template<typename T>
        const ComponentStorage<T>& GetStorage() const;

    private:
        // Allocate or reuse an entity slot, returns (index, generation)
        std::pair<Entity, uint32_t> AllocateEntitySlot();

        std::string m_Name;

        // Entity management
        std::vector<Entity> m_Entities;              // All active entity indices
        std::vector<uint32_t> m_Generations;         // Generation per entity slot
        std::vector<Entity> m_FreeList;              // Recycled entity indices

        // GUID lookup for serialization
        std::unordered_map<GUID, Entity, GUIDHash> m_GUIDToEntity;

        // Component registry - stores ComponentStorage<T> for any component type
        // Allows custom components without modifying engine code
        mutable std::unordered_map<std::type_index, std::unique_ptr<IComponentStorage>> m_ComponentRegistry;

        // Mutex protecting m_ComponentRegistry for thread-safe access during parallel system execution
        mutable std::shared_mutex m_RegistryMutex;

        // Helper to get or create storage for a component type
        template<typename T>
        ComponentStorage<T>& GetOrCreateStorage() const;
    };

    // Template implementations

    // Get or create storage for any component type (lazy initialization)
    // Thread-safe via double-checked locking pattern
    template<typename T>
    ComponentStorage<T>& Scene::GetOrCreateStorage() const
    {
        std::type_index typeIndex(typeid(T));

        // Fast path: check with shared (read) lock
        {
            std::shared_lock<std::shared_mutex> readLock(m_RegistryMutex);
            auto it = m_ComponentRegistry.find(typeIndex);
            if (it != m_ComponentRegistry.end())
            {
                return *static_cast<ComponentStorage<T>*>(it->second.get());
            }
        }

        // Slow path: acquire exclusive (write) lock and double-check
        {
            std::unique_lock<std::shared_mutex> writeLock(m_RegistryMutex);

            // Double-check: another thread may have created it while we waited for the lock
            auto it = m_ComponentRegistry.find(typeIndex);
            if (it != m_ComponentRegistry.end())
            {
                return *static_cast<ComponentStorage<T>*>(it->second.get());
            }

            // Create new storage for this component type
            auto storage = std::make_unique<ComponentStorage<T>>();
            auto* ptr = storage.get();
            m_ComponentRegistry[typeIndex] = std::move(storage);
            return *ptr;
        }
    }

    template<typename T>
    ComponentStorage<T>& Scene::GetStorage()
    {
        return GetOrCreateStorage<T>();
    }

    template<typename T>
    const ComponentStorage<T>& Scene::GetStorage() const
    {
        return GetOrCreateStorage<T>();
    }

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

}
