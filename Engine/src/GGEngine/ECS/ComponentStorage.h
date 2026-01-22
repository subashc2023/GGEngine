#pragma once

#include "Entity.h"
#include "GGEngine/Core/Core.h"

#include <vector>
#include <unordered_map>

namespace GGEngine {

    // SoA component storage for a single component type
    // Each component type has its own storage instance
    template<typename T>
    class ComponentStorage
    {
    public:
        // Add component to entity (returns reference)
        T& Add(Entity entity)
        {
            GG_CORE_ASSERT(!Has(entity), "Entity already has this component");

            size_t index = m_Components.size();
            m_EntityToIndex[entity] = index;
            m_IndexToEntity.push_back(entity);
            m_Components.push_back(T{});
            return m_Components.back();
        }

        // Add component with initial value
        T& Add(Entity entity, const T& component)
        {
            T& comp = Add(entity);
            comp = component;
            return comp;
        }

        // Remove component from entity (O(1) via swap-with-last)
        void Remove(Entity entity)
        {
            auto it = m_EntityToIndex.find(entity);
            if (it == m_EntityToIndex.end()) return;

            size_t indexToRemove = it->second;
            size_t lastIndex = m_Components.size() - 1;

            // Swap with last element
            if (indexToRemove != lastIndex)
            {
                m_Components[indexToRemove] = std::move(m_Components[lastIndex]);
                Entity lastEntity = m_IndexToEntity[lastIndex];
                m_IndexToEntity[indexToRemove] = lastEntity;
                m_EntityToIndex[lastEntity] = indexToRemove;
            }

            m_Components.pop_back();
            m_IndexToEntity.pop_back();
            m_EntityToIndex.erase(entity);
        }

        // Check if entity has component
        bool Has(Entity entity) const
        {
            return m_EntityToIndex.find(entity) != m_EntityToIndex.end();
        }

        // Get component (returns nullptr if not found)
        T* Get(Entity entity)
        {
            auto it = m_EntityToIndex.find(entity);
            if (it == m_EntityToIndex.end()) return nullptr;
            return &m_Components[it->second];
        }

        const T* Get(Entity entity) const
        {
            auto it = m_EntityToIndex.find(entity);
            if (it == m_EntityToIndex.end()) return nullptr;
            return &m_Components[it->second];
        }

        // Iteration support
        size_t Size() const { return m_Components.size(); }

        // Direct array access for cache-friendly iteration
        T* Data() { return m_Components.data(); }
        const T* Data() const { return m_Components.data(); }

        Entity GetEntity(size_t index) const { return m_IndexToEntity[index]; }

        // Clear all components
        void Clear()
        {
            m_Components.clear();
            m_EntityToIndex.clear();
            m_IndexToEntity.clear();
        }

    private:
        std::vector<T> m_Components;                          // Dense component array
        std::unordered_map<Entity, size_t> m_EntityToIndex;   // Sparse lookup
        std::vector<Entity> m_IndexToEntity;                  // Reverse lookup
    };

}
