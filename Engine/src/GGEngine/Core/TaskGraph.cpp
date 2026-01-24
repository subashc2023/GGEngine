#include "ggpch.h"
#include "TaskGraph.h"

namespace GGEngine {

    // Static empty result for invalid queries
    const TaskResult TaskGraph::s_EmptyResult;

    TaskGraph& TaskGraph::Get()
    {
        static TaskGraph instance;
        return instance;
    }

    void TaskGraph::Init(uint32_t numWorkers)
    {
        if (m_Initialized)
        {
            GG_CORE_WARN("TaskGraph::Init called when already initialized");
            return;
        }

        m_Shutdown = false;
        m_PendingCount = 0;
        m_ReadyCount = 0;
        m_RunningCount = 0;

        // Default to hardware_concurrency - 1 (reserve one for main thread)
        uint32_t workerCount = numWorkers;
        if (workerCount == 0)
        {
            workerCount = std::max(1u, std::thread::hardware_concurrency() - 1);
        }

        // Reserve some initial capacity for tasks
        m_Tasks.reserve(256);

        // Create worker threads
        m_Workers.reserve(workerCount);
        for (uint32_t i = 0; i < workerCount; i++)
        {
            m_Workers.emplace_back(&TaskGraph::WorkerLoop, this);
        }

        m_Initialized = true;
        GG_CORE_INFO("TaskGraph initialized with {} worker thread(s)", workerCount);
    }

    void TaskGraph::Shutdown()
    {
        if (!m_Initialized)
            return;

        GG_CORE_TRACE("TaskGraph shutting down...");

        // Signal shutdown
        {
            std::lock_guard<std::mutex> lock(m_ReadyMutex);
            m_Shutdown = true;
        }
        m_ReadyCondition.notify_all();

        // Also wake any tasks waiting
        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            for (auto& taskPtr : m_Tasks)
            {
                if (taskPtr)
                {
                    taskPtr->WaitCondition.notify_all();
                }
            }
        }

        // Wait for all workers to finish
        for (auto& worker : m_Workers)
        {
            if (worker.joinable())
                worker.join();
        }
        m_Workers.clear();

        // Process any remaining callbacks
        ProcessCompletedCallbacks();

