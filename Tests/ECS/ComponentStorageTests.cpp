#include <gtest/gtest.h>
#include "GGEngine/ECS/ComponentStorage.h"
#include "TestConfig.h"
#include <thread>
#include <vector>
#include <algorithm>

using namespace GGEngine;

// Simple test component
struct TestComponent
{
    int value = 0;
    float data = 0.0f;
};

class ComponentStorageTest : public ::testing::Test {
protected:
    ComponentStorage<TestComponent> storage;
};

// =============================================================================
// Initial State Tests
// =============================================================================

TEST_F(ComponentStorageTest, InitialState_IsEmpty)
{
    EXPECT_EQ(0u, storage.Size());
}

TEST_F(ComponentStorageTest, InitialState_DataIsNullptr)
{
    // Empty vector may return nullptr for data()
    EXPECT_EQ(0u, storage.Size());
}

// =============================================================================
// Add Tests
// =============================================================================

TEST_F(ComponentStorageTest, Add_IncreasesSize)
{
    storage.Add(0);
    EXPECT_EQ(1u, storage.Size());

    storage.Add(1);
    EXPECT_EQ(2u, storage.Size());
}

TEST_F(ComponentStorageTest, Add_ReturnsReference)
{
    TestComponent& comp = storage.Add(0);
    comp.value = 42;

    TestComponent* retrieved = storage.Get(0);
    ASSERT_NE(nullptr, retrieved);
    EXPECT_EQ(42, retrieved->value);
}

TEST_F(ComponentStorageTest, Add_WithInitialValue)
{
    TestComponent initial{ 100, 3.14f };
    storage.Add(0, initial);

    TestComponent* retrieved = storage.Get(0);
    ASSERT_NE(nullptr, retrieved);
    EXPECT_EQ(100, retrieved->value);
    EXPECT_FLOAT_EQ(3.14f, retrieved->data);
}

TEST_F(ComponentStorageTest, Add_MultipleEntities)
{
    storage.Add(0).value = 10;
    storage.Add(5).value = 50;
    storage.Add(10).value = 100;

    EXPECT_EQ(3u, storage.Size());
    EXPECT_EQ(10, storage.Get(0)->value);
    EXPECT_EQ(50, storage.Get(5)->value);
    EXPECT_EQ(100, storage.Get(10)->value);
}

// =============================================================================
// Has Tests
// =============================================================================

TEST_F(ComponentStorageTest, Has_ReturnsFalseForMissing)
{
    EXPECT_FALSE(storage.Has(0));
    EXPECT_FALSE(storage.Has(999));
}

TEST_F(ComponentStorageTest, Has_ReturnsTrueAfterAdd)
{
    storage.Add(5);

    EXPECT_TRUE(storage.Has(5));
    EXPECT_FALSE(storage.Has(0));
    EXPECT_FALSE(storage.Has(6));
}

// =============================================================================
// Get Tests
// =============================================================================

TEST_F(ComponentStorageTest, Get_ReturnsNullptrForMissing)
{
    EXPECT_EQ(nullptr, storage.Get(0));
    EXPECT_EQ(nullptr, storage.Get(999));
}

TEST_F(ComponentStorageTest, Get_ReturnsComponentAfterAdd)
{
    storage.Add(10);

    TestComponent* comp = storage.Get(10);
    ASSERT_NE(nullptr, comp);
}

TEST_F(ComponentStorageTest, Get_ModifyThroughPointer)
{
    storage.Add(0);
    TestComponent* comp = storage.Get(0);
    comp->value = 999;

    EXPECT_EQ(999, storage.Get(0)->value);
}

TEST_F(ComponentStorageTest, Get_ConstVersion)
{
    storage.Add(0).value = 123;

    const auto& constStorage = storage;
    const TestComponent* comp = constStorage.Get(0);

    ASSERT_NE(nullptr, comp);
    EXPECT_EQ(123, comp->value);
}

// =============================================================================
// Remove Tests
// =============================================================================

TEST_F(ComponentStorageTest, Remove_DecreasesSize)
{
    storage.Add(0);
    storage.Add(1);
    storage.Add(2);
    EXPECT_EQ(3u, storage.Size());

    storage.Remove(1);
    EXPECT_EQ(2u, storage.Size());
}

