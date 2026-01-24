#include <gtest/gtest.h>
#include "GGEngine/Renderer/Camera.h"
#include "GGEngine/Core/Math.h"
#include "TestConfig.h"

using namespace GGEngine;
using namespace GGEngine::Testing;

class Mat4Test : public ::testing::Test {};

// =============================================================================
// Identity Matrix Tests
// =============================================================================

TEST_F(Mat4Test, Identity_ReturnsIdentityMatrix)
{
    Mat4 m = Mat4::Identity();

    float expected[16] = {
        1, 0, 0, 0,
        0, 1, 0, 0,
        0, 0, 1, 0,
        0, 0, 0, 1
    };

    ExpectMat4Near(expected, m.data);
}

TEST_F(Mat4Test, DefaultConstructor_CreatesIdentity)
{
    Mat4 m{};
    Mat4 identity = Mat4::Identity();

    ExpectMat4Near(identity.data, m.data);
}

// =============================================================================
// Translation Tests
// =============================================================================

TEST_F(Mat4Test, Translate_CreatesCorrectMatrix)
{
    Mat4 m = Mat4::Translate(3.0f, 4.0f, 5.0f);

    // Column-major: translation in indices 12, 13, 14
    EXPECT_FLOAT_NEAR_EPS(3.0f, m.data[12]);
    EXPECT_FLOAT_NEAR_EPS(4.0f, m.data[13]);
    EXPECT_FLOAT_NEAR_EPS(5.0f, m.data[14]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m.data[15]);

    // Diagonal should still be 1
    EXPECT_FLOAT_NEAR_EPS(1.0f, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m.data[5]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m.data[10]);
}

TEST_F(Mat4Test, Translate_ZeroTranslation_IsIdentity)
{
    Mat4 m = Mat4::Translate(0.0f, 0.0f, 0.0f);
    Mat4 identity = Mat4::Identity();

    ExpectMat4Near(identity.data, m.data);
}

TEST_F(Mat4Test, Translate_NegativeValues)
{
    Mat4 m = Mat4::Translate(-10.0f, -20.0f, -30.0f);

    EXPECT_FLOAT_NEAR_EPS(-10.0f, m.data[12]);
    EXPECT_FLOAT_NEAR_EPS(-20.0f, m.data[13]);
    EXPECT_FLOAT_NEAR_EPS(-30.0f, m.data[14]);
}

// =============================================================================
// Scale Tests
// =============================================================================

TEST_F(Mat4Test, Scale_CreatesCorrectMatrix)
{
    Mat4 m = Mat4::Scale(2.0f, 3.0f, 4.0f);

    EXPECT_FLOAT_NEAR_EPS(2.0f, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(3.0f, m.data[5]);
    EXPECT_FLOAT_NEAR_EPS(4.0f, m.data[10]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m.data[15]);
}

TEST_F(Mat4Test, Scale_UniformScale)
{
    Mat4 m = Mat4::Scale(5.0f, 5.0f, 5.0f);

    EXPECT_FLOAT_NEAR_EPS(5.0f, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(5.0f, m.data[5]);
    EXPECT_FLOAT_NEAR_EPS(5.0f, m.data[10]);
}

TEST_F(Mat4Test, Scale_IdentityScale)
{
    Mat4 m = Mat4::Scale(1.0f, 1.0f, 1.0f);
    Mat4 identity = Mat4::Identity();

    ExpectMat4Near(identity.data, m.data);
}

// =============================================================================
// Rotation Tests
// =============================================================================

TEST_F(Mat4Test, RotateZ_ZeroAngle_IsIdentity)
{
    Mat4 m = Mat4::RotateZ(0.0f);
    Mat4 identity = Mat4::Identity();

    ExpectMat4Near(identity.data, m.data);
}

TEST_F(Mat4Test, RotateZ_90Degrees)
{
    Mat4 m = Mat4::RotateZ(Math::HalfPi);  // 90 degrees

    // cos(90) = 0, sin(90) = 1
    EXPECT_FLOAT_NEAR_EPS(0.0f, m.data[0]);   // cos
    EXPECT_FLOAT_NEAR_EPS(1.0f, m.data[1]);   // sin
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m.data[4]);  // -sin
    EXPECT_FLOAT_NEAR_EPS(0.0f, m.data[5]);   // cos
}

TEST_F(Mat4Test, RotateZ_180Degrees)
{
    Mat4 m = Mat4::RotateZ(Math::Pi);  // 180 degrees

    // cos(180) = -1, sin(180) = 0
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m.data[1]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m.data[4]);
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m.data[5]);
}

