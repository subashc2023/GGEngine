#include <gtest/gtest.h>
#include "GGEngine/Core/JobSystem.h"
#include "TestConfig.h"
#include <atomic>
#include <chrono>
#include <thread>

using namespace GGEngine;

class JobSystemTest : public ::testing::Test {
protected:
    void SetUp() override
    {
        // Initialize with 2 workers for testing (if not already initialized)
        if (!JobSystem::Get().IsInitialized())
            JobSystem::Get().Init(2);
    }

    void TearDown() override
    {
        // Wait for any pending jobs before next test
        WaitForJobs();
    }

    // Helper to wait for pending jobs with timeout
    bool WaitForJobs(std::chrono::milliseconds timeout = std::chrono::milliseconds(1000))
    {
        auto start = std::chrono::steady_clock::now();
        while (JobSystem::Get().GetPendingJobCount() > 0)
        {
            if (std::chrono::steady_clock::now() - start > timeout)
                return false;
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
        // Small additional delay for job completion
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
        return true;
    }
};

// =============================================================================
// Initialization Tests
// =============================================================================

TEST_F(JobSystemTest, IsInitialized)
{
    EXPECT_TRUE(JobSystem::Get().IsInitialized());
}

TEST_F(JobSystemTest, InitialPendingJobCount_IsZero)
{
    EXPECT_EQ(0u, JobSystem::Get().GetPendingJobCount());
}

// =============================================================================
// Job Submission Tests
// =============================================================================

TEST_F(JobSystemTest, Submit_ExecutesJob)
{
    std::atomic<bool> executed{false};

    JobSystem::Get().Submit([&executed]() {
        executed = true;
    });

    ASSERT_TRUE(WaitForJobs());
    EXPECT_TRUE(executed.load());
}

TEST_F(JobSystemTest, Submit_MultipleJobs)
{
    std::atomic<int> counter{0};
    const int jobCount = 50;

    for (int i = 0; i < jobCount; i++)
    {
        JobSystem::Get().Submit([&counter]() {
            counter++;
        });
    }

    ASSERT_TRUE(WaitForJobs());
    EXPECT_EQ(jobCount, counter.load());
}

TEST_F(JobSystemTest, Submit_JobsExecuteConcurrently)
{
    std::atomic<int> concurrentCount{0};
    std::atomic<int> maxConcurrent{0};
    std::mutex maxMutex;

    const int jobCount = 10;

    for (int i = 0; i < jobCount; i++)
    {
        JobSystem::Get().Submit([&]() {
            int current = ++concurrentCount;

            // Track max concurrent jobs
            {
                std::lock_guard<std::mutex> lock(maxMutex);
                if (current > maxConcurrent.load())
                    maxConcurrent.store(current);
            }

            // Simulate some work
            std::this_thread::sleep_for(std::chrono::milliseconds(20));

            --concurrentCount;
        });
    }

    ASSERT_TRUE(WaitForJobs(std::chrono::milliseconds(2000)));

    // With 2 workers, we should see at least 2 concurrent jobs at some point
    EXPECT_GE(maxConcurrent.load(), 2);
}

// =============================================================================
// Callback Tests
// =============================================================================

TEST_F(JobSystemTest, Submit_WithCallback)
{
    std::atomic<bool> callbackExecuted{false};

    JobSystem::Get().Submit(
        []() { /* job work */ },
        [&callbackExecuted]() { callbackExecuted = true; }
    );

    ASSERT_TRUE(WaitForJobs());

    // Process callbacks on "main thread"
    JobSystem::Get().ProcessCompletedCallbacks();

    EXPECT_TRUE(callbackExecuted.load());
}

TEST_F(JobSystemTest, Submit_CallbackReceivesAfterJobCompletes)
{
    std::atomic<int> jobValue{0};
    std::atomic<int> callbackValue{0};

    JobSystem::Get().Submit(
        [&jobValue]() {
            std::this_thread::sleep_for(std::chrono::milliseconds(10));
            jobValue = 42;
        },
        [&jobValue, &callbackValue]() {
            callbackValue = jobValue.load();
        }
    );

    ASSERT_TRUE(WaitForJobs());
    JobSystem::Get().ProcessCompletedCallbacks();

    EXPECT_EQ(42, callbackValue.load());
}

TEST_F(JobSystemTest, ProcessCompletedCallbacks_MultipleCallbacks)
{
    std::atomic<int> callbackCount{0};
    const int jobCount = 10;

    for (int i = 0; i < jobCount; i++)
    {
        JobSystem::Get().Submit(
            []() { /* work */ },
            [&callbackCount]() { callbackCount++; }
        );
    }

    ASSERT_TRUE(WaitForJobs());
    JobSystem::Get().ProcessCompletedCallbacks();

    EXPECT_EQ(jobCount, callbackCount.load());
}

// =============================================================================
// Priority Tests
// =============================================================================

TEST_F(JobSystemTest, Priority_HighPriorityJobsPreferred)
{
    // This test verifies that high priority jobs are generally processed before low priority
    // Note: Due to threading, this is probabilistic - we test with many jobs

    std::mutex orderMutex;
    std::vector<int> executionOrder;
    std::atomic<bool> startFlag{false};

    // First, queue up many jobs before they start executing
    // We use a latch pattern to delay execution

    // Submit low priority jobs first
    for (int i = 0; i < 5; i++)
    {
        JobSystem::Get().Submit(
            [&, id = i]() {
                while (!startFlag.load())
                    std::this_thread::yield();
                std::lock_guard<std::mutex> lock(orderMutex);
                executionOrder.push_back(id);  // Low priority: 0-4
            },
            nullptr,
            JobPriority::Low
        );
    }

    // Submit high priority jobs after
    for (int i = 0; i < 5; i++)
    {
        JobSystem::Get().Submit(
            [&, id = i + 100]() {
                while (!startFlag.load())
                    std::this_thread::yield();
                std::lock_guard<std::mutex> lock(orderMutex);
                executionOrder.push_back(id);  // High priority: 100-104
            },
            nullptr,
            JobPriority::High
        );
    }

    // Release all jobs
    startFlag = true;

    ASSERT_TRUE(WaitForJobs());

    // Count how many high priority jobs (100+) were in the first half
    int highInFirstHalf = 0;
    for (size_t i = 0; i < executionOrder.size() / 2 && i < executionOrder.size(); i++)
    {
        if (executionOrder[i] >= 100)
            highInFirstHalf++;
    }

    // High priority jobs should generally execute earlier
    // Due to threading, we just check that at least some high priority ran early
    EXPECT_GE(highInFirstHalf, 2) << "Expected more high priority jobs in first half";
}

// =============================================================================
// Stress Tests
// =============================================================================

TEST_F(JobSystemTest, Stress_ManySmallJobs)
{
    std::atomic<int> counter{0};
    const int jobCount = 500;

    for (int i = 0; i < jobCount; i++)
    {
        JobSystem::Get().Submit([&counter]() {
            counter++;
        });
    }

    ASSERT_TRUE(WaitForJobs(std::chrono::milliseconds(5000)));
    EXPECT_EQ(jobCount, counter.load());
}

TEST_F(JobSystemTest, Stress_JobsWithCallbacks)
{
    std::atomic<int> jobCounter{0};
    std::atomic<int> callbackCounter{0};
    const int jobCount = 100;

    for (int i = 0; i < jobCount; i++)
    {
        JobSystem::Get().Submit(
            [&jobCounter]() { jobCounter++; },
            [&callbackCounter]() { callbackCounter++; }
        );
    }

    ASSERT_TRUE(WaitForJobs(std::chrono::milliseconds(5000)));
    JobSystem::Get().ProcessCompletedCallbacks();

    EXPECT_EQ(jobCount, jobCounter.load());
    EXPECT_EQ(jobCount, callbackCounter.load());
}
