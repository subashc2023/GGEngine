#include <gtest/gtest.h>
#include "GGEngine/ECS/Scene.h"
#include "TestConfig.h"

#include <thread>
#include <vector>
#include <atomic>

using namespace GGEngine;

class SceneIntegrationTest : public ::testing::Test
{
protected:
    void SetUp() override
    {
        m_Scene = std::make_unique<Scene>("TestScene");
    }

    void TearDown() override
    {
        m_Scene.reset();
    }

    std::unique_ptr<Scene> m_Scene;
};

// =============================================================================
// Entity Lifecycle with Components
// =============================================================================

TEST_F(SceneIntegrationTest, CreateEntity_HasDefaultComponents)
{
    EntityID entity = m_Scene->CreateEntity("Player");

    EXPECT_TRUE(m_Scene->IsEntityValid(entity));
    EXPECT_TRUE(m_Scene->HasComponent<TagComponent>(entity));
    EXPECT_TRUE(m_Scene->HasComponent<TransformComponent>(entity));

    auto* tag = m_Scene->GetComponent<TagComponent>(entity);
    ASSERT_NE(nullptr, tag);
    EXPECT_EQ("Player", tag->Name);
}

TEST_F(SceneIntegrationTest, CreateEntity_TransformHasDefaultValues)
{
    EntityID entity = m_Scene->CreateEntity("Entity");

    auto* transform = m_Scene->GetComponent<TransformComponent>(entity);
    ASSERT_NE(nullptr, transform);

    EXPECT_FLOAT_EQ(0.0f, transform->Position[0]);
    EXPECT_FLOAT_EQ(0.0f, transform->Position[1]);
    EXPECT_FLOAT_EQ(0.0f, transform->Position[2]);
    EXPECT_FLOAT_EQ(0.0f, transform->Rotation);
    EXPECT_FLOAT_EQ(1.0f, transform->Scale[0]);
    EXPECT_FLOAT_EQ(1.0f, transform->Scale[1]);
}

TEST_F(SceneIntegrationTest, DestroyEntity_RemovesAllComponents)
{
    EntityID entity = m_Scene->CreateEntity("ToDestroy");

    // Add additional components
    m_Scene->AddComponent<SpriteRendererComponent>(entity);
    m_Scene->AddComponent<CameraComponent>(entity);

    EXPECT_TRUE(m_Scene->HasComponent<TagComponent>(entity));
    EXPECT_TRUE(m_Scene->HasComponent<TransformComponent>(entity));
    EXPECT_TRUE(m_Scene->HasComponent<SpriteRendererComponent>(entity));
    EXPECT_TRUE(m_Scene->HasComponent<CameraComponent>(entity));

    Entity index = entity.Index;
    m_Scene->DestroyEntity(entity);

    // Entity should be invalid after destruction
    EXPECT_FALSE(m_Scene->IsEntityValid(entity));

    // Components should be removed from storage
    EXPECT_FALSE(m_Scene->GetStorage<TagComponent>().Has(index));
    EXPECT_FALSE(m_Scene->GetStorage<TransformComponent>().Has(index));
    EXPECT_FALSE(m_Scene->GetStorage<SpriteRendererComponent>().Has(index));
    EXPECT_FALSE(m_Scene->GetStorage<CameraComponent>().Has(index));
}

TEST_F(SceneIntegrationTest, DestroyEntity_InvalidatesStaleReferences)
{
    EntityID entity = m_Scene->CreateEntity("Original");
    EntityID staleRef = entity;  // Keep a copy

    m_Scene->DestroyEntity(entity);

    // Stale reference should be invalid
    EXPECT_FALSE(m_Scene->IsEntityValid(staleRef));

    // Operations with stale reference should fail gracefully
    EXPECT_FALSE(m_Scene->HasComponent<TransformComponent>(staleRef));
    EXPECT_EQ(nullptr, m_Scene->GetComponent<TransformComponent>(staleRef));
}

