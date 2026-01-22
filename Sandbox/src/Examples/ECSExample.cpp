#include "ECSExample.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/ECS/Components/TransformComponent.h"
#include "GGEngine/ECS/Components/SpriteRendererComponent.h"
#include "GGEngine/ECS/Components/TagComponent.h"

#include <imgui.h>
#include <cstdlib>
#include <cmath>

ECSExample::ECSExample()
    : Example("Entity Component System",
              "Demonstrates entity creation, components, and scene rendering")
{
}

void ECSExample::OnAttach()
{
    m_Scene = GGEngine::CreateScope<GGEngine::Scene>("ECS Demo Scene");

    // Create a few starter entities
    for (int i = 0; i < 5; i++)
    {
        CreateRandomEntity();
    }
}

void ECSExample::OnDetach()
{
    m_Scene.reset();
    m_SelectedEntity = GGEngine::InvalidEntityID;
    m_EntityCount = 0;
}

void ECSExample::CreateRandomEntity()
{
    using namespace GGEngine;

    std::string name = "Entity_" + std::to_string(m_EntityCount++);
    EntityID entity = m_Scene->CreateEntity(name);

    // Set random transform
    TransformComponent* transform = m_Scene->GetComponent<TransformComponent>(entity);
    if (transform)
    {
        transform->Position[0] = (static_cast<float>(rand()) / RAND_MAX * 2.0f - 1.0f) * m_SpawnRangeX;
        transform->Position[1] = (static_cast<float>(rand()) / RAND_MAX * 2.0f - 1.0f) * m_SpawnRangeY;
        transform->Rotation = static_cast<float>(rand()) / RAND_MAX * 360.0f;
        float scale = 0.3f + static_cast<float>(rand()) / RAND_MAX * 0.7f;
        transform->Scale[0] = scale;
        transform->Scale[1] = scale;
    }

    // Add sprite with random color
    SpriteRendererComponent sprite;
    sprite.Color[0] = static_cast<float>(rand()) / RAND_MAX;
    sprite.Color[1] = static_cast<float>(rand()) / RAND_MAX;
    sprite.Color[2] = static_cast<float>(rand()) / RAND_MAX;
    sprite.Color[3] = 0.7f + static_cast<float>(rand()) / RAND_MAX * 0.3f;
    m_Scene->AddComponent<SpriteRendererComponent>(entity, sprite);

    // Auto-select first created entity
    if (!m_Scene->IsEntityValid(m_SelectedEntity))
    {
        m_SelectedEntity = entity;
    }
}

void ECSExample::DestroySelectedEntity()
{
    if (m_Scene->IsEntityValid(m_SelectedEntity))
    {
        m_Scene->DestroyEntity(m_SelectedEntity);
        m_SelectedEntity = GGEngine::InvalidEntityID;

        // Try to select another entity
        const auto& entities = m_Scene->GetAllEntities();
        if (!entities.empty())
        {
            m_SelectedEntity = m_Scene->GetEntityID(entities[0]);
        }
    }
}

void ECSExample::OnUpdate(GGEngine::Timestep ts)
{
    // Could add entity movement/animation here
    m_Scene->OnUpdate(ts);
}

void ECSExample::OnRender(const GGEngine::Camera& camera)
{
    using namespace GGEngine;

    // Manual rendering to demonstrate ECS iteration pattern
    // (Scene::OnRender would require RHI handles - we'll do it manually here)

    Renderer2D::ResetStats();
    Renderer2D::BeginScene(camera);

    // Iterate over all entities with Transform and Sprite
    const auto& entities = m_Scene->GetAllEntities();
    for (Entity index : entities)
    {
        EntityID entityID = m_Scene->GetEntityID(index);

        const TransformComponent* transform = m_Scene->GetComponent<TransformComponent>(entityID);
        const SpriteRendererComponent* sprite = m_Scene->GetComponent<SpriteRendererComponent>(entityID);

        if (transform && sprite)
        {
            // Use matrix-based rendering
            Renderer2D::DrawQuad(
                transform->GetMatrix(),
                sprite->Color[0], sprite->Color[1], sprite->Color[2], sprite->Color[3]);

            // Highlight selected entity
            if (entityID.Index == m_SelectedEntity.Index &&
                entityID.Generation == m_SelectedEntity.Generation)
            {
                // Draw selection outline (slightly larger, behind)
                glm::mat4 outlineMat = glm::translate(glm::mat4(1.0f),
                    glm::vec3(transform->Position[0], transform->Position[1], transform->Position[2] - 0.01f))
                    * glm::rotate(glm::mat4(1.0f), glm::radians(transform->Rotation), glm::vec3(0, 0, 1))
                    * glm::scale(glm::mat4(1.0f), glm::vec3(transform->Scale[0] * 1.2f, transform->Scale[1] * 1.2f, 1.0f));
                Renderer2D::DrawQuad(outlineMat, 1.0f, 1.0f, 0.0f, 0.8f);
            }
        }
    }

    Renderer2D::EndScene();
}

