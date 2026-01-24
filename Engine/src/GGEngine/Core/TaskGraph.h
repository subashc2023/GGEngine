#pragma once

#include "Core.h"
#include "JobSystem.h"  // For JobPriority

#include <functional>
#include <queue>
#include <mutex>
#include <condition_variable>
#include <thread>
#include <atomic>
#include <any>
#include <string>
#include <vector>
#include <unordered_map>

namespace GGEngine {

    // =============================================================================
    // Task State
    // =============================================================================
    enum class TaskState : uint8_t
    {
        Pending,      // Waiting for dependencies to complete
        Ready,        // Dependencies satisfied, queued for execution
        Running,      // Currently executing on a worker thread
        Completed,    // Successfully finished
        Failed,       // Execution failed with error
        Cancelled     // Cancelled before execution
    };

    // =============================================================================
    // Task Identifier
    // =============================================================================
    struct TaskID
    {
        uint32_t Index = UINT32_MAX;
        uint32_t Generation = 0;

        bool IsValid() const { return Index != UINT32_MAX; }

        bool operator==(const TaskID& other) const
        {
            return Index == other.Index && Generation == other.Generation;
        }

        bool operator!=(const TaskID& other) const
        {
            return !(*this == other);
        }
    };

    // Hash function for TaskID (for use in unordered containers)
    struct TaskIDHash
    {
        size_t operator()(const TaskID& id) const
        {
            return std::hash<uint64_t>()(
                (static_cast<uint64_t>(id.Index) << 32) | id.Generation
            );
        }
    };

    // =============================================================================
    // Task Result
    // =============================================================================
    // Type-erased result container for passing data between tasks
    class GG_API TaskResult
    {
    public:
        TaskResult() = default;

        // Set a value result
        template<typename T>
        void Set(T value)
        {
            m_Value = std::move(value);
            m_HasValue = true;
        }

        // Get the value (throws if wrong type or no value)
        template<typename T>
        T Get() const
        {
            if (!m_HasValue)
                throw std::runtime_error("TaskResult has no value");
            return std::any_cast<T>(m_Value);
        }

        // Try to get value, returns nullptr if wrong type or no value
        template<typename T>
        const T* TryGet() const
        {
            if (!m_HasValue) return nullptr;
            return std::any_cast<T>(&m_Value);
        }

        bool HasValue() const { return m_HasValue; }
        bool HasError() const { return !m_Error.empty(); }
        const std::string& GetError() const { return m_Error; }

        void SetError(std::string error)
        {
            m_Error = std::move(error);
            m_HasValue = false;
        }

        // Create a successful empty result
        static TaskResult Success() { return TaskResult{}; }

        // Create a failed result
        static TaskResult Failure(std::string error)
        {
            TaskResult result;
            result.SetError(std::move(error));
            return result;
        }

    private:
        std::any m_Value;
        std::string m_Error;
        bool m_HasValue = false;
    };

    // =============================================================================
    // Task Specification
    // =============================================================================
    // Describes a task to be created
    struct TaskSpec
    {
        std::string Name;                                              // Debug name
        std::function<TaskResult()> Work;                              // The work to execute
        std::vector<TaskID> Dependencies;                              // Tasks that must complete first
        std::function<void(TaskID, const TaskResult&)> OnComplete;     // Main thread callback
        JobPriority Priority = JobPriority::Normal;
    };

    // =============================================================================
    // Task Graph
    // =============================================================================
    // Advanced job system with task dependencies, results, and error propagation.
    // Tasks can depend on other tasks and pass results between them.
    class GG_API TaskGraph
    {
    public:
        static TaskGraph& Get();

        // Initialize with number of worker threads
        // Default: hardware_concurrency - 1 (reserve one for main thread)
        void Init(uint32_t numWorkers = 0);

        // Shutdown the task graph, cancels pending tasks and waits for running tasks
        void Shutdown();

