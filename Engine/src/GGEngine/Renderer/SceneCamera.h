#pragma once

#include "GGEngine/Core/Core.h"
#include "Camera.h"

namespace GGEngine {

    class GG_API SceneCamera
    {
    public:
        enum class ProjectionType { Perspective = 0, Orthographic = 1 };

        SceneCamera();
        ~SceneCamera() = default;

        // Projection type
        void SetProjectionType(ProjectionType type);
        ProjectionType GetProjectionType() const { return m_ProjectionType; }

        // Perspective projection
        void SetPerspective(float fovDegrees, float nearClip, float farClip);
        float GetPerspectiveFOV() const { return m_PerspectiveFOV; }
        void SetPerspectiveFOV(float fov) { m_PerspectiveFOV = fov; RecalculateProjection(); }
        float GetPerspectiveNearClip() const { return m_PerspectiveNear; }
        void SetPerspectiveNearClip(float nearClip) { m_PerspectiveNear = nearClip; RecalculateProjection(); }
        float GetPerspectiveFarClip() const { return m_PerspectiveFar; }
        void SetPerspectiveFarClip(float farClip) { m_PerspectiveFar = farClip; RecalculateProjection(); }

        // Orthographic projection
        void SetOrthographic(float size, float nearClip, float farClip);
        float GetOrthographicSize() const { return m_OrthographicSize; }
        void SetOrthographicSize(float size) { m_OrthographicSize = size; RecalculateProjection(); }
        float GetOrthographicNearClip() const { return m_OrthographicNear; }
        void SetOrthographicNearClip(float nearClip) { m_OrthographicNear = nearClip; RecalculateProjection(); }
        float GetOrthographicFarClip() const { return m_OrthographicFar; }
        void SetOrthographicFarClip(float farClip) { m_OrthographicFar = farClip; RecalculateProjection(); }

        // Viewport (aspect ratio)
        void SetViewportSize(uint32_t width, uint32_t height);

        // Get the projection matrix
        const Mat4& GetProjection() const { return m_Projection; }

    private:
        void RecalculateProjection();

        ProjectionType m_ProjectionType = ProjectionType::Orthographic;

        // Perspective parameters
        float m_PerspectiveFOV = 45.0f;  // Degrees
        float m_PerspectiveNear = 0.01f;
        float m_PerspectiveFar = 1000.0f;

        // Orthographic parameters
        float m_OrthographicSize = 10.0f;  // Half-height in world units
        float m_OrthographicNear = -1.0f;
        float m_OrthographicFar = 1.0f;

        float m_AspectRatio = 1.0f;
        Mat4 m_Projection;
    };

}
