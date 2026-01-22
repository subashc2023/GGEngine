#pragma once

namespace GGEngine {

    namespace Math {

        constexpr float Pi = 3.14159265358979323846f;
        constexpr float TwoPi = Pi * 2.0f;
        constexpr float HalfPi = Pi * 0.5f;

        constexpr float DegToRad = Pi / 180.0f;
        constexpr float RadToDeg = 180.0f / Pi;

        constexpr float ToRadians(float degrees) { return degrees * DegToRad; }
        constexpr float ToDegrees(float radians) { return radians * RadToDeg; }

    }

}
