#include <gtest/gtest.h>
#include "GGEngine/Core/TaskGraph.h"
#include "TestConfig.h"
#include <atomic>
#include <chrono>
#include <thread>

using namespace GGEngine;

// =============================================================================
// TaskID Tests (No TaskGraph needed)
// =============================================================================

class TaskIDTest : public ::testing::Test {};

TEST_F(TaskIDTest, DefaultConstruction_IsInvalid)
{
    TaskID id{};
    EXPECT_FALSE(id.IsValid());
    EXPECT_EQ(UINT32_MAX, id.Index);
}

TEST_F(TaskIDTest, CustomConstruction_IsValid)
{
    TaskID id{ 5, 10 };
    EXPECT_TRUE(id.IsValid());
    EXPECT_EQ(5u, id.Index);
    EXPECT_EQ(10u, id.Generation);
}

TEST_F(TaskIDTest, Equality)
{
    TaskID a{ 1, 2 };
    TaskID b{ 1, 2 };
    TaskID c{ 1, 3 };
    TaskID d{ 2, 2 };

    EXPECT_EQ(a, b);
    EXPECT_NE(a, c);
    EXPECT_NE(a, d);
}

TEST_F(TaskIDTest, Hash_CanBeUsedInContainer)
{
    std::unordered_set<TaskID, TaskIDHash> taskSet;

    TaskID t1{ 1, 1 };
    TaskID t2{ 2, 1 };
    TaskID t3{ 1, 2 };

    taskSet.insert(t1);
    taskSet.insert(t2);
    taskSet.insert(t3);
    taskSet.insert(t1);  // Duplicate

    EXPECT_EQ(3u, taskSet.size());
}

// =============================================================================
// TaskResult Tests (No TaskGraph needed)
// =============================================================================

class TaskResultTest : public ::testing::Test {};

TEST_F(TaskResultTest, DefaultConstruction_NoValue)
{
    TaskResult result;
    EXPECT_FALSE(result.HasValue());
    EXPECT_FALSE(result.HasError());
}

TEST_F(TaskResultTest, SetAndGet_Int)
{
    TaskResult result;
    result.Set<int>(42);

    EXPECT_TRUE(result.HasValue());
    EXPECT_FALSE(result.HasError());
    EXPECT_EQ(42, result.Get<int>());
}

TEST_F(TaskResultTest, SetAndGet_String)
{
    TaskResult result;
    result.Set<std::string>("Hello");

    EXPECT_TRUE(result.HasValue());
    EXPECT_EQ("Hello", result.Get<std::string>());
}

TEST_F(TaskResultTest, SetAndGet_Float)
{
    TaskResult result;
    result.Set<float>(3.14f);

    EXPECT_TRUE(result.HasValue());
    EXPECT_FLOAT_EQ(3.14f, result.Get<float>());
}

TEST_F(TaskResultTest, TryGet_ValidType)
{
    TaskResult result;
    result.Set<int>(100);

    const int* ptr = result.TryGet<int>();
    ASSERT_NE(nullptr, ptr);
    EXPECT_EQ(100, *ptr);
}

TEST_F(TaskResultTest, TryGet_InvalidType)
{
    TaskResult result;
    result.Set<int>(100);

    const float* ptr = result.TryGet<float>();
    EXPECT_EQ(nullptr, ptr);
}

TEST_F(TaskResultTest, TryGet_NoValue)
{
    TaskResult result;

    const int* ptr = result.TryGet<int>();
    EXPECT_EQ(nullptr, ptr);
}

TEST_F(TaskResultTest, SetError)
{
    TaskResult result;
    result.SetError("Something went wrong");

    EXPECT_FALSE(result.HasValue());
    EXPECT_TRUE(result.HasError());
    EXPECT_EQ("Something went wrong", result.GetError());
}

TEST_F(TaskResultTest, Success_Factory)
{
    TaskResult result = TaskResult::Success();

    EXPECT_FALSE(result.HasValue());
    EXPECT_FALSE(result.HasError());
}

TEST_F(TaskResultTest, Failure_Factory)
{
    TaskResult result = TaskResult::Failure("Test error");

    EXPECT_FALSE(result.HasValue());
    EXPECT_TRUE(result.HasError());
    EXPECT_EQ("Test error", result.GetError());
}

// =============================================================================
// TaskGraph Tests
// =============================================================================

