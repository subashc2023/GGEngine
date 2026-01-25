#pragma once

#include "GGEngine/Core/Core.h"
#include <glm/glm.hpp>
#include <glm/gtc/matrix_transform.hpp>

namespace GGEngine {

    // Uniform buffer data for camera matrices
    struct GG_API CameraUBO
    {
        glm::mat4 view;
        glm::mat4 projection;
        glm::mat4 viewProjection;
    };

    // Simple orbit camera for the editor
    class GG_API Camera
    {
    public:
        Camera() = default;

        void SetPerspective(float fovDegrees, float aspect, float nearPlane, float farPlane);
        void SetOrthographic(float width, float height, float nearPlane, float farPlane);
        void SetPosition(float x, float y, float z);
        void Translate(float dx, float dy, float dz = 0.0f);
        void LookAt(float targetX, float targetY, float targetZ);

        bool IsOrthographic() const { return m_IsOrthographic; }

        // Zoom (works for both ortho and perspective)
        void Zoom(float delta);

        // Orthographic size control
        void SetOrthoSize(float width, float height);
        float GetOrthoWidth() const { return m_OrthoWidth; }
        float GetOrthoHeight() const { return m_OrthoHeight; }

        // Rotation (for 2D orthographic cameras)
        void SetRotation(float rotationRadians);
        float GetRotation() const { return m_Rotation; }
        void Rotate(float deltaRadians);

        // Orbit camera controls (perspective)
        void SetOrbitTarget(float x, float y, float z);
        void SetOrbitDistance(float distance);
        void SetOrbitAngles(float pitch, float yaw);  // In radians
        void OrbitRotate(float deltaPitch, float deltaYaw);
        void OrbitZoom(float delta);

        void UpdateMatrices();

        const glm::mat4& GetViewMatrix() const { return m_ViewMatrix; }
        const glm::mat4& GetProjectionMatrix() const { return m_ProjectionMatrix; }
        const glm::mat4& GetViewProjectionMatrix() const { return m_ViewProjectionMatrix; }

        CameraUBO GetUBO() const
        {
            CameraUBO ubo;
            ubo.view = m_ViewMatrix;
            ubo.projection = m_ProjectionMatrix;
            ubo.viewProjection = m_ViewProjectionMatrix;
            return ubo;
        }

        float GetPitch() const { return m_Pitch; }
        float GetYaw() const { return m_Yaw; }
        float GetDistance() const { return m_Distance; }
        float GetPositionX() const { return m_PositionX; }
        float GetPositionY() const { return m_PositionY; }
        float GetPositionZ() const { return m_PositionZ; }

    private:
        void UpdateOrbitPosition();

        // Position and orientation
        float m_PositionX = 0.0f, m_PositionY = 0.0f, m_PositionZ = 3.0f;
        float m_TargetX = 0.0f, m_TargetY = 0.0f, m_TargetZ = 0.0f;

        // Orbit parameters
        float m_Pitch = 0.0f;  // Vertical angle (radians)
        float m_Yaw = 0.0f;    // Horizontal angle (radians)
        float m_Distance = 3.0f;

        // Projection parameters
        bool m_IsOrthographic = false;
        float m_FovRadians = 0.785f;  // ~45 degrees
        float m_OrthoWidth = 10.0f;
        float m_OrthoHeight = 10.0f;
        float m_Aspect = 16.0f / 9.0f;
        float m_Near = -100.0f;
        float m_Far = 100.0f;

        // 2D rotation (for orthographic cameras)
        float m_Rotation = 0.0f;  // Radians

        // Cached matrices
        glm::mat4 m_ViewMatrix{1.0f};
        glm::mat4 m_ProjectionMatrix{1.0f};
        glm::mat4 m_ViewProjectionMatrix{1.0f};
    };

}
