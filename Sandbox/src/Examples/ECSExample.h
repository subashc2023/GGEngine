#pragma once

#include "Example.h"
#include "GGEngine/ECS/Scene.h"
#include "GGEngine/ECS/Entity.h"

// Demonstrates the Entity Component System:
// - Creating entities
// - Adding/removing components
// - Component data manipulation
// - Scene iteration and rendering
class ECSExample : public Example
{
public:
    ECSExample();

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate(GGEngine::Timestep ts) override;
    void OnRender(const GGEngine::Camera& camera) override;
    void OnImGuiRender() override;

private:
    void CreateRandomEntity();
    void DestroySelectedEntity();

    GGEngine::Scope<GGEngine::Scene> m_Scene;

    // Selected entity for editing
    GGEngine::EntityID m_SelectedEntity = GGEngine::InvalidEntityID;

    // Entity spawning settings
    float m_SpawnRangeX = 4.0f;
    float m_SpawnRangeY = 3.0f;
    int m_EntityCount = 0;
};