class TaskGraphTest : public ::testing::Test {
protected:
    void SetUp() override
    {
        // Initialize with 2 workers for testing (if not already initialized)
        if (!TaskGraph::Get().IsInitialized())
            TaskGraph::Get().Init(2);
    }

    void TearDown() override
    {
        // Just process any remaining callbacks, don't shutdown
        TaskGraph::Get().ProcessCompletedCallbacks();
    }
};

// =============================================================================
// Initialization Tests
// =============================================================================

TEST_F(TaskGraphTest, IsInitialized)
{
    EXPECT_TRUE(TaskGraph::Get().IsInitialized());
}

TEST_F(TaskGraphTest, GetWorkerCount)
{
    EXPECT_GE(TaskGraph::Get().GetWorkerCount(), 1u);
}

// =============================================================================
// Task Creation Tests
// =============================================================================

TEST_F(TaskGraphTest, CreateTask_ReturnsValidID)
{
    TaskID id = TaskGraph::Get().CreateTask("Test", []() -> TaskResult {
        return TaskResult::Success();
    });

    EXPECT_TRUE(id.IsValid());
}

TEST_F(TaskGraphTest, CreateTask_MultipleTasksHaveDifferentIDs)
{
    TaskID id1 = TaskGraph::Get().CreateTask("Task1", []() -> TaskResult {
        return TaskResult::Success();
    });

    TaskID id2 = TaskGraph::Get().CreateTask("Task2", []() -> TaskResult {
        return TaskResult::Success();
    });

    EXPECT_NE(id1, id2);
}

// =============================================================================
// Wait Tests
// =============================================================================

TEST_F(TaskGraphTest, Wait_BlocksUntilComplete)
{
    std::atomic<bool> executed{false};

    TaskID id = TaskGraph::Get().CreateTask("Test", [&executed]() -> TaskResult {
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
        executed = true;
        return TaskResult::Success();
    });

    TaskGraph::Get().Wait(id);

    EXPECT_TRUE(executed.load());
    EXPECT_TRUE(TaskGraph::Get().IsComplete(id));
}

TEST_F(TaskGraphTest, WaitAll_MultipleTasks)
{
    std::atomic<int> counter{0};

    std::vector<TaskID> tasks;
    for (int i = 0; i < 5; i++)
    {
        tasks.push_back(TaskGraph::Get().CreateTask("Task", [&counter]() -> TaskResult {
            counter++;
            return TaskResult::Success();
        }));
    }

    TaskGraph::Get().WaitAll(tasks);

    EXPECT_EQ(5, counter.load());

    for (const auto& id : tasks)
    {
        EXPECT_TRUE(TaskGraph::Get().IsComplete(id));
    }
}

// =============================================================================
// Task Result Tests
// =============================================================================

TEST_F(TaskGraphTest, TaskWithResult_Int)
{
    TaskID id = TaskGraph::Get().CreateTask<int>("Compute", []() -> int {
        return 42;
    });

    TaskGraph::Get().Wait(id);

    const TaskResult& result = TaskGraph::Get().GetResult(id);
    EXPECT_TRUE(result.HasValue());
    EXPECT_EQ(42, result.Get<int>());
}

TEST_F(TaskGraphTest, TaskWithResult_String)
{
    TaskID id = TaskGraph::Get().CreateTask<std::string>("Compute", []() -> std::string {
        return "Hello World";
    });

    TaskGraph::Get().Wait(id);

    const TaskResult& result = TaskGraph::Get().GetResult(id);
    EXPECT_TRUE(result.HasValue());
    EXPECT_EQ("Hello World", result.Get<std::string>());
}

// =============================================================================
// State Tests
// =============================================================================

TEST_F(TaskGraphTest, GetState_Completed)
{
    TaskID id = TaskGraph::Get().CreateTask("Test", []() -> TaskResult {
        return TaskResult::Success();
    });

    TaskGraph::Get().Wait(id);

    EXPECT_EQ(TaskState::Completed, TaskGraph::Get().GetState(id));
}

TEST_F(TaskGraphTest, IsComplete_ReturnsTrueAfterWait)
{
    TaskID id = TaskGraph::Get().CreateTask("Test", []() -> TaskResult {
        return TaskResult::Success();
    });

    TaskGraph::Get().Wait(id);

    EXPECT_TRUE(TaskGraph::Get().IsComplete(id));
}

