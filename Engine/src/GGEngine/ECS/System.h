#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Core/Timestep.h"

#include <vector>
#include <string>
#include <typeindex>

namespace GGEngine {

    class Scene;

    // =============================================================================
    // Component Access Modes
    // =============================================================================
    // Used to declare what access a system needs to each component type.
    // This enables automatic parallelization of systems with non-conflicting access.
    //
    enum class AccessMode : uint8_t
    {
        Read,       // Read-only access (multiple systems can read concurrently)
        Write,      // Read-write access (exclusive access required)
        Exclude     // Entities with this component are excluded from iteration
    };

    // =============================================================================
    // Component Requirement
    // =============================================================================
    // Describes a system's requirement for a specific component type.
    //
    struct ComponentRequirement
    {
        std::type_index Type;
        AccessMode Access;

        ComponentRequirement(std::type_index type, AccessMode access)
            : Type(type), Access(access) {}
    };

    // Helper to create a requirement for a component type
    template<typename T>
    ComponentRequirement Require(AccessMode access)
    {
        return ComponentRequirement(std::type_index(typeid(T)), access);
    }

    // =============================================================================
    // ISystem Interface
    // =============================================================================
    // Base class for all ECS systems. Systems declare their component requirements
    // and implement Execute() to process entities.
    //
    // The SystemScheduler uses GetRequirements() to determine which systems can
    // run in parallel (systems with non-conflicting access modes).
    //
    // Example:
    //   class MovementSystem : public ISystem {
    //       std::vector<ComponentRequirement> GetRequirements() const override {
    //           return {
    //               Require<TransformComponent>(AccessMode::Write),
    //               Require<VelocityComponent>(AccessMode::Read)
    //           };
    //       }
    //       void Execute(Scene& scene, float dt) override {
    //           // Update transform positions based on velocity
    //       }
    //   };
    //
    class GG_API ISystem
    {
    public:
        virtual ~ISystem() = default;

        // Return the component requirements for this system
        // Used for automatic dependency analysis and parallelization
        virtual std::vector<ComponentRequirement> GetRequirements() const = 0;

        // Execute the system logic for one frame
        // Called by SystemScheduler when all dependencies are satisfied
        virtual void Execute(Scene& scene, float deltaTime) = 0;

        // Optional: System name for debugging/profiling
        virtual const char* GetName() const { return "UnnamedSystem"; }

        // Optional: Whether this system supports parallel chunk execution
        // If true, ExecuteChunk() will be called from multiple threads
        virtual bool SupportsParallelChunks() const { return false; }

        // Optional: Execute a subset of entities (for parallel chunked execution)
        // Only called if SupportsParallelChunks() returns true
        virtual void ExecuteChunk(Scene& scene, float deltaTime,
                                  size_t startIndex, size_t count)
        {
            (void)scene; (void)deltaTime; (void)startIndex; (void)count;
        }

        // Optional: Called once when system is first registered
        virtual void OnRegister(Scene& scene) { (void)scene; }

        // Optional: Called once when system is removed
        virtual void OnUnregister(Scene& scene) { (void)scene; }
    };

}
