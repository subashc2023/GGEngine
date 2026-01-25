#include <gtest/gtest.h>
#include "GGEngine/Core/Math.h"
#include "TestConfig.h"
#include <glm/glm.hpp>
#include <glm/gtc/matrix_transform.hpp>

using namespace GGEngine;
using namespace GGEngine::Testing;

class GLMMatrixTest : public ::testing::Test {};

// =============================================================================
// Identity Matrix Tests
// =============================================================================

TEST_F(GLMMatrixTest, Identity_ReturnsIdentityMatrix)
{
    glm::mat4 m(1.0f);

    EXPECT_FLOAT_NEAR_EPS(1.0f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[0][1]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[0][2]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[0][3]);

    EXPECT_FLOAT_NEAR_EPS(0.0f, m[1][0]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[1][1]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[1][2]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[1][3]);

    EXPECT_FLOAT_NEAR_EPS(0.0f, m[2][0]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[2][1]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[2][2]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[2][3]);

    EXPECT_FLOAT_NEAR_EPS(0.0f, m[3][0]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[3][1]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[3][2]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[3][3]);
}

// =============================================================================
// Translation Tests
// =============================================================================

TEST_F(GLMMatrixTest, Translate_CreatesCorrectMatrix)
{
    glm::mat4 m = glm::translate(glm::mat4(1.0f), glm::vec3(3.0f, 4.0f, 5.0f));

    // Column-major: translation in column 3
    EXPECT_FLOAT_NEAR_EPS(3.0f, m[3][0]);
    EXPECT_FLOAT_NEAR_EPS(4.0f, m[3][1]);
    EXPECT_FLOAT_NEAR_EPS(5.0f, m[3][2]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[3][3]);

    // Diagonal should still be 1
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[1][1]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[2][2]);
}

TEST_F(GLMMatrixTest, Translate_ZeroTranslation_IsIdentity)
{
    glm::mat4 m = glm::translate(glm::mat4(1.0f), glm::vec3(0.0f, 0.0f, 0.0f));
    glm::mat4 identity(1.0f);

    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++)
            EXPECT_FLOAT_NEAR_EPS(identity[col][row], m[col][row]);
}

TEST_F(GLMMatrixTest, Translate_NegativeValues)
{
    glm::mat4 m = glm::translate(glm::mat4(1.0f), glm::vec3(-10.0f, -20.0f, -30.0f));

    EXPECT_FLOAT_NEAR_EPS(-10.0f, m[3][0]);
    EXPECT_FLOAT_NEAR_EPS(-20.0f, m[3][1]);
    EXPECT_FLOAT_NEAR_EPS(-30.0f, m[3][2]);
}

// =============================================================================
// Scale Tests
// =============================================================================

TEST_F(GLMMatrixTest, Scale_CreatesCorrectMatrix)
{
    glm::mat4 m = glm::scale(glm::mat4(1.0f), glm::vec3(2.0f, 3.0f, 4.0f));

    EXPECT_FLOAT_NEAR_EPS(2.0f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(3.0f, m[1][1]);
    EXPECT_FLOAT_NEAR_EPS(4.0f, m[2][2]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[3][3]);
}

TEST_F(GLMMatrixTest, Scale_UniformScale)
{
    glm::mat4 m = glm::scale(glm::mat4(1.0f), glm::vec3(5.0f, 5.0f, 5.0f));

    EXPECT_FLOAT_NEAR_EPS(5.0f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(5.0f, m[1][1]);
    EXPECT_FLOAT_NEAR_EPS(5.0f, m[2][2]);
}

TEST_F(GLMMatrixTest, Scale_IdentityScale)
{
    glm::mat4 m = glm::scale(glm::mat4(1.0f), glm::vec3(1.0f, 1.0f, 1.0f));
    glm::mat4 identity(1.0f);

    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++)
            EXPECT_FLOAT_NEAR_EPS(identity[col][row], m[col][row]);
}

// =============================================================================
// Rotation Tests
// =============================================================================

TEST_F(GLMMatrixTest, RotateZ_ZeroAngle_IsIdentity)
{
    glm::mat4 m = glm::rotate(glm::mat4(1.0f), 0.0f, glm::vec3(0.0f, 0.0f, 1.0f));
    glm::mat4 identity(1.0f);

    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++)
            EXPECT_FLOAT_NEAR_EPS(identity[col][row], m[col][row]);
}

TEST_F(GLMMatrixTest, RotateZ_90Degrees)
{
    glm::mat4 m = glm::rotate(glm::mat4(1.0f), Math::HalfPi, glm::vec3(0.0f, 0.0f, 1.0f));

    // cos(90) = 0, sin(90) = 1
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[0][0]);   // cos
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[0][1]);   // sin
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m[1][0]);  // -sin
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[1][1]);   // cos
}

TEST_F(GLMMatrixTest, RotateZ_180Degrees)
{
    glm::mat4 m = glm::rotate(glm::mat4(1.0f), Math::Pi, glm::vec3(0.0f, 0.0f, 1.0f));

    // cos(180) = -1, sin(180) = 0
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[0][1]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[1][0]);
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m[1][1]);
}