        // Check if initialized
        bool IsInitialized() const { return m_Initialized; }

        // Get number of worker threads
        uint32_t GetWorkerCount() const { return static_cast<uint32_t>(m_Workers.size()); }

        // -------------------------------------------------------------------------
        // Task Creation
        // -------------------------------------------------------------------------

        // Create a task from a TaskSpec
        TaskID CreateTask(const TaskSpec& spec);

        // Convenience: Create a simple task with a work function
        TaskID CreateTask(const std::string& name,
                          std::function<TaskResult()> work,
                          JobPriority priority = JobPriority::Normal);

        // Convenience: Create a task with dependencies
        TaskID CreateTask(const std::string& name,
                          std::function<TaskResult()> work,
                          std::vector<TaskID> dependencies,
                          JobPriority priority = JobPriority::Normal);

        // Convenience: Create a task that returns a value
        template<typename T>
        TaskID CreateTask(const std::string& name,
                          std::function<T()> work,
                          std::vector<TaskID> dependencies = {},
                          JobPriority priority = JobPriority::Normal)
        {
            TaskSpec spec;
            spec.Name = name;
            spec.Work = [work = std::move(work)]() -> TaskResult {
                TaskResult result;
                result.Set(work());
                return result;
            };
            spec.Dependencies = std::move(dependencies);
            spec.Priority = priority;
            return CreateTask(spec);
        }

        // Chain a continuation task that receives the result of a predecessor
        template<typename TIn, typename TOut>
        TaskID Then(TaskID predecessor,
                    const std::string& name,
                    std::function<TOut(const TIn&)> continuation,
                    JobPriority priority = JobPriority::Normal)
        {
            TaskSpec spec;
            spec.Name = name;
            spec.Dependencies = { predecessor };
            spec.Priority = priority;

            // Capture 'this' to access GetResult
            spec.Work = [this, predecessor, continuation = std::move(continuation)]() -> TaskResult {
                const TaskResult& predResult = GetResult(predecessor);
                if (predResult.HasError()) {
                    return TaskResult::Failure("Predecessor failed: " + predResult.GetError());
                }

                const TIn* inputPtr = predResult.TryGet<TIn>();
                if (!inputPtr) {
                    return TaskResult::Failure("Type mismatch: predecessor result is not expected type");
                }

                TaskResult result;
                result.Set(continuation(*inputPtr));
                return result;
            };

            return CreateTask(spec);
        }

        // Chain a void continuation (no result passing)
        TaskID Then(TaskID predecessor,
                    const std::string& name,
                    std::function<void()> continuation,
                    JobPriority priority = JobPriority::Normal);

        // -------------------------------------------------------------------------
        // Task Queries
        // -------------------------------------------------------------------------

        // Wait for a task to complete (blocking)
        // Returns false if task was cancelled or doesn't exist
        bool Wait(TaskID task);

        // Wait for multiple tasks to complete
        void WaitAll(const std::vector<TaskID>& tasks);

        // Check if task is complete (non-blocking)
        bool IsComplete(TaskID task) const;

        // Check if task failed
        bool IsFailed(TaskID task) const;

        // Get task state
        TaskState GetState(TaskID task) const;

        // Get task result (only valid after completion)
        // Returns empty result if task doesn't exist or isn't complete
        const TaskResult& GetResult(TaskID task) const;

        // -------------------------------------------------------------------------
        // Task Control
        // -------------------------------------------------------------------------

        // Cancel a pending task (no effect if already running/complete)
        // Also cancels all tasks that depend on this one
        void Cancel(TaskID task);

        // -------------------------------------------------------------------------
        // Main Thread Processing
        // -------------------------------------------------------------------------

        // Process completed task callbacks on main thread
        // Call this from the main loop each frame
        void ProcessCompletedCallbacks();

        // -------------------------------------------------------------------------
        // Statistics
        // -------------------------------------------------------------------------

