#include <gtest/gtest.h>
#include "GGEngine/ECS/Components/TransformComponent.h"
#include "GGEngine/Core/Math.h"
#include "TestConfig.h"

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
// GetMat4 Tests
// =============================================================================

TEST_F(TransformComponentTest, GetMat4_DefaultIsIdentity)
{
    Mat4 m = transform.GetMat4();
    Mat4 identity = Mat4::Identity();

    ExpectMat4Near(identity.data, m.data);
}

TEST_F(TransformComponentTest, GetMat4_WithTranslationOnly)
{
    transform.Position[0] = 5.0f;
    transform.Position[1] = 10.0f;
    transform.Position[2] = 15.0f;

    Mat4 m = transform.GetMat4();

    EXPECT_FLOAT_NEAR_EPS(5.0f, m.data[12]);
    EXPECT_FLOAT_NEAR_EPS(10.0f, m.data[13]);
    EXPECT_FLOAT_NEAR_EPS(15.0f, m.data[14]);
}

TEST_F(TransformComponentTest, GetMat4_WithScaleOnly)
{
    transform.Scale[0] = 2.0f;
    transform.Scale[1] = 3.0f;

    Mat4 m = transform.GetMat4();

    // Scale affects diagonal (with rotation at 0)
    EXPECT_FLOAT_NEAR_EPS(2.0f, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(3.0f, m.data[5]);
}

TEST_F(TransformComponentTest, GetMat4_WithRotationOnly)
{
    transform.Rotation = 90.0f;  // 90 degrees

    Mat4 m = transform.GetMat4();

    // 90 degree Z rotation: cos(90)=0, sin(90)=1
    EXPECT_FLOAT_NEAR_EPS(0.0f, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m.data[1]);
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m.data[4]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m.data[5]);
}

TEST_F(TransformComponentTest, GetMat4_CombinedTRS_PreservesTranslation)
{
    transform.Position[0] = 100.0f;
    transform.Position[1] = 200.0f;
    transform.Position[2] = 0.0f;
    transform.Rotation = 45.0f;
    transform.Scale[0] = 2.0f;
    transform.Scale[1] = 2.0f;

    Mat4 m = transform.GetMat4();

    // Translation should be preserved in the final matrix
    EXPECT_FLOAT_NEAR_EPS(100.0f, m.data[12]);
    EXPECT_FLOAT_NEAR_EPS(200.0f, m.data[13]);
}

TEST_F(TransformComponentTest, GetMat4_CombinedTRS_AffectsUpperLeft)
{
    transform.Position[0] = 10.0f;
    transform.Position[1] = 20.0f;
    transform.Rotation = 45.0f;
    transform.Scale[0] = 2.0f;
    transform.Scale[1] = 3.0f;

    Mat4 m = transform.GetMat4();

    // Upper-left 2x2 should be affected by rotation and scale
    // For 45 degrees: cos = sin = ~0.707
    float cos45 = std::cos(Math::ToRadians(45.0f));
    float sin45 = std::sin(Math::ToRadians(45.0f));

    // With scale: [cos*sx, sin*sx; -sin*sy, cos*sy]
    EXPECT_FLOAT_NEAR_EPS(cos45 * 2.0f, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(sin45 * 2.0f, m.data[1]);
    EXPECT_FLOAT_NEAR_EPS(-sin45 * 3.0f, m.data[4]);
    EXPECT_FLOAT_NEAR_EPS(cos45 * 3.0f, m.data[5]);
}

TEST_F(TransformComponentTest, GetMat4_NegativeRotation)
{
    transform.Rotation = -90.0f;

    Mat4 m = transform.GetMat4();

    // -90 degrees: cos(-90)=0, sin(-90)=-1
    EXPECT_FLOAT_NEAR_EPS(0.0f, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(-1.0f, m.data[1]);
    EXPECT_FLOAT_NEAR_EPS(1.0f, m.data[4]);
    EXPECT_FLOAT_NEAR_EPS(0.0f, m.data[5]);
}

TEST_F(TransformComponentTest, GetMat4_NonUniformScale)
{
    transform.Scale[0] = 0.5f;
    transform.Scale[1] = 4.0f;

    Mat4 m = transform.GetMat4();

    EXPECT_FLOAT_NEAR_EPS(0.5f, m.data[0]);
    EXPECT_FLOAT_NEAR_EPS(4.0f, m.data[5]);
}

// =============================================================================
// GLM Comparison Tests (Verify Mat4 matches GLM)
// =============================================================================

TEST_F(TransformComponentTest, GetMatrix_MatchesGetMat4_Default)
{
    glm::mat4 glmMat = transform.GetMatrix();
    Mat4 ourMat = transform.GetMat4();

    for (int col = 0; col < 4; col++)
    {
        for (int row = 0; row < 4; row++)
        {
            EXPECT_FLOAT_NEAR_EPS(glmMat[col][row], ourMat.data[col * 4 + row])
                << "Mismatch at [" << col << "][" << row << "]";
        }
    }
}

TEST_F(TransformComponentTest, GetMatrix_MatchesGetMat4_WithTransform)
{
    transform.Position[0] = 10.0f;
    transform.Position[1] = 20.0f;
    transform.Position[2] = 5.0f;
    transform.Rotation = 30.0f;
    transform.Scale[0] = 1.5f;
    transform.Scale[1] = 2.5f;

    glm::mat4 glmMat = transform.GetMatrix();
    Mat4 ourMat = transform.GetMat4();

    for (int col = 0; col < 4; col++)
    {
        for (int row = 0; row < 4; row++)
        {
            EXPECT_FLOAT_NEAR_EPS(glmMat[col][row], ourMat.data[col * 4 + row])
                << "Mismatch at [" << col << "][" << row << "]";
        }
    }
}

TEST_F(TransformComponentTest, GetMatrix_MatchesGetMat4_EdgeCases)
{
    // Test with extreme values
    transform.Position[0] = -1000.0f;
    transform.Position[1] = 1000.0f;
    transform.Position[2] = 0.001f;
    transform.Rotation = 359.0f;
    transform.Scale[0] = 0.01f;
    transform.Scale[1] = 100.0f;

    glm::mat4 glmMat = transform.GetMatrix();
    Mat4 ourMat = transform.GetMat4();

    for (int col = 0; col < 4; col++)
    {
        for (int row = 0; row < 4; row++)
        {
            EXPECT_NEAR(glmMat[col][row], ourMat.data[col * 4 + row], 1e-4f)
                << "Mismatch at [" << col << "][" << row << "]";
        }
    }
}