TEST_F(GLMMatrixTest, RotateZ_360Degrees_IsIdentity)
{
    glm::mat4 m = glm::rotate(glm::mat4(1.0f), Math::TwoPi, glm::vec3(0.0f, 0.0f, 1.0f));
    glm::mat4 identity(1.0f);

    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++)
            EXPECT_FLOAT_NEAR_EPS(identity[col][row], m[col][row]);
}

TEST_F(GLMMatrixTest, RotateZ_45Degrees)
{
    glm::mat4 m = glm::rotate(glm::mat4(1.0f), Math::Pi / 4.0f, glm::vec3(0.0f, 0.0f, 1.0f));

    float cos45 = std::cos(Math::Pi / 4.0f);
    float sin45 = std::sin(Math::Pi / 4.0f);

    EXPECT_FLOAT_NEAR_EPS(cos45, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(sin45, m[0][1]);
    EXPECT_FLOAT_NEAR_EPS(-sin45, m[1][0]);
    EXPECT_FLOAT_NEAR_EPS(cos45, m[1][1]);
}

// =============================================================================
// Matrix Multiplication Tests
// =============================================================================

TEST_F(GLMMatrixTest, Multiply_IdentityByIdentity_IsIdentity)
{
    glm::mat4 a(1.0f);
    glm::mat4 b(1.0f);
    glm::mat4 result = a * b;

    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++)
            EXPECT_FLOAT_NEAR_EPS(a[col][row], result[col][row]);
}

TEST_F(GLMMatrixTest, Multiply_MatrixByIdentity_IsSameMatrix)
{
    glm::mat4 t = glm::translate(glm::mat4(1.0f), glm::vec3(1.0f, 2.0f, 3.0f));
    glm::mat4 identity(1.0f);
    glm::mat4 result = t * identity;

    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++)
            EXPECT_FLOAT_NEAR_EPS(t[col][row], result[col][row]);
}

TEST_F(GLMMatrixTest, Multiply_TwoTranslations)
{
    glm::mat4 t1 = glm::translate(glm::mat4(1.0f), glm::vec3(1.0f, 0.0f, 0.0f));
    glm::mat4 t2 = glm::translate(glm::mat4(1.0f), glm::vec3(0.0f, 2.0f, 0.0f));
    glm::mat4 result = t1 * t2;

    // Combined translation
    EXPECT_FLOAT_NEAR_EPS(1.0f, result[3][0]);
    EXPECT_FLOAT_NEAR_EPS(2.0f, result[3][1]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, result[3][2]);
}

TEST_F(GLMMatrixTest, Multiply_TwoScales)
{
    glm::mat4 s1 = glm::scale(glm::mat4(1.0f), glm::vec3(2.0f, 2.0f, 2.0f));
    glm::mat4 s2 = glm::scale(glm::mat4(1.0f), glm::vec3(3.0f, 3.0f, 3.0f));
    glm::mat4 result = s1 * s2;

    EXPECT_FLOAT_NEAR_EPS(6.0f, result[0][0]);
    EXPECT_FLOAT_NEAR_EPS(6.0f, result[1][1]);
    EXPECT_FLOAT_NEAR_EPS(6.0f, result[2][2]);
}

// =============================================================================
// Inverse Tests
// =============================================================================

TEST_F(GLMMatrixTest, Inverse_OfIdentity_IsIdentity)
{
    glm::mat4 identity(1.0f);
    glm::mat4 inv = glm::inverse(identity);

    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++)
            EXPECT_FLOAT_NEAR_EPS(identity[col][row], inv[col][row]);
}

TEST_F(GLMMatrixTest, Inverse_OfTranslation)
{
    glm::mat4 t = glm::translate(glm::mat4(1.0f), glm::vec3(5.0f, 10.0f, 15.0f));
    glm::mat4 inv = glm::inverse(t);

    // Inverse translation should negate the values
    EXPECT_FLOAT_NEAR_EPS(-5.0f, inv[3][0]);
    EXPECT_FLOAT_NEAR_EPS(-10.0f, inv[3][1]);
    EXPECT_FLOAT_NEAR_EPS(-15.0f, inv[3][2]);
}

TEST_F(GLMMatrixTest, Inverse_MultiplyByInverse_IsIdentity)
{
    glm::mat4 t = glm::translate(glm::mat4(1.0f), glm::vec3(3.0f, 4.0f, 5.0f));
    glm::mat4 inv = glm::inverse(t);
    glm::mat4 result = t * inv;
    glm::mat4 identity(1.0f);

    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++)
            EXPECT_FLOAT_NEAR_EPS(identity[col][row], result[col][row]);
}

TEST_F(GLMMatrixTest, Inverse_OfScale)
{
    glm::mat4 s = glm::scale(glm::mat4(1.0f), glm::vec3(2.0f, 4.0f, 8.0f));
    glm::mat4 inv = glm::inverse(s);

    EXPECT_FLOAT_NEAR_EPS(0.5f, inv[0][0]);
    EXPECT_FLOAT_NEAR_EPS(0.25f, inv[1][1]);
    EXPECT_FLOAT_NEAR_EPS(0.125f, inv[2][2]);
}

