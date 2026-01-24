#include "ggpch.h"
#include "DeferredCommands.h"
#include "Scene.h"
#include "GGEngine/Core/Log.h"

namespace GGEngine {

    DeferredCommands& DeferredCommands::Get()
    {
        static DeferredCommands instance;
        return instance;
    }

    void DeferredCommands::CreateEntity(const std::string& name)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);

        Command cmd;
        cmd.Type = CommandType::CreateEntity;
        cmd.Name = name;

        m_Commands.push_back(std::move(cmd));
    }

    void DeferredCommands::DestroyEntity(EntityID entity)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);

        Command cmd;
        cmd.Type = CommandType::DestroyEntity;
        cmd.Entity = entity;

        m_Commands.push_back(std::move(cmd));
    }

    void DeferredCommands::QueueCommand(std::function<void(Scene&)> command)
    {
        std::lock_guard<std::mutex> lock(m_Mutex);

        Command cmd;
        cmd.Type = CommandType::Custom;
        cmd.CustomCommand = std::move(command);

        m_Commands.push_back(std::move(cmd));
    }

    void DeferredCommands::Flush(Scene& scene)
    {
        // Swap to local vector to minimize lock time
        std::vector<Command> commands;
        {
            std::lock_guard<std::mutex> lock(m_Mutex);
            std::swap(commands, m_Commands);
        }

        if (commands.empty())
            return;

        GG_CORE_TRACE("Flushing {} deferred commands", commands.size());

        // Execute all commands
        for (auto& cmd : commands)
        {
            switch (cmd.Type)
            {
                case CommandType::CreateEntity:
                {
                    scene.CreateEntity(cmd.Name);
                    break;
                }

                case CommandType::DestroyEntity:
                {
                    if (scene.IsEntityValid(cmd.Entity))
                    {
                        scene.DestroyEntity(cmd.Entity);
                    }
                    else
                    {
                        GG_CORE_WARN("DeferredCommands: Attempted to destroy invalid entity");
                    }
                    break;
                }

                case CommandType::AddComponent:
                {
                    auto it = m_ComponentAdders.find(cmd.ComponentType);
                    if (it != m_ComponentAdders.end() && scene.IsEntityValid(cmd.Entity))
                    {
                        it->second(scene, cmd.Entity, cmd.ComponentData);
                    }
                    else if (!scene.IsEntityValid(cmd.Entity))
                    {
                        GG_CORE_WARN("DeferredCommands: Attempted to add component to invalid entity");
                    }
                    break;
                }

                case CommandType::RemoveComponent:
                {
                    auto it = m_ComponentRemovers.find(cmd.ComponentType);
                    if (it != m_ComponentRemovers.end() && scene.IsEntityValid(cmd.Entity))
                    {
                        it->second(scene, cmd.Entity);
                    }
                    else if (!scene.IsEntityValid(cmd.Entity))
                    {
                        GG_CORE_WARN("DeferredCommands: Attempted to remove component from invalid entity");
                    }
                    break;
                }

                case CommandType::Custom:
                {
                    if (cmd.CustomCommand)
                    {
                        cmd.CustomCommand(scene);
                    }
                    break;
                }
            }
        }
    }

    size_t DeferredCommands::GetPendingCount() const
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        return m_Commands.size();
    }

    void DeferredCommands::Clear()
    {
        std::lock_guard<std::mutex> lock(m_Mutex);
        m_Commands.clear();
    }

}