// =============================================================================
// Entity Slot Reuse and Generation Tracking
// =============================================================================

TEST_F(SceneIntegrationTest, EntitySlotReuse_GenerationIncrementsOnReuse)
{
    EntityID first = m_Scene->CreateEntity("First");
    Entity originalIndex = first.Index;
    uint32_t originalGeneration = first.Generation;

    m_Scene->DestroyEntity(first);

    // Create new entity - should reuse the slot
    EntityID second = m_Scene->CreateEntity("Second");

    EXPECT_EQ(originalIndex, second.Index);
    EXPECT_GT(second.Generation, originalGeneration);
}

TEST_F(SceneIntegrationTest, EntitySlotReuse_OldReferenceStaysInvalid)
{
    EntityID first = m_Scene->CreateEntity("First");
    EntityID oldRef = first;

    m_Scene->DestroyEntity(first);
    EntityID second = m_Scene->CreateEntity("Second");

    // Old reference should still be invalid even though index is reused
    EXPECT_FALSE(m_Scene->IsEntityValid(oldRef));

    // New entity should be valid
    EXPECT_TRUE(m_Scene->IsEntityValid(second));

    // Same index, different generation
    EXPECT_EQ(oldRef.Index, second.Index);
    EXPECT_NE(oldRef.Generation, second.Generation);
}

TEST_F(SceneIntegrationTest, EntitySlotReuse_MultipleReusesCycle)
{
    std::vector<uint32_t> generations;

    for (int i = 0; i < 5; i++)
    {
        EntityID entity = m_Scene->CreateEntity("Temp");
        generations.push_back(entity.Generation);
        m_Scene->DestroyEntity(entity);
    }

    // Each reuse should increment generation
    for (size_t i = 1; i < generations.size(); i++)
    {
        EXPECT_GT(generations[i], generations[i - 1]);
    }
}

// =============================================================================
// GUID Persistence and Lookup
// =============================================================================

TEST_F(SceneIntegrationTest, GUIDLookup_FindsEntityByGUID)
{
    EntityID entity = m_Scene->CreateEntity("Findable");
    auto* tag = m_Scene->GetComponent<TagComponent>(entity);
    GGEngine::GUID entityGUID = tag->ID;

    EntityID found = m_Scene->FindEntityByGUID(entityGUID);

    EXPECT_EQ(entity, found);
    EXPECT_TRUE(m_Scene->IsEntityValid(found));
}

TEST_F(SceneIntegrationTest, GUIDLookup_ReturnsInvalidForUnknownGUID)
{
    m_Scene->CreateEntity("SomeEntity");

    GGEngine::GUID unknownGUID = GGEngine::GUID::Generate();
    EntityID found = m_Scene->FindEntityByGUID(unknownGUID);

    EXPECT_EQ(InvalidEntityID, found);
    EXPECT_FALSE(found.IsValid());
}

TEST_F(SceneIntegrationTest, GUIDLookup_RemovedAfterEntityDestruction)
{
    EntityID entity = m_Scene->CreateEntity("WillBeDestroyed");
    auto* tag = m_Scene->GetComponent<TagComponent>(entity);
    GGEngine::GUID entityGUID = tag->ID;

    // Should find before destruction
    EXPECT_EQ(entity, m_Scene->FindEntityByGUID(entityGUID));

    m_Scene->DestroyEntity(entity);

    // Should not find after destruction
    EXPECT_EQ(InvalidEntityID, m_Scene->FindEntityByGUID(entityGUID));
}

TEST_F(SceneIntegrationTest, CreateEntityWithGUID_UsesProvidedGUID)
{
    GGEngine::GUID specificGUID = GGEngine::GUID::Generate();
    EntityID entity = m_Scene->CreateEntityWithGUID("WithSpecificGUID", specificGUID);

    auto* tag = m_Scene->GetComponent<TagComponent>(entity);
    ASSERT_NE(nullptr, tag);
    EXPECT_EQ(specificGUID, tag->ID);

    // Should be findable by the provided GUID
    EXPECT_EQ(entity, m_Scene->FindEntityByGUID(specificGUID));
}

