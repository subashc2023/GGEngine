#include "MultithreadingExample.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Renderer/InstancedRenderer2D.h"
#include "GGEngine/ECS/Components/TransformComponent.h"
#include "GGEngine/ECS/Components/SpriteRendererComponent.h"
#include "GGEngine/ECS/ComponentStorage.h"
#include "GGEngine/Core/Profiler.h"
#include "GGEngine/Core/TaskGraph.h"
#include "GGEngine/Core/Log.h"
#include "GGEngine/Core/Math.h"
#include "GGEngine/Asset/TextureLibrary.h"

#include <imgui.h>
#include <cstdlib>
#include <cmath>
#include <ctime>

// =============================================================================
// MovementSystem Implementation
// =============================================================================

std::vector<GGEngine::ComponentRequirement> MovementSystem::GetRequirements() const
{
    return {
        GGEngine::Require<GGEngine::TransformComponent>(GGEngine::AccessMode::Write)
    };
}

void MovementSystem::Execute(GGEngine::Scene& scene, float deltaTime)
{
    GG_PROFILE_SCOPE("MovementSystem");

    const auto& entities = scene.GetAllEntities();
    for (GGEngine::Entity index : entities)
    {
        auto entityID = scene.GetEntityID(index);
        auto* transform = scene.GetComponent<GGEngine::TransformComponent>(entityID);
        if (!transform) continue;

        // Simple oscillating movement based on entity index
        float phase = static_cast<float>(index) * 0.5f;
        transform->Position[0] += std::sin(phase + deltaTime * Speed) * deltaTime * 0.5f;
        transform->Position[1] += std::cos(phase + deltaTime * Speed * 0.7f) * deltaTime * 0.3f;

        // Simulate heavy work (e.g., physics, pathfinding)
        volatile float dummy = 0.0f;
        for (int i = 0; i < ExtraIterations; i++)
        {
            dummy += std::sin(static_cast<float>(i) * 0.01f) * std::cos(static_cast<float>(i) * 0.02f);
        }

        // Bounce off bounds
        if (transform->Position[0] > BoundsX) transform->Position[0] = BoundsX;
        if (transform->Position[0] < -BoundsX) transform->Position[0] = -BoundsX;
        if (transform->Position[1] > BoundsY) transform->Position[1] = BoundsY;
        if (transform->Position[1] < -BoundsY) transform->Position[1] = -BoundsY;
    }
}

// =============================================================================
// RotationSystem Implementation
// =============================================================================

std::vector<GGEngine::ComponentRequirement> RotationSystem::GetRequirements() const
{
    return {
        GGEngine::Require<GGEngine::TransformComponent>(GGEngine::AccessMode::Write)
    };
}

void RotationSystem::Execute(GGEngine::Scene& scene, float deltaTime)
{
    GG_PROFILE_SCOPE("RotationSystem");

    const auto& entities = scene.GetAllEntities();
    for (GGEngine::Entity index : entities)
    {
        auto entityID = scene.GetEntityID(index);
        auto* transform = scene.GetComponent<GGEngine::TransformComponent>(entityID);
        if (!transform) continue;

        // Rotate based on entity index for variety
        float rotSpeed = RotationSpeed * (1.0f + static_cast<float>(index % 5) * 0.2f);
        transform->Rotation += rotSpeed * deltaTime;

        // Keep rotation in [0, 360)
        while (transform->Rotation >= 360.0f) transform->Rotation -= 360.0f;
        while (transform->Rotation < 0.0f) transform->Rotation += 360.0f;
    }
}

// =============================================================================
// ColorCycleSystem Implementation
// =============================================================================

std::vector<GGEngine::ComponentRequirement> ColorCycleSystem::GetRequirements() const
{
    return {
        GGEngine::Require<GGEngine::SpriteRendererComponent>(GGEngine::AccessMode::Write)
    };
}

