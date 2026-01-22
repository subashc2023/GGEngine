#pragma once

#include "Example.h"

// Demonstrates the Input system:
// - Keyboard polling (IsKeyPressed)
// - Mouse position
// - Mouse button polling
// - Event-based input vs polling
class InputExample : public Example
{
public:
    InputExample();

    void OnUpdate(GGEngine::Timestep ts) override;
    void OnRender(const GGEngine::Camera& camera) override;
    void OnImGuiRender() override;
    void OnEvent(GGEngine::Event& event) override;

private:
    // Controllable position
    float m_Position[2] = { 0.0f, 0.0f };
    float m_MoveSpeed = 3.0f;

    // Mouse tracking
    float m_MouseWorldX = 0.0f;
    float m_MouseWorldY = 0.0f;
    bool m_LeftMouseDown = false;
    bool m_RightMouseDown = false;

    // Event log
    static const int MaxLogEntries = 10;
    std::string m_EventLog[MaxLogEntries];
    int m_LogIndex = 0;

    void LogEvent(const std::string& msg);
};
