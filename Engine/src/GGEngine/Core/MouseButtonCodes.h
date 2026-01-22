#pragma once

#include <cstdint>

namespace GGEngine {

    // From glfw3.h - using GLFW mouse button codes as the standard
    enum class MouseCode : uint8_t
    {
        Button0 = 0,
        Button1 = 1,
        Button2 = 2,
        Button3 = 3,
        Button4 = 4,
        Button5 = 5,
        Button6 = 6,
        Button7 = 7,

        Left = Button0,
        Right = Button1,
        Middle = Button2
    };

    inline uint8_t ToInt(MouseCode button) { return static_cast<uint8_t>(button); }

}
