#include "ECSCameraExample.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/ECS/Components.h"
#include "GGEngine/Core/Input.h"
#include "GGEngine/Core/KeyCodes.h"
#include "GGEngine/RHI/RHIDevice.h"

#include <imgui.h>
#include <cmath>

ECSCameraExample::ECSCameraExample()
    : Example("ECS Camera", "Demonstrates camera as an ECS entity with CameraComponent")
{
}

void ECSCameraExample::OnAttach()
{
    m_Scene = GGEngine::CreateScope<GGEngine::Scene>("ECS Camera Demo");

    // Create primary camera entity
    m_CameraEntity = m_Scene->CreateEntity("Main Camera");
    auto& cameraComp = m_Scene->AddComponent<GGEngine::CameraComponent>(m_CameraEntity);
    cameraComp.Primary = true;
    cameraComp.Camera.SetOrthographic(5.0f, -10.0f, 10.0f);

    // Create second camera with different zoom
    m_SecondCameraEntity = m_Scene->CreateEntity("Zoomed Camera");
    auto& camera2Comp = m_Scene->AddComponent<GGEngine::CameraComponent>(m_SecondCameraEntity);
    camera2Comp.Primary = false;
    camera2Comp.Camera.SetOrthographic(2.0f, -10.0f, 10.0f);  // More zoomed in

    // Position second camera slightly offset
    auto* cam2Transform = m_Scene->GetComponent<GGEngine::TransformComponent>(m_SecondCameraEntity);
    cam2Transform->Position[0] = 2.0f;

    // Create some sprite entities to view
    for (int i = 0; i < 5; i++)
    {
        auto entity = m_Scene->CreateEntity("Sprite " + std::to_string(i));
        auto* transform = m_Scene->GetComponent<GGEngine::TransformComponent>(entity);
        transform->Position[0] = (i - 2) * 2.0f;
        transform->Scale[0] = 1.0f;
        transform->Scale[1] = 1.0f;

        auto& sprite = m_Scene->AddComponent<GGEngine::SpriteRendererComponent>(entity);
        sprite.Color[0] = 0.2f + (i * 0.15f);
        sprite.Color[1] = 0.6f;
        sprite.Color[2] = 1.0f - (i * 0.15f);
    }

    // Create a rotating sprite
    auto rotatingEntity = m_Scene->CreateEntity("Rotating Sprite");
    auto* rotTransform = m_Scene->GetComponent<GGEngine::TransformComponent>(rotatingEntity);
    rotTransform->Position[1] = 2.0f;
    rotTransform->Scale[0] = 1.5f;
    rotTransform->Scale[1] = 1.5f;
    auto& rotSprite = m_Scene->AddComponent<GGEngine::SpriteRendererComponent>(rotatingEntity);
    rotSprite.Color[0] = 1.0f;
    rotSprite.Color[1] = 0.5f;
    rotSprite.Color[2] = 0.0f;
}

void ECSCameraExample::OnDetach()
{
    m_Scene.reset();
}

void ECSCameraExample::OnUpdate(GGEngine::Timestep ts)
{
    // Update camera primary flag based on toggle
    auto* cam1 = m_Scene->GetComponent<GGEngine::CameraComponent>(m_CameraEntity);
    auto* cam2 = m_Scene->GetComponent<GGEngine::CameraComponent>(m_SecondCameraEntity);
    if (cam1 && cam2)
    {
        cam1->Primary = !m_UseSecondCamera;
        cam2->Primary = m_UseSecondCamera;
    }

    // Get primary camera's transform
    GGEngine::EntityID activeCamera = m_UseSecondCamera ? m_SecondCameraEntity : m_CameraEntity;
    auto* camTransform = m_Scene->GetComponent<GGEngine::TransformComponent>(activeCamera);

    if (camTransform)
    {
        float speed = m_CameraMoveSpeed * ts;
        float rotSpeed = m_CameraRotateSpeed * ts;

        // WASD movement
        if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::W))
            camTransform->Position[1] += speed;
        if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::S))
            camTransform->Position[1] -= speed;
        if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::A))
            camTransform->Position[0] -= speed;
        if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::D))
            camTransform->Position[0] += speed;

        // Q/E rotation
        if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::Q))
            camTransform->Rotation += rotSpeed;
        if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::E))
            camTransform->Rotation -= rotSpeed;
    }

    // Rotate the rotating sprite
    auto rotEntity = m_Scene->FindEntityByName("Rotating Sprite");
    if (m_Scene->IsEntityValid(rotEntity))
    {
        auto* rotTransform = m_Scene->GetComponent<GGEngine::TransformComponent>(rotEntity);
        if (rotTransform)
        {
            rotTransform->Rotation += 45.0f * ts;
        }
    }

    m_Scene->OnUpdate(ts);
}

