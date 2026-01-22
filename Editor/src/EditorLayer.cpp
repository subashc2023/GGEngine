#include "EditorLayer.h"
#include "Platform/Vulkan/VulkanContext.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/ImGui/DebugUI.h"
#include "GGEngine/Core/Input.h"
#include "GGEngine/Core/KeyCodes.h"
#include "GGEngine/Asset/Texture.h"
#include "GGEngine/Asset/TextureLibrary.h"
#include "GGEngine/ECS/Components.h"
#include "GGEngine/ECS/SceneSerializer.h"
#include "GGEngine/Utils/FileDialogs.h"

#include <imgui.h>
#include <cstring>

EditorLayer::EditorLayer()
    : Layer("EditorLayer"), m_CameraController(1280.0f / 720.0f, 1.0f, true)
{
}

void EditorLayer::OnAttach()
{
    GGEngine::FramebufferSpecification spec;
    spec.Width = 1280;
    spec.Height = 720;
    m_ViewportFramebuffer = GGEngine::CreateScope<GGEngine::Framebuffer>(spec);

    CreateDefaultScene();

    GG_INFO("EditorLayer attached - using ECS Scene");
}

void EditorLayer::CreateDefaultScene()
{
    m_ActiveScene = GGEngine::CreateScope<GGEngine::Scene>("Demo Scene");

    // Create grid of colored quads
    const int gridSize = 10;
    const float spacing = 0.11f;
    const float quadSize = 0.1f;
    const float offset = (gridSize - 1) * spacing * 0.5f;

    for (int y = 0; y < gridSize; y++)
    {
        for (int x = 0; x < gridSize; x++)
        {
            char name[32];
            snprintf(name, sizeof(name), "Grid[%d,%d]", x, y);
            auto entity = m_ActiveScene->CreateEntity(name);

            auto* transform = m_ActiveScene->GetComponent<GGEngine::TransformComponent>(entity);
            transform->Position[0] = x * spacing - offset;
            transform->Position[1] = y * spacing - offset;
            transform->Scale[0] = quadSize;
            transform->Scale[1] = quadSize;

            auto& sprite = m_ActiveScene->AddComponent<GGEngine::SpriteRendererComponent>(entity);
            sprite.Color[0] = static_cast<float>(x) / (gridSize - 1);
            sprite.Color[1] = static_cast<float>(y) / (gridSize - 1);
            sprite.Color[2] = 0.5f;
            sprite.Color[3] = 1.0f;
        }
    }

    // Create movable entity
    {
        auto entity = m_ActiveScene->CreateEntity("Player Quad");
        auto& sprite = m_ActiveScene->AddComponent<GGEngine::SpriteRendererComponent>(entity);
        sprite.Color[0] = 1.0f;
        sprite.Color[1] = 1.0f;
        sprite.Color[2] = 1.0f;
        sprite.Color[3] = 1.0f;

        m_SelectedEntity = entity;  // Select by default
    }

    // Create textured entity
    {
        auto entity = m_ActiveScene->CreateEntity("Textured Quad");
        auto* transform = m_ActiveScene->GetComponent<GGEngine::TransformComponent>(entity);
        transform->Position[0] = 1.5f;

        auto& sprite = m_ActiveScene->AddComponent<GGEngine::SpriteRendererComponent>(entity);
        sprite.TextureName = "Checkerboard";  // Use built-in checkerboard texture
    }
}

void EditorLayer::OnDetach()
{
    m_ActiveScene.reset();
    m_ViewportFramebuffer.reset();
    GG_INFO("EditorLayer detached");
}

void EditorLayer::OnRenderOffscreen(GGEngine::Timestep ts)
{
    if (!m_ViewportFramebuffer || !m_ActiveScene)
        return;

    // Handle pending resize before rendering
    if (m_NeedsResize && m_PendingViewportWidth > 0 && m_PendingViewportHeight > 0)
    {
        m_ViewportFramebuffer->Resize(
            static_cast<uint32_t>(m_PendingViewportWidth),
            static_cast<uint32_t>(m_PendingViewportHeight));
        m_ViewportWidth = m_PendingViewportWidth;
        m_ViewportHeight = m_PendingViewportHeight;
        m_NeedsResize = false;

        m_CameraController.SetAspectRatio(m_ViewportWidth / m_ViewportHeight);
    }

    auto& vkContext = GGEngine::VulkanContext::Get();
    VkCommandBuffer cmd = vkContext.GetCurrentCommandBuffer();

    if (cmd == VK_NULL_HANDLE)
        return;

    m_ViewportFramebuffer->BeginRenderPass(cmd);

    // Scene renders all entities with SpriteRenderer components
    m_ActiveScene->OnRender(
        m_CameraController.GetCamera(),
        m_ViewportFramebuffer->GetRenderPass(),
        cmd,
        m_ViewportFramebuffer->GetWidth(),
        m_ViewportFramebuffer->GetHeight());

    m_ViewportFramebuffer->EndRenderPass(cmd);
}

