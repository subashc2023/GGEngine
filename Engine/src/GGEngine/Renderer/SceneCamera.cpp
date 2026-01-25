#include "ggpch.h"
#include "SceneCamera.h"
#include <glm/gtc/matrix_transform.hpp>

namespace GGEngine {

    SceneCamera::SceneCamera()
    {
        RecalculateProjection();
    }

    void SceneCamera::SetProjectionType(ProjectionType type)
    {
        m_ProjectionType = type;
        RecalculateProjection();
    }

    void SceneCamera::SetPerspective(float fovDegrees, float nearClip, float farClip)
    {
        m_ProjectionType = ProjectionType::Perspective;
        m_PerspectiveFOV = fovDegrees;
        m_PerspectiveNear = nearClip;
        m_PerspectiveFar = farClip;
        RecalculateProjection();
    }

    void SceneCamera::SetOrthographic(float size, float nearClip, float farClip)
    {
        m_ProjectionType = ProjectionType::Orthographic;
        m_OrthographicSize = size;
        m_OrthographicNear = nearClip;
        m_OrthographicFar = farClip;
        RecalculateProjection();
    }

    void SceneCamera::SetViewportSize(uint32_t width, uint32_t height)
    {
        if (height == 0) height = 1;
        m_AspectRatio = static_cast<float>(width) / static_cast<float>(height);
        RecalculateProjection();
    }

    void SceneCamera::RecalculateProjection()
    {
        if (m_ProjectionType == ProjectionType::Perspective)
        {
            float fovRadians = glm::radians(m_PerspectiveFOV);
            m_Projection = glm::perspective(fovRadians, m_AspectRatio,
                                            m_PerspectiveNear, m_PerspectiveFar);
            // Flip Y for Vulkan coordinate system
            m_Projection[1][1] *= -1.0f;
        }
        else
        {
            float orthoLeft = -m_OrthographicSize * m_AspectRatio;
            float orthoRight = m_OrthographicSize * m_AspectRatio;
            float orthoBottom = -m_OrthographicSize;
            float orthoTop = m_OrthographicSize;
            m_Projection = glm::ortho(orthoLeft, orthoRight, orthoBottom, orthoTop,
                                      m_OrthographicNear, m_OrthographicFar);
        }
    }

}