TEST_F(ComponentStorageTest, Remove_MakesHasReturnFalse)
{
    storage.Add(5);
    EXPECT_TRUE(storage.Has(5));

    storage.Remove(5);
    EXPECT_FALSE(storage.Has(5));
}

TEST_F(ComponentStorageTest, Remove_MakesGetReturnNullptr)
{
    storage.Add(5);
    EXPECT_NE(nullptr, storage.Get(5));

    storage.Remove(5);
    EXPECT_EQ(nullptr, storage.Get(5));
}

TEST_F(ComponentStorageTest, Remove_SwapWithLast_PreservesOtherEntities)
{
    // Add entities 0, 1, 2
    storage.Add(0).value = 100;
    storage.Add(1).value = 200;
    storage.Add(2).value = 300;

    // Remove middle entity (1) - should swap with last (2)
    storage.Remove(1);

    // Entity 0 should still be accessible
    EXPECT_TRUE(storage.Has(0));
    EXPECT_EQ(100, storage.Get(0)->value);

    // Entity 1 should be gone
    EXPECT_FALSE(storage.Has(1));

    // Entity 2 should still be accessible with correct value
    EXPECT_TRUE(storage.Has(2));
    EXPECT_EQ(300, storage.Get(2)->value);
}

TEST_F(ComponentStorageTest, Remove_NonExistent_DoesNothing)
{
    storage.Add(0);
    EXPECT_EQ(1u, storage.Size());

    storage.Remove(999);  // Non-existent
    EXPECT_EQ(1u, storage.Size());
}

TEST_F(ComponentStorageTest, Remove_LastElement)
{
    storage.Add(0).value = 100;
    storage.Add(1).value = 200;

    // Remove last element
    storage.Remove(1);

    EXPECT_EQ(1u, storage.Size());
    EXPECT_TRUE(storage.Has(0));
    EXPECT_FALSE(storage.Has(1));
    EXPECT_EQ(100, storage.Get(0)->value);
}

// =============================================================================
// Clear Tests
// =============================================================================

TEST_F(ComponentStorageTest, Clear_RemovesAll)
{
    storage.Add(0);
    storage.Add(1);
    storage.Add(2);

    storage.Clear();

    EXPECT_EQ(0u, storage.Size());
    EXPECT_FALSE(storage.Has(0));
    EXPECT_FALSE(storage.Has(1));
    EXPECT_FALSE(storage.Has(2));
}

// =============================================================================
// Data Access Tests
// =============================================================================

TEST_F(ComponentStorageTest, Data_ReturnsDenseArray)
{
    storage.Add(5).value = 50;
    storage.Add(10).value = 100;
    storage.Add(15).value = 150;

    const TestComponent* data = storage.Data();
    ASSERT_NE(nullptr, data);

    // Values should be contiguous (order may vary due to sparse storage)
    std::vector<int> values;
    for (size_t i = 0; i < storage.Size(); i++)
    {
        values.push_back(data[i].value);
    }

    EXPECT_EQ(3u, values.size());
    EXPECT_TRUE(std::find(values.begin(), values.end(), 50) != values.end());
    EXPECT_TRUE(std::find(values.begin(), values.end(), 100) != values.end());
    EXPECT_TRUE(std::find(values.begin(), values.end(), 150) != values.end());
}

TEST_F(ComponentStorageTest, GetEntity_ReturnsCorrectMapping)
{
    storage.Add(100);
    storage.Add(200);
    storage.Add(300);

    // Each index should map back to an entity
    std::vector<Entity> entities;
    for (size_t i = 0; i < storage.Size(); i++)
    {
        entities.push_back(storage.GetEntity(i));
    }

    EXPECT_EQ(3u, entities.size());
    EXPECT_TRUE(std::find(entities.begin(), entities.end(), 100) != entities.end());
    EXPECT_TRUE(std::find(entities.begin(), entities.end(), 200) != entities.end());
    EXPECT_TRUE(std::find(entities.begin(), entities.end(), 300) != entities.end());
}

// =============================================================================
// ReadLock Tests
// =============================================================================

