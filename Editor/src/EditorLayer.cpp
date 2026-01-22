#include "EditorLayer.h"
#include "GGEngine/Core/Application.h"
#include "GGEngine/ImGui/ImGuiLayer.h"
#include "GGEngine/RHI/RHIDevice.h"
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
#include <algorithm>

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

    // Load spritesheets from game assets folder
    GGEngine::TextureLibrary::Get().ScanDirectory("game");

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

    auto& device = GGEngine::RHIDevice::Get();
    GGEngine::RHICommandBufferHandle cmd = device.GetCurrentCommandBuffer();

    if (!cmd.IsValid())
        return;

    m_ViewportFramebuffer->BeginRenderPass(cmd);

    // Scene renders all entities with SpriteRenderer components
    m_ActiveScene->OnRender(
        m_CameraController.GetCamera(),
        m_ViewportFramebuffer->GetRenderPassHandle(),
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
            if (ImGui::MenuItem("Create Tilemap"))
            {
                auto entity = m_ActiveScene->CreateEntity("Tilemap");
                auto& tilemap = m_ActiveScene->AddComponent<GGEngine::TilemapComponent>(entity);
                tilemap.ResizeTiles();
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

                // Spritesheet/Atlas settings
                ImGui::Separator();
                ImGui::Checkbox("Use Spritesheet", &sprite->UseAtlas);

                if (sprite->UseAtlas)
                {
                    ImGui::Indent();

                    // Cell size
                    float cellSize[2] = { sprite->AtlasCellWidth, sprite->AtlasCellHeight };
                    if (ImGui::DragFloat2("Cell Size (px)", cellSize, 1.0f, 1.0f, 1024.0f))
                    {
                        sprite->AtlasCellWidth = cellSize[0];
                        sprite->AtlasCellHeight = cellSize[1];
                    }

                    // Grid position
                    int cellPos[2] = { static_cast<int>(sprite->AtlasCellX), static_cast<int>(sprite->AtlasCellY) };
                    if (ImGui::DragInt2("Cell Position", cellPos, 0.1f, 0, 100))
                    {
                        sprite->AtlasCellX = static_cast<uint32_t>(std::max(0, cellPos[0]));
                        sprite->AtlasCellY = static_cast<uint32_t>(std::max(0, cellPos[1]));
                    }

                    // Sprite size (in cells, for multi-cell sprites)
                    float spriteSize[2] = { sprite->AtlasSpriteWidth, sprite->AtlasSpriteHeight };
                    if (ImGui::DragFloat2("Sprite Size (cells)", spriteSize, 0.1f, 0.1f, 10.0f))
                    {
                        sprite->AtlasSpriteWidth = spriteSize[0];
                        sprite->AtlasSpriteHeight = spriteSize[1];
                    }

                    // Show calculated UV info
                    if (!sprite->TextureName.empty())
                    {
                        auto* tex = textureLib.GetTexturePtr(sprite->TextureName);
                        if (tex)
                        {
                            uint32_t gridW = static_cast<uint32_t>(tex->GetWidth() / sprite->AtlasCellWidth);
                            uint32_t gridH = static_cast<uint32_t>(tex->GetHeight() / sprite->AtlasCellHeight);
                            ImGui::TextDisabled("Grid: %dx%d cells", gridW, gridH);
                        }
                    }

                    ImGui::Unindent();
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

        // TilemapComponent
        if (m_ActiveScene->HasComponent<GGEngine::TilemapComponent>(m_SelectedEntity))
        {
            if (ImGui::CollapsingHeader("Tilemap", ImGuiTreeNodeFlags_DefaultOpen))
            {
                auto* tilemap = m_ActiveScene->GetComponent<GGEngine::TilemapComponent>(m_SelectedEntity);

                // Grid dimensions
                int dims[2] = { static_cast<int>(tilemap->Width), static_cast<int>(tilemap->Height) };
                if (ImGui::DragInt2("Grid Size (tiles)", dims, 0.1f, 1, 256))
                {
                    tilemap->Width = static_cast<uint32_t>(std::max(1, dims[0]));
                    tilemap->Height = static_cast<uint32_t>(std::max(1, dims[1]));
                    tilemap->ResizeTiles();
                }

                // Tile size in world units
                float tileSize[2] = { tilemap->TileWidth, tilemap->TileHeight };
                if (ImGui::DragFloat2("Tile Size (world)", tileSize, 0.01f, 0.01f, 10.0f))
                {
                    tilemap->TileWidth = tileSize[0];
                    tilemap->TileHeight = tileSize[1];
                }

                // Z-depth offset
                ImGui::DragFloat("Z Offset", &tilemap->ZOffset, 0.01f, -10.0f, 10.0f);

                // Tint color
                ImGui::ColorEdit4("Tint", tilemap->Color);

                ImGui::Separator();

                // Atlas settings
                ImGui::Text("Atlas Settings");

                // Texture dropdown (same pattern as SpriteRenderer)
                auto& textureLib = GGEngine::TextureLibrary::Get();
                auto textureNames = textureLib.GetAllNames();
                const char* currentTexture = tilemap->TextureName.empty() ? "None" : tilemap->TextureName.c_str();

                if (ImGui::BeginCombo("Atlas Texture", currentTexture))
                {
                    bool isNoneSelected = tilemap->TextureName.empty();
                    if (ImGui::Selectable("None", isNoneSelected))
                    {
                        tilemap->TextureName.clear();
                    }
                    if (isNoneSelected)
                        ImGui::SetItemDefaultFocus();

                    for (const auto& name : textureNames)
                    {
                        bool isSelected = (tilemap->TextureName == name);
                        if (ImGui::Selectable(name.c_str(), isSelected))
                        {
                            tilemap->TextureName = name;
                            // Auto-calculate columns when texture changes
                            if (auto* tex = textureLib.GetTexturePtr(name))
                            {
                                tilemap->AtlasColumns = static_cast<uint32_t>(tex->GetWidth() / tilemap->AtlasCellWidth);
                            }
                        }
                        if (isSelected)
                            ImGui::SetItemDefaultFocus();
                    }
                    ImGui::EndCombo();
                }

                // Atlas cell size
                float cellSize[2] = { tilemap->AtlasCellWidth, tilemap->AtlasCellHeight };
                if (ImGui::DragFloat2("Cell Size (px)", cellSize, 1.0f, 1.0f, 256.0f))
                {
                    tilemap->AtlasCellWidth = cellSize[0];
                    tilemap->AtlasCellHeight = cellSize[1];
                    // Recalculate columns
                    if (!tilemap->TextureName.empty())
                    {
                        if (auto* tex = textureLib.GetTexturePtr(tilemap->TextureName))
                        {
                            tilemap->AtlasColumns = static_cast<uint32_t>(tex->GetWidth() / tilemap->AtlasCellWidth);
                        }
                    }
                }

                // Show atlas info
                if (!tilemap->TextureName.empty())
                {
                    if (auto* tex = textureLib.GetTexturePtr(tilemap->TextureName))
                    {
                        uint32_t cols = static_cast<uint32_t>(tex->GetWidth() / tilemap->AtlasCellWidth);
                        uint32_t rows = static_cast<uint32_t>(tex->GetHeight() / tilemap->AtlasCellHeight);
                        ImGui::TextDisabled("Atlas: %dx%d cells (%d total)", cols, rows, cols * rows);
                    }
                }

                ImGui::Separator();

                // Tile editing section
                ImGui::Text("Tile Editing");
                ImGui::Checkbox("Edit Mode", &m_TilemapEditMode);

                if (m_TilemapEditMode)
                {
                    ImGui::Indent();

                    // Selected tile display
                    uint32_t selCellX, selCellY;
                    tilemap->IndexToCell(m_SelectedAtlasTile, selCellX, selCellY);
                    ImGui::Text("Selected: Index %d (Cell %d, %d)", m_SelectedAtlasTile, selCellX, selCellY);

                    // Tile index input
                    if (ImGui::DragInt("Brush Tile Index", &m_SelectedAtlasTile, 0.1f, -1, 9999))
                    {
                        // -1 is eraser
                    }
                    ImGui::TextDisabled("(-1 = Eraser)");

                    ImGui::Separator();

                    // Visual tile grid preview
                    ImGui::Text("Tile Grid Preview:");
                    ImGui::BeginChild("TileGrid", ImVec2(0, 200), true, ImGuiWindowFlags_HorizontalScrollbar);

                    ImDrawList* drawList = ImGui::GetWindowDrawList();
                    ImVec2 canvasPos = ImGui::GetCursorScreenPos();
                    float previewTileSize = 16.0f;

                    for (uint32_t ty = 0; ty < tilemap->Height; ty++)
                    {
                        for (uint32_t tx = 0; tx < tilemap->Width; tx++)
                        {
                            int32_t tileIdx = tilemap->GetTile(tx, ty);

                            float x0 = canvasPos.x + tx * previewTileSize;
                            float y0 = canvasPos.y + (tilemap->Height - 1 - ty) * previewTileSize;  // Flip Y for display
                            float x1 = x0 + previewTileSize;
                            float y1 = y0 + previewTileSize;

                            // Draw tile background
                            ImU32 color = (tileIdx < 0) ? IM_COL32(40, 40, 40, 255) : IM_COL32(80, 80, 200, 255);
                            drawList->AddRectFilled(ImVec2(x0, y0), ImVec2(x1, y1), color);
                            drawList->AddRect(ImVec2(x0, y0), ImVec2(x1, y1), IM_COL32(60, 60, 60, 255));
                        }
                    }

                    // Handle click to paint
                    if (ImGui::IsWindowHovered() && ImGui::IsMouseDown(0))
                    {
                        ImVec2 mousePos = ImGui::GetMousePos();
                        int clickX = static_cast<int>((mousePos.x - canvasPos.x) / previewTileSize);
                        int clickY = static_cast<int>(tilemap->Height) - 1 - static_cast<int>((mousePos.y - canvasPos.y) / previewTileSize);

                        if (clickX >= 0 && clickX < static_cast<int>(tilemap->Width) &&
                            clickY >= 0 && clickY < static_cast<int>(tilemap->Height))
                        {
                            tilemap->SetTile(static_cast<uint32_t>(clickX), static_cast<uint32_t>(clickY), m_SelectedAtlasTile);
                        }
                    }

                    // Expand child area to fit content
                    ImGui::Dummy(ImVec2(tilemap->Width * previewTileSize, tilemap->Height * previewTileSize));
                    ImGui::EndChild();

                    ImGui::Unindent();
                }

                // Fill/Clear buttons
                if (ImGui::Button("Fill All"))
                {
                    std::fill(tilemap->Tiles.begin(), tilemap->Tiles.end(), m_SelectedAtlasTile);
                }
                ImGui::SameLine();
                if (ImGui::Button("Clear All"))
                {
                    std::fill(tilemap->Tiles.begin(), tilemap->Tiles.end(), -1);
                }
            }
        }
        else
        {
            // Add Tilemap button
            if (ImGui::Button("Add Tilemap"))
            {
                auto& tilemap = m_ActiveScene->AddComponent<GGEngine::TilemapComponent>(m_SelectedEntity);
                tilemap.ResizeTiles();
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

    // Block ImGui events when viewport is focused or hovered (allow camera control)
    GGEngine::Application::Get().GetImGuiLayer()->SetBlockEvents(!m_ViewportFocused && !m_ViewportHovered);

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
                if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::I))
                    transform->Position[1] += velocity;
                if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::K))
                    transform->Position[1] -= velocity;
                if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::J))
                    transform->Position[0] -= velocity;
                if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::L))
                    transform->Position[0] += velocity;

                float rotationSpeed = 90.0f * ts;
                if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::U))
                    transform->Rotation += rotationSpeed;
                if (GGEngine::Input::IsKeyPressed(GGEngine::KeyCode::O))
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