TEST_F(SceneIntegrationTest, GUIDUniqueness_MultipleEntitiesHaveUniqueGUIDs)
{
    std::vector<GGEngine::GUID> guids;

    for (int i = 0; i < 100; i++)
    {
        EntityID entity = m_Scene->CreateEntity("Entity" + std::to_string(i));
        auto* tag = m_Scene->GetComponent<TagComponent>(entity);
        guids.push_back(tag->ID);
    }

    // Check all GUIDs are unique
    for (size_t i = 0; i < guids.size(); i++)
    {
        for (size_t j = i + 1; j < guids.size(); j++)
        {
            EXPECT_NE(guids[i], guids[j]) << "GUID collision at indices " << i << " and " << j;
        }
    }
}

// =============================================================================
// Name Lookup
// =============================================================================

TEST_F(SceneIntegrationTest, FindByName_FindsFirstMatch)
{
    EntityID player = m_Scene->CreateEntity("Player");
    m_Scene->CreateEntity("Enemy");
    m_Scene->CreateEntity("NPC");

    EntityID found = m_Scene->FindEntityByName("Player");

    EXPECT_EQ(player, found);
}

TEST_F(SceneIntegrationTest, FindByName_ReturnsInvalidForUnknownName)
{
    m_Scene->CreateEntity("Player");

    EntityID found = m_Scene->FindEntityByName("NonExistent");

    EXPECT_EQ(InvalidEntityID, found);
}

// =============================================================================
// Multi-Component Entity Workflows
// =============================================================================

TEST_F(SceneIntegrationTest, MultiComponentEntity_AddAndModifyComponents)
{
    EntityID entity = m_Scene->CreateEntity("ComplexEntity");

    // Add sprite renderer with custom color
    auto& sprite = m_Scene->AddComponent<SpriteRendererComponent>(entity);
    sprite.Color[0] = 1.0f;
    sprite.Color[1] = 0.0f;
    sprite.Color[2] = 0.0f;
    sprite.Color[3] = 1.0f;

    // Modify transform
    auto* transform = m_Scene->GetComponent<TransformComponent>(entity);
    transform->Position[0] = 10.0f;
    transform->Position[1] = 20.0f;
    transform->Scale[0] = 2.0f;

    // Verify all modifications persisted
    auto* retrievedSprite = m_Scene->GetComponent<SpriteRendererComponent>(entity);
    EXPECT_FLOAT_EQ(1.0f, retrievedSprite->Color[0]);
    EXPECT_FLOAT_EQ(0.0f, retrievedSprite->Color[1]);

    auto* retrievedTransform = m_Scene->GetComponent<TransformComponent>(entity);
    EXPECT_FLOAT_EQ(10.0f, retrievedTransform->Position[0]);
    EXPECT_FLOAT_EQ(20.0f, retrievedTransform->Position[1]);
    EXPECT_FLOAT_EQ(2.0f, retrievedTransform->Scale[0]);
}

TEST_F(SceneIntegrationTest, MultiComponentEntity_RemoveSingleComponent)
{
    EntityID entity = m_Scene->CreateEntity("Entity");
    m_Scene->AddComponent<SpriteRendererComponent>(entity);
    m_Scene->AddComponent<CameraComponent>(entity);

    EXPECT_TRUE(m_Scene->HasComponent<SpriteRendererComponent>(entity));
    EXPECT_TRUE(m_Scene->HasComponent<CameraComponent>(entity));

    m_Scene->RemoveComponent<SpriteRendererComponent>(entity);

    EXPECT_FALSE(m_Scene->HasComponent<SpriteRendererComponent>(entity));
    EXPECT_TRUE(m_Scene->HasComponent<CameraComponent>(entity));

    // Entity should still be valid
    EXPECT_TRUE(m_Scene->IsEntityValid(entity));
}

