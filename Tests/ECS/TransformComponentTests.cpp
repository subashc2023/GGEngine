#include <gtest/gtest.h>
#include "GGEngine/ECS/Components/TransformComponent.h"
#include "GGEngine/Core/Math.h"
#include "TestConfig.h"
#include <glm/glm.hpp>
#include <glm/gtc/matrix_transform.hpp>

using namespace GGEngine;
using namespace GGEngine::Testing;

class TransformComponentTest : public ::testing::Test {
protected:
    TransformComponent transform;

    void SetUp() override
    {
        transform = TransformComponent{};  // Reset to defaults
    }
};

// =============================================================================
// Default Values Tests
// =============================================================================

TEST_F(TransformComponentTest, DefaultValues_Position)
{
    EXPECT_FLOAT_NEAR_EPS(0.0f, transform.Position[0]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, transform.Position[1]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, transform.Position[2]);
}

TEST_F(TransformComponentTest, DefaultValues_Rotation)
{
    EXPECT_FLOAT_NEAR_EPS(0.0f, transform.Rotation);
}

TEST_F(TransformComponentTest, DefaultValues_Scale)
{
    EXPECT_FLOAT_NEAR_EPS(1.0f, transform.Scale[0]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, transform.Scale[1]);
}

// =============================================================================
// GetMatrix Tests
// =============================================================================

TEST_F(TransformComponentTest, GetMatrix_DefaultIsIdentity)
{
    glm::mat4 m = transform.GetMatrix();
    glm::mat4 identity(1.0f);

    for (int col = 0; col < 4; col++)
    {
        for (int row = 0; row < 4; row++)
        {
            EXPECT_FLOAT_NEAR_EPS(identity[col][row], m[col][row]);
        }
    }
}

TEST_F(TransformComponentTest, GetMatrix_WithTranslationOnly)
{
    transform.Position[0] = 5.0f;
    transform.Position[1] = 10.0f;
    transform.Position[2] = 15.0f;

    glm::mat4 m = transform.GetMatrix();

    EXPECT_FLOAT_NEAR_EPS(5.0f, m[3][0]);
    EXPECT_FLOAT_NEAR_EPS(10.0f, m[3][1]);
    EXPECT_FLOAT_NEAR_EPS(15.0f, m[3][2]);
}

TEST_F(TransformComponentTest, GetMatrix_WithScaleOnly)
{
    transform.Scale[0] = 2.0f;
    transform.Scale[1] = 3.0f;

    glm::mat4 m = transform.GetMatrix();

    // Scale affects diagonal (with rotation at 0)
    EXPECT_FLOAT_NEAR_EPS(2.0f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(3.0f, m[1][1]);
}

TEST_F(TransformComponentTest, GetMatrix_WithRotationOnly)
{
    transform.Rotation = 90.0f;  // 90 degrees

    glm::mat4 m = transform.GetMatrix();

    // 90 degree Z rotation: cos(90)=0, sin(90)=1
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[0][1]);
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m[1][0]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[1][1]);
}

TEST_F(TransformComponentTest, GetMatrix_CombinedTRS_PreservesTranslation)
{
    transform.Position[0] = 100.0f;
    transform.Position[1] = 200.0f;
    transform.Position[2] = 0.0f;
    transform.Rotation = 45.0f;
    transform.Scale[0] = 2.0f;
    transform.Scale[1] = 2.0f;

    glm::mat4 m = transform.GetMatrix();

    // Translation should be preserved in the final matrix
    EXPECT_FLOAT_NEAR_EPS(100.0f, m[3][0]);
    EXPECT_FLOAT_NEAR_EPS(200.0f, m[3][1]);
}

