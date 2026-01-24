#include "ggpch.h"
#include "JobSystem.h"

namespace GGEngine {

    JobSystem& JobSystem::Get()
    {
        static JobSystem instance;
        return instance;
    }

    void JobSystem::Init(uint32_t numWorkers)
    {
        if (m_Initialized)
        {
            GG_CORE_WARN("JobSystem::Init called when already initialized");
            return;
        }

        m_Shutdown = false;
        m_PendingJobCount = 0;

        // Create worker threads
        uint32_t workerCount = std::max(1u, numWorkers);
        m_Workers.reserve(workerCount);

        for (uint32_t i = 0; i < workerCount; i++)
        {
            m_Workers.emplace_back(&JobSystem::WorkerLoop, this);
        }

        m_Initialized = true;
        GG_CORE_INFO("JobSystem initialized with {} worker thread(s)", workerCount);
    }

    void JobSystem::Shutdown()
    {
        if (!m_Initialized)
            return;

        // Signal shutdown
        {
            std::lock_guard<std::mutex> lock(m_JobMutex);
            m_Shutdown = true;
        }
        m_JobCondition.notify_all();

        // Wait for all workers to finish
        for (auto& worker : m_Workers)
        {
            if (worker.joinable())
                worker.join();
        }
        m_Workers.clear();

        // Clear any remaining jobs
        {
            std::lock_guard<std::mutex> lock(m_JobMutex);
            while (!m_JobQueue.empty())
                m_JobQueue.pop();
        }

        // Process any remaining callbacks
        ProcessCompletedCallbacks();

        m_Initialized = false;
        GG_CORE_TRACE("JobSystem shutdown complete");
    }

    void JobSystem::Submit(Job job, Callback onComplete, JobPriority priority)
    {
        if (!m_Initialized)
        {
            GG_CORE_ERROR("JobSystem::Submit called before Init()");
            return;
        }

        if (!job)
        {
            GG_CORE_WARN("JobSystem::Submit called with null job");
            return;
        }

        {
            std::lock_guard<std::mutex> lock(m_JobMutex);
            m_JobQueue.push(JobEntry{std::move(job), std::move(onComplete), priority});
            m_PendingJobCount++;
        }
        m_JobCondition.notify_one();
    }

    void JobSystem::ProcessCompletedCallbacks()
    {
        // Swap out the callback queue to minimize lock time
        std::queue<Callback> callbacksToProcess;
        {
            std::lock_guard<std::mutex> lock(m_CallbackMutex);
            std::swap(callbacksToProcess, m_CompletedCallbacks);
        }

        // Execute callbacks on main thread
        while (!callbacksToProcess.empty())
        {
            auto& callback = callbacksToProcess.front();
            if (callback)
            {
                callback();
            }
            callbacksToProcess.pop();
        }
    }

    size_t JobSystem::GetPendingCallbackCount() const
    {
        std::lock_guard<std::mutex> lock(const_cast<std::mutex&>(m_CallbackMutex));
        return m_CompletedCallbacks.size();
    }

    void JobSystem::WorkerLoop()
    {
        while (true)
        {
            JobEntry entry;

            // Wait for a job
            {
                std::unique_lock<std::mutex> lock(m_JobMutex);
                m_JobCondition.wait(lock, [this]() {
                    return m_Shutdown || !m_JobQueue.empty();
                });

                if (m_Shutdown && m_JobQueue.empty())
                    return;

                entry = std::move(const_cast<JobEntry&>(m_JobQueue.top()));
                m_JobQueue.pop();
            }

            // Execute the job
            if (entry.job)
            {
                entry.job();
            }
            m_PendingJobCount--;

            // Queue callback for main thread if provided
            if (entry.callback)
            {
                std::lock_guard<std::mutex> lock(m_CallbackMutex);
                m_CompletedCallbacks.push(std::move(entry.callback));
            }
        }
    }

}