        // Clear task storage
        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            m_Tasks.clear();
            while (!m_FreeIndices.empty())
                m_FreeIndices.pop();
        }

        // Clear ready queue
        {
            std::lock_guard<std::mutex> lock(m_ReadyMutex);
            while (!m_ReadyQueue.empty())
                m_ReadyQueue.pop();
        }

        m_Initialized = false;
        GG_CORE_TRACE("TaskGraph shutdown complete");
    }

    TaskID TaskGraph::CreateTask(const TaskSpec& spec)
    {
        if (!m_Initialized)
        {
            GG_CORE_ERROR("TaskGraph::CreateTask called before Init()");
            return TaskID{};
        }

        if (!spec.Work)
        {
            GG_CORE_WARN("TaskGraph::CreateTask called with null work function");
            return TaskID{};
        }

        TaskID id;

        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);

            // Get or create a slot
            if (!m_FreeIndices.empty())
            {
                id.Index = m_FreeIndices.front();
                m_FreeIndices.pop();
                id.Generation = m_Tasks[id.Index]->Generation + 1;
                // Reset the task data
                m_Tasks[id.Index] = std::make_unique<TaskData>();
                m_Tasks[id.Index]->Generation = id.Generation;
            }
            else
            {
                id.Index = static_cast<uint32_t>(m_Tasks.size());
                id.Generation = 1;
                m_Tasks.emplace_back(std::make_unique<TaskData>());
                m_Tasks.back()->Generation = id.Generation;
            }

            TaskData& task = *m_Tasks[id.Index];
            task.Spec = spec;

            // Count unmet dependencies (only valid ones)
            uint32_t unmetDeps = 0;
            for (const TaskID& depId : spec.Dependencies)
            {
                if (IsValidTaskInternal(depId))
                {
                    TaskData* depTask = GetTaskDataInternal(depId);
                    if (depTask)
                    {
                        TaskState depState = depTask->State.load(std::memory_order_acquire);
                        if (depState == TaskState::Completed)
                        {
                            // Already done, don't count
                        }
                        else if (depState == TaskState::Failed || depState == TaskState::Cancelled)
                        {
                            // Dependency failed - mark this task as failed too
                            task.State = TaskState::Failed;
                            task.Result.SetError("Dependency task failed or was cancelled");
                            return id;
                        }
                        else
                        {
                            // Dependency is pending/ready/running - register as dependent
                            depTask->Dependents.push_back(id);
                            unmetDeps++;
                        }
                    }
                }
            }

            task.UnmetDependencies = unmetDeps;

            if (unmetDeps == 0)
            {
                // No dependencies or all completed - mark as ready
                task.State = TaskState::Ready;
                m_ReadyCount.fetch_add(1, std::memory_order_relaxed);
            }
            else
            {
                task.State = TaskState::Pending;
                m_PendingCount.fetch_add(1, std::memory_order_relaxed);
            }
        }

        // If ready, add to ready queue
        TaskState state;
        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            state = m_Tasks[id.Index]->State.load(std::memory_order_acquire);
        }

        if (state == TaskState::Ready)
        {
            std::lock_guard<std::mutex> lock(m_ReadyMutex);
            m_ReadyQueue.push(id);
            m_ReadyCondition.notify_one();
        }

        return id;
    }

    TaskID TaskGraph::CreateTask(const std::string& name,
                                  std::function<TaskResult()> work,
                                  JobPriority priority)
    {
        TaskSpec spec;
        spec.Name = name;
        spec.Work = std::move(work);
        spec.Priority = priority;
        return CreateTask(spec);
    }

    TaskID TaskGraph::CreateTask(const std::string& name,
                                  std::function<TaskResult()> work,
                                  std::vector<TaskID> dependencies,
                                  JobPriority priority)
    {
        TaskSpec spec;
        spec.Name = name;
        spec.Work = std::move(work);
        spec.Dependencies = std::move(dependencies);
        spec.Priority = priority;
        return CreateTask(spec);
    }

    TaskID TaskGraph::Then(TaskID predecessor,
                           const std::string& name,
                           std::function<void()> continuation,
                           JobPriority priority)
    {
        TaskSpec spec;
        spec.Name = name;
        spec.Dependencies = { predecessor };
        spec.Priority = priority;
        spec.Work = [continuation = std::move(continuation)]() -> TaskResult {
            continuation();
            return TaskResult::Success();
        };
        return CreateTask(spec);
    }

    bool TaskGraph::Wait(TaskID task)
    {
        if (!IsValidTask(task))
            return false;

        TaskData* data = nullptr;
        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            data = GetTaskDataInternal(task);
            if (!data) return false;
        }

        std::unique_lock<std::mutex> waitLock(data->WaitMutex);
        data->WaitCondition.wait(waitLock, [this, task, data]() {
            TaskState state = data->State.load(std::memory_order_acquire);
            return state == TaskState::Completed ||
                   state == TaskState::Failed ||
                   state == TaskState::Cancelled ||
                   m_Shutdown;
        });

        TaskState finalState = data->State.load(std::memory_order_acquire);
        return finalState == TaskState::Completed;
    }

    void TaskGraph::WaitAll(const std::vector<TaskID>& tasks)
    {
        for (const TaskID& task : tasks)
        {
            Wait(task);
        }
    }

    bool TaskGraph::IsComplete(TaskID task) const
    {
        return GetState(task) == TaskState::Completed;
    }

    bool TaskGraph::IsFailed(TaskID task) const
    {
        TaskState state = GetState(task);
        return state == TaskState::Failed || state == TaskState::Cancelled;
    }

    TaskState TaskGraph::GetState(TaskID task) const
    {
        std::lock_guard<std::mutex> lock(m_TaskMutex);
        const TaskData* data = GetTaskDataInternal(task);
        if (!data) return TaskState::Failed;
        return data->State.load(std::memory_order_acquire);
    }

    const TaskResult& TaskGraph::GetResult(TaskID task) const
    {
        std::lock_guard<std::mutex> lock(m_TaskMutex);
        const TaskData* data = GetTaskDataInternal(task);
        if (!data) return s_EmptyResult;
        return data->Result;
    }

    void TaskGraph::Cancel(TaskID task)
    {
        if (!IsValidTask(task))
            return;

        std::vector<TaskID> toCancelDependents;

        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            TaskData* data = GetTaskDataInternal(task);
            if (!data) return;

            TaskState state = data->State.load(std::memory_order_acquire);

            // Can only cancel pending or ready tasks
            if (state != TaskState::Pending && state != TaskState::Ready)
                return;

            // Update stats
            if (state == TaskState::Pending)
                m_PendingCount.fetch_sub(1, std::memory_order_relaxed);
            else if (state == TaskState::Ready)
                m_ReadyCount.fetch_sub(1, std::memory_order_relaxed);

            data->State.store(TaskState::Cancelled, std::memory_order_release);
            data->Result.SetError("Task was cancelled");

            // Copy dependents for cascade cancellation
            toCancelDependents = data->Dependents;
        }

        // Wake any waiters
        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            TaskData* data = GetTaskDataInternal(task);
            if (data)
            {
                std::lock_guard<std::mutex> waitLock(data->WaitMutex);
                data->WaitCondition.notify_all();
            }
        }

        // Cascade cancel to dependents
        for (const TaskID& depId : toCancelDependents)
        {
            Cancel(depId);
        }
    }

    void TaskGraph::ProcessCompletedCallbacks()
    {
        // Swap out the callback queue to minimize lock time
        std::queue<CompletedCallback> callbacksToProcess;
        {
            std::lock_guard<std::mutex> lock(m_CallbackMutex);
            std::swap(callbacksToProcess, m_CompletedCallbacks);
        }

        // Execute callbacks on main thread
        while (!callbacksToProcess.empty())
        {
            auto& cc = callbacksToProcess.front();
            if (cc.Callback)
            {
                cc.Callback(cc.Task, cc.Result);
            }
            callbacksToProcess.pop();
        }
    }

    void TaskGraph::WorkerLoop()
    {
        while (true)
        {
            TaskID taskId;

            // Wait for a ready task
            {
                std::unique_lock<std::mutex> lock(m_ReadyMutex);
                m_ReadyCondition.wait(lock, [this]() {
                    return m_Shutdown || !m_ReadyQueue.empty();
                });

                if (m_Shutdown && m_ReadyQueue.empty())
                    return;

                taskId = m_ReadyQueue.top();
                m_ReadyQueue.pop();
            }

            m_ReadyCount.fetch_sub(1, std::memory_order_relaxed);
            m_RunningCount.fetch_add(1, std::memory_order_relaxed);

            // Get task data and mark as running
            TaskSpec spec;
            {
                std::lock_guard<std::mutex> lock(m_TaskMutex);
                TaskData* data = GetTaskDataInternal(taskId);
                if (!data || data->State.load(std::memory_order_acquire) != TaskState::Ready)
                {
                    m_RunningCount.fetch_sub(1, std::memory_order_relaxed);
                    continue;
                }

                data->State.store(TaskState::Running, std::memory_order_release);
                spec = data->Spec;  // Copy spec since we'll release the lock
            }

            // Execute the task
            TaskResult result;
            try
            {
                if (spec.Work)
                {
                    result = spec.Work();
                }
            }
            catch (const std::exception& e)
            {
                result.SetError(std::string("Exception: ") + e.what());
            }
            catch (...)
            {
                result.SetError("Unknown exception");
            }

            m_RunningCount.fetch_sub(1, std::memory_order_relaxed);

            // Complete the task
            OnTaskCompleted(taskId, std::move(result));
        }
    }

    void TaskGraph::OnTaskCompleted(TaskID id, TaskResult result)
    {
        std::vector<TaskID> dependentsToCheck;
        std::function<void(TaskID, const TaskResult&)> callback;
        bool failed = result.HasError();

        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            TaskData* data = GetTaskDataInternal(id);
            if (!data) return;

            data->Result = std::move(result);
            data->State.store(failed ? TaskState::Failed : TaskState::Completed, std::memory_order_release);

            // Copy data we need after releasing lock
            dependentsToCheck = data->Dependents;
            callback = data->Spec.OnComplete;

            // Wake any waiters
            {
                std::lock_guard<std::mutex> waitLock(data->WaitMutex);
                data->WaitCondition.notify_all();
            }
        }

        // Queue callback for main thread if provided
        if (callback)
        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            TaskData* data = GetTaskDataInternal(id);
            if (data)
            {
                std::lock_guard<std::mutex> cbLock(m_CallbackMutex);
                m_CompletedCallbacks.push({id, data->Result, callback});
            }
        }

        // Handle dependents
        if (failed)
        {
            // Propagate failure
            for (const TaskID& depId : dependentsToCheck)
            {
                PropagateFailure(depId, "Dependency failed");
            }
        }
        else
        {
            // Check if dependents can now run
            for (const TaskID& depId : dependentsToCheck)
            {
                TryMakeReady(depId);
            }
        }
    }

    void TaskGraph::PropagateFailure(TaskID id, const std::string& error)
    {
        std::vector<TaskID> dependentsToFail;

        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            TaskData* data = GetTaskDataInternal(id);
            if (!data) return;

            TaskState state = data->State.load(std::memory_order_acquire);
            if (state != TaskState::Pending && state != TaskState::Ready)
                return;

            // Update stats
            if (state == TaskState::Pending)
                m_PendingCount.fetch_sub(1, std::memory_order_relaxed);
            else if (state == TaskState::Ready)
                m_ReadyCount.fetch_sub(1, std::memory_order_relaxed);

            data->State.store(TaskState::Failed, std::memory_order_release);
            data->Result.SetError(error);

            dependentsToFail = data->Dependents;

            // Wake any waiters
            {
                std::lock_guard<std::mutex> waitLock(data->WaitMutex);
                data->WaitCondition.notify_all();
            }
        }

        // Cascade failure
        for (const TaskID& depId : dependentsToFail)
        {
            PropagateFailure(depId, "Dependency failed");
        }
    }

    void TaskGraph::TryMakeReady(TaskID id)
    {
        bool shouldQueue = false;

        {
            std::lock_guard<std::mutex> lock(m_TaskMutex);
            TaskData* data = GetTaskDataInternal(id);
            if (!data) return;

            TaskState state = data->State.load(std::memory_order_acquire);
            if (state != TaskState::Pending)
                return;

            uint32_t remaining = data->UnmetDependencies.fetch_sub(1, std::memory_order_acq_rel);
            if (remaining == 1)  // Was 1, now 0
            {
                data->State.store(TaskState::Ready, std::memory_order_release);
                m_PendingCount.fetch_sub(1, std::memory_order_relaxed);
                m_ReadyCount.fetch_add(1, std::memory_order_relaxed);
                shouldQueue = true;
            }
        }

        if (shouldQueue)
        {
            std::lock_guard<std::mutex> lock(m_ReadyMutex);
            m_ReadyQueue.push(id);
            m_ReadyCondition.notify_one();
        }
    }

    bool TaskGraph::IsValidTask(TaskID id) const
    {
        if (!id.IsValid()) return false;

        std::lock_guard<std::mutex> lock(m_TaskMutex);
        return IsValidTaskInternal(id);
    }

    bool TaskGraph::IsValidTaskInternal(TaskID id) const
    {
        if (!id.IsValid()) return false;
        if (id.Index >= m_Tasks.size()) return false;
        if (!m_Tasks[id.Index]) return false;
        return m_Tasks[id.Index]->Generation == id.Generation;
    }

    TaskGraph::TaskData* TaskGraph::GetTaskData(TaskID id)
    {
        std::lock_guard<std::mutex> lock(m_TaskMutex);
        return GetTaskDataInternal(id);
    }

    const TaskGraph::TaskData* TaskGraph::GetTaskData(TaskID id) const
    {
        std::lock_guard<std::mutex> lock(m_TaskMutex);
        return GetTaskDataInternal(id);
    }

    TaskGraph::TaskData* TaskGraph::GetTaskDataInternal(TaskID id)
    {
        if (!id.IsValid()) return nullptr;
        if (id.Index >= m_Tasks.size()) return nullptr;
        if (!m_Tasks[id.Index]) return nullptr;
        if (m_Tasks[id.Index]->Generation != id.Generation) return nullptr;
        return m_Tasks[id.Index].get();
    }

    const TaskGraph::TaskData* TaskGraph::GetTaskDataInternal(TaskID id) const
    {
        if (!id.IsValid()) return nullptr;
        if (id.Index >= m_Tasks.size()) return nullptr;
        if (!m_Tasks[id.Index]) return nullptr;
        if (m_Tasks[id.Index]->Generation != id.Generation) return nullptr;
        return m_Tasks[id.Index].get();
    }

    bool TaskGraph::TaskPriorityComparator::operator()(const TaskID& a, const TaskID& b) const
    {
        if (!Graph) return false;

        // We need to access task data, but we're called from within locked context
        // so we can't take the lock again. Access directly.
        if (a.Index >= Graph->m_Tasks.size() || b.Index >= Graph->m_Tasks.size())
            return false;

        const auto& taskAPtr = Graph->m_Tasks[a.Index];
        const auto& taskBPtr = Graph->m_Tasks[b.Index];

        if (!taskAPtr || !taskBPtr)
            return false;

        // Lower value means lower priority in priority_queue (max-heap)
        // So we want higher priority to return false (be at top)
        return static_cast<uint8_t>(taskAPtr->Spec.Priority) < static_cast<uint8_t>(taskBPtr->Spec.Priority);
    }

}
