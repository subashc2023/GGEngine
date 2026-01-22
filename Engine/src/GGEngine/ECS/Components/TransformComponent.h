#pragma once

#include "GGEngine/Renderer/Camera.h"
#include <glm/glm.hpp>
#include <glm/gtc/matrix_transform.hpp>

namespace GGEngine {

    // 2D Transform component
    struct TransformComponent
    {
        float Position[3] = { 0.0f, 0.0f, 0.0f };  // x, y, z
        float Rotation = 0.0f;                      // Degrees (2D rotation around Z)
        float Scale[2] = { 1.0f, 1.0f };           // width, height

        // Compute the full 4x4 transformation matrix from decomposed values (GLM version)
        glm::mat4 GetMatrix() const
        {
            return glm::translate(glm::mat4(1.0f), glm::vec3(Position[0], Position[1], Position[2]))
                 * glm::rotate(glm::mat4(1.0f), glm::radians(Rotation), glm::vec3(0.0f, 0.0f, 1.0f))
                 * glm::scale(glm::mat4(1.0f), glm::vec3(Scale[0], Scale[1], 1.0f));
        }

        // Native Mat4 version for camera view matrix computation
        Mat4 GetMat4() const
        {
            float rotationRadians = Rotation * 3.14159265359f / 180.0f;
            return Mat4::Translate(Position[0], Position[1], Position[2])
                 * Mat4::RotateZ(rotationRadians)
                 * Mat4::Scale(Scale[0], Scale[1], 1.0f);
        }
    };

}