TEST_F(TaskGraphTest, IsFailed_ReturnsTrueOnError)
{
    TaskID id = TaskGraph::Get().CreateTask("Test", []() -> TaskResult {
        return TaskResult::Failure("Test error");
    });

    TaskGraph::Get().Wait(id);

    EXPECT_TRUE(TaskGraph::Get().IsFailed(id));
    EXPECT_TRUE(TaskGraph::Get().GetResult(id).HasError());
}

// =============================================================================
// Dependency Tests
// =============================================================================

TEST_F(TaskGraphTest, Dependencies_ExecuteInOrder)
{
    std::atomic<int> order{0};
    int firstOrder = -1;
    int secondOrder = -1;

    TaskID first = TaskGraph::Get().CreateTask("First", [&]() -> TaskResult {
        firstOrder = order++;
        return TaskResult::Success();
    });

    TaskID second = TaskGraph::Get().CreateTask("Second", [&]() -> TaskResult {
        secondOrder = order++;
        return TaskResult::Success();
    }, { first });

    TaskGraph::Get().Wait(second);

    EXPECT_LT(firstOrder, secondOrder);
}

TEST_F(TaskGraphTest, Dependencies_MultipleDependencies)
{
    std::atomic<int> completedCount{0};
    std::atomic<bool> finalRan{false};

    TaskID dep1 = TaskGraph::Get().CreateTask("Dep1", [&]() -> TaskResult {
        completedCount++;
        return TaskResult::Success();
    });

    TaskID dep2 = TaskGraph::Get().CreateTask("Dep2", [&]() -> TaskResult {
        completedCount++;
        return TaskResult::Success();
    });

    TaskID dep3 = TaskGraph::Get().CreateTask("Dep3", [&]() -> TaskResult {
        completedCount++;
        return TaskResult::Success();
    });

    TaskID final = TaskGraph::Get().CreateTask("Final", [&]() -> TaskResult {
        // All dependencies should be complete
        EXPECT_EQ(3, completedCount.load());
        finalRan = true;
        return TaskResult::Success();
    }, { dep1, dep2, dep3 });

    TaskGraph::Get().Wait(final);

    EXPECT_TRUE(finalRan.load());
    EXPECT_EQ(3, completedCount.load());
}

TEST_F(TaskGraphTest, Dependencies_ChainedDependencies)
{
    std::vector<int> order;
    std::mutex orderMutex;

    TaskID t1 = TaskGraph::Get().CreateTask("T1", [&]() -> TaskResult {
        std::lock_guard<std::mutex> lock(orderMutex);
        order.push_back(1);
        return TaskResult::Success();
    });

    TaskID t2 = TaskGraph::Get().CreateTask("T2", [&]() -> TaskResult {
        std::lock_guard<std::mutex> lock(orderMutex);
        order.push_back(2);
        return TaskResult::Success();
    }, { t1 });

    TaskID t3 = TaskGraph::Get().CreateTask("T3", [&]() -> TaskResult {
        std::lock_guard<std::mutex> lock(orderMutex);
        order.push_back(3);
        return TaskResult::Success();
    }, { t2 });

    TaskGraph::Get().Wait(t3);

    ASSERT_EQ(3u, order.size());
    EXPECT_EQ(1, order[0]);
    EXPECT_EQ(2, order[1]);
    EXPECT_EQ(3, order[2]);
}

// =============================================================================
// Then (Continuation) Tests
// =============================================================================

TEST_F(TaskGraphTest, Then_ContinuationReceivesResult)
{
    TaskID producer = TaskGraph::Get().CreateTask<int>("Producer", []() -> int {
        return 21;
    });

    TaskID consumer = TaskGraph::Get().Then<int, int>(
        producer,
        "Consumer",
        [](const int& value) -> int { return value * 2; }
    );

    TaskGraph::Get().Wait(consumer);

    const TaskResult& result = TaskGraph::Get().GetResult(consumer);
    EXPECT_TRUE(result.HasValue());
    EXPECT_EQ(42, result.Get<int>());
}

TEST_F(TaskGraphTest, Then_ChainedContinuations)
{
    TaskID t1 = TaskGraph::Get().CreateTask<int>("T1", []() -> int {
        return 10;
    });

    TaskID t2 = TaskGraph::Get().Then<int, int>(t1, "T2", [](const int& v) -> int {
        return v + 5;
    });

    TaskID t3 = TaskGraph::Get().Then<int, int>(t2, "T3", [](const int& v) -> int {
        return v * 2;
    });

    TaskGraph::Get().Wait(t3);

    // (10 + 5) * 2 = 30
    EXPECT_EQ(30, TaskGraph::Get().GetResult(t3).Get<int>());
}

