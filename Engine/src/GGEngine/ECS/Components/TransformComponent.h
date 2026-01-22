#pragma once

namespace GGEngine {

    // 2D Transform component
    struct TransformComponent
    {
        float Position[3] = { 0.0f, 0.0f, 0.0f };  // x, y, z
        float Rotation = 0.0f;                      // Degrees (2D rotation around Z)
        float Scale[2] = { 1.0f, 1.0f };           // width, height
    };

}
