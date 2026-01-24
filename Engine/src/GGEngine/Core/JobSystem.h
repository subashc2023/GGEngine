#pragma once

#include "Core.h"

#include <functional>
#include <queue>
#include <mutex>
#include <condition_variable>
#include <thread>
#include <atomic>

namespace GGEngine {

    // Priority levels for jobs
    enum class JobPriority : uint8_t
    {
        Low = 0,
        Normal = 1,
        High = 2
    };

    // Lightweight job system with a single worker thread for async asset loading.
    // Jobs are executed on the worker thread, callbacks are queued for main thread.
    class GG_API JobSystem
    {
    public:
        using Job = std::function<void()>;
        using Callback = std::function<void()>;

        static JobSystem& Get();

        // Initialize the job system with specified number of worker threads (default: 1)
        void Init(uint32_t numWorkers = 1);

        // Shutdown the job system, waits for pending jobs to complete
        void Shutdown();

        // Check if the job system is initialized
        bool IsInitialized() const { return m_Initialized; }

        // Submit a job to be executed on a worker thread
        // If callback is provided, it will be queued for main thread after job completes
        void Submit(Job job, Callback onComplete = nullptr, JobPriority priority = JobPriority::Normal);

        // Process completed job callbacks on the main thread
        // Call this from the main loop
        void ProcessCompletedCallbacks();

        // Get number of pending jobs (approximate, not synchronized)
        size_t GetPendingJobCount() const { return m_PendingJobCount.load(); }

        // Get number of pending callbacks waiting for main thread
        size_t GetPendingCallbackCount() const;

    private:
        JobSystem() = default;
        ~JobSystem() = default;
        JobSystem(const JobSystem&) = delete;
        JobSystem& operator=(const JobSystem&) = delete;

        // Worker thread function
        void WorkerLoop();

        struct JobEntry
        {
            Job job;
            Callback callback;
            JobPriority priority;

            // For priority queue ordering (higher priority first)
            bool operator<(const JobEntry& other) const
            {
                return priority < other.priority;
            }
        };

        std::vector<std::thread> m_Workers;
        std::priority_queue<JobEntry> m_JobQueue;
        std::mutex m_JobMutex;
        std::condition_variable m_JobCondition;

        std::queue<Callback> m_CompletedCallbacks;
        std::mutex m_CallbackMutex;

        std::atomic<bool> m_Shutdown{false};
        std::atomic<size_t> m_PendingJobCount{0};
        bool m_Initialized = false;
    };

}