TEST_F(TransformComponentTest, GetMatrix_CombinedTRS_AffectsUpperLeft)
{
    transform.Position[0] = 10.0f;
    transform.Position[1] = 20.0f;
    transform.Rotation = 45.0f;
    transform.Scale[0] = 2.0f;
    transform.Scale[1] = 3.0f;

    glm::mat4 m = transform.GetMatrix();

    // Upper-left 2x2 should be affected by rotation and scale
    // For 45 degrees: cos = sin = ~0.707
    float cos45 = std::cos(glm::radians(45.0f));
    float sin45 = std::sin(glm::radians(45.0f));

    // With scale: [cos*sx, sin*sx; -sin*sy, cos*sy]
    EXPECT_FLOAT_NEAR_EPS(cos45 * 2.0f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(sin45 * 2.0f, m[0][1]);
    EXPECT_FLOAT_NEAR_EPS(-sin45 * 3.0f, m[1][0]);
    EXPECT_FLOAT_NEAR_EPS(cos45 * 3.0f, m[1][1]);
}

TEST_F(TransformComponentTest, GetMatrix_NegativeRotation)
{
    transform.Rotation = -90.0f;

    glm::mat4 m = transform.GetMatrix();

    // -90 degrees: cos(-90)=0, sin(-90)=-1
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m[0][1]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m[1][0]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m[1][1]);
}

TEST_F(TransformComponentTest, GetMatrix_NonUniformScale)
{
    transform.Scale[0] = 0.5f;
    transform.Scale[1] = 4.0f;

    glm::mat4 m = transform.GetMatrix();

    EXPECT_FLOAT_NEAR_EPS(0.5f, m[0][0]);
    EXPECT_FLOAT_NEAR_EPS(4.0f, m[1][1]);
}

// =============================================================================
// GLM Verification Tests
// =============================================================================

TEST_F(TransformComponentTest, GetMatrix_MatchesManualGLMConstruction)
{
    transform.Position[0] = 10.0f;
    transform.Position[1] = 20.0f;
    transform.Position[2] = 5.0f;
    transform.Rotation = 30.0f;
    transform.Scale[0] = 1.5f;
    transform.Scale[1] = 2.5f;

    glm::mat4 componentMat = transform.GetMatrix();

    // Manually construct the same transform using GLM
    glm::mat4 expected = glm::translate(glm::mat4(1.0f), glm::vec3(10.0f, 20.0f, 5.0f))
                       * glm::rotate(glm::mat4(1.0f), glm::radians(30.0f), glm::vec3(0.0f, 0.0f, 1.0f))
                       * glm::scale(glm::mat4(1.0f), glm::vec3(1.5f, 2.5f, 1.0f));

    for (int col = 0; col < 4; col++)
    {
        for (int row = 0; row < 4; row++)
        {
            EXPECT_FLOAT_NEAR_EPS(expected[col][row], componentMat[col][row])
                << "Mismatch at [" << col << "][" << row << "]";
        }
    }
}

TEST_F(TransformComponentTest, GetMatrix_EdgeCases)
{
    // Test with extreme values
    transform.Position[0] = -1000.0f;
    transform.Position[1] = 1000.0f;
    transform.Position[2] = 0.001f;
    transform.Rotation = 359.0f;
    transform.Scale[0] = 0.01f;
    transform.Scale[1] = 100.0f;

    glm::mat4 componentMat = transform.GetMatrix();

    // Manually construct the same transform
    glm::mat4 expected = glm::translate(glm::mat4(1.0f), glm::vec3(-1000.0f, 1000.0f, 0.001f))
                       * glm::rotate(glm::mat4(1.0f), glm::radians(359.0f), glm::vec3(0.0f, 0.0f, 1.0f))
                       * glm::scale(glm::mat4(1.0f), glm::vec3(0.01f, 100.0f, 1.0f));

    for (int col = 0; col < 4; col++)
    {
        for (int row = 0; row < 4; row++)
        {
            EXPECT_NEAR(expected[col][row], componentMat[col][row], 1e-4f)
                << "Mismatch at [" << col << "][" << row << "]";
        }
    }
}
