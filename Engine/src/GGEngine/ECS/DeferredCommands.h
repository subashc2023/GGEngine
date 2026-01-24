#pragma once

#include "Entity.h"
#include "Scene.h"
#include "GGEngine/Core/Core.h"

#include <vector>
#include <string>
#include <mutex>
#include <functional>
#include <any>
#include <typeindex>

namespace GGEngine {

    // =============================================================================
    // DeferredCommands
    // =============================================================================
    // Thread-safe command buffer for ECS modifications. Worker threads queue
    // commands that are applied on the main thread during Flush().
    //
    // This pattern allows systems running in parallel to request structural
    // changes (create/destroy entities, add/remove components) without data races.
    //
    // Usage:
    //   // Worker thread:
    //   DeferredCommands& cmds = DeferredCommands::Get();
    //   cmds.DestroyEntity(deadEnemy);
    //   cmds.AddComponent<ExplosionComponent>(position, ExplosionComponent{...});
    //
    //   // Main thread (after parallel systems complete):
    //   cmds.Flush(scene);
    //
    class GG_API DeferredCommands
    {
    public:
        static DeferredCommands& Get();

        // -------------------------------------------------------------------------
        // Thread-Safe Command Queueing
        // -------------------------------------------------------------------------

        // Queue entity creation (returns a placeholder ID that will be resolved on Flush)
        // Note: The returned ID is a placeholder and cannot be used until after Flush()
        void CreateEntity(const std::string& name = "Entity");

        // Queue entity destruction
        void DestroyEntity(EntityID entity);

        // Queue component addition
        template<typename T>
        void AddComponent(EntityID entity, const T& component);

        // Queue component removal
        template<typename T>
        void RemoveComponent(EntityID entity);

        // Queue a custom command (for complex operations)
        // The callback will be invoked with the scene during Flush()
        void QueueCommand(std::function<void(Scene&)> command);

        // -------------------------------------------------------------------------
        // Main Thread Operations
        // -------------------------------------------------------------------------

        // Apply all queued commands to the scene (call from main thread)
        void Flush(Scene& scene);

        // Get the number of pending commands
        size_t GetPendingCount() const;

        // Clear all pending commands without executing them
        void Clear();

    private:
        DeferredCommands() = default;
        ~DeferredCommands() = default;
        DeferredCommands(const DeferredCommands&) = delete;
        DeferredCommands& operator=(const DeferredCommands&) = delete;

        // Command types
        enum class CommandType : uint8_t
        {
            CreateEntity,
            DestroyEntity,
            AddComponent,
            RemoveComponent,
            Custom
        };

        // Generic command storage
        struct Command
        {
            CommandType Type;
            EntityID Entity;                            // For entity-specific commands
            std::string Name;                           // For CreateEntity
            std::type_index ComponentType;              // For component commands
            std::any ComponentData;                     // For AddComponent
            std::function<void(Scene&)> CustomCommand;  // For custom commands

            Command()
                : Type(CommandType::CreateEntity)
                , Entity{}
                , ComponentType(typeid(void))
            {}
        };

        // Type-erased component adder
        using ComponentAdder = std::function<void(Scene&, EntityID, const std::any&)>;
        using ComponentRemover = std::function<void(Scene&, EntityID)>;

        // Component type registry for deferred operations
        std::unordered_map<std::type_index, ComponentAdder> m_ComponentAdders;
        std::unordered_map<std::type_index, ComponentRemover> m_ComponentRemovers;

        // Command queue
        std::vector<Command> m_Commands;
        mutable std::mutex m_Mutex;

        // Register component type handlers (called lazily)
        template<typename T>
        void EnsureComponentRegistered();
    };

    // =========================================================================
    // Template Implementations
    // =========================================================================

    template<typename T>
    void DeferredCommands::AddComponent(EntityID entity, const T& component)
    {
        EnsureComponentRegistered<T>();

        std::lock_guard<std::mutex> lock(m_Mutex);

        Command cmd;
        cmd.Type = CommandType::AddComponent;
        cmd.Entity = entity;
        cmd.ComponentType = std::type_index(typeid(T));
        cmd.ComponentData = component;

        m_Commands.push_back(std::move(cmd));
    }

    template<typename T>
    void DeferredCommands::RemoveComponent(EntityID entity)
    {
        EnsureComponentRegistered<T>();

        std::lock_guard<std::mutex> lock(m_Mutex);

        Command cmd;
        cmd.Type = CommandType::RemoveComponent;
        cmd.Entity = entity;
        cmd.ComponentType = std::type_index(typeid(T));

        m_Commands.push_back(std::move(cmd));
    }

    template<typename T>
    void DeferredCommands::EnsureComponentRegistered()
    {
        std::lock_guard<std::mutex> lock(m_Mutex);

        auto typeIndex = std::type_index(typeid(T));

        if (m_ComponentAdders.find(typeIndex) == m_ComponentAdders.end())
        {
            // Register adder
            m_ComponentAdders[typeIndex] = [](Scene& scene, EntityID entity, const std::any& data) {
                const T& component = std::any_cast<const T&>(data);
                scene.AddComponent<T>(entity, component);
            };

            // Register remover
            m_ComponentRemovers[typeIndex] = [](Scene& scene, EntityID entity) {
                scene.RemoveComponent<T>(entity);
            };
        }
    }

}