TEST_F(GLMMatrixTest, Inverse_OfRotation)
{
    glm::mat4 r = glm::rotate(glm::mat4(1.0f), Math::Pi / 3.0f, glm::vec3(0.0f, 0.0f, 1.0f));
    glm::mat4 inv = glm::inverse(r);
    glm::mat4 result = r * inv;
    glm::mat4 identity(1.0f);

    for (int col = 0; col < 4; col++)
        for (int row = 0; row < 4; row++)
            EXPECT_FLOAT_NEAR_EPS(identity[col][row], result[col][row]);
}

// =============================================================================
// Projection Tests
// =============================================================================

TEST_F(GLMMatrixTest, Orthographic_CreatesValidMatrix)
{
    glm::mat4 ortho = glm::ortho(-10.0f, 10.0f, -10.0f, 10.0f, -1.0f, 1.0f);

    // For symmetric ortho: [0][0] = 2/(right-left) = 2/20 = 0.1
    EXPECT_FLOAT_NEAR_EPS(0.1f, ortho[0][0]);
    EXPECT_FLOAT_NEAR_EPS(0.1f, ortho[1][1]);
}

TEST_F(GLMMatrixTest, Orthographic_AsymmetricBounds)
{
    glm::mat4 ortho = glm::ortho(0.0f, 100.0f, 0.0f, 50.0f, -1.0f, 1.0f);

    // [0][0] = 2/(100-0) = 0.02
    EXPECT_FLOAT_NEAR_EPS(0.02f, ortho[0][0]);
    // [1][1] = 2/(50-0) = 0.04
    EXPECT_FLOAT_NEAR_EPS(0.04f, ortho[1][1]);
}

TEST_F(GLMMatrixTest, Perspective_CreatesValidMatrix)
{
    float fov = glm::radians(45.0f);
    glm::mat4 persp = glm::perspective(fov, 16.0f / 9.0f, 0.1f, 100.0f);

    // Perspective matrix should have -1 at [2][3] (perspective divide)
    EXPECT_FLOAT_NEAR_EPS(-1.0f, persp[2][3]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, persp[3][3]);
}

TEST_F(GLMMatrixTest, Perspective_DifferentAspectRatios)
{
    float fov = glm::radians(45.0f);
    glm::mat4 wide = glm::perspective(fov, 2.0f, 0.1f, 100.0f);
    glm::mat4 tall = glm::perspective(fov, 0.5f, 0.1f, 100.0f);

    // Wide aspect ratio should have smaller x scale
    EXPECT_LT(wide[0][0], tall[0][0]);
    // Y scale should be the same (based on fov)
    EXPECT_FLOAT_NEAR_EPS(wide[1][1], tall[1][1]);
}

// =============================================================================
// LookAt Tests
// =============================================================================

TEST_F(GLMMatrixTest, LookAt_LookingDownNegativeZ)
{
    glm::mat4 view = glm::lookAt(
        glm::vec3(0.0f, 0.0f, 5.0f),   // eye
        glm::vec3(0.0f, 0.0f, 0.0f),   // target
        glm::vec3(0.0f, 1.0f, 0.0f)    // up
    );

    // The view matrix should have a non-zero Z translation
    EXPECT_NE(0.0f, view[3][2]);
}

TEST_F(GLMMatrixTest, LookAt_EyeAtOrigin)
{
    glm::mat4 view = glm::lookAt(
        glm::vec3(0.0f, 0.0f, 0.0f),   // eye at origin
        glm::vec3(0.0f, 0.0f, -1.0f),  // looking down -Z
        glm::vec3(0.0f, 1.0f, 0.0f)    // up
    );

    // Translation should be zero since eye is at origin
    EXPECT_FLOAT_NEAR_EPS(0.0f, view[3][0]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, view[3][1]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, view[3][2]);
}

TEST_F(GLMMatrixTest, LookAt_ProducesOrthonormalMatrix)
{
    glm::mat4 view = glm::lookAt(
        glm::vec3(5.0f, 5.0f, 5.0f),
        glm::vec3(0.0f, 0.0f, 0.0f),
        glm::vec3(0.0f, 1.0f, 0.0f)
    );

    // Check that the upper 3x3 columns are unit vectors
    float col0LenSq = view[0][0]*view[0][0] + view[0][1]*view[0][1] + view[0][2]*view[0][2];
    float col1LenSq = view[1][0]*view[1][0] + view[1][1]*view[1][1] + view[1][2]*view[1][2];
    float col2LenSq = view[2][0]*view[2][0] + view[2][1]*view[2][1] + view[2][2]*view[2][2];

    EXPECT_FLOAT_NEAR_EPS(1.0f, col0LenSq);
    EXPECT_FLOAT_NEAR_EPS(1.0f, col1LenSq);
    EXPECT_FLOAT_NEAR_EPS(1.0f, col2LenSq);
}
