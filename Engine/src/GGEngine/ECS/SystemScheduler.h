#pragma once

#include "System.h"
#include "GGEngine/Core/Core.h"

#include <vector>
#include <memory>
#include <typeindex>
#include <unordered_map>
#include <unordered_set>

namespace GGEngine {

    class Scene;
    class TaskGraph;

    // =============================================================================
    // SystemScheduler
    // =============================================================================
    // Manages system registration and parallel execution based on component access.
    //
    // Systems are analyzed at registration time to build a dependency graph:
    // - Systems that only READ the same components can run in parallel
    // - Systems that WRITE to a component block other readers/writers of that type
    //
    // Example:
    //   SystemScheduler scheduler;
    //   scheduler.RegisterSystem<MovementSystem>();
    //   scheduler.RegisterSystem<RenderSystem>();
    //   scheduler.RegisterSystem<PhysicsSystem>();
    //
    //   // In game loop:
    //   scheduler.Execute(scene, deltaTime);  // Runs compatible systems in parallel
    //
    class GG_API SystemScheduler
    {
    public:
        SystemScheduler();
        ~SystemScheduler();

        // Non-copyable
        SystemScheduler(const SystemScheduler&) = delete;
        SystemScheduler& operator=(const SystemScheduler&) = delete;

        // Register a system (takes ownership)
        template<typename T, typename... Args>
        T* RegisterSystem(Args&&... args);

        // Register a pre-created system (takes ownership)
        void RegisterSystem(std::unique_ptr<ISystem> system);

        // Remove a system by type
        template<typename T>
        void UnregisterSystem();

        // Get a registered system by type
        template<typename T>
        T* GetSystem();

        // Execute all systems for one frame
        // Systems with non-conflicting access run in parallel
        void Execute(Scene& scene, float deltaTime);

        // Execute systems sequentially (for debugging)
        void ExecuteSequential(Scene& scene, float deltaTime);

        // Get number of registered systems
        size_t GetSystemCount() const { return m_Systems.size(); }

        // Check if a system type is registered
        template<typename T>
        bool HasSystem() const;

    private:
        // Internal system node with dependency info
        struct SystemNode
        {
            std::unique_ptr<ISystem> System;
            std::type_index TypeIndex;
            std::vector<ComponentRequirement> Requirements;

            // Systems that must complete before this one can start
            std::unordered_set<size_t> Dependencies;

            // Systems that depend on this one (blocked until this completes)
            std::unordered_set<size_t> Dependents;

            SystemNode(std::unique_ptr<ISystem> sys, std::type_index type)
                : System(std::move(sys))
                , TypeIndex(type)
                , Requirements(System->GetRequirements())
            {}
        };

        // Check if two systems have conflicting access
        bool HasConflict(const SystemNode& a, const SystemNode& b) const;

        // Rebuild dependency graph after adding/removing systems
        void RebuildDependencyGraph();

        // Topological sort for execution order
        std::vector<size_t> GetExecutionOrder() const;

        std::vector<std::unique_ptr<SystemNode>> m_Systems;
        std::unordered_map<std::type_index, size_t> m_TypeToIndex;
        bool m_DirtyGraph = false;
    };

    // =========================================================================
    // Template Implementations
    // =========================================================================

    template<typename T, typename... Args>
    T* SystemScheduler::RegisterSystem(Args&&... args)
    {
        static_assert(std::is_base_of_v<ISystem, T>,
            "T must derive from ISystem");

        auto typeIndex = std::type_index(typeid(T));

        // Check if already registered
        if (m_TypeToIndex.find(typeIndex) != m_TypeToIndex.end())
        {
            GG_CORE_WARN("System {} is already registered", typeid(T).name());
            return static_cast<T*>(m_Systems[m_TypeToIndex[typeIndex]]->System.get());
        }

        // Create and register
        auto system = std::make_unique<T>(std::forward<Args>(args)...);
        T* ptr = system.get();

        size_t index = m_Systems.size();
        m_Systems.push_back(std::make_unique<SystemNode>(std::move(system), typeIndex));
        m_TypeToIndex[typeIndex] = index;
        m_DirtyGraph = true;

        GG_CORE_TRACE("Registered system: {}", ptr->GetName());
        return ptr;
    }

    template<typename T>
    void SystemScheduler::UnregisterSystem()
    {
        auto typeIndex = std::type_index(typeid(T));
        auto it = m_TypeToIndex.find(typeIndex);

        if (it == m_TypeToIndex.end())
        {
            GG_CORE_WARN("System {} is not registered", typeid(T).name());
            return;
        }

        size_t index = it->second;

        // Remove from systems vector (swap with last and pop)
        if (index != m_Systems.size() - 1)
        {
            std::swap(m_Systems[index], m_Systems.back());
            // Update the index of the swapped system
            m_TypeToIndex[m_Systems[index]->TypeIndex] = index;
        }

        m_Systems.pop_back();
        m_TypeToIndex.erase(it);
        m_DirtyGraph = true;
    }

    template<typename T>
    T* SystemScheduler::GetSystem()
    {
        auto typeIndex = std::type_index(typeid(T));
        auto it = m_TypeToIndex.find(typeIndex);

        if (it == m_TypeToIndex.end())
            return nullptr;

        return static_cast<T*>(m_Systems[it->second]->System.get());
    }

    template<typename T>
    bool SystemScheduler::HasSystem() const
    {
        return m_TypeToIndex.find(std::type_index(typeid(T))) != m_TypeToIndex.end();
    }

}
