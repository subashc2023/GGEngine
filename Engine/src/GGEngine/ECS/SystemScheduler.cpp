#include "ggpch.h"
#include "SystemScheduler.h"
#include "Scene.h"
#include "GGEngine/Core/TaskGraph.h"
#include "GGEngine/Core/Profiler.h"

#include <queue>
#include <algorithm>

namespace GGEngine {

    SystemScheduler::SystemScheduler() = default;
    SystemScheduler::~SystemScheduler() = default;

    void SystemScheduler::RegisterSystem(std::unique_ptr<ISystem> system)
    {
        auto typeIndex = std::type_index(typeid(*system));

        // Check if already registered
        if (m_TypeToIndex.find(typeIndex) != m_TypeToIndex.end())
        {
            GG_CORE_WARN("System {} is already registered", system->GetName());
            return;
        }

        size_t index = m_Systems.size();
        GG_CORE_TRACE("Registered system: {}", system->GetName());

        m_Systems.push_back(std::make_unique<SystemNode>(std::move(system), typeIndex));
        m_TypeToIndex[typeIndex] = index;
        m_DirtyGraph = true;
    }

    bool SystemScheduler::HasConflict(const SystemNode& a, const SystemNode& b) const
    {
        // Check all pairs of requirements
        for (const auto& reqA : a.Requirements)
        {
            for (const auto& reqB : b.Requirements)
            {
                // Same component type?
                if (reqA.Type == reqB.Type)
                {
                    // Write-Write conflict
                    if (reqA.Access == AccessMode::Write && reqB.Access == AccessMode::Write)
                        return true;

                    // Write-Read or Read-Write conflict
                    if (reqA.Access == AccessMode::Write && reqB.Access == AccessMode::Read)
                        return true;

                    if (reqA.Access == AccessMode::Read && reqB.Access == AccessMode::Write)
                        return true;

                    // Read-Read is OK (no conflict)
                    // Exclude mode doesn't conflict with anything
                }
            }
        }

        return false;
    }

    void SystemScheduler::RebuildDependencyGraph()
    {
        if (!m_DirtyGraph)
            return;

        GG_PROFILE_FUNCTION();

        // Clear existing dependencies
        for (auto& node : m_Systems)
        {
            node->Dependencies.clear();
            node->Dependents.clear();
        }

        // Build dependency graph based on access conflicts
        // Systems that conflict must run sequentially
        for (size_t i = 0; i < m_Systems.size(); ++i)
        {
            for (size_t j = i + 1; j < m_Systems.size(); ++j)
            {
                if (HasConflict(*m_Systems[i], *m_Systems[j]))
                {
                    // j depends on i (i must complete before j starts)
                    m_Systems[j]->Dependencies.insert(i);
                    m_Systems[i]->Dependents.insert(j);
                }
            }
        }

        m_DirtyGraph = false;

        GG_CORE_TRACE("Rebuilt system dependency graph with {} systems", m_Systems.size());
    }

    std::vector<size_t> SystemScheduler::GetExecutionOrder() const
    {
        // Kahn's algorithm for topological sort
        std::vector<size_t> order;
        order.reserve(m_Systems.size());

        // Calculate in-degree for each node
        std::vector<size_t> inDegree(m_Systems.size(), 0);
        for (size_t i = 0; i < m_Systems.size(); ++i)
        {
            inDegree[i] = m_Systems[i]->Dependencies.size();
        }

        // Queue nodes with no dependencies
        std::queue<size_t> ready;
        for (size_t i = 0; i < m_Systems.size(); ++i)
        {
            if (inDegree[i] == 0)
                ready.push(i);
        }

        while (!ready.empty())
        {
            size_t current = ready.front();
            ready.pop();
            order.push_back(current);

            // Decrease in-degree for all dependents
            for (size_t dependent : m_Systems[current]->Dependents)
            {
                if (--inDegree[dependent] == 0)
                    ready.push(dependent);
            }
        }

        // Check for cycles
        if (order.size() != m_Systems.size())
        {
            GG_CORE_ERROR("Cycle detected in system dependency graph!");
            // Return what we have, though execution may be incorrect
        }

        return order;
    }

    void SystemScheduler::Execute(Scene& scene, float deltaTime)
    {
        if (m_Systems.empty())
            return;

        GG_PROFILE_FUNCTION();

        // Rebuild graph if dirty
        RebuildDependencyGraph();

        auto& taskGraph = TaskGraph::Get();

        // Track TaskIDs for each system
        std::vector<TaskID> systemTasks(m_Systems.size());

        // Get topological order
        auto order = GetExecutionOrder();

        // Create tasks in dependency order
        for (size_t idx : order)
        {
            auto& node = m_Systems[idx];

            // Collect dependency TaskIDs
            std::vector<TaskID> deps;
            for (size_t depIdx : node->Dependencies)
            {
                if (systemTasks[depIdx].IsValid())
                {
                    deps.push_back(systemTasks[depIdx]);
                }
            }

            // Create task for this system
            ISystem* systemPtr = node->System.get();

            systemTasks[idx] = taskGraph.CreateTask(
                std::string("System:") + systemPtr->GetName(),
                [systemPtr, &scene, deltaTime]() -> TaskResult {
                    GG_PROFILE_SCOPE(systemPtr->GetName());
                    systemPtr->Execute(scene, deltaTime);
                    return TaskResult::Success();
                },
                deps
            );
        }

        // Wait for all systems to complete
        for (size_t i = 0; i < m_Systems.size(); ++i)
        {
            if (systemTasks[i].IsValid())
            {
                taskGraph.Wait(systemTasks[i]);
            }
        }
    }

    void SystemScheduler::ExecuteSequential(Scene& scene, float deltaTime)
    {
        if (m_Systems.empty())
            return;

        GG_PROFILE_FUNCTION();

        // Rebuild graph if dirty (for consistency)
        RebuildDependencyGraph();

        // Get topological order
        auto order = GetExecutionOrder();

        // Execute each system in order
        for (size_t idx : order)
        {
            auto& node = m_Systems[idx];
            GG_PROFILE_SCOPE(node->System->GetName());
            node->System->Execute(scene, deltaTime);
        }
    }

}