void ColorCycleSystem::Execute(GGEngine::Scene& scene, float deltaTime)
{
    GG_PROFILE_SCOPE("ColorCycleSystem");

    static float time = 0.0f;
    time += deltaTime * CycleSpeed;

    const auto& entities = scene.GetAllEntities();
    for (GGEngine::Entity index : entities)
    {
        auto entityID = scene.GetEntityID(index);
        auto* sprite = scene.GetComponent<GGEngine::SpriteRendererComponent>(entityID);
        if (!sprite) continue;

        // Cycle hue based on time and entity index
        float hue = std::fmod(time + static_cast<float>(index) * 0.1f, 1.0f);

        // Simple HSV to RGB (assumes S=1, V=1)
        float h = hue * 6.0f;
        float c = 1.0f;
        float x = c * (1.0f - std::fabs(std::fmod(h, 2.0f) - 1.0f));

        float r = 0, g = 0, b = 0;
        if (h < 1) { r = c; g = x; }
        else if (h < 2) { r = x; g = c; }
        else if (h < 3) { g = c; b = x; }
        else if (h < 4) { g = x; b = c; }
        else if (h < 5) { r = x; b = c; }
        else { r = c; b = x; }

        // Simulate heavy work (e.g., texture sampling, shader computation)
        volatile float dummy = 0.0f;
        for (int i = 0; i < ExtraIterations; i++)
        {
            dummy += std::sin(static_cast<float>(i) * 0.01f) * std::cos(static_cast<float>(i) * 0.02f);
        }

        sprite->Color[0] = r;
        sprite->Color[1] = g;
        sprite->Color[2] = b;
    }
}

// =============================================================================
// MultithreadingExample Implementation
// =============================================================================

MultithreadingExample::MultithreadingExample()
    : Example("Multithreading",
              "Demonstrates parallel ECS systems, TaskGraph, and asset hot-reloading")
{
}

void MultithreadingExample::OnAttach()
{
    m_Scene = GGEngine::CreateScope<GGEngine::Scene>("Multithreading Demo");

    // Register systems
    m_MovementSystem = m_Scheduler.RegisterSystem<MovementSystem>();
    m_RotationSystem = m_Scheduler.RegisterSystem<RotationSystem>();
    m_ColorCycleSystem = m_Scheduler.RegisterSystem<ColorCycleSystem>();

    // Create initial entities
    CreateEntities(m_EntityCount);

    // Setup hot reload demo
#ifndef GG_DIST
    auto& assetManager = GGEngine::AssetManager::Get();
    assetManager.EnableHotReload(true);
    assetManager.WatchDirectory("textures");

    // Try to load a test texture (create one if it doesn't exist)
    m_HotReloadTexture = assetManager.LoadTextureAsync("textures/hotreload_test.png");

    // Register reload callback
    if (m_HotReloadTexture.IsValid())
    {
        assetManager.OnAssetReload(m_HotReloadTexture.GetID(), [this](GGEngine::AssetID) {
            m_ReloadCount++;
            auto now = std::chrono::system_clock::now();
            auto time = std::chrono::system_clock::to_time_t(now);
            m_LastReloadTime = std::ctime(&time);
            // Remove trailing newline
            if (!m_LastReloadTime.empty() && m_LastReloadTime.back() == '\n')
                m_LastReloadTime.pop_back();
            GG_INFO("Hot reload detected! Count: {}", m_ReloadCount);
        });
    }
#endif

    GG_INFO("MultithreadingExample attached with {} entities", m_EntityCount);
}

void MultithreadingExample::OnDetach()
{
    m_Scene.reset();
    m_HotReloadTexture = {};
    m_ReloadCount = 0;
}

void MultithreadingExample::CreateEntities(int count)
{
    m_Scene->Clear();

    std::srand(static_cast<unsigned>(std::time(nullptr)));

    for (int i = 0; i < count; i++)
    {
        std::string name = "Entity_" + std::to_string(i);
        auto entity = m_Scene->CreateEntity(name);

        // Random position
        auto* transform = m_Scene->GetComponent<GGEngine::TransformComponent>(entity);
        if (transform)
        {
            transform->Position[0] = (static_cast<float>(rand()) / RAND_MAX * 2.0f - 1.0f) * 7.0f;
            transform->Position[1] = (static_cast<float>(rand()) / RAND_MAX * 2.0f - 1.0f) * 4.0f;
            transform->Rotation = static_cast<float>(rand()) / RAND_MAX * 360.0f;
            float scale = 0.2f + static_cast<float>(rand()) / RAND_MAX * 0.3f;
            transform->Scale[0] = scale;
            transform->Scale[1] = scale;
        }

        // Random color sprite
        GGEngine::SpriteRendererComponent sprite;
        sprite.Color[0] = static_cast<float>(rand()) / RAND_MAX;
        sprite.Color[1] = static_cast<float>(rand()) / RAND_MAX;
        sprite.Color[2] = static_cast<float>(rand()) / RAND_MAX;
        sprite.Color[3] = 0.9f;
        m_Scene->AddComponent<GGEngine::SpriteRendererComponent>(entity, sprite);
    }
}

