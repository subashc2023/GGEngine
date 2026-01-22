#include "ggpch.h"
#include "OrthographicCameraController.h"
#include "GGEngine/Core/Input.h"
#include "GGEngine/Core/KeyCodes.h"
#include "GGEngine/Core/MouseButtonCodes.h"

namespace GGEngine {

    OrthographicCameraController::OrthographicCameraController(float aspectRatio, float zoomLevel, bool enableRotation)
        : m_AspectRatio(aspectRatio), m_ZoomLevel(zoomLevel), m_RotationEnabled(enableRotation)
    {
        UpdateProjection();
        m_Camera.SetPosition(0.0f, 0.0f, 0.0f);
        m_Camera.UpdateMatrices();
    }

    void OrthographicCameraController::OnUpdate(Timestep ts)
    {
        // WASD movement
        float velocity = m_MoveSpeed * m_ZoomLevel * ts; // Scale speed with zoom

        if (Input::IsKeyPressed(GG_KEY_W))
            m_Camera.Translate(0.0f, velocity, 0.0f);
        if (Input::IsKeyPressed(GG_KEY_S))
            m_Camera.Translate(0.0f, -velocity, 0.0f);
        if (Input::IsKeyPressed(GG_KEY_A))
            m_Camera.Translate(-velocity, 0.0f, 0.0f);
        if (Input::IsKeyPressed(GG_KEY_D))
            m_Camera.Translate(velocity, 0.0f, 0.0f);

        // Q/E rotation (if enabled)
        if (m_RotationEnabled)
        {
            float rotationVelocity = m_RotationSpeed * ts;
            if (Input::IsKeyPressed(GG_KEY_Q))
                m_Camera.Rotate(rotationVelocity);
            if (Input::IsKeyPressed(GG_KEY_E))
                m_Camera.Rotate(-rotationVelocity);
        }

        // Right mouse drag to pan
        if (Input::IsMouseButtonPressed(GG_MOUSE_BUTTON_RIGHT))
        {
            auto [mouseX, mouseY] = Input::GetMousePosition();

            if (!m_IsDragging)
            {
                m_IsDragging = true;
                m_LastMouseX = mouseX;
                m_LastMouseY = mouseY;
            }
            else
            {
                float dx = mouseX - m_LastMouseX;
                float dy = mouseY - m_LastMouseY;
                m_LastMouseX = mouseX;
                m_LastMouseY = mouseY;

                // Pan speed scales with zoom level
                float panSpeed = 0.005f * m_ZoomLevel;
                m_Camera.Translate(-dx * panSpeed, dy * panSpeed, 0.0f);
            }
        }
        else
        {
            m_IsDragging = false;
        }

        m_Camera.UpdateMatrices();
    }

    void OrthographicCameraController::OnEvent(Event& e)
    {
        EventDispatcher dispatcher(e);
        dispatcher.Dispatch<MouseScrolledEvent>(GG_BIND_EVENT_FN(OrthographicCameraController::OnMouseScrolled));
    }

    void OrthographicCameraController::SetZoomLevel(float level)
    {
        m_ZoomLevel = level;
        ClampZoom();
        UpdateProjection();
    }

    void OrthographicCameraController::SetAspectRatio(float aspectRatio)
    {
        m_AspectRatio = aspectRatio;
        UpdateProjection();
    }

    bool OrthographicCameraController::OnMouseScrolled(MouseScrolledEvent& e)
    {
        m_ZoomLevel -= e.GetYOffset() * m_ZoomSpeed;
        ClampZoom();
        UpdateProjection();
        return false; // Don't consume - allow other handlers
    }

    void OrthographicCameraController::ClampZoom()
    {
        m_ZoomLevel = std::clamp(m_ZoomLevel, 0.1f, 100.0f);
    }

    void OrthographicCameraController::UpdateProjection()
    {
        float width = m_ZoomLevel * 2.0f;
        float height = width / m_AspectRatio;
        m_Camera.SetOrthographic(width, height, -100.0f, 100.0f);
    }

}
