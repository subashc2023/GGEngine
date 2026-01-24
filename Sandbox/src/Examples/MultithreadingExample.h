#pragma once

#include "Example.h"
#include "GGEngine/ECS/Scene.h"
#include "GGEngine/ECS/System.h"
#include "GGEngine/ECS/SystemScheduler.h"
#include "GGEngine/Asset/AssetManager.h"
#include "GGEngine/Asset/AssetHandle.h"
#include "GGEngine/Asset/Texture.h"

#include <chrono>

// =============================================================================
// Movement System - Writes TransformComponent
// =============================================================================
class MovementSystem : public GGEngine::ISystem
{
public:
    std::vector<GGEngine::ComponentRequirement> GetRequirements() const override;
    void Execute(GGEngine::Scene& scene, float deltaTime) override;
    const char* GetName() const override { return "MovementSystem"; }

    float Speed = 2.0f;
    float BoundsX = 8.0f;
    float BoundsY = 5.0f;
    int ExtraIterations = 0;  // Simulate heavy work
};

// =============================================================================
// Rotation System - Writes TransformComponent (CONFLICT with Movement!)
// This demonstrates that conflicting systems run sequentially
// =============================================================================
class RotationSystem : public GGEngine::ISystem
{
public:
    std::vector<GGEngine::ComponentRequirement> GetRequirements() const override;
    void Execute(GGEngine::Scene& scene, float deltaTime) override;
    const char* GetName() const override { return "RotationSystem"; }

    float RotationSpeed = 90.0f;  // degrees per second
};

// =============================================================================
// Color Cycle System - Writes SpriteRendererComponent (NO conflict with above)
// Can run in parallel with Movement/Rotation
// =============================================================================
class ColorCycleSystem : public GGEngine::ISystem
{
public:
    std::vector<GGEngine::ComponentRequirement> GetRequirements() const override;
    void Execute(GGEngine::Scene& scene, float deltaTime) override;
    const char* GetName() const override { return "ColorCycleSystem"; }

    float CycleSpeed = 1.0f;
    int ExtraIterations = 0;  // Simulate heavy work
};

// =============================================================================
// Multithreading Example
// =============================================================================
class MultithreadingExample : public Example
{
public:
    MultithreadingExample();

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate(GGEngine::Timestep ts, const GGEngine::Camera& camera) override;
    void OnRender(const GGEngine::Camera& camera) override;
    void OnImGuiRender() override;

private:
    void CreateEntities(int count);
    void RunTaskGraphBenchmark();

    GGEngine::Scope<GGEngine::Scene> m_Scene;
    GGEngine::SystemScheduler m_Scheduler;

    // System pointers for ImGui control
    MovementSystem* m_MovementSystem = nullptr;
    RotationSystem* m_RotationSystem = nullptr;
    ColorCycleSystem* m_ColorCycleSystem = nullptr;

    // Settings
    int m_EntityCount = 100;
    bool m_UseParallelExecution = true;
    bool m_UseInstancedRendering = true;  // Toggle between batched and instanced
    bool m_EnableMovement = true;
    bool m_EnableRotation = true;
    bool m_EnableColorCycle = true;
    int m_WorkloadIterations = 0;  // Extra iterations per entity to simulate heavy work

    // Timing stats
    float m_LastUpdateTimeMs = 0.0f;
    float m_LastRenderTimeMs = 0.0f;  // Track render time
    float m_SequentialTimeMs = 0.0f;
    float m_ParallelTimeMs = 0.0f;

    // Hot reload demo
    GGEngine::AssetHandle<GGEngine::Texture> m_HotReloadTexture;
    int m_ReloadCount = 0;
    std::string m_LastReloadTime;

    // Benchmark state
    bool m_BenchmarkRunning = false;
    int m_BenchmarkIterations = 0;
    float m_BenchmarkSequentialTotal = 0.0f;
    float m_BenchmarkParallelTotal = 0.0f;
};
