#include "ExamplesLayer.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/ImGui/DebugUI.h"
#include "GGEngine/Core/Input.h"
#include "GGEngine/Core/KeyCodes.h"
#include "GGEngine/Core/Application.h"
#include "GGEngine/Core/Profiler.h"

// Include all examples
#include "Examples/Renderer2DBasicsExample.h"
#include "Examples/Renderer2DTexturesExample.h"
#include "Examples/ECSExample.h"
#include "Examples/ECSCameraExample.h"
#include "Examples/InputExample.h"
#include "Examples/ParticleExample.h"
#include "Examples/MultithreadingExample.h"
#include "Examples/TimestepExample.h"

#include <imgui.h>

ExamplesLayer::ExamplesLayer()
    : Layer("ExamplesLayer"), m_CameraController(16.0f / 9.0f, 5.0f, true)  // Default, will be updated in OnAttach
{
}

void ExamplesLayer::OnAttach()
{
    GG_INFO("ExamplesLayer attached - API Examples");

    // Set correct aspect ratio based on actual window size
    auto& window = GGEngine::Application::Get().GetWindow();
    float aspectRatio = static_cast<float>(window.GetWidth()) / static_cast<float>(window.GetHeight());
    m_CameraController.SetAspectRatio(aspectRatio);

    // Register all examples
    m_Examples.push_back(std::make_unique<Renderer2DBasicsExample>());
    m_Examples.push_back(std::make_unique<Renderer2DTexturesExample>());
    m_Examples.push_back(std::make_unique<ECSExample>());
    m_Examples.push_back(std::make_unique<ECSCameraExample>());
    m_Examples.push_back(std::make_unique<InputExample>());
    m_Examples.push_back(std::make_unique<ParticleExample>());
    m_Examples.push_back(std::make_unique<MultithreadingExample>());
    m_Examples.push_back(std::make_unique<TimestepExample>());

    // Start with first example
    SwitchExample(0);
}

void ExamplesLayer::OnDetach()
{
    if (m_CurrentExample)
    {
        m_CurrentExample->OnDetach();
    }
    m_Examples.clear();
    GG_INFO("ExamplesLayer detached");
}

void ExamplesLayer::SwitchExample(int index)
{
    if (index < 0 || index >= static_cast<int>(m_Examples.size()))
        return;

    // Detach current
    if (m_CurrentExample)
    {
        m_CurrentExample->OnDetach();
    }

    // Switch
    m_CurrentExampleIndex = index;
    m_CurrentExample = m_Examples[index].get();

    // Attach new
    m_CurrentExample->OnAttach();

    GG_INFO("Switched to example: {}", m_CurrentExample->GetName());
}

void ExamplesLayer::OnFixedUpdate(float fixedDeltaTime)
{
    // Forward to current example
    if (m_CurrentExample)
    {
        m_CurrentExample->OnFixedUpdate(fixedDeltaTime);
    }
}

void ExamplesLayer::OnUpdate(GGEngine::Timestep ts)
{
    using namespace GGEngine;

    // Camera controls (always active)
    m_CameraController.OnUpdate(ts);

    // Number keys to switch examples (1-9)
    for (int i = 0; i < std::min(9, static_cast<int>(m_Examples.size())); i++)
    {
        // KeyCode::D1 through D9
        KeyCode key = static_cast<KeyCode>(static_cast<int>(KeyCode::D1) + i);
        if (Input::IsKeyPressed(key))
        {
            if (m_CurrentExampleIndex != i)
            {
                SwitchExample(i);
            }
            break;  // Only process one key
        }
    }

    // Update current example
    if (m_CurrentExample)
    {
        GG_PROFILE_SCOPE("Example::OnUpdate");
        m_CurrentExample->OnUpdate(ts, m_CameraController.GetCamera());
    }

    // Render current example
    if (m_CurrentExample)
    {
        GG_PROFILE_SCOPE("Example::OnRender");
        m_CurrentExample->OnRender(m_CameraController.GetCamera());
    }

    // ImGui
    ImGui::Begin("Examples");

    // Example selector
    ImGui::Text("Select Example (or press 1-5):");
    for (int i = 0; i < static_cast<int>(m_Examples.size()); i++)
    {
        bool isSelected = (i == m_CurrentExampleIndex);
        char label[64];
        snprintf(label, sizeof(label), "[%d] %s", i + 1, m_Examples[i]->GetName().c_str());

        if (ImGui::Selectable(label, isSelected))
        {
            SwitchExample(i);
        }
    }

    ImGui::Separator();

    // Current example info
    if (m_CurrentExample)
    {
        ImGui::TextColored(ImVec4(0.4f, 0.8f, 1.0f, 1.0f), "%s", m_CurrentExample->GetName().c_str());
        ImGui::TextWrapped("%s", m_CurrentExample->GetDescription().c_str());
        ImGui::Separator();

        // Example-specific controls
        m_CurrentExample->OnImGuiRender();
    }

    ImGui::Separator();

    // Camera info
    if (ImGui::CollapsingHeader("Camera Controls"))
    {
        ImGui::Text("WASD: Pan camera");
        ImGui::Text("Q/E: Rotate camera");
        ImGui::Text("Scroll: Zoom");
        ImGui::Text("RMB + Drag: Pan");
    }

    // Stats
    ImGui::Separator();
    GGEngine::DebugUI::ShowStatsContent(ts);

    ImGui::End();

    // Show profiler window separately
    GGEngine::DebugUI::ShowProfiler();
}

void ExamplesLayer::OnEvent(GGEngine::Event& event)
{
    // Camera events
    m_CameraController.OnEvent(event);

    // Forward to current example
    if (m_CurrentExample)
    {
        m_CurrentExample->OnEvent(event);
    }
}

void ExamplesLayer::OnWindowResize(uint32_t width, uint32_t height)
{
    if (width > 0 && height > 0)
    {
        float aspectRatio = static_cast<float>(width) / static_cast<float>(height);
        m_CameraController.SetAspectRatio(aspectRatio);
    }
}
