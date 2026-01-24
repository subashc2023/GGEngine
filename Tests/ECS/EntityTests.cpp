#include <gtest/gtest.h>
#include "GGEngine/ECS/Entity.h"
#include "TestConfig.h"

using namespace GGEngine;

class EntityTest : public ::testing::Test {};
class EntityIDTest : public ::testing::Test {};

// =============================================================================
// Entity Type Tests
// =============================================================================

TEST_F(EntityTest, InvalidEntityConstant)
{
    EXPECT_EQ(UINT32_MAX, InvalidEntity);
}

TEST_F(EntityTest, EntityIsUint32)
{
    Entity e = 42;
    EXPECT_EQ(42u, e);
}

// =============================================================================
// EntityID Construction Tests
// =============================================================================

TEST_F(EntityIDTest, DefaultConstruction)
{
    EntityID id{};
    EXPECT_EQ(InvalidEntity, id.Index);
    EXPECT_EQ(0u, id.Generation);
}

TEST_F(EntityIDTest, CustomConstruction)
{
    EntityID id{ 5, 10 };
    EXPECT_EQ(5u, id.Index);
    EXPECT_EQ(10u, id.Generation);
}

// =============================================================================
// IsValid Tests
// =============================================================================

TEST_F(EntityIDTest, IsValid_InvalidEntity)
{
    EntityID id{ InvalidEntity, 0 };
    EXPECT_FALSE(id.IsValid());
}

TEST_F(EntityIDTest, IsValid_InvalidEntityWithGeneration)
{
    EntityID id{ InvalidEntity, 100 };
    EXPECT_FALSE(id.IsValid());
}

TEST_F(EntityIDTest, IsValid_ValidEntity)
{
    EntityID id{ 0, 1 };
    EXPECT_TRUE(id.IsValid());
}

TEST_F(EntityIDTest, IsValid_ValidEntityZeroGeneration)
{
    EntityID id{ 0, 0 };
    EXPECT_TRUE(id.IsValid());
}

TEST_F(EntityIDTest, IsValid_LargeIndex)
{
    EntityID id{ UINT32_MAX - 1, 0 };
    EXPECT_TRUE(id.IsValid());
}

// =============================================================================
// Equality Tests
// =============================================================================

TEST_F(EntityIDTest, Equality_SameValues)
{
    EntityID a{ 5, 10 };
    EntityID b{ 5, 10 };
    EXPECT_EQ(a, b);
}

TEST_F(EntityIDTest, Equality_DifferentIndex)
{
    EntityID a{ 5, 10 };
    EntityID b{ 6, 10 };
    EXPECT_NE(a, b);
}

TEST_F(EntityIDTest, Equality_DifferentGeneration)
{
    EntityID a{ 5, 10 };
    EntityID b{ 5, 11 };
    EXPECT_NE(a, b);
}

TEST_F(EntityIDTest, Equality_BothInvalid)
{
    EntityID a{ InvalidEntity, 0 };
    EntityID b{ InvalidEntity, 0 };
    EXPECT_EQ(a, b);
}

TEST_F(EntityIDTest, Inequality_Operator)
{
    EntityID a{ 1, 1 };
    EntityID b{ 2, 1 };
    EXPECT_TRUE(a != b);
    EXPECT_FALSE(a != a);
}

// =============================================================================
// InvalidEntityID Constant Tests
// =============================================================================

TEST_F(EntityIDTest, InvalidEntityIDConstant_IsInvalid)
{
    EXPECT_FALSE(InvalidEntityID.IsValid());
}

TEST_F(EntityIDTest, InvalidEntityIDConstant_HasInvalidIndex)
{
    EXPECT_EQ(InvalidEntity, InvalidEntityID.Index);
}

TEST_F(EntityIDTest, InvalidEntityIDConstant_HasZeroGeneration)
{
    EXPECT_EQ(0u, InvalidEntityID.Generation);
}

TEST_F(EntityIDTest, InvalidEntityIDConstant_ComparesCorrectly)
{
    EntityID id{ InvalidEntity, 0 };
    EXPECT_EQ(InvalidEntityID, id);
}

// =============================================================================
// Generation Tracking Use Cases
// =============================================================================

TEST_F(EntityIDTest, GenerationTracking_SameIndexDifferentGeneration)
{
    // Simulate entity reuse scenario
    EntityID original{ 10, 1 };
    EntityID reused{ 10, 2 };

    // Same index but different generation means different entity
    EXPECT_NE(original, reused);
    EXPECT_EQ(original.Index, reused.Index);
    EXPECT_NE(original.Generation, reused.Generation);
}

TEST_F(EntityIDTest, GenerationTracking_CanDetectStaleReference)
{
    EntityID original{ 5, 1 };
    uint32_t currentGeneration = 2;  // Generation has been incremented

    // A stale reference would have a lower generation
    bool isStale = original.Generation < currentGeneration;
    EXPECT_TRUE(isStale);
}