// =============================================================================
// Camera System Integration
// =============================================================================

TEST_F(SceneIntegrationTest, PrimaryCamera_FindsFirstPrimaryCamera)
{
    EntityID camera1 = m_Scene->CreateEntity("Camera1");
    auto& cam1 = m_Scene->AddComponent<CameraComponent>(camera1);
    cam1.Primary = true;

    EntityID camera2 = m_Scene->CreateEntity("Camera2");
    auto& cam2 = m_Scene->AddComponent<CameraComponent>(camera2);
    cam2.Primary = false;

    EntityID primary = m_Scene->GetPrimaryCameraEntity();

    EXPECT_EQ(camera1, primary);
}

TEST_F(SceneIntegrationTest, PrimaryCamera_ReturnsInvalidWhenNoCameras)
{
    m_Scene->CreateEntity("NonCameraEntity");

    EntityID primary = m_Scene->GetPrimaryCameraEntity();

    EXPECT_EQ(InvalidEntityID, primary);
}

TEST_F(SceneIntegrationTest, PrimaryCamera_ReturnsInvalidWhenNoPrimary)
{
    EntityID camera = m_Scene->CreateEntity("Camera");
    auto& cam = m_Scene->AddComponent<CameraComponent>(camera);
    cam.Primary = false;

    EntityID primary = m_Scene->GetPrimaryCameraEntity();

    EXPECT_EQ(InvalidEntityID, primary);
}

TEST_F(SceneIntegrationTest, ViewportResize_UpdatesNonFixedCameras)
{
    EntityID camera1 = m_Scene->CreateEntity("FlexibleCamera");
    auto& cam1 = m_Scene->AddComponent<CameraComponent>(camera1);
    cam1.FixedAspectRatio = false;

    EntityID camera2 = m_Scene->CreateEntity("FixedCamera");
    auto& cam2 = m_Scene->AddComponent<CameraComponent>(camera2);
    cam2.FixedAspectRatio = true;

    m_Scene->OnViewportResize(1920, 1080);

    // Both cameras should still exist and be valid
    EXPECT_TRUE(m_Scene->HasComponent<CameraComponent>(camera1));
    EXPECT_TRUE(m_Scene->HasComponent<CameraComponent>(camera2));
}

// =============================================================================
// Scene Clear
// =============================================================================

TEST_F(SceneIntegrationTest, Clear_RemovesAllEntities)
{
    std::vector<EntityID> entities;
    for (int i = 0; i < 10; i++)
    {
        entities.push_back(m_Scene->CreateEntity("Entity" + std::to_string(i)));
    }

    EXPECT_EQ(10u, m_Scene->GetEntityCount());

    m_Scene->Clear();

    EXPECT_EQ(0u, m_Scene->GetEntityCount());

    // All old references should be invalid
    for (const auto& entity : entities)
    {
        EXPECT_FALSE(m_Scene->IsEntityValid(entity));
    }
}

TEST_F(SceneIntegrationTest, Clear_ClearsAllComponentStorages)
{
    for (int i = 0; i < 5; i++)
    {
        EntityID entity = m_Scene->CreateEntity("Entity");
        m_Scene->AddComponent<SpriteRendererComponent>(entity);
        m_Scene->AddComponent<CameraComponent>(entity);
    }

    EXPECT_EQ(5u, m_Scene->GetStorage<TagComponent>().Size());
    EXPECT_EQ(5u, m_Scene->GetStorage<TransformComponent>().Size());
    EXPECT_EQ(5u, m_Scene->GetStorage<SpriteRendererComponent>().Size());
    EXPECT_EQ(5u, m_Scene->GetStorage<CameraComponent>().Size());

    m_Scene->Clear();

    EXPECT_EQ(0u, m_Scene->GetStorage<TagComponent>().Size());
    EXPECT_EQ(0u, m_Scene->GetStorage<TransformComponent>().Size());
    EXPECT_EQ(0u, m_Scene->GetStorage<SpriteRendererComponent>().Size());
    EXPECT_EQ(0u, m_Scene->GetStorage<CameraComponent>().Size());
}

