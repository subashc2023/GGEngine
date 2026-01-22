#pragma once

#include "GGEngine/Core/Core.h"
#include <cmath>

namespace GGEngine {

    // Simple 4x4 matrix (column-major for Vulkan/GLM compatibility)
    struct GG_API Mat4
    {
        float data[16] = {
            1, 0, 0, 0,
            0, 1, 0, 0,
            0, 0, 1, 0,
            0, 0, 0, 1
        };

        static Mat4 Identity() { return Mat4{}; }

        static Mat4 Translate(float x, float y, float z)
        {
            Mat4 m{};
            m.data[12] = x;
            m.data[13] = y;
            m.data[14] = z;
            return m;
        }

        static Mat4 Scale(float x, float y, float z)
        {
            Mat4 m{};
            m.data[0] = x;
            m.data[5] = y;
            m.data[10] = z;
            return m;
        }

        static Mat4 RotateZ(float angleRadians)
        {
            Mat4 m{};
            float c = std::cos(angleRadians);
            float s = std::sin(angleRadians);
            m.data[0] = c;
            m.data[1] = s;
            m.data[4] = -s;
            m.data[5] = c;
            return m;
        }

        static Mat4 Perspective(float fovY, float aspect, float nearPlane, float farPlane)
        {
            Mat4 m{};
            float tanHalfFov = std::tan(fovY / 2.0f);

            // Standard perspective projection (Y-flip handled by viewport)
            m.data[0] = 1.0f / (aspect * tanHalfFov);
            m.data[5] = 1.0f / tanHalfFov;
            m.data[10] = farPlane / (nearPlane - farPlane);
            m.data[11] = -1.0f;
            m.data[14] = (nearPlane * farPlane) / (nearPlane - farPlane);
            m.data[15] = 0.0f;

            return m;
        }

        static Mat4 Orthographic(float left, float right, float bottom, float top, float nearPlane, float farPlane)
        {
            Mat4 m{};

            // Standard orthographic projection (Y-flip handled by viewport)
            m.data[0] = 2.0f / (right - left);
            m.data[5] = 2.0f / (top - bottom);
            m.data[10] = 1.0f / (nearPlane - farPlane);
            m.data[12] = -(right + left) / (right - left);
            m.data[13] = -(top + bottom) / (top - bottom);
            m.data[14] = nearPlane / (nearPlane - farPlane);
            m.data[15] = 1.0f;

            return m;
        }

        static Mat4 LookAt(float eyeX, float eyeY, float eyeZ,
                          float targetX, float targetY, float targetZ,
                          float upX, float upY, float upZ)
        {
            // Forward (from target to eye for RH coordinate system)
            float fX = eyeX - targetX;
            float fY = eyeY - targetY;
            float fZ = eyeZ - targetZ;
            float fLen = std::sqrt(fX * fX + fY * fY + fZ * fZ);
            fX /= fLen; fY /= fLen; fZ /= fLen;

            // Right = up x forward
            float rX = upY * fZ - upZ * fY;
            float rY = upZ * fX - upX * fZ;
            float rZ = upX * fY - upY * fX;
            float rLen = std::sqrt(rX * rX + rY * rY + rZ * rZ);
            rX /= rLen; rY /= rLen; rZ /= rLen;

            // Up = forward x right
            float uX = fY * rZ - fZ * rY;
            float uY = fZ * rX - fX * rZ;
            float uZ = fX * rY - fY * rX;

            Mat4 m{};
            m.data[0] = rX;  m.data[4] = rY;  m.data[8] = rZ;   m.data[12] = -(rX * eyeX + rY * eyeY + rZ * eyeZ);
            m.data[1] = uX;  m.data[5] = uY;  m.data[9] = uZ;   m.data[13] = -(uX * eyeX + uY * eyeY + uZ * eyeZ);
            m.data[2] = fX;  m.data[6] = fY;  m.data[10] = fZ;  m.data[14] = -(fX * eyeX + fY * eyeY + fZ * eyeZ);
            m.data[3] = 0;   m.data[7] = 0;   m.data[11] = 0;   m.data[15] = 1;

            return m;
        }

