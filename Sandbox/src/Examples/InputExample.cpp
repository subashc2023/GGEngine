#include "InputExample.h"
#include "GGEngine/Core/Input.h"
#include "GGEngine/Core/KeyCodes.h"
#include "GGEngine/Core/MouseButtonCodes.h"
#include "GGEngine/Core/Application.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Events/KeyEvent.h"
#include "GGEngine/Events/MouseEvent.h"

#include <imgui.h>
#include <sstream>

InputExample::InputExample()
    : Example("Input System",
              "Demonstrates keyboard and mouse input (polling and events)")
{
}

void InputExample::LogEvent(const std::string& msg)
{
    m_EventLog[m_LogIndex] = msg;
    m_LogIndex = (m_LogIndex + 1) % MaxLogEntries;
}

void InputExample::OnUpdate(GGEngine::Timestep ts)
{
    using namespace GGEngine;

    // Keyboard polling - Arrow keys move the blue box (WASD reserved for camera)
    float velocity = m_MoveSpeed * ts;

    if (Input::IsKeyPressed(KeyCode::Up))
        m_Position[1] += velocity;
    if (Input::IsKeyPressed(KeyCode::Down))
        m_Position[1] -= velocity;
    if (Input::IsKeyPressed(KeyCode::Left))
        m_Position[0] -= velocity;
    if (Input::IsKeyPressed(KeyCode::Right))
        m_Position[0] += velocity;

    // Mouse polling
    auto [mouseX, mouseY] = Input::GetMousePosition();
    auto& window = Application::Get().GetWindow();

    // Convert to normalized device coordinates (-1 to 1)
    float ndcX = (mouseX / window.GetWidth()) * 2.0f - 1.0f;
    float ndcY = 1.0f - (mouseY / window.GetHeight()) * 2.0f;

    // Simple world coords (assuming 5 unit view)
    m_MouseWorldX = ndcX * 5.0f;
    m_MouseWorldY = ndcY * 3.0f;

    m_LeftMouseDown = Input::IsMouseButtonPressed(MouseCode::Left);
    m_RightMouseDown = Input::IsMouseButtonPressed(MouseCode::Right);
}

void InputExample::OnRender(const GGEngine::Camera& camera)
{
    using namespace GGEngine;

    Renderer2D::ResetStats();
    Renderer2D::BeginScene(camera);

    // Player quad (controlled by WASD)
    Renderer2D::DrawQuad(m_Position[0], m_Position[1], 0.5f, 0.5f, 0.2f, 0.6f, 0.9f);

    // Mouse cursor indicator
    float cursorColor = m_LeftMouseDown ? 1.0f : 0.3f;
    Renderer2D::DrawQuad(m_MouseWorldX, m_MouseWorldY, 0.2f, 0.2f,
                        cursorColor, cursorColor * 0.5f, 0.2f);

    // Right-click draws a marker
    if (m_RightMouseDown)
    {
        Renderer2D::DrawQuad(m_MouseWorldX, m_MouseWorldY, 0.0f, 0.4f, 0.4f, 0.9f, 0.2f, 0.2f, 0.5f);
    }

    // Visual keyboard indicator - Arrow keys
    float indicatorY = -2.5f;
    float indicatorSize = 0.3f;

    // Up arrow
    float upBright = Input::IsKeyPressed(KeyCode::Up) ? 1.0f : 0.3f;
    Renderer2D::DrawQuad(-3.0f, indicatorY + 0.35f, indicatorSize, indicatorSize, upBright, upBright, upBright);

    // Left, Down, Right arrows
    float leftBright = Input::IsKeyPressed(KeyCode::Left) ? 1.0f : 0.3f;
    float downBright = Input::IsKeyPressed(KeyCode::Down) ? 1.0f : 0.3f;
    float rightBright = Input::IsKeyPressed(KeyCode::Right) ? 1.0f : 0.3f;
    Renderer2D::DrawQuad(-3.35f, indicatorY, indicatorSize, indicatorSize, leftBright, leftBright, leftBright);
    Renderer2D::DrawQuad(-3.0f, indicatorY, indicatorSize, indicatorSize, downBright, downBright, downBright);
    Renderer2D::DrawQuad(-2.65f, indicatorY, indicatorSize, indicatorSize, rightBright, rightBright, rightBright);

    // Space bar
    float spaceBright = Input::IsKeyPressed(KeyCode::Space) ? 1.0f : 0.3f;
    Renderer2D::DrawQuad(-3.0f, indicatorY - 0.4f, 1.0f, indicatorSize, spaceBright, spaceBright, spaceBright);

    Renderer2D::EndScene();
}