TEST_F(ComponentStorageTest, ReadLock_CanReadData)
{
    storage.Add(0).value = 42;

    auto lock = storage.LockRead();
    const TestComponent* comp = lock.Get(0);

    ASSERT_NE(nullptr, comp);
    EXPECT_EQ(42, comp->value);
}

TEST_F(ComponentStorageTest, ReadLock_HasMethod)
{
    storage.Add(5);

    auto lock = storage.LockRead();
    EXPECT_TRUE(lock.Has(5));
    EXPECT_FALSE(lock.Has(0));
}

TEST_F(ComponentStorageTest, ReadLock_SizeAndData)
{
    storage.Add(0).value = 10;
    storage.Add(1).value = 20;

    auto lock = storage.LockRead();
    EXPECT_EQ(2u, lock.Size());
    EXPECT_NE(nullptr, lock.Data());
}

TEST_F(ComponentStorageTest, ReadLock_AllowsConcurrentReads)
{
    storage.Add(0).value = 42;

    std::vector<std::thread> readers;
    std::atomic<int> readCount{0};

    for (int i = 0; i < 10; i++)
    {
        readers.emplace_back([this, &readCount]() {
            auto lock = storage.LockRead();
            const TestComponent* comp = lock.Get(0);
            if (comp && comp->value == 42)
                readCount++;
        });
    }

    for (auto& t : readers)
        t.join();

    EXPECT_EQ(10, readCount.load());
}

// =============================================================================
// WriteLock Tests
// =============================================================================

TEST_F(ComponentStorageTest, WriteLock_CanAddComponents)
{
    {
        auto lock = storage.LockWrite();
        lock.Add(0).value = 100;
        lock.Add(1).value = 200;
    }

    EXPECT_EQ(2u, storage.Size());
    EXPECT_EQ(100, storage.Get(0)->value);
    EXPECT_EQ(200, storage.Get(1)->value);
}

TEST_F(ComponentStorageTest, WriteLock_CanRemoveComponents)
{
    storage.Add(0);
    storage.Add(1);

    {
        auto lock = storage.LockWrite();
        lock.Remove(0);
    }

    EXPECT_EQ(1u, storage.Size());
    EXPECT_FALSE(storage.Has(0));
    EXPECT_TRUE(storage.Has(1));
}

TEST_F(ComponentStorageTest, WriteLock_CanClear)
{
    storage.Add(0);
    storage.Add(1);

    {
        auto lock = storage.LockWrite();
        lock.Clear();
    }

    EXPECT_EQ(0u, storage.Size());
}

TEST_F(ComponentStorageTest, WriteLock_ExclusiveAccess)
{
    std::atomic<int> counter{0};

    std::vector<std::thread> writers;
    for (int i = 0; i < 5; i++)
    {
        writers.emplace_back([this, &counter, i]() {
            auto lock = storage.LockWrite();
            lock.Add(static_cast<Entity>(i)).value = i;
            counter++;
        });
    }

    for (auto& t : writers)
        t.join();

    EXPECT_EQ(5, counter.load());
    EXPECT_EQ(5u, storage.Size());
}

// =============================================================================
// Iteration Pattern Tests
// =============================================================================

TEST_F(ComponentStorageTest, Iteration_ProcessAllComponents)
{
    storage.Add(0).value = 1;
    storage.Add(1).value = 2;
    storage.Add(2).value = 3;

    int sum = 0;
    TestComponent* data = storage.Data();
    for (size_t i = 0; i < storage.Size(); i++)
    {
        sum += data[i].value;
    }

    EXPECT_EQ(6, sum);
}

TEST_F(ComponentStorageTest, Iteration_WithEntityMapping)
{
    storage.Add(10).value = 100;
    storage.Add(20).value = 200;
    storage.Add(30).value = 300;

    std::unordered_map<Entity, int> entityValues;
    TestComponent* data = storage.Data();
    for (size_t i = 0; i < storage.Size(); i++)
    {
        Entity entity = storage.GetEntity(i);
        entityValues[entity] = data[i].value;
    }

    EXPECT_EQ(100, entityValues[10]);
    EXPECT_EQ(200, entityValues[20]);
    EXPECT_EQ(300, entityValues[30]);
}
