#include <gtest/gtest.h>
#include "GGEngine/Core/Math.h"
#include "TestConfig.h"

using namespace GGEngine;
using namespace GGEngine::Testing;

// =============================================================================
// Math Constants Tests
// =============================================================================

class MathConstantsTest : public ::testing::Test {};

TEST_F(MathConstantsTest, PiConstant)
{
    EXPECT_FLOAT_NEAR_EPS(3.14159265358979323846f, Math::Pi);
}

TEST_F(MathConstantsTest, TwoPiConstant)
{
    EXPECT_FLOAT_NEAR_EPS(Math::Pi * 2.0f, Math::TwoPi);
}

TEST_F(MathConstantsTest, HalfPiConstant)
{
    EXPECT_FLOAT_NEAR_EPS(Math::Pi * 0.5f, Math::HalfPi);
}

TEST_F(MathConstantsTest, DegToRadConstant)
{
    EXPECT_FLOAT_NEAR_EPS(Math::Pi / 180.0f, Math::DegToRad);
}

TEST_F(MathConstantsTest, RadToDegConstant)
{
    EXPECT_FLOAT_NEAR_EPS(180.0f / Math::Pi, Math::RadToDeg);
}

// =============================================================================
// Degree/Radian Conversion Tests
// =============================================================================

class DegreeRadianConversionTest : public ::testing::TestWithParam<std::pair<float, float>> {};

TEST_P(DegreeRadianConversionTest, ToRadians)
{
    auto [degrees, expectedRadians] = GetParam();
    EXPECT_FLOAT_NEAR_EPS(expectedRadians, Math::ToRadians(degrees));
}

TEST_P(DegreeRadianConversionTest, ToDegrees)
{
    auto [expectedDegrees, radians] = GetParam();
    EXPECT_FLOAT_NEAR_EPS(expectedDegrees, Math::ToDegrees(radians));
}

INSTANTIATE_TEST_SUITE_P(
    CommonAngles,
    DegreeRadianConversionTest,
    ::testing::Values(
        std::make_pair(0.0f, 0.0f),
        std::make_pair(90.0f, Math::HalfPi),
        std::make_pair(180.0f, Math::Pi),
        std::make_pair(360.0f, Math::TwoPi),
        std::make_pair(-90.0f, -Math::HalfPi),
        std::make_pair(45.0f, Math::Pi / 4.0f)
    )
);

// =============================================================================
// Constexpr Tests (Compile-Time Verification)
// =============================================================================

TEST(MathConstexprTest, ToRadiansIsConstexpr)
{
    constexpr float rad = Math::ToRadians(180.0f);
    EXPECT_FLOAT_NEAR_EPS(Math::Pi, rad);
}

TEST(MathConstexprTest, ToDegreesIsConstexpr)
{
    constexpr float deg = Math::ToDegrees(Math::Pi);
    EXPECT_FLOAT_NEAR_EPS(180.0f, deg);
}

TEST(MathConstexprTest, RoundTripConversion)
{
    constexpr float original = 45.0f;
    constexpr float radians = Math::ToRadians(original);
    constexpr float backToDegrees = Math::ToDegrees(radians);
    EXPECT_FLOAT_NEAR_EPS(original, backToDegrees);
}