TEST_F(SceneIntegrationTest, Clear_AllowsNewEntityCreation)
{
    m_Scene->CreateEntity("Old");
    m_Scene->Clear();

    EntityID newEntity = m_Scene->CreateEntity("New");

    EXPECT_TRUE(m_Scene->IsEntityValid(newEntity));
    EXPECT_EQ(1u, m_Scene->GetEntityCount());
}

// =============================================================================
// Scene Metadata
// =============================================================================

TEST_F(SceneIntegrationTest, SceneName_SetAndGet)
{
    EXPECT_EQ("TestScene", m_Scene->GetName());

    m_Scene->SetName("RenamedScene");

    EXPECT_EQ("RenamedScene", m_Scene->GetName());
}

// =============================================================================
// Entity Iteration
// =============================================================================

TEST_F(SceneIntegrationTest, GetAllEntities_ReturnsActiveEntityIndices)
{
    EntityID e1 = m_Scene->CreateEntity("E1");
    EntityID e2 = m_Scene->CreateEntity("E2");
    EntityID e3 = m_Scene->CreateEntity("E3");

    m_Scene->DestroyEntity(e2);

    const auto& entities = m_Scene->GetAllEntities();

    EXPECT_EQ(2u, entities.size());

    // Should contain e1 and e3 indices
    bool hasE1 = std::find(entities.begin(), entities.end(), e1.Index) != entities.end();
    bool hasE3 = std::find(entities.begin(), entities.end(), e3.Index) != entities.end();
    bool hasE2 = std::find(entities.begin(), entities.end(), e2.Index) != entities.end();

    EXPECT_TRUE(hasE1);
    EXPECT_TRUE(hasE3);
    EXPECT_FALSE(hasE2);
}

// =============================================================================
// Storage Access and Bulk Iteration
// =============================================================================

TEST_F(SceneIntegrationTest, GetStorage_AllowsBulkIteration)
{
    // Create multiple entities with transforms
    for (int i = 0; i < 5; i++)
    {
        EntityID entity = m_Scene->CreateEntity("Entity" + std::to_string(i));
        auto* transform = m_Scene->GetComponent<TransformComponent>(entity);
        transform->Position[0] = static_cast<float>(i * 10);
    }

    // Bulk iterate over all transforms
    auto& transforms = m_Scene->GetStorage<TransformComponent>();
    float sum = 0.0f;
    for (size_t i = 0; i < transforms.Size(); i++)
    {
        sum += transforms.Data()[i].Position[0];
    }

    // 0 + 10 + 20 + 30 + 40 = 100
    EXPECT_FLOAT_EQ(100.0f, sum);
}

// =============================================================================
// Concurrent Access (Basic Thread Safety)
// =============================================================================

TEST_F(SceneIntegrationTest, ConcurrentComponentStorageCreation_ThreadSafe)
{
    // Test that multiple threads can trigger component storage creation safely
    // This exercises the double-checked locking in GetOrCreateStorage

    constexpr int NUM_THREADS = 4;
    std::atomic<int> successCount{ 0 };
    std::vector<std::thread> threads;

    // Pre-create an entity to work with
    EntityID entity = m_Scene->CreateEntity("SharedEntity");

    // Multiple threads trying to access a new component type simultaneously
    // Using SpriteRendererComponent which isn't created by default
    for (int i = 0; i < NUM_THREADS; i++)
    {
        threads.emplace_back([this, entity, &successCount]()
        {
            // All threads try to access the same component storage
            bool hasComponent = m_Scene->HasComponent<SpriteRendererComponent>(entity);
            if (!hasComponent)
            {
                successCount++;
            }
        });
    }

    for (auto& t : threads)
    {
        t.join();
    }

    // All threads should have completed without crashing
    EXPECT_EQ(NUM_THREADS, successCount.load());
}