void InputExample::OnEvent(GGEngine::Event& event)
{
    using namespace GGEngine;

    EventDispatcher dispatcher(event);

    dispatcher.Dispatch<KeyPressedEvent>([this](KeyPressedEvent& e) {
        std::stringstream ss;
        ss << "Key Pressed: " << static_cast<int>(e.GetKeyCode());
        if (e.GetRepeatCount() > 0)
            ss << " (repeat)";
        LogEvent(ss.str());
        return false;
    });

    dispatcher.Dispatch<KeyReleasedEvent>([this](KeyReleasedEvent& e) {
        std::stringstream ss;
        ss << "Key Released: " << static_cast<int>(e.GetKeyCode());
        LogEvent(ss.str());
        return false;
    });

    dispatcher.Dispatch<MouseButtonPressedEvent>([this](MouseButtonPressedEvent& e) {
        std::stringstream ss;
        ss << "Mouse Pressed: " << static_cast<int>(e.GetMouseButton());
        LogEvent(ss.str());
        return false;
    });

    dispatcher.Dispatch<MouseButtonReleasedEvent>([this](MouseButtonReleasedEvent& e) {
        std::stringstream ss;
        ss << "Mouse Released: " << static_cast<int>(e.GetMouseButton());
        LogEvent(ss.str());
        return false;
    });

    dispatcher.Dispatch<MouseScrolledEvent>([this](MouseScrolledEvent& e) {
        std::stringstream ss;
        ss << "Mouse Scroll: " << e.GetXOffset() << ", " << e.GetYOffset();
        LogEvent(ss.str());
        return false;
    });
}

void InputExample::OnImGuiRender()
{
    using namespace GGEngine;

    ImGui::Text("Controls:");
    ImGui::BulletText("Arrow Keys: Move blue quad");
    ImGui::BulletText("WASD: Move camera (handled by ExamplesLayer)");
    ImGui::BulletText("Mouse: Orange cursor follows");
    ImGui::BulletText("Left Click: Brightens cursor");
    ImGui::BulletText("Right Click: Red marker");

    ImGui::Separator();
    ImGui::Text("Polling State:");
    ImGui::Text("Position: %.2f, %.2f", m_Position[0], m_Position[1]);
    ImGui::Text("Mouse Screen: %.0f, %.0f", Input::GetMouseX(), Input::GetMouseY());
    ImGui::Text("Mouse World: %.2f, %.2f", m_MouseWorldX, m_MouseWorldY);
    ImGui::Text("LMB: %s  RMB: %s",
               m_LeftMouseDown ? "DOWN" : "up",
               m_RightMouseDown ? "DOWN" : "up");

    ImGui::Separator();
    ImGui::SliderFloat("Move Speed", &m_MoveSpeed, 1.0f, 10.0f);

    ImGui::Separator();
    ImGui::Text("Key States (sample):");
    ImGui::Text("Up:%d Down:%d Left:%d Right:%d Space:%d",
               Input::IsKeyPressed(KeyCode::Up),
               Input::IsKeyPressed(KeyCode::Down),
               Input::IsKeyPressed(KeyCode::Left),
               Input::IsKeyPressed(KeyCode::Right),
               Input::IsKeyPressed(KeyCode::Space));

    ImGui::Separator();
    ImGui::Text("Event Log:");
    ImGui::BeginChild("EventLog", ImVec2(0, 120), true);
    for (int i = 0; i < MaxLogEntries; i++)
    {
        int idx = (m_LogIndex - 1 - i + MaxLogEntries) % MaxLogEntries;
        if (!m_EventLog[idx].empty())
        {
            ImGui::Text("%s", m_EventLog[idx].c_str());
        }
    }
    ImGui::EndChild();
}
