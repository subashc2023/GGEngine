#pragma once

#include <gtest/gtest.h>
#include <cmath>

namespace GGEngine::Testing {

    // Float comparison epsilon for matrix/transform tests
    constexpr float EPSILON = 1e-5f;

    inline bool FloatEquals(float a, float b, float epsilon = EPSILON)
    {
        return std::abs(a - b) < epsilon;
    }

    // Matrix comparison helper - compares all 16 elements
    inline void ExpectMat4Near(const float* expected, const float* actual, float epsilon = EPSILON)
    {
        for (int i = 0; i < 16; i++)
        {
            EXPECT_NEAR(expected[i], actual[i], epsilon)
                << "Mismatch at index " << i;
        }
    }

}

// Convenience macro for float comparison with default epsilon
#define EXPECT_FLOAT_NEAR_EPS(expected, actual) \
    EXPECT_NEAR(expected, actual, GGEngine::Testing::EPSILON)