TEST_F(SceneIntegrationTest, ConcurrentReads_ThreadSafe)
{
    // Create entities with components
    std::vector<EntityID> entities;
    for (int i = 0; i < 100; i++)
    {
        entities.push_back(m_Scene->CreateEntity("Entity" + std::to_string(i)));
    }

    constexpr int NUM_THREADS = 4;
    std::atomic<int> totalReads{ 0 };
    std::vector<std::thread> threads;

    // Multiple threads reading component data concurrently
    for (int t = 0; t < NUM_THREADS; t++)
    {
        threads.emplace_back([this, &entities, &totalReads]()
        {
            int localReads = 0;
            for (const auto& entity : entities)
            {
                if (m_Scene->HasComponent<TransformComponent>(entity))
                {
                    auto* transform = m_Scene->GetComponent<TransformComponent>(entity);
                    if (transform != nullptr)
                    {
                        localReads++;
                    }
                }
            }
            totalReads += localReads;
        });
    }

    for (auto& t : threads)
    {
        t.join();
    }

    // Each thread should have read all 100 transforms
    EXPECT_EQ(NUM_THREADS * 100, totalReads.load());
}

// =============================================================================
// Edge Cases
// =============================================================================

TEST_F(SceneIntegrationTest, OperationsOnInvalidEntity_HandleGracefully)
{
    // HasComponent on invalid entity
    EXPECT_FALSE(m_Scene->HasComponent<TransformComponent>(InvalidEntityID));

    // GetComponent on invalid entity
    EXPECT_EQ(nullptr, m_Scene->GetComponent<TransformComponent>(InvalidEntityID));

    // RemoveComponent on invalid entity should not crash
    m_Scene->RemoveComponent<TransformComponent>(InvalidEntityID);

    // DestroyEntity on invalid entity should not crash
    m_Scene->DestroyEntity(InvalidEntityID);
}

TEST_F(SceneIntegrationTest, LargeSceneStressTest)
{
    constexpr int ENTITY_COUNT = 1000;

    // Create many entities
    std::vector<EntityID> entities;
    entities.reserve(ENTITY_COUNT);

    for (int i = 0; i < ENTITY_COUNT; i++)
    {
        EntityID entity = m_Scene->CreateEntity("Entity" + std::to_string(i));
        entities.push_back(entity);

        // Add some variety in components
        if (i % 2 == 0)
        {
            m_Scene->AddComponent<SpriteRendererComponent>(entity);
        }
        if (i % 3 == 0)
        {
            m_Scene->AddComponent<CameraComponent>(entity);
        }
    }

    EXPECT_EQ(ENTITY_COUNT, m_Scene->GetEntityCount());
    EXPECT_EQ(ENTITY_COUNT, m_Scene->GetStorage<TagComponent>().Size());
    EXPECT_EQ(ENTITY_COUNT, m_Scene->GetStorage<TransformComponent>().Size());
    EXPECT_EQ(500u, m_Scene->GetStorage<SpriteRendererComponent>().Size());  // Every 2nd
    EXPECT_EQ(334u, m_Scene->GetStorage<CameraComponent>().Size());           // Every 3rd (0,3,6,...,999)

    // Destroy half the entities
    for (int i = 0; i < ENTITY_COUNT / 2; i++)
    {
        m_Scene->DestroyEntity(entities[i]);
    }

    EXPECT_EQ(ENTITY_COUNT / 2, m_Scene->GetEntityCount());

    // Remaining entities should still be valid
    for (int i = ENTITY_COUNT / 2; i < ENTITY_COUNT; i++)
    {
        EXPECT_TRUE(m_Scene->IsEntityValid(entities[i]));
    }
}
