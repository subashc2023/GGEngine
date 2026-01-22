#pragma once

#include "Example.h"
#include "GGEngine/ECS/Scene.h"
#include "GGEngine/ECS/Entity.h"

// Demonstrates the ECS Camera System:
// - Camera as an entity with CameraComponent
// - Primary camera rendering
// - Camera movement via TransformComponent
// - Multiple cameras with primary switching
class ECSCameraExample : public Example
{
public:
    ECSCameraExample();

    void OnAttach() override;
    void OnDetach() override;
    void OnUpdate(GGEngine::Timestep ts, const GGEngine::Camera& camera) override;
    void OnRender(const GGEngine::Camera& camera) override;
    void OnImGuiRender() override;

private:
    GGEngine::Scope<GGEngine::Scene> m_Scene;

    GGEngine::EntityID m_CameraEntity = GGEngine::InvalidEntityID;
    GGEngine::EntityID m_SecondCameraEntity = GGEngine::InvalidEntityID;

    bool m_UseSecondCamera = false;
    float m_CameraMoveSpeed = 3.0f;
    float m_CameraRotateSpeed = 90.0f;
};