void ECSCameraExample::OnRender(const GGEngine::Camera& camera)
{
    // Instead of using the passed camera, use our scene's ECS camera
    auto& device = GGEngine::RHIDevice::Get();
    m_Scene->OnRenderRuntime(
        device.GetSwapchainRenderPass(),
        device.GetCurrentCommandBuffer(),
        device.GetSwapchainWidth(),
        device.GetSwapchainHeight()
    );
}

void ECSCameraExample::OnImGuiRender()
{
    ImGui::Text("ECS Camera System Demo");
    ImGui::Separator();

    ImGui::Text("Controls:");
    ImGui::BulletText("WASD - Move camera");
    ImGui::BulletText("Q/E - Rotate camera");

    ImGui::Separator();

    // Camera switching
    if (ImGui::Checkbox("Use Second Camera", &m_UseSecondCamera))
    {
        // Primary flag updates happen in OnUpdate
    }

    ImGui::Separator();

    // Show camera info
    auto* cam1 = m_Scene->GetComponent<GGEngine::CameraComponent>(m_CameraEntity);
    auto* cam2 = m_Scene->GetComponent<GGEngine::CameraComponent>(m_SecondCameraEntity);
    auto* cam1Transform = m_Scene->GetComponent<GGEngine::TransformComponent>(m_CameraEntity);
    auto* cam2Transform = m_Scene->GetComponent<GGEngine::TransformComponent>(m_SecondCameraEntity);

    if (cam1 && cam1Transform)
    {
        ImGui::Text("Main Camera:");
        ImGui::Text("  Primary: %s", cam1->Primary ? "Yes" : "No");
        ImGui::Text("  Size: %.1f", cam1->Camera.GetOrthographicSize());
        ImGui::Text("  Position: (%.2f, %.2f)", cam1Transform->Position[0], cam1Transform->Position[1]);
        ImGui::Text("  Rotation: %.1f deg", cam1Transform->Rotation);
    }

    if (cam2 && cam2Transform)
    {
        ImGui::Text("Zoomed Camera:");
        ImGui::Text("  Primary: %s", cam2->Primary ? "Yes" : "No");
        ImGui::Text("  Size: %.1f", cam2->Camera.GetOrthographicSize());
        ImGui::Text("  Position: (%.2f, %.2f)", cam2Transform->Position[0], cam2Transform->Position[1]);
    }

    ImGui::Separator();

    // Camera settings
    ImGui::DragFloat("Move Speed", &m_CameraMoveSpeed, 0.1f, 0.1f, 20.0f);
    ImGui::DragFloat("Rotate Speed", &m_CameraRotateSpeed, 1.0f, 1.0f, 360.0f);

    // Edit camera sizes
    if (cam1)
    {
        float size1 = cam1->Camera.GetOrthographicSize();
        if (ImGui::DragFloat("Main Cam Size", &size1, 0.1f, 0.5f, 20.0f))
            cam1->Camera.SetOrthographicSize(size1);
    }

    if (cam2)
    {
        float size2 = cam2->Camera.GetOrthographicSize();
        if (ImGui::DragFloat("Zoomed Cam Size", &size2, 0.1f, 0.5f, 20.0f))
            cam2->Camera.SetOrthographicSize(size2);
    }
}
