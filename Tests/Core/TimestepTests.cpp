#include <gtest/gtest.h>
#include "GGEngine/Core/Timestep.h"
#include "TestConfig.h"

using namespace GGEngine;
using namespace GGEngine::Testing;

class TimestepTest : public ::testing::Test {};

// =============================================================================
// Construction Tests
// =============================================================================

TEST_F(TimestepTest, DefaultConstruction)
{
    Timestep ts;
    EXPECT_FLOAT_NEAR_EPS(0.0f, ts.GetSeconds());
    EXPECT_FLOAT_NEAR_EPS(1.0f, ts.GetAlpha());
}

TEST_F(TimestepTest, ConstructionWithTime)
{
    Timestep ts(0.016f);  // ~60 FPS
    EXPECT_FLOAT_NEAR_EPS(0.016f, ts.GetSeconds());
    EXPECT_FLOAT_NEAR_EPS(1.0f, ts.GetAlpha());  // Default alpha is 1.0
}

TEST_F(TimestepTest, ConstructionWithTimeAndAlpha)
{
    Timestep ts(0.016f, 0.5f);
    EXPECT_FLOAT_NEAR_EPS(0.016f, ts.GetSeconds());
    EXPECT_FLOAT_NEAR_EPS(0.5f, ts.GetAlpha());
}

// =============================================================================
// Conversion Tests
// =============================================================================

TEST_F(TimestepTest, GetMilliseconds_Conversion)
{
    Timestep ts(1.0f);  // 1 second
    EXPECT_FLOAT_NEAR_EPS(1000.0f, ts.GetMilliseconds());
}

TEST_F(TimestepTest, GetMilliseconds_60FPS)
{
    Timestep ts(1.0f / 60.0f);
    EXPECT_NEAR(16.666f, ts.GetMilliseconds(), 0.001f);
}

TEST_F(TimestepTest, GetMilliseconds_30FPS)
{
    Timestep ts(1.0f / 30.0f);
    EXPECT_NEAR(33.333f, ts.GetMilliseconds(), 0.001f);
}

// =============================================================================
// Implicit Conversion Tests
// =============================================================================

TEST_F(TimestepTest, ImplicitConversionToFloat)
{
    Timestep ts(0.033f);  // ~30 FPS
    float seconds = ts;   // Implicit conversion
    EXPECT_FLOAT_NEAR_EPS(0.033f, seconds);
}

TEST_F(TimestepTest, ImplicitConversionInExpression)
{
    Timestep ts(0.5f);
    float result = ts * 2.0f;  // Uses implicit conversion
    EXPECT_FLOAT_NEAR_EPS(1.0f, result);
}

// =============================================================================
// Alpha Interpolation Tests
// =============================================================================

TEST_F(TimestepTest, Alpha_ZeroValue)
{
    Timestep ts(0.016f, 0.0f);
    EXPECT_FLOAT_NEAR_EPS(0.0f, ts.GetAlpha());
}

TEST_F(TimestepTest, Alpha_FullValue)
{
    Timestep ts(0.016f, 1.0f);
    EXPECT_FLOAT_NEAR_EPS(1.0f, ts.GetAlpha());
}

TEST_F(TimestepTest, Alpha_MidValue)
{
    Timestep ts(0.016f, 0.5f);
    EXPECT_FLOAT_NEAR_EPS(0.5f, ts.GetAlpha());
}

TEST_F(TimestepTest, Alpha_ForInterpolation)
{
    Timestep ts(0.016f, 0.75f);

    // Simulate interpolation: lerp(prev, curr, alpha)
    float prev = 0.0f;
    float curr = 100.0f;
    float interpolated = prev + (curr - prev) * ts.GetAlpha();

    EXPECT_FLOAT_NEAR_EPS(75.0f, interpolated);
}

TEST_F(TimestepTest, Alpha_InterpolationAtZero)
{
    Timestep ts(0.016f, 0.0f);

    float prev = 50.0f;
    float curr = 100.0f;
    float interpolated = prev + (curr - prev) * ts.GetAlpha();

    EXPECT_FLOAT_NEAR_EPS(50.0f, interpolated);  // Should equal prev
}

TEST_F(TimestepTest, Alpha_InterpolationAtOne)
{
    Timestep ts(0.016f, 1.0f);

    float prev = 50.0f;
    float curr = 100.0f;
    float interpolated = prev + (curr - prev) * ts.GetAlpha();

    EXPECT_FLOAT_NEAR_EPS(100.0f, interpolated);  // Should equal curr
}