        Mat4 operator*(const Mat4& other) const
        {
            Mat4 result{};
            for (int col = 0; col < 4; col++)
            {
                for (int row = 0; row < 4; row++)
                {
                    result.data[col * 4 + row] =
                        data[0 * 4 + row] * other.data[col * 4 + 0] +
                        data[1 * 4 + row] * other.data[col * 4 + 1] +
                        data[2 * 4 + row] * other.data[col * 4 + 2] +
                        data[3 * 4 + row] * other.data[col * 4 + 3];
                }
            }
            return result;
        }

        static Mat4 Inverse(const Mat4& m)
        {
            Mat4 inv;

            float a00 = m.data[0], a01 = m.data[1], a02 = m.data[2], a03 = m.data[3];
            float a10 = m.data[4], a11 = m.data[5], a12 = m.data[6], a13 = m.data[7];
            float a20 = m.data[8], a21 = m.data[9], a22 = m.data[10], a23 = m.data[11];
            float a30 = m.data[12], a31 = m.data[13], a32 = m.data[14], a33 = m.data[15];

            float b00 = a00 * a11 - a01 * a10;
            float b01 = a00 * a12 - a02 * a10;
            float b02 = a00 * a13 - a03 * a10;
            float b03 = a01 * a12 - a02 * a11;
            float b04 = a01 * a13 - a03 * a11;
            float b05 = a02 * a13 - a03 * a12;
            float b06 = a20 * a31 - a21 * a30;
            float b07 = a20 * a32 - a22 * a30;
            float b08 = a20 * a33 - a23 * a30;
            float b09 = a21 * a32 - a22 * a31;
            float b10 = a21 * a33 - a23 * a31;
            float b11 = a22 * a33 - a23 * a32;

            float det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;

            if (std::abs(det) < 1e-8f)
                return Mat4::Identity();

            float invDet = 1.0f / det;

            inv.data[0]  = ( a11 * b11 - a12 * b10 + a13 * b09) * invDet;
            inv.data[1]  = (-a01 * b11 + a02 * b10 - a03 * b09) * invDet;
            inv.data[2]  = ( a31 * b05 - a32 * b04 + a33 * b03) * invDet;
            inv.data[3]  = (-a21 * b05 + a22 * b04 - a23 * b03) * invDet;
            inv.data[4]  = (-a10 * b11 + a12 * b08 - a13 * b07) * invDet;
            inv.data[5]  = ( a00 * b11 - a02 * b08 + a03 * b07) * invDet;
            inv.data[6]  = (-a30 * b05 + a32 * b02 - a33 * b01) * invDet;
            inv.data[7]  = ( a20 * b05 - a22 * b02 + a23 * b01) * invDet;
            inv.data[8]  = ( a10 * b10 - a11 * b08 + a13 * b06) * invDet;
            inv.data[9]  = (-a00 * b10 + a01 * b08 - a03 * b06) * invDet;
            inv.data[10] = ( a30 * b04 - a31 * b02 + a33 * b00) * invDet;
            inv.data[11] = (-a20 * b04 + a21 * b02 - a23 * b00) * invDet;
            inv.data[12] = (-a10 * b09 + a11 * b07 - a12 * b06) * invDet;
            inv.data[13] = ( a00 * b09 - a01 * b07 + a02 * b06) * invDet;
            inv.data[14] = (-a30 * b03 + a31 * b01 - a32 * b00) * invDet;
            inv.data[15] = ( a20 * b03 - a21 * b01 + a22 * b00) * invDet;

            return inv;
        }
    };

    // Uniform buffer data for camera matrices
    struct GG_API CameraUBO
    {
        Mat4 view;
        Mat4 projection;
        Mat4 viewProjection;
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

        const Mat4& GetViewMatrix() const { return m_ViewMatrix; }
        const Mat4& GetProjectionMatrix() const { return m_ProjectionMatrix; }
        const Mat4& GetViewProjectionMatrix() const { return m_ViewProjectionMatrix; }

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
        Mat4 m_ViewMatrix;
        Mat4 m_ProjectionMatrix;
        Mat4 m_ViewProjectionMatrix;
    };

}
