#pragma once

#include <glm/glm.hpp>
#include <glm/gtc/matrix_transform.hpp>

namespace GGEngine {

    // Stores previous frame's transform state for interpolation
    // Used with fixed timestep to render smoothly between physics updates
    struct InterpolationComponent
    {
        // Previous frame state
        float PrevPosition[3] = { 0.0f, 0.0f, 0.0f };
        float PrevRotation = 0.0f;
        float PrevScale[2] = { 1.0f, 1.0f };

        // Current (physics) state - copied from TransformComponent after each FixedUpdate
        float Position[3] = { 0.0f, 0.0f, 0.0f };
        float Rotation = 0.0f;
        float Scale[2] = { 1.0f, 1.0f };

        // Compute interpolated matrix for rendering
        // alpha: interpolation factor from Timestep::GetAlpha() [0, 1]
        glm::mat4 GetInterpolatedMatrix(float alpha) const
        {
            // Lerp position
            float x = PrevPosition[0] + (Position[0] - PrevPosition[0]) * alpha;
            float y = PrevPosition[1] + (Position[1] - PrevPosition[1]) * alpha;
            float z = PrevPosition[2] + (Position[2] - PrevPosition[2]) * alpha;

            // Lerp rotation (simple for 2D, would need slerp for 3D quaternions)
            float rot = PrevRotation + (Rotation - PrevRotation) * alpha;

            // Lerp scale
            float sx = PrevScale[0] + (Scale[0] - PrevScale[0]) * alpha;
            float sy = PrevScale[1] + (Scale[1] - PrevScale[1]) * alpha;

            return glm::translate(glm::mat4(1.0f), glm::vec3(x, y, z))
                 * glm::rotate(glm::mat4(1.0f), glm::radians(rot), glm::vec3(0.0f, 0.0f, 1.0f))
                 * glm::scale(glm::mat4(1.0f), glm::vec3(sx, sy, 1.0f));
        }

        // Call at start of FixedUpdate to save current state as previous
        void SavePreviousState()
        {
            PrevPosition[0] = Position[0];
            PrevPosition[1] = Position[1];
            PrevPosition[2] = Position[2];
            PrevRotation = Rotation;
            PrevScale[0] = Scale[0];
            PrevScale[1] = Scale[1];
        }

        // Copy current transform values (call after physics updates transform)
        void CopyFromTransform(const float pos[3], float rot, const float scale[2])
        {
            Position[0] = pos[0];
            Position[1] = pos[1];
            Position[2] = pos[2];
            Rotation = rot;
            Scale[0] = scale[0];
            Scale[1] = scale[1];
        }
    };

}