        // Get number of tasks in various states (approximate)
        size_t GetPendingTaskCount() const { return m_PendingCount.load(std::memory_order_relaxed); }
        size_t GetReadyTaskCount() const { return m_ReadyCount.load(std::memory_order_relaxed); }
        size_t GetRunningTaskCount() const { return m_RunningCount.load(std::memory_order_relaxed); }

    private:
        TaskGraph() : m_ReadyQueue(TaskPriorityComparator(this)) {}
        ~TaskGraph() = default;
        TaskGraph(const TaskGraph&) = delete;
        TaskGraph& operator=(const TaskGraph&) = delete;

        // Internal task data
        struct TaskData
        {
            TaskSpec Spec;
            std::atomic<TaskState> State{TaskState::Pending};
            TaskResult Result;
            std::atomic<uint32_t> UnmetDependencies{0};
            std::vector<TaskID> Dependents;  // Tasks waiting on this one
            uint32_t Generation = 0;

            // For wait support
            mutable std::mutex WaitMutex;
            mutable std::condition_variable WaitCondition;

            // Non-copyable due to atomics and mutex
            TaskData() = default;
            ~TaskData() = default;
            TaskData(const TaskData&) = delete;
            TaskData& operator=(const TaskData&) = delete;
            TaskData(TaskData&&) = delete;
            TaskData& operator=(TaskData&&) = delete;
        };

        // Priority comparator for ready queue
        struct TaskPriorityComparator
        {
            TaskPriorityComparator() = default;
            explicit TaskPriorityComparator(const TaskGraph* graph) : Graph(graph) {}
            bool operator()(const TaskID& a, const TaskID& b) const;
            const TaskGraph* Graph = nullptr;
        };

        // Worker thread function
        void WorkerLoop();

        // Called when a task completes - resolves dependencies
        void OnTaskCompleted(TaskID id, TaskResult result);

        // Propagate failure to dependent tasks
        void PropagateFailure(TaskID id, const std::string& error);

        // Move task to ready queue if all dependencies met
        void TryMakeReady(TaskID id);

        // Check if a TaskID is valid and matches current generation (takes lock)
        bool IsValidTask(TaskID id) const;

        // Internal version (must be called with m_TaskMutex held)
        bool IsValidTaskInternal(TaskID id) const;

        // Get task data (takes lock)
        TaskData* GetTaskData(TaskID id);
        const TaskData* GetTaskData(TaskID id) const;

        // Internal versions (must be called with m_TaskMutex held)
        TaskData* GetTaskDataInternal(TaskID id);
        const TaskData* GetTaskDataInternal(TaskID id) const;

        // Task storage with generation-based recycling
        // Using unique_ptr because TaskData is non-copyable/non-movable
        std::vector<std::unique_ptr<TaskData>> m_Tasks;
        std::queue<uint32_t> m_FreeIndices;
        mutable std::mutex m_TaskMutex;

        // Ready queue (tasks with all dependencies met)
        std::priority_queue<TaskID, std::vector<TaskID>, TaskPriorityComparator> m_ReadyQueue;
        mutable std::mutex m_ReadyMutex;
        std::condition_variable m_ReadyCondition;

        // Worker threads
        std::vector<std::thread> m_Workers;
        std::atomic<bool> m_Shutdown{false};

        // Completed callbacks for main thread
        struct CompletedCallback
        {
            TaskID Task;
            TaskResult Result;
            std::function<void(TaskID, const TaskResult&)> Callback;
        };
        std::queue<CompletedCallback> m_CompletedCallbacks;
        mutable std::mutex m_CallbackMutex;

        // Statistics
        std::atomic<size_t> m_PendingCount{0};
        std::atomic<size_t> m_ReadyCount{0};
        std::atomic<size_t> m_RunningCount{0};

        bool m_Initialized = false;

        // Empty result for invalid queries
        static const TaskResult s_EmptyResult;
    };

}