void ECSExample::OnImGuiRender()
{
    using namespace GGEngine;

    // Entity management buttons
    if (ImGui::Button("Create Entity"))
    {
        CreateRandomEntity();
    }
    ImGui::SameLine();
    if (ImGui::Button("Destroy Selected"))
    {
        DestroySelectedEntity();
    }
    ImGui::SameLine();
    if (ImGui::Button("Clear All"))
    {
        m_Scene->Clear();
        m_SelectedEntity = InvalidEntityID;
    }

    ImGui::SliderFloat("Spawn Range X", &m_SpawnRangeX, 1.0f, 10.0f);
    ImGui::SliderFloat("Spawn Range Y", &m_SpawnRangeY, 1.0f, 10.0f);

    ImGui::Separator();
    ImGui::Text("Entities: %zu", m_Scene->GetEntityCount());

    // Entity list
    ImGui::BeginChild("EntityList", ImVec2(0, 150), true);
    const auto& entities = m_Scene->GetAllEntities();
    for (Entity index : entities)
    {
        EntityID entityID = m_Scene->GetEntityID(index);
        const TagComponent* tag = m_Scene->GetComponent<TagComponent>(entityID);

        bool isSelected = (entityID.Index == m_SelectedEntity.Index &&
                          entityID.Generation == m_SelectedEntity.Generation);

        std::string label = tag ? tag->Name : ("Entity " + std::to_string(index));
        if (ImGui::Selectable(label.c_str(), isSelected))
        {
            m_SelectedEntity = entityID;
        }
    }
    ImGui::EndChild();

    // Selected entity inspector
    ImGui::Separator();
    ImGui::Text("Inspector:");

    if (m_Scene->IsEntityValid(m_SelectedEntity))
    {
        // Tag component
        TagComponent* tag = m_Scene->GetComponent<TagComponent>(m_SelectedEntity);
        if (tag)
        {
            char nameBuf[256];
            strncpy(nameBuf, tag->Name.c_str(), sizeof(nameBuf) - 1);
            nameBuf[sizeof(nameBuf) - 1] = '\0';
            if (ImGui::InputText("Name", nameBuf, sizeof(nameBuf)))
            {
                tag->Name = nameBuf;
            }
            ImGui::Text("GUID: %s", tag->ID.ToString().c_str());
        }

        // Transform component
        TransformComponent* transform = m_Scene->GetComponent<TransformComponent>(m_SelectedEntity);
        if (transform)
        {
            ImGui::Separator();
            ImGui::Text("Transform:");
            ImGui::DragFloat3("Position", transform->Position, 0.1f);
            ImGui::DragFloat("Rotation", &transform->Rotation, 1.0f);
            ImGui::DragFloat2("Scale", transform->Scale, 0.1f, 0.1f, 10.0f);
        }

        // Sprite component
        SpriteRendererComponent* sprite = m_Scene->GetComponent<SpriteRendererComponent>(m_SelectedEntity);
        if (sprite)
        {
            ImGui::Separator();
            ImGui::Text("Sprite Renderer:");
            ImGui::ColorEdit4("Color", sprite->Color);

            // Option to remove sprite component
            if (ImGui::Button("Remove Sprite"))
            {
                m_Scene->RemoveComponent<SpriteRendererComponent>(m_SelectedEntity);
            }
        }
        else
        {
            // Option to add sprite component
            if (ImGui::Button("Add Sprite"))
            {
                m_Scene->AddComponent<SpriteRendererComponent>(m_SelectedEntity);
            }
        }
    }
    else
    {
        ImGui::Text("No entity selected");
    }

    ImGui::Separator();
    auto stats = Renderer2D::GetStats();
    ImGui::Text("Draw Calls: %d", stats.DrawCalls);
    ImGui::Text("Quads: %d", stats.QuadCount);
}