void EditorLayer::DrawSceneHierarchyPanel()
{
    ImGui::Begin("Scene Hierarchy");

    if (m_ActiveScene)
    {
        for (GGEngine::Entity index : m_ActiveScene->GetAllEntities())
        {
            GGEngine::EntityID entityId = m_ActiveScene->GetEntityID(index);

            const auto* tag = m_ActiveScene->GetComponent<GGEngine::TagComponent>(entityId);
            if (!tag) continue;

            ImGuiTreeNodeFlags flags = ImGuiTreeNodeFlags_OpenOnArrow |
                                       ImGuiTreeNodeFlags_SpanAvailWidth |
                                       ImGuiTreeNodeFlags_Leaf;

            if (m_SelectedEntity == entityId)
                flags |= ImGuiTreeNodeFlags_Selected;

            bool opened = ImGui::TreeNodeEx(
                reinterpret_cast<void*>(static_cast<uint64_t>(index)),
                flags, "%s", tag->Name.c_str());

            if (ImGui::IsItemClicked())
            {
                m_SelectedEntity = entityId;
            }

            if (opened)
            {
                ImGui::TreePop();
            }
        }

        // Right-click context menu for creating entities
        if (ImGui::BeginPopupContextWindow(nullptr, ImGuiPopupFlags_NoOpenOverItems))
        {
            if (ImGui::MenuItem("Create Empty Entity"))
            {
                m_SelectedEntity = m_ActiveScene->CreateEntity("New Entity");
            }
            if (ImGui::MenuItem("Create Sprite"))
            {
                auto entity = m_ActiveScene->CreateEntity("Sprite");
                m_ActiveScene->AddComponent<GGEngine::SpriteRendererComponent>(entity);
                m_SelectedEntity = entity;
            }
            ImGui::EndPopup();
        }
    }

    ImGui::End();
}

void EditorLayer::DrawPropertiesPanel(GGEngine::Timestep ts)
{
    ImGui::Begin("Properties");

    if (m_ActiveScene && m_ActiveScene->IsEntityValid(m_SelectedEntity))
    {
        // Tag/Name editing
        if (auto* tag = m_ActiveScene->GetComponent<GGEngine::TagComponent>(m_SelectedEntity))
        {
            char buffer[256];
            std::strncpy(buffer, tag->Name.c_str(), sizeof(buffer) - 1);
            buffer[sizeof(buffer) - 1] = '\0';
            if (ImGui::InputText("##Name", buffer, sizeof(buffer)))
            {
                tag->Name = buffer;
            }

            // Show GUID (read-only)
            ImGui::TextDisabled("GUID: %s", tag->ID.ToString().substr(0, 16).c_str());
            ImGui::Separator();
        }

        // Transform component
        if (m_ActiveScene->HasComponent<GGEngine::TransformComponent>(m_SelectedEntity))
        {
            if (ImGui::CollapsingHeader("Transform", ImGuiTreeNodeFlags_DefaultOpen))
            {
                auto* transform = m_ActiveScene->GetComponent<GGEngine::TransformComponent>(m_SelectedEntity);

                ImGui::DragFloat3("Position", transform->Position, 0.01f);
                ImGui::DragFloat("Rotation", &transform->Rotation, 1.0f, -360.0f, 360.0f, "%.1f deg");
                ImGui::DragFloat2("Scale", transform->Scale, 0.01f, 0.01f, 10.0f);
            }
        }

        // SpriteRenderer component
        if (m_ActiveScene->HasComponent<GGEngine::SpriteRendererComponent>(m_SelectedEntity))
        {
            if (ImGui::CollapsingHeader("Sprite Renderer", ImGuiTreeNodeFlags_DefaultOpen))
            {
                auto* sprite = m_ActiveScene->GetComponent<GGEngine::SpriteRendererComponent>(m_SelectedEntity);

                ImGui::ColorEdit4("Color", sprite->Color);
                ImGui::DragFloat("Tiling Factor", &sprite->TilingFactor, 0.1f, 0.0f, 100.0f);

                // Texture picker dropdown
                auto& textureLib = GGEngine::TextureLibrary::Get();
                auto textureNames = textureLib.GetAllNames();

                // Current texture display name
                const char* currentTexture = sprite->TextureName.empty() ? "None" : sprite->TextureName.c_str();

                if (ImGui::BeginCombo("Texture", currentTexture))
                {
                    // "None" option to clear texture
                    bool isNoneSelected = sprite->TextureName.empty();
                    if (ImGui::Selectable("None", isNoneSelected))
                    {
                        sprite->TextureName.clear();
                    }
                    if (isNoneSelected)
                        ImGui::SetItemDefaultFocus();

                    // List all available textures
                    for (const auto& name : textureNames)
                    {
                        bool isSelected = (sprite->TextureName == name);
                        if (ImGui::Selectable(name.c_str(), isSelected))
                        {
                            sprite->TextureName = name;
                        }
                        if (isSelected)
                            ImGui::SetItemDefaultFocus();
                    }

                    ImGui::EndCombo();
                }
            }
        }
        else
        {
            // Add component button
            if (ImGui::Button("Add Sprite Renderer"))
            {
                m_ActiveScene->AddComponent<GGEngine::SpriteRendererComponent>(m_SelectedEntity);
            }
        }

        ImGui::Separator();

        // Delete entity button
        ImGui::PushStyleColor(ImGuiCol_Button, ImVec4(0.6f, 0.2f, 0.2f, 1.0f));
        if (ImGui::Button("Delete Entity"))
        {
            m_ActiveScene->DestroyEntity(m_SelectedEntity);
            m_SelectedEntity = GGEngine::InvalidEntityID;
        }
        ImGui::PopStyleColor();
    }
    else
    {
        ImGui::Text("No entity selected");
    }

    ImGui::Separator();

    // Stats section
    if (ImGui::CollapsingHeader("Stats", ImGuiTreeNodeFlags_DefaultOpen))
    {
        auto stats = GGEngine::Renderer2D::GetStats();
        ImGui::Text("Renderer2D Stats:");
        ImGui::Text("  Draw Calls: %d", stats.DrawCalls);
        ImGui::Text("  Quads: %d", stats.QuadCount);
        ImGui::Separator();
        if (m_ActiveScene)
        {
            ImGui::Text("Scene: %s", m_ActiveScene->GetName().c_str());
            ImGui::Text("Entities: %zu", m_ActiveScene->GetEntityCount());
        }
        ImGui::Separator();
        GGEngine::DebugUI::ShowStatsContent(ts);
    }

    ImGui::End();
}