void MultithreadingExample::OnUpdate(GGEngine::Timestep ts, const GGEngine::Camera& /*camera*/)
{
    // Update system workload settings
    if (m_MovementSystem) m_MovementSystem->ExtraIterations = m_WorkloadIterations;
    if (m_ColorCycleSystem) m_ColorCycleSystem->ExtraIterations = m_WorkloadIterations;

    auto startTime = std::chrono::high_resolution_clock::now();

    // Execute systems
    if (m_UseParallelExecution)
    {
        GG_PROFILE_SCOPE("SystemScheduler::Execute (Parallel)");
        m_Scheduler.Execute(*m_Scene, ts.GetSeconds());
    }
    else
    {
        GG_PROFILE_SCOPE("SystemScheduler::ExecuteSequential");
        m_Scheduler.ExecuteSequential(*m_Scene, ts.GetSeconds());
    }

    auto endTime = std::chrono::high_resolution_clock::now();
    auto duration = std::chrono::duration_cast<std::chrono::microseconds>(endTime - startTime);
    m_LastUpdateTimeMs = static_cast<float>(duration.count()) / 1000.0f;

    // Benchmark if running
    if (m_BenchmarkRunning)
    {
        if (m_UseParallelExecution)
            m_BenchmarkParallelTotal += m_LastUpdateTimeMs;
        else
            m_BenchmarkSequentialTotal += m_LastUpdateTimeMs;

        m_BenchmarkIterations++;

        // Alternate between modes every 60 frames
        if (m_BenchmarkIterations % 60 == 0)
        {
            m_UseParallelExecution = !m_UseParallelExecution;
        }

        // End benchmark after 240 frames (2 cycles each mode)
        if (m_BenchmarkIterations >= 240)
        {
            m_SequentialTimeMs = m_BenchmarkSequentialTotal / 120.0f;
            m_ParallelTimeMs = m_BenchmarkParallelTotal / 120.0f;
            m_BenchmarkRunning = false;
            GG_INFO("Benchmark complete: Sequential={:.3f}ms, Parallel={:.3f}ms",
                    m_SequentialTimeMs, m_ParallelTimeMs);
        }
    }
}

