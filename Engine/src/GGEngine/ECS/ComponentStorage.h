#pragma once

#include "Entity.h"
#include "GGEngine/Core/Core.h"

#include <vector>
#include <unordered_map>
#include <shared_mutex>

namespace GGEngine {

    // SoA component storage for a single component type
    // Each component type has its own storage instance
    //
    // Thread Safety:
    // - Use LockRead() for concurrent read-only access from multiple threads
    // - Use LockWrite() for exclusive write access
    // - Direct method calls (Add, Remove, Get) are NOT thread-safe
    //   and should only be used when you have external synchronization
    //
    template<typename T>
    class ComponentStorage
    {
    public:
        class ReadLock;
        class WriteLock;
        friend class ReadLock;
        friend class WriteLock;
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

        // =========================================================================
        // Thread-Safe Access (RAII locks)
        // =========================================================================

        // RAII read lock - allows concurrent read access from multiple threads
        class ReadLock
        {
        public:
            explicit ReadLock(const ComponentStorage& storage)
                : m_Storage(storage)
                , m_Lock(storage.m_Mutex)
            {
            }

            // Non-copyable, movable
            ReadLock(const ReadLock&) = delete;
            ReadLock& operator=(const ReadLock&) = delete;
            ReadLock(ReadLock&&) = default;
            ReadLock& operator=(ReadLock&&) = default;

            const T* Data() const { return m_Storage.m_Components.data(); }
            size_t Size() const { return m_Storage.m_Components.size(); }
            Entity GetEntity(size_t index) const { return m_Storage.m_IndexToEntity[index]; }

            const T* Get(Entity entity) const
            {
                auto it = m_Storage.m_EntityToIndex.find(entity);
                if (it == m_Storage.m_EntityToIndex.end()) return nullptr;
                return &m_Storage.m_Components[it->second];
            }

            bool Has(Entity entity) const
            {
                return m_Storage.m_EntityToIndex.find(entity) != m_Storage.m_EntityToIndex.end();
            }

        private:
            const ComponentStorage& m_Storage;
            std::shared_lock<std::shared_mutex> m_Lock;
        };

        // RAII write lock - exclusive access for modifications
        class WriteLock
        {
        public:
            explicit WriteLock(ComponentStorage& storage)
                : m_Storage(storage)
                , m_Lock(storage.m_Mutex)
            {
            }

            // Non-copyable, movable
            WriteLock(const WriteLock&) = delete;
            WriteLock& operator=(const WriteLock&) = delete;
            WriteLock(WriteLock&&) = default;
            WriteLock& operator=(WriteLock&&) = default;

            T* Data() { return m_Storage.m_Components.data(); }
            const T* Data() const { return m_Storage.m_Components.data(); }
            size_t Size() const { return m_Storage.m_Components.size(); }
            Entity GetEntity(size_t index) const { return m_Storage.m_IndexToEntity[index]; }

            T* Get(Entity entity)
            {
                auto it = m_Storage.m_EntityToIndex.find(entity);
                if (it == m_Storage.m_EntityToIndex.end()) return nullptr;
                return &m_Storage.m_Components[it->second];
            }

            bool Has(Entity entity) const
            {
                return m_Storage.m_EntityToIndex.find(entity) != m_Storage.m_EntityToIndex.end();
            }

            T& Add(Entity entity)
            {
                GG_CORE_ASSERT(!Has(entity), "Entity already has this component");

                size_t index = m_Storage.m_Components.size();
                m_Storage.m_EntityToIndex[entity] = index;
                m_Storage.m_IndexToEntity.push_back(entity);
                m_Storage.m_Components.push_back(T{});
                return m_Storage.m_Components.back();
            }

            T& Add(Entity entity, const T& component)
            {
                T& comp = Add(entity);
                comp = component;
                return comp;
            }

            void Remove(Entity entity)
            {
                auto it = m_Storage.m_EntityToIndex.find(entity);
                if (it == m_Storage.m_EntityToIndex.end()) return;

                size_t indexToRemove = it->second;
                size_t lastIndex = m_Storage.m_Components.size() - 1;

                if (indexToRemove != lastIndex)
                {
                    m_Storage.m_Components[indexToRemove] = std::move(m_Storage.m_Components[lastIndex]);
                    Entity lastEntity = m_Storage.m_IndexToEntity[lastIndex];
                    m_Storage.m_IndexToEntity[indexToRemove] = lastEntity;
                    m_Storage.m_EntityToIndex[lastEntity] = indexToRemove;
                }

                m_Storage.m_Components.pop_back();
                m_Storage.m_IndexToEntity.pop_back();
                m_Storage.m_EntityToIndex.erase(entity);
            }

            void Clear()
            {
                m_Storage.m_Components.clear();
                m_Storage.m_EntityToIndex.clear();
                m_Storage.m_IndexToEntity.clear();
            }

        private:
            ComponentStorage& m_Storage;
            std::unique_lock<std::shared_mutex> m_Lock;
        };

        // Acquire a read lock (multiple readers allowed)
        ReadLock LockRead() const { return ReadLock(*this); }

        // Acquire a write lock (exclusive access)
        WriteLock LockWrite() { return WriteLock(*this); }

    private:
        std::vector<T> m_Components;                          // Dense component array
        std::unordered_map<Entity, size_t> m_EntityToIndex;   // Sparse lookup
        std::vector<Entity> m_IndexToEntity;                  // Reverse lookup
        mutable std::shared_mutex m_Mutex;                    // Reader-writer lock
    };

}