void EditorLayer::OnUpdate(GGEngine::Timestep ts)
{
    // Dockspace setup
    static bool dockspaceOpen = true;
    static ImGuiDockNodeFlags dockspaceFlags = ImGuiDockNodeFlags_None;

    ImGuiWindowFlags windowFlags = ImGuiWindowFlags_MenuBar | ImGuiWindowFlags_NoDocking;
    ImGuiViewport* viewport = ImGui::GetMainViewport();
    ImGui::SetNextWindowPos(viewport->Pos);
    ImGui::SetNextWindowSize(viewport->Size);
    ImGui::SetNextWindowViewport(viewport->ID);
    ImGui::PushStyleVar(ImGuiStyleVar_WindowRounding, 0.0f);
    ImGui::PushStyleVar(ImGuiStyleVar_WindowBorderSize, 0.0f);
    windowFlags |= ImGuiWindowFlags_NoTitleBar | ImGuiWindowFlags_NoCollapse | ImGuiWindowFlags_NoResize | ImGuiWindowFlags_NoMove;
    windowFlags |= ImGuiWindowFlags_NoBringToFrontOnFocus | ImGuiWindowFlags_NoNavFocus;

    if (dockspaceFlags & ImGuiDockNodeFlags_PassthruCentralNode)
        windowFlags |= ImGuiWindowFlags_NoBackground;

    ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, ImVec2(0.0f, 0.0f));
    ImGui::Begin("DockSpace Demo", &dockspaceOpen, windowFlags);
    ImGui::PopStyleVar(3);

    // DockSpace
    ImGuiIO& io = ImGui::GetIO();
    if (io.ConfigFlags & ImGuiConfigFlags_DockingEnable)
    {
        ImGuiID dockspaceId = ImGui::GetID("MyDockSpace");
        ImGui::DockSpace(dockspaceId, ImVec2(0.0f, 0.0f), dockspaceFlags);
    }

    // Menu bar
    if (ImGui::BeginMenuBar())
    {
        if (ImGui::BeginMenu("File"))
        {
            if (ImGui::MenuItem("New Scene", "Ctrl+N"))
                NewScene();
            if (ImGui::MenuItem("Open Scene...", "Ctrl+O"))
                OpenScene();
            ImGui::Separator();
            if (ImGui::MenuItem("Save Scene", "Ctrl+S"))
                SaveScene();
            if (ImGui::MenuItem("Save Scene As...", "Ctrl+Shift+S"))
                SaveSceneAs();
            ImGui::Separator();
            if (ImGui::MenuItem("Exit"))
            {
                // Could dispatch a window close event here
            }
            ImGui::EndMenu();
        }
        ImGui::EndMenuBar();
    }

    // Draw panels
    DrawSceneHierarchyPanel();
    DrawPropertiesPanel(ts);

    // Viewport window
    ImGui::PushStyleVar(ImGuiStyleVar_WindowPadding, ImVec2(0, 0));
    ImGui::Begin("Viewport");

    m_ViewportFocused = ImGui::IsWindowFocused();
    m_ViewportHovered = ImGui::IsWindowHovered();

    ImVec2 viewportPanelSize = ImGui::GetContentRegionAvail();
    if (m_ViewportWidth != viewportPanelSize.x || m_ViewportHeight != viewportPanelSize.y)
    {
        m_PendingViewportWidth = viewportPanelSize.x;
        m_PendingViewportHeight = viewportPanelSize.y;
        m_NeedsResize = true;
    }

    // Display framebuffer texture
    if (m_ViewportFramebuffer && m_ViewportFramebuffer->GetImGuiTextureID())
    {
        float displayWidth = m_ViewportWidth > 0 ? m_ViewportWidth : static_cast<float>(m_ViewportFramebuffer->GetWidth());
        float displayHeight = m_ViewportHeight > 0 ? m_ViewportHeight : static_cast<float>(m_ViewportFramebuffer->GetHeight());
        ImGui::Image(
            m_ViewportFramebuffer->GetImGuiTextureID(),
            ImVec2(displayWidth, displayHeight));
    }

    ImGui::End();
    ImGui::PopStyleVar();

    // Update camera controller when viewport is hovered
    if (m_ViewportHovered)
    {
        m_CameraController.OnUpdate(ts);

        // IJKL to move the selected entity
        if (m_ActiveScene && m_ActiveScene->IsEntityValid(m_SelectedEntity))
        {
            auto* transform = m_ActiveScene->GetComponent<GGEngine::TransformComponent>(m_SelectedEntity);
            if (transform)
            {
                float velocity = 2.0f * ts;
                if (GGEngine::Input::IsKeyPressed(GG_KEY_I))
                    transform->Position[1] += velocity;
                if (GGEngine::Input::IsKeyPressed(GG_KEY_K))
                    transform->Position[1] -= velocity;
                if (GGEngine::Input::IsKeyPressed(GG_KEY_J))
                    transform->Position[0] -= velocity;
                if (GGEngine::Input::IsKeyPressed(GG_KEY_L))
                    transform->Position[0] += velocity;

                float rotationSpeed = 90.0f * ts;
                if (GGEngine::Input::IsKeyPressed(GG_KEY_U))
                    transform->Rotation += rotationSpeed;
                if (GGEngine::Input::IsKeyPressed(GG_KEY_O))
                    transform->Rotation -= rotationSpeed;
            }
        }
    }

    ImGui::End(); // DockSpace
}

