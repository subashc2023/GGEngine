#include <gtest/gtest.h>
#include "GGEngine/ECS/GUID.h"
#include "TestConfig.h"
#include <unordered_set>

// Note: Don't use "using namespace GGEngine" to avoid name collision with Windows GUID type
using GGEngine::GUIDHash;

class GUIDTest : public ::testing::Test {};

// =============================================================================
// Construction Tests
// =============================================================================

TEST_F(GUIDTest, DefaultConstruction_IsInvalid)
{
    GGEngine::GUID guid{};
    EXPECT_FALSE(guid.IsValid());
    EXPECT_EQ(0u, guid.High);
    EXPECT_EQ(0u, guid.Low);
}

TEST_F(GUIDTest, CustomConstruction)
{
    GGEngine::GUID guid{ 123, 456 };
    EXPECT_EQ(123u, guid.High);
    EXPECT_EQ(456u, guid.Low);
}

// =============================================================================
// IsValid Tests
// =============================================================================

TEST_F(GUIDTest, IsValid_ZeroIsInvalid)
{
    GGEngine::GUID guid{ 0, 0 };
    EXPECT_FALSE(guid.IsValid());
}

TEST_F(GUIDTest, IsValid_NonZeroHighIsValid)
{
    GGEngine::GUID guid{ 1, 0 };
    EXPECT_TRUE(guid.IsValid());
}

TEST_F(GUIDTest, IsValid_NonZeroLowIsValid)
{
    GGEngine::GUID guid{ 0, 1 };
    EXPECT_TRUE(guid.IsValid());
}

TEST_F(GUIDTest, IsValid_BothNonZeroIsValid)
{
    GGEngine::GUID guid{ 1, 1 };
    EXPECT_TRUE(guid.IsValid());
}

// =============================================================================
// Generate Tests
// =============================================================================

TEST_F(GUIDTest, Generate_CreatesValidGUID)
{
    GGEngine::GUID guid = GGEngine::GUID::Generate();
    EXPECT_TRUE(guid.IsValid());
}

TEST_F(GUIDTest, Generate_CreatesUniqueGUIDs)
{
    const int count = 100;
    std::unordered_set<std::string> guids;

    for (int i = 0; i < count; i++)
    {
        GGEngine::GUID guid = GGEngine::GUID::Generate();
        guids.insert(guid.ToString());
    }

    EXPECT_EQ(count, guids.size()) << "All generated GUIDs should be unique";
}

TEST_F(GUIDTest, Generate_MultipleCallsNeverReturnSame)
{
    GGEngine::GUID guid1 = GGEngine::GUID::Generate();
    GGEngine::GUID guid2 = GGEngine::GUID::Generate();

    EXPECT_NE(guid1, guid2);
}

// =============================================================================
// Equality Tests
// =============================================================================

TEST_F(GUIDTest, Equality_SameValues)
{
    GGEngine::GUID a{ 123, 456 };
    GGEngine::GUID b{ 123, 456 };
    EXPECT_EQ(a, b);
}

TEST_F(GUIDTest, Equality_DifferentHigh)
{
    GGEngine::GUID a{ 123, 456 };
    GGEngine::GUID b{ 124, 456 };
    EXPECT_NE(a, b);
}

TEST_F(GUIDTest, Equality_DifferentLow)
{
    GGEngine::GUID a{ 123, 456 };
    GGEngine::GUID b{ 123, 457 };
    EXPECT_NE(a, b);
}

TEST_F(GUIDTest, Inequality_Operator)
{
    GGEngine::GUID a{ 1, 2 };
    GGEngine::GUID b{ 3, 4 };
    EXPECT_TRUE(a != b);
    EXPECT_FALSE(a != a);
}

// =============================================================================
// ToString/FromString Tests
// =============================================================================

TEST_F(GUIDTest, ToString_Format)
{
    GGEngine::GUID guid{ 0x0123456789ABCDEF, 0xFEDCBA9876543210 };
    std::string str = guid.ToString();

    // Should be 32 hex characters (128 bits)
    EXPECT_EQ(32u, str.length());
}

TEST_F(GUIDTest, ToString_ZeroGUID)
{
    GGEngine::GUID guid{ 0, 0 };
    std::string str = guid.ToString();

    EXPECT_EQ(32u, str.length());
    EXPECT_EQ("00000000000000000000000000000000", str);
}

TEST_F(GUIDTest, FromString_ValidString)
{
    GGEngine::GUID original{ 0x123456789ABCDEF0, 0xFEDCBA9876543210 };
    std::string str = original.ToString();
    GGEngine::GUID reconstructed = GGEngine::GUID::FromString(str);

    EXPECT_EQ(original.High, reconstructed.High);
    EXPECT_EQ(original.Low, reconstructed.Low);
}

TEST_F(GUIDTest, RoundTrip_ToStringFromString)
{
    GGEngine::GUID original = GGEngine::GUID::Generate();
    std::string str = original.ToString();
    GGEngine::GUID reconstructed = GGEngine::GUID::FromString(str);

    EXPECT_EQ(original, reconstructed);
}

TEST_F(GUIDTest, RoundTrip_MultipleGUIDs)
{
    for (int i = 0; i < 10; i++)
    {
        GGEngine::GUID original = GGEngine::GUID::Generate();
        std::string str = original.ToString();
        GGEngine::GUID reconstructed = GGEngine::GUID::FromString(str);

        EXPECT_EQ(original, reconstructed)
            << "Round-trip failed for iteration " << i;
    }
}

// =============================================================================
// GUIDHash Tests
// =============================================================================

TEST_F(GUIDTest, Hash_SameGUIDSameHash)
{
    GUIDHash hasher;
    GGEngine::GUID a{ 100, 200 };
    GGEngine::GUID b{ 100, 200 };

    EXPECT_EQ(hasher(a), hasher(b));
}

TEST_F(GUIDTest, Hash_DifferentGUIDsDifferentHash)
{
    GUIDHash hasher;
    GGEngine::GUID a{ 1, 2 };
    GGEngine::GUID b{ 3, 4 };

    // Very likely different (not guaranteed, but extremely unlikely to collide)
    EXPECT_NE(hasher(a), hasher(b));
}

TEST_F(GUIDTest, Hash_CanBeUsedInUnorderedSet)
{
    std::unordered_set<GGEngine::GUID, GUIDHash> guidSet;

    GGEngine::GUID g1 = GGEngine::GUID::Generate();
    GGEngine::GUID g2 = GGEngine::GUID::Generate();
    GGEngine::GUID g3 = GGEngine::GUID::Generate();

    guidSet.insert(g1);
    guidSet.insert(g2);
    guidSet.insert(g3);
    guidSet.insert(g1);  // Duplicate

    EXPECT_EQ(3u, guidSet.size());
    EXPECT_TRUE(guidSet.count(g1) > 0);
    EXPECT_TRUE(guidSet.count(g2) > 0);
    EXPECT_TRUE(guidSet.count(g3) > 0);
}