void MultithreadingExample::OnRender(const GGEngine::Camera& camera)
{
    GG_PROFILE_SCOPE("MultithreadingExample::Render");

    auto startTime = std::chrono::high_resolution_clock::now();

    if (m_UseInstancedRendering)
    {
        GG_PROFILE_SCOPE("InstancedRenderer2D Path");

        GGEngine::InstancedRenderer2D::ResetStats();
        GGEngine::InstancedRenderer2D::BeginScene(camera);

        // Get component storage for direct access
        const auto& entities = m_Scene->GetAllEntities();
        const uint32_t whiteTexIndex = GGEngine::InstancedRenderer2D::GetWhiteTextureIndex();

        // Allocate instances for all entities
        GGEngine::QuadInstanceData* instances = GGEngine::InstancedRenderer2D::AllocateInstances(
            static_cast<uint32_t>(entities.size()));

        if (instances)
        {
            // Use TaskGraph for parallel instance preparation
            auto& taskGraph = GGEngine::TaskGraph::Get();
            const size_t entityCount = entities.size();
            const uint32_t workerCount = taskGraph.GetWorkerCount();
            const size_t chunkSize = std::max(size_t(256), (entityCount + workerCount) / (workerCount + 1));

            std::vector<GGEngine::TaskID> tasks;

            for (size_t start = 0; start < entityCount; start += chunkSize)
            {
                size_t end = std::min(start + chunkSize, entityCount);

                GGEngine::TaskID taskId = taskGraph.CreateTask("PrepareQuadInstances",
                    [this, &entities, instances, whiteTexIndex, start, end]() -> GGEngine::TaskResult
                    {
                        for (size_t i = start; i < end; ++i)
                        {
                            GGEngine::Entity index = entities[i];
                            auto entityID = m_Scene->GetEntityID(index);

                            const auto* transform = m_Scene->GetComponent<GGEngine::TransformComponent>(entityID);
                            const auto* sprite = m_Scene->GetComponent<GGEngine::SpriteRendererComponent>(entityID);

                            if (transform && sprite)
                            {
                                GGEngine::QuadInstanceData& inst = instances[i];
                                inst.SetTransform(
                                    transform->Position[0],
                                    transform->Position[1],
                                    transform->Position[2],
                                    GGEngine::Math::ToRadians(transform->Rotation),
                                    transform->Scale[0],
                                    transform->Scale[1]
                                );
                                inst.SetColor(
                                    sprite->Color[0],
                                    sprite->Color[1],
                                    sprite->Color[2],
                                    sprite->Color[3]
                                );
                                inst.SetFullTexture(whiteTexIndex, 1.0f);
                            }
                        }
                        return GGEngine::TaskResult::Success();
                    },
                    GGEngine::JobPriority::High
                );
                tasks.push_back(taskId);
            }

            // Wait for all parallel tasks
            taskGraph.WaitAll(tasks);
        }

        GGEngine::InstancedRenderer2D::EndScene();
    }
    else
    {
        GG_PROFILE_SCOPE("Renderer2D Batched Path");

        GGEngine::Renderer2D::ResetStats();
        GGEngine::Renderer2D::BeginScene(camera);

        // Render all entities (batched, single-threaded)
        const auto& entities = m_Scene->GetAllEntities();
        for (GGEngine::Entity index : entities)
        {
            auto entityID = m_Scene->GetEntityID(index);

            const auto* transform = m_Scene->GetComponent<GGEngine::TransformComponent>(entityID);
            const auto* sprite = m_Scene->GetComponent<GGEngine::SpriteRendererComponent>(entityID);

            if (transform && sprite)
            {
                auto mat = transform->GetMatrix();
                GGEngine::Renderer2D::DrawQuad(GGEngine::QuadSpec()
                    .SetTransform(&mat)
                    .SetColor(sprite->Color[0], sprite->Color[1], sprite->Color[2], sprite->Color[3]));
            }
        }

        GGEngine::Renderer2D::EndScene();
    }

    auto endTime = std::chrono::high_resolution_clock::now();
    auto duration = std::chrono::duration_cast<std::chrono::microseconds>(endTime - startTime);
    m_LastRenderTimeMs = static_cast<float>(duration.count()) / 1000.0f;
}