TEST_F(Mat4Test, RotateZ_360Degrees_IsIdentity)
{
    Mat4 m = Mat4::RotateZ(Math::TwoPi);
    Mat4 identity = Mat4::Identity();

    ExpectMat4Near(identity.data, m.data);
}

TEST_F(Mat4Test, RotateZ_45Degrees)
{
    Mat4 m = Mat4::RotateZ(Math::Pi / 4.0f);

    float cos45 = std::cos(Math::Pi / 4.0f);
    float sin45 = std::sin(Math::Pi / 4.0f);

    EXPECT_FLOAT_NEAR_EPS(cos45, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(sin45, m.data[1]);
    EXPECT_FLOAT_NEAR_EPS(-sin45, m.data[4]);
    EXPECT_FLOAT_NEAR_EPS(cos45, m.data[5]);
}

// =============================================================================
// Matrix Multiplication Tests
// =============================================================================

TEST_F(Mat4Test, Multiply_IdentityByIdentity_IsIdentity)
{
    Mat4 a = Mat4::Identity();
    Mat4 b = Mat4::Identity();
    Mat4 result = a * b;

    ExpectMat4Near(a.data, result.data);
}

TEST_F(Mat4Test, Multiply_MatrixByIdentity_IsSameMatrix)
{
    Mat4 t = Mat4::Translate(1.0f, 2.0f, 3.0f);
    Mat4 identity = Mat4::Identity();
    Mat4 result = t * identity;

    ExpectMat4Near(t.data, result.data);
}

TEST_F(Mat4Test, Multiply_IdentityByMatrix_IsSameMatrix)
{
    Mat4 t = Mat4::Translate(1.0f, 2.0f, 3.0f);
    Mat4 identity = Mat4::Identity();
    Mat4 result = identity * t;

    ExpectMat4Near(t.data, result.data);
}

TEST_F(Mat4Test, Multiply_TwoTranslations)
{
    Mat4 t1 = Mat4::Translate(1.0f, 0.0f, 0.0f);
    Mat4 t2 = Mat4::Translate(0.0f, 2.0f, 0.0f);
    Mat4 result = t1 * t2;

    // Combined translation
    EXPECT_FLOAT_NEAR_EPS(1.0f, result.data[12]);
    EXPECT_FLOAT_NEAR_EPS(2.0f, result.data[13]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, result.data[14]);
}

TEST_F(Mat4Test, Multiply_TwoScales)
{
    Mat4 s1 = Mat4::Scale(2.0f, 2.0f, 2.0f);
    Mat4 s2 = Mat4::Scale(3.0f, 3.0f, 3.0f);
    Mat4 result = s1 * s2;

    EXPECT_FLOAT_NEAR_EPS(6.0f, result.data[0]);
    EXPECT_FLOAT_NEAR_EPS(6.0f, result.data[5]);
    EXPECT_FLOAT_NEAR_EPS(6.0f, result.data[10]);
}

// =============================================================================
// Inverse Tests
// =============================================================================

TEST_F(Mat4Test, Inverse_OfIdentity_IsIdentity)
{
    Mat4 identity = Mat4::Identity();
    Mat4 inv = Mat4::Inverse(identity);

    ExpectMat4Near(identity.data, inv.data);
}

TEST_F(Mat4Test, Inverse_OfTranslation)
{
    Mat4 t = Mat4::Translate(5.0f, 10.0f, 15.0f);
    Mat4 inv = Mat4::Inverse(t);

    // Inverse translation should negate the values
    EXPECT_FLOAT_NEAR_EPS(-5.0f, inv.data[12]);
    EXPECT_FLOAT_NEAR_EPS(-10.0f, inv.data[13]);
    EXPECT_FLOAT_NEAR_EPS(-15.0f, inv.data[14]);
}

TEST_F(Mat4Test, Inverse_MultiplyByInverse_IsIdentity)
{
    Mat4 t = Mat4::Translate(3.0f, 4.0f, 5.0f);
    Mat4 inv = Mat4::Inverse(t);
    Mat4 result = t * inv;
    Mat4 identity = Mat4::Identity();

    ExpectMat4Near(identity.data, result.data);
}

TEST_F(Mat4Test, Inverse_OfScale)
{
    Mat4 s = Mat4::Scale(2.0f, 4.0f, 8.0f);
    Mat4 inv = Mat4::Inverse(s);

    EXPECT_FLOAT_NEAR_EPS(0.5f, inv.data[0]);
    EXPECT_FLOAT_NEAR_EPS(0.25f, inv.data[5]);
    EXPECT_FLOAT_NEAR_EPS(0.125f, inv.data[10]);
}

TEST_F(Mat4Test, Inverse_ScaleMultiplyByInverse_IsIdentity)
{
    Mat4 s = Mat4::Scale(2.0f, 3.0f, 4.0f);
    Mat4 inv = Mat4::Inverse(s);
    Mat4 result = s * inv;
    Mat4 identity = Mat4::Identity();

    ExpectMat4Near(identity.data, result.data);
}

TEST_F(Mat4Test, Inverse_OfRotation)
{
    Mat4 r = Mat4::RotateZ(Math::Pi / 3.0f);  // 60 degrees
    Mat4 inv = Mat4::Inverse(r);
    Mat4 result = r * inv;
    Mat4 identity = Mat4::Identity();

    ExpectMat4Near(identity.data, result.data);
}

// =============================================================================
// Projection Tests
// =============================================================================

TEST_F(Mat4Test, Orthographic_CreatesValidMatrix)
{
    Mat4 ortho = Mat4::Orthographic(-10.0f, 10.0f, -10.0f, 10.0f, -1.0f, 1.0f);

    // For symmetric ortho: data[0] = 2/(right-left) = 2/20 = 0.1
    EXPECT_FLOAT_NEAR_EPS(0.1f, ortho.data[0]);
    EXPECT_FLOAT_NEAR_EPS(0.1f, ortho.data[5]);
}

TEST_F(Mat4Test, Orthographic_AsymmetricBounds)
{
    Mat4 ortho = Mat4::Orthographic(0.0f, 100.0f, 0.0f, 50.0f, -1.0f, 1.0f);

    // data[0] = 2/(100-0) = 0.02
    EXPECT_FLOAT_NEAR_EPS(0.02f, ortho.data[0]);
    // data[5] = 2/(50-0) = 0.04
    EXPECT_FLOAT_NEAR_EPS(0.04f, ortho.data[5]);
}

TEST_F(Mat4Test, Perspective_CreatesValidMatrix)
{
    float fov = Math::ToRadians(45.0f);
    Mat4 persp = Mat4::Perspective(fov, 16.0f / 9.0f, 0.1f, 100.0f);

    // Perspective matrix should have -1 at data[11] (perspective divide)
    EXPECT_FLOAT_NEAR_EPS(-1.0f, persp.data[11]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, persp.data[15]);
}

TEST_F(Mat4Test, Perspective_DifferentAspectRatios)
{
    float fov = Math::ToRadians(45.0f);
    Mat4 wide = Mat4::Perspective(fov, 2.0f, 0.1f, 100.0f);
    Mat4 tall = Mat4::Perspective(fov, 0.5f, 0.1f, 100.0f);

    // Wide aspect ratio should have smaller x scale
    EXPECT_LT(wide.data[0], tall.data[0]);
    // Y scale should be the same (based on fov)
    EXPECT_FLOAT_NEAR_EPS(wide.data[5], tall.data[5]);
}

// =============================================================================
// LookAt Tests
// =============================================================================

TEST_F(Mat4Test, LookAt_LookingDownNegativeZ)
{
    Mat4 view = Mat4::LookAt(
        0.0f, 0.0f, 5.0f,   // eye
        0.0f, 0.0f, 0.0f,   // target
        0.0f, 1.0f, 0.0f    // up
    );

    // The view matrix should have a non-zero Z translation
    EXPECT_NE(0.0f, view.data[14]);
}

TEST_F(Mat4Test, LookAt_EyeAtOrigin)
{
    Mat4 view = Mat4::LookAt(
        0.0f, 0.0f, 0.0f,   // eye at origin
        0.0f, 0.0f, -1.0f,  // looking down -Z
        0.0f, 1.0f, 0.0f    // up
    );

    // Translation should be zero since eye is at origin
    EXPECT_FLOAT_NEAR_EPS(0.0f, view.data[12]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, view.data[13]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, view.data[14]);
}

TEST_F(Mat4Test, LookAt_ProducesOrthonormalMatrix)
{
    Mat4 view = Mat4::LookAt(
        5.0f, 5.0f, 5.0f,
        0.0f, 0.0f, 0.0f,
        0.0f, 1.0f, 0.0f
    );

    // Check that the upper 3x3 is orthonormal by verifying rows are unit vectors
    float row0LenSq = view.data[0]*view.data[0] + view.data[4]*view.data[4] + view.data[8]*view.data[8];
    float row1LenSq = view.data[1]*view.data[1] + view.data[5]*view.data[5] + view.data[9]*view.data[9];
    float row2LenSq = view.data[2]*view.data[2] + view.data[6]*view.data[6] + view.data[10]*view.data[10];

    EXPECT_FLOAT_NEAR_EPS(1.0f, row0LenSq);
    EXPECT_FLOAT_NEAR_EPS(1.0f, row1LenSq);
    EXPECT_FLOAT_NEAR_EPS(1.0f, row2LenSq);
}