void EditorLayer::OnEvent(GGEngine::Event& event)
{
    if (m_ViewportHovered)
    {
        m_CameraController.OnEvent(event);
    }
}

void EditorLayer::OnWindowResize(uint32_t width, uint32_t height)
{
    // Editor uses ImGui viewport sizing for the framebuffer camera,
    // so we don't need to update aspect ratio here.
}

void EditorLayer::NewScene()
{
    m_ActiveScene = GGEngine::CreateScope<GGEngine::Scene>("Untitled Scene");
    m_SelectedEntity = GGEngine::InvalidEntityID;
    m_CurrentScenePath.clear();
    GG_INFO("Created new scene");
}

void EditorLayer::OpenScene()
{
    std::string filepath = GGEngine::FileDialogs::OpenFile("*.scene", "Open Scene");
    if (!filepath.empty())
    {
        m_ActiveScene = GGEngine::CreateScope<GGEngine::Scene>();
        GGEngine::SceneSerializer serializer(m_ActiveScene.get());
        if (serializer.Deserialize(filepath))
        {
            m_CurrentScenePath = filepath;
            m_SelectedEntity = GGEngine::InvalidEntityID;
            GG_INFO("Opened scene: {}", filepath);
        }
    }
}

void EditorLayer::SaveScene()
{
    if (m_CurrentScenePath.empty())
    {
        SaveSceneAs();
    }
    else
    {
        GGEngine::SceneSerializer serializer(m_ActiveScene.get());
        serializer.Serialize(m_CurrentScenePath);
    }
}

void EditorLayer::SaveSceneAs()
{
    std::string filepath = GGEngine::FileDialogs::SaveFile("*.scene", "Save Scene As");
    if (!filepath.empty())
    {
        // Ensure .scene extension
        if (filepath.find(".scene") == std::string::npos)
        {
            filepath += ".scene";
        }
        GGEngine::SceneSerializer serializer(m_ActiveScene.get());
        serializer.Serialize(filepath);
        m_CurrentScenePath = filepath;
        GG_INFO("Saved scene as: {}", filepath);
    }
}