void MultithreadingExample::OnImGuiRender()
{
    // Entity controls
    ImGui::Text("Entity Count:");
    ImGui::SameLine();
    if (ImGui::InputInt("##entitycount", &m_EntityCount, 50, 500))
    {
        m_EntityCount = std::max(10, std::min(10000, m_EntityCount));
    }
    if (ImGui::Button("Recreate Entities"))
    {
        CreateEntities(m_EntityCount);
    }

    ImGui::Separator();

    // Rendering mode
    ImGui::Text("Rendering Mode:");
    if (ImGui::RadioButton("Instanced (GPU + Parallel)", m_UseInstancedRendering))
        m_UseInstancedRendering = true;
    ImGui::SameLine();
    if (ImGui::RadioButton("Batched (CPU)", !m_UseInstancedRendering))
        m_UseInstancedRendering = false;

    ImGui::Separator();

    // Execution mode
    ImGui::Text("ECS Execution Mode:");
    if (ImGui::RadioButton("Parallel (SystemScheduler)", m_UseParallelExecution))
        m_UseParallelExecution = true;
    ImGui::SameLine();
    if (ImGui::RadioButton("Sequential", !m_UseParallelExecution))
        m_UseParallelExecution = false;

    // Workload simulation
    ImGui::SliderInt("Work per Entity", &m_WorkloadIterations, 0, 1000);
    if (ImGui::IsItemHovered())
    {
        ImGui::SetTooltip("Extra math iterations per entity.\n"
                          "0 = lightweight (parallel overhead dominates)\n"
                          "100+ = heavy work (parallel wins)");
    }

    ImGui::Separator();

    // System controls
    ImGui::Text("Systems:");
    if (m_MovementSystem)
    {
        ImGui::Checkbox("Movement", &m_EnableMovement);
        if (m_EnableMovement)
        {
            ImGui::SameLine();
            ImGui::SliderFloat("Speed##mov", &m_MovementSystem->Speed, 0.0f, 10.0f);
        }
    }
    if (m_RotationSystem)
    {
        ImGui::Checkbox("Rotation", &m_EnableRotation);
        if (m_EnableRotation)
        {
            ImGui::SameLine();
            ImGui::SliderFloat("Speed##rot", &m_RotationSystem->RotationSpeed, 0.0f, 360.0f);
        }
    }
    if (m_ColorCycleSystem)
    {
        ImGui::Checkbox("Color Cycle", &m_EnableColorCycle);
        if (m_EnableColorCycle)
        {
            ImGui::SameLine();
            ImGui::SliderFloat("Speed##col", &m_ColorCycleSystem->CycleSpeed, 0.0f, 5.0f);
        }
    }

    ImGui::Separator();

    // Timing info
    ImGui::Text("Performance:");
    ImGui::Text("ECS Update: %.3f ms", m_LastUpdateTimeMs);
    ImGui::Text("Render:     %.3f ms", m_LastRenderTimeMs);
    ImGui::Text("Mode: %s", m_UseInstancedRendering ? "Instanced (GPU)" : "Batched (CPU)");

    if (m_BenchmarkRunning)
    {
        ImGui::TextColored(ImVec4(1, 1, 0, 1), "Benchmark running... %d/240",
                          m_BenchmarkIterations);
    }
    else
    {
        if (ImGui::Button("Run Benchmark"))
        {
            m_BenchmarkRunning = true;
            m_BenchmarkIterations = 0;
            m_BenchmarkSequentialTotal = 0.0f;
            m_BenchmarkParallelTotal = 0.0f;
            m_UseParallelExecution = false;
        }

        if (m_SequentialTimeMs > 0 && m_ParallelTimeMs > 0)
        {
            ImGui::Text("Sequential: %.3f ms", m_SequentialTimeMs);
            ImGui::Text("Parallel:   %.3f ms", m_ParallelTimeMs);
            float speedup = m_SequentialTimeMs / m_ParallelTimeMs;
            ImGui::Text("Speedup:    %.2fx", speedup);
        }
    }

    ImGui::Separator();

    // Hot reload info
#ifndef GG_DIST
    ImGui::Text("Hot Reload:");
    ImGui::Text("Watched: textures/");
    ImGui::Text("Reload Count: %d", m_ReloadCount);
    if (!m_LastReloadTime.empty())
    {
        ImGui::Text("Last Reload: %s", m_LastReloadTime.c_str());
    }
    ImGui::TextWrapped("Modify a texture in Engine/assets/textures/ to trigger hot reload");
#else
    ImGui::TextDisabled("Hot reload disabled in Dist builds");
#endif

    ImGui::Separator();

    // System dependency info
    if (ImGui::CollapsingHeader("System Dependencies"))
    {
        ImGui::TextWrapped(
            "MovementSystem: Writes TransformComponent\n"
            "RotationSystem: Writes TransformComponent (CONFLICT!)\n"
            "ColorCycleSystem: Writes SpriteRendererComponent\n\n"
            "Movement and Rotation CANNOT run in parallel (same component).\n"
            "ColorCycle CAN run in parallel with both (different component).\n\n"
            "NOTE: With lightweight work (slider=0), parallel is SLOWER due to:\n"
            "- Task creation overhead\n"
            "- Thread synchronization costs\n"
            "- Mutex locks in TaskGraph\n\n"
            "Increase 'Work per Entity' to 100+ to see parallel benefits!"
        );
    }

    // Renderer stats
    ImGui::Separator();
    if (m_UseInstancedRendering)
    {
        auto stats = GGEngine::InstancedRenderer2D::GetStats();
        ImGui::Text("Renderer: Instanced");
        ImGui::Text("Draw Calls: %d", stats.DrawCalls);
        ImGui::Text("Instances: %d", stats.InstanceCount);
        ImGui::Text("Max Capacity: %d", stats.MaxInstanceCapacity);
    }
    else
    {
        auto stats = GGEngine::Renderer2D::GetStats();
        ImGui::Text("Renderer: Batched");
        ImGui::Text("Draw Calls: %d", stats.DrawCalls);
        ImGui::Text("Quads: %d", stats.QuadCount);
        ImGui::Text("Max Capacity: %d", stats.MaxQuadCapacity);
    }
}
