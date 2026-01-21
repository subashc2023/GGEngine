#include "ggpch.h"
#include "Camera.h"

namespace GGEngine {

    constexpr float PI = 3.14159265358979323846f;

    void Camera::SetPerspective(float fovDegrees, float aspect, float nearPlane, float farPlane)
    {
        m_IsOrthographic = false;
        m_FovRadians = fovDegrees * PI / 180.0f;
        m_Aspect = aspect;
        m_Near = nearPlane;
        m_Far = farPlane;
        m_ProjectionMatrix = Mat4::Perspective(m_FovRadians, m_Aspect, m_Near, m_Far);
    }

    void Camera::SetOrthographic(float width, float height, float nearPlane, float farPlane)
    {
        m_IsOrthographic = true;
        m_OrthoWidth = width;
        m_OrthoHeight = height;
        m_Near = nearPlane;
        m_Far = farPlane;
        float halfWidth = width * 0.5f;
        float halfHeight = height * 0.5f;
        m_ProjectionMatrix = Mat4::Orthographic(-halfWidth, halfWidth, -halfHeight, halfHeight, m_Near, m_Far);
    }

    void Camera::SetPosition(float x, float y, float z)
    {
        m_PositionX = x;
        m_PositionY = y;
        m_PositionZ = z;
    }

    void Camera::Translate(float dx, float dy, float dz)
    {
        m_PositionX += dx;
        m_PositionY += dy;
        m_PositionZ += dz;

        // For orbit camera, also move the target to maintain relative position
        if (!m_IsOrthographic)
        {
            m_TargetX += dx;
            m_TargetY += dy;
            m_TargetZ += dz;
        }
    }

    void Camera::Zoom(float delta)
    {
        if (m_IsOrthographic)
        {
            // Zoom by scaling ortho size (positive delta = zoom in = smaller size)
            float scale = 1.0f - delta * 0.1f;
            if (scale < 0.01f) scale = 0.01f;
            m_OrthoWidth *= scale;
            m_OrthoHeight *= scale;
            // Clamp to reasonable bounds
            if (m_OrthoWidth < 0.1f) m_OrthoWidth = 0.1f;
            if (m_OrthoWidth > 1000.0f) m_OrthoWidth = 1000.0f;
            if (m_OrthoHeight < 0.1f) m_OrthoHeight = 0.1f;
            if (m_OrthoHeight > 1000.0f) m_OrthoHeight = 1000.0f;
        }
        else
        {
            // For perspective, use existing orbit zoom behavior
            OrbitZoom(delta);
        }
    }

    void Camera::SetOrthoSize(float width, float height)
    {
        m_OrthoWidth = width;
        m_OrthoHeight = height;
    }

    void Camera::SetRotation(float rotationRadians)
    {
        m_Rotation = rotationRadians;
    }

    void Camera::Rotate(float deltaRadians)
    {
        m_Rotation += deltaRadians;
    }

    void Camera::LookAt(float targetX, float targetY, float targetZ)
    {
        m_TargetX = targetX;
        m_TargetY = targetY;
        m_TargetZ = targetZ;
    }

    void Camera::SetOrbitTarget(float x, float y, float z)
    {
        m_TargetX = x;
        m_TargetY = y;
        m_TargetZ = z;
        UpdateOrbitPosition();
    }

    void Camera::SetOrbitDistance(float distance)
    {
        m_Distance = distance;
        if (m_Distance < 0.1f) m_Distance = 0.1f;
        UpdateOrbitPosition();
    }

    void Camera::SetOrbitAngles(float pitch, float yaw)
    {
        m_Pitch = pitch;
        m_Yaw = yaw;

        // Clamp pitch to avoid gimbal lock
        constexpr float maxPitch = PI / 2.0f - 0.01f;
        if (m_Pitch > maxPitch) m_Pitch = maxPitch;
        if (m_Pitch < -maxPitch) m_Pitch = -maxPitch;

        UpdateOrbitPosition();
    }

    void Camera::OrbitRotate(float deltaPitch, float deltaYaw)
    {
        SetOrbitAngles(m_Pitch + deltaPitch, m_Yaw + deltaYaw);
    }

    void Camera::OrbitZoom(float delta)
    {
        SetOrbitDistance(m_Distance - delta);
    }

    void Camera::UpdateOrbitPosition()
    {
        // Calculate position on sphere around target
        float cosPitch = std::cos(m_Pitch);
        float sinPitch = std::sin(m_Pitch);
        float cosYaw = std::cos(m_Yaw);
        float sinYaw = std::sin(m_Yaw);

        m_PositionX = m_TargetX + m_Distance * cosPitch * sinYaw;
        m_PositionY = m_TargetY + m_Distance * sinPitch;
        m_PositionZ = m_TargetZ + m_Distance * cosPitch * cosYaw;
    }

    void Camera::UpdateMatrices()
    {
        if (m_IsOrthographic)
        {
            float halfWidth = m_OrthoWidth * 0.5f;
            float halfHeight = m_OrthoHeight * 0.5f;
            m_ProjectionMatrix = Mat4::Orthographic(-halfWidth, halfWidth, -halfHeight, halfHeight, m_Near, m_Far);

            // 2D view matrix: rotation then translation
            // V = R * T (rotate first, then translate in rotated space)
            Mat4 translation = Mat4::Translate(-m_PositionX, -m_PositionY, 0.0f);
            Mat4 rotation = Mat4::RotateZ(-m_Rotation);  // Negative to rotate world opposite to camera
            m_ViewMatrix = rotation * translation;
        }
        else
        {
            m_ProjectionMatrix = Mat4::Perspective(m_FovRadians, m_Aspect, m_Near, m_Far);
            m_ViewMatrix = Mat4::LookAt(
                m_PositionX, m_PositionY, m_PositionZ,
                m_TargetX, m_TargetY, m_TargetZ,
                0.0f, 1.0f, 0.0f  // Up vector
            );
        }
        m_ViewProjectionMatrix = m_ProjectionMatrix * m_ViewMatrix;
    }

}
