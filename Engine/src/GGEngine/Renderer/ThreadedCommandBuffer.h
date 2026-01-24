#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"

#include <vulkan/vulkan.h>
#include <vector>
#include <mutex>
#include <unordered_map>
#include <thread>

namespace GGEngine {

    // Maximum frames in flight (must match VulkanContext)
    static constexpr uint32_t MaxFramesInFlight = 2;

    // =============================================================================
    // ThreadedCommandBuffer
    // =============================================================================
    // Manages thread-local command pools and secondary command buffers for
    // parallel command recording. Each worker thread gets its own command pool
    // to avoid synchronization overhead.
    //
    // Usage:
    //   1. Call Init() with the number of worker threads
    //   2. On worker threads, call AllocateSecondary() to get a command buffer
    //   3. Record commands with BeginSecondary/EndSecondary
    //   4. Call SubmitSecondary() to queue for execution
    //   5. On main thread, call ExecuteSecondaries() to execute all queued buffers
    //   6. Call ResetPools() at frame start to reset all command pools
    //
    class GG_API ThreadedCommandBuffer
    {
    public:
        static ThreadedCommandBuffer& Get();

        // Initialize with the number of worker threads
        void Init(uint32_t workerCount);
        void Shutdown();

        bool IsInitialized() const { return m_Initialized; }
        uint32_t GetWorkerCount() const { return m_WorkerCount; }

        // -------------------------------------------------------------------------
        // Thread-Local Operations (safe to call from any worker thread)
        // -------------------------------------------------------------------------

        // Allocate a secondary command buffer for the current thread
        // Thread-safe: uses thread-local command pool
        VkCommandBuffer AllocateSecondary(uint32_t frameIndex);

        // Begin recording to a secondary command buffer
        // Must specify the render pass and framebuffer for inheritance
        void BeginSecondary(VkCommandBuffer cmd,
                           VkRenderPass renderPass,
                           VkFramebuffer framebuffer,
                           uint32_t subpass = 0);

        // End recording to a secondary command buffer
        void EndSecondary(VkCommandBuffer cmd);

        // Submit a recorded secondary buffer for execution
        // Thread-safe: adds to pending queue
        void SubmitSecondary(VkCommandBuffer secondary, uint32_t frameIndex);

        // -------------------------------------------------------------------------
        // Main Thread Operations
        // -------------------------------------------------------------------------

        // Execute all submitted secondary buffers into the primary command buffer
        // Called from main thread after all workers complete their recording
        void ExecuteSecondaries(VkCommandBuffer primaryCmd, uint32_t frameIndex);

        // Reset all command pools for a frame (called at frame start)
        void ResetPools(uint32_t frameIndex);

        // Get the number of pending secondary buffers for a frame
        size_t GetPendingCount(uint32_t frameIndex) const;

    private:
        ThreadedCommandBuffer() = default;
        ~ThreadedCommandBuffer() = default;
        ThreadedCommandBuffer(const ThreadedCommandBuffer&) = delete;
        ThreadedCommandBuffer& operator=(const ThreadedCommandBuffer&) = delete;

        // Per-thread data for command buffer management
        struct ThreadData
        {
            VkCommandPool CommandPool = VK_NULL_HANDLE;
            std::vector<VkCommandBuffer> SecondaryBuffers[MaxFramesInFlight];
            uint32_t NextBufferIndex[MaxFramesInFlight] = {0, 0};
            std::thread::id ThreadId;
        };

        // Get or create thread data for the current thread
        ThreadData* GetOrCreateThreadData();

        // Thread data storage
        std::vector<std::unique_ptr<ThreadData>> m_ThreadDataPool;
        std::unordered_map<std::thread::id, ThreadData*> m_ThreadDataMap;
        mutable std::mutex m_ThreadDataMutex;

        // Submitted secondaries waiting for execution (per frame)
        std::vector<VkCommandBuffer> m_PendingSecondaries[MaxFramesInFlight];
        mutable std::mutex m_PendingMutex;

        uint32_t m_WorkerCount = 0;
        bool m_Initialized = false;
    };

}
