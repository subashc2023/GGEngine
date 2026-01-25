#pragma once

#include <gtest/gtest.h>
#include <glm/glm.hpp>
#include <cmath>

namespace GGEngine::Testing {

    // Float comparison epsilon for matrix/transform tests
    constexpr float EPSILON = 1e-5f;

    inline bool FloatEquals(float a, float b, float epsilon = EPSILON)
    {
        return std::abs(a - b) < epsilon;
    }

    // Matrix comparison helper for glm::mat4
    inline void ExpectMat4Near(const glm::mat4& expected, const glm::mat4& actual, float epsilon = EPSILON)
    {
        for (int col = 0; col < 4; col++)
        {
            for (int row = 0; row < 4; row++)
            {
                EXPECT_NEAR(expected[col][row], actual[col][row], epsilon)
                    << "Mismatch at [" << col << "][" << row << "]";
            }
        }
    }

}

// Convenience macro for float comparison with default epsilon
#define EXPECT_FLOAT_NEAR_EPS(expected, actual) \
    EXPECT_NEAR(expected, actual, GGEngine::Testing::EPSILON)