// =============================================================================
// Cancel Tests
// =============================================================================

TEST_F(TaskGraphTest, Cancel_PendingTask)
{
    // Create a task that waits
    std::atomic<bool> task1Started{false};
    std::atomic<bool> task2Ran{false};

    TaskID blocker = TaskGraph::Get().CreateTask("Blocker", [&]() -> TaskResult {
        task1Started = true;
        std::this_thread::sleep_for(std::chrono::milliseconds(100));
        return TaskResult::Success();
    });

    TaskID dependent = TaskGraph::Get().CreateTask("Dependent", [&]() -> TaskResult {
        task2Ran = true;
        return TaskResult::Success();
    }, { blocker });

    // Wait for blocker to start
    while (!task1Started.load())
        std::this_thread::yield();

    // Cancel the dependent task
    TaskGraph::Get().Cancel(dependent);

    TaskGraph::Get().Wait(blocker);

    // Give a moment for any errant execution
    std::this_thread::sleep_for(std::chrono::milliseconds(50));

    EXPECT_EQ(TaskState::Cancelled, TaskGraph::Get().GetState(dependent));
    EXPECT_FALSE(task2Ran.load());
}

// =============================================================================
// Statistics Tests
// =============================================================================

TEST_F(TaskGraphTest, Statistics_CountsAreReasonable)
{
    // Initially should have no tasks
    size_t initialPending = TaskGraph::Get().GetPendingTaskCount();
    size_t initialReady = TaskGraph::Get().GetReadyTaskCount();
    size_t initialRunning = TaskGraph::Get().GetRunningTaskCount();

    // These should be 0 or very small
    EXPECT_LE(initialPending + initialReady + initialRunning, 1u);

    // Create a task and wait for it
    TaskID id = TaskGraph::Get().CreateTask("Test", []() -> TaskResult {
        return TaskResult::Success();
    });

    TaskGraph::Get().Wait(id);

    // After completion, counts should be back to near zero
    size_t finalPending = TaskGraph::Get().GetPendingTaskCount();
    size_t finalReady = TaskGraph::Get().GetReadyTaskCount();
    size_t finalRunning = TaskGraph::Get().GetRunningTaskCount();

    EXPECT_LE(finalPending + finalReady + finalRunning, 1u);
}

// =============================================================================
// Stress Tests
// =============================================================================

TEST_F(TaskGraphTest, Stress_ManyIndependentTasks)
{
    std::atomic<int> counter{0};
    const int taskCount = 100;

    std::vector<TaskID> tasks;
    for (int i = 0; i < taskCount; i++)
    {
        tasks.push_back(TaskGraph::Get().CreateTask("Task", [&counter]() -> TaskResult {
            counter++;
            return TaskResult::Success();
        }));
    }

    TaskGraph::Get().WaitAll(tasks);

    EXPECT_EQ(taskCount, counter.load());
}

TEST_F(TaskGraphTest, Stress_DiamondDependency)
{
    // Diamond pattern:
    //       A
    //      / \
    //     B   C
    //      \ /
    //       D

    std::atomic<int> order{0};
    int aOrder = -1, bOrder = -1, cOrder = -1, dOrder = -1;

    TaskID a = TaskGraph::Get().CreateTask("A", [&]() -> TaskResult {
        aOrder = order++;
        return TaskResult::Success();
    });

    TaskID b = TaskGraph::Get().CreateTask("B", [&]() -> TaskResult {
        bOrder = order++;
        return TaskResult::Success();
    }, { a });

    TaskID c = TaskGraph::Get().CreateTask("C", [&]() -> TaskResult {
        cOrder = order++;
        return TaskResult::Success();
    }, { a });

    TaskID d = TaskGraph::Get().CreateTask("D", [&]() -> TaskResult {
        dOrder = order++;
        return TaskResult::Success();
    }, { b, c });

    TaskGraph::Get().Wait(d);

    // A must run before B and C
    EXPECT_LT(aOrder, bOrder);
    EXPECT_LT(aOrder, cOrder);

    // B and C must run before D
    EXPECT_LT(bOrder, dOrder);
    EXPECT_LT(cOrder, dOrder);
}
