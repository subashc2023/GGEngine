#pragma once

#include "GGEngine/Core.h"
#include "GGEngine/Core/Timestep.h"
#include "GGEngine/Renderer/Camera.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Events/MouseEvent.h"

namespace GGEngine {

    class GG_API OrthographicCameraController
    {
    public:
        OrthographicCameraController(float aspectRatio, float zoomLevel = 1.0f);

        void OnUpdate(Timestep ts);
        void OnEvent(Event& e);

        void SetZoomLevel(float level);
        float GetZoomLevel() const { return m_ZoomLevel; }

        void SetAspectRatio(float aspectRatio);

        Camera& GetCamera() { return m_Camera; }
        const Camera& GetCamera() const { return m_Camera; }

        float GetMoveSpeed() const { return m_MoveSpeed; }
        void SetMoveSpeed(float speed) { m_MoveSpeed = speed; }

        float GetZoomSpeed() const { return m_ZoomSpeed; }
        void SetZoomSpeed(float speed) { m_ZoomSpeed = speed; }

    private:
        bool OnMouseScrolled(MouseScrolledEvent& e);

        void UpdateProjection();

        float m_AspectRatio;
        float m_ZoomLevel;
        Camera m_Camera;

        float m_MoveSpeed = 2.0f;
        float m_ZoomSpeed = 0.1f;

        // Mouse drag state
        bool m_IsDragging = false;
        float m_LastMouseX = 0.0f;
        float m_LastMouseY = 0.0f;
    };

}
