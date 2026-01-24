#include "ggpch.h"
#include "ThreadedCommandBuffer.h"
#include "Platform/Vulkan/VulkanContext.h"

namespace GGEngine {

    ThreadedCommandBuffer& ThreadedCommandBuffer::Get()
    {
        static ThreadedCommandBuffer instance;
        return instance;
    }

    void ThreadedCommandBuffer::Init(uint32_t workerCount)
    {
        if (m_Initialized)
        {
            GG_CORE_WARN("ThreadedCommandBuffer::Init called when already initialized");
            return;
        }

        m_WorkerCount = workerCount;

        // Pre-allocate thread data pool
        // +1 for main thread that might also record secondary buffers
        m_ThreadDataPool.reserve(workerCount + 1);

        m_Initialized = true;
        GG_CORE_INFO("ThreadedCommandBuffer initialized for {} worker threads", workerCount);
    }

    void ThreadedCommandBuffer::Shutdown()
    {
        if (!m_Initialized)
            return;

        GG_CORE_TRACE("ThreadedCommandBuffer shutting down...");

        VkDevice device = VulkanContext::Get().GetDevice();

        // Destroy all command pools
        {
            std::lock_guard<std::mutex> lock(m_ThreadDataMutex);
            for (auto& threadData : m_ThreadDataPool)
            {
                if (threadData && threadData->CommandPool != VK_NULL_HANDLE)
                {
                    vkDestroyCommandPool(device, threadData->CommandPool, nullptr);
                    threadData->CommandPool = VK_NULL_HANDLE;
                }
            }
            m_ThreadDataPool.clear();
            m_ThreadDataMap.clear();
        }

        // Clear pending secondaries
        {
            std::lock_guard<std::mutex> lock(m_PendingMutex);
            for (uint32_t i = 0; i < MaxFramesInFlight; i++)
            {
                m_PendingSecondaries[i].clear();
            }
        }

        m_Initialized = false;
        GG_CORE_TRACE("ThreadedCommandBuffer shutdown complete");
    }

    ThreadedCommandBuffer::ThreadData* ThreadedCommandBuffer::GetOrCreateThreadData()
    {
        std::thread::id thisId = std::this_thread::get_id();

        // Fast path: check if we already have thread data
        {
            std::lock_guard<std::mutex> lock(m_ThreadDataMutex);
            auto it = m_ThreadDataMap.find(thisId);
            if (it != m_ThreadDataMap.end())
            {
                return it->second;
            }
        }

        // Slow path: create new thread data
        VkDevice device = VulkanContext::Get().GetDevice();
        uint32_t queueFamily = VulkanContext::Get().GetGraphicsQueueFamily();

        // Create command pool for this thread
        VkCommandPoolCreateInfo poolInfo{};
        poolInfo.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO;
        poolInfo.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
        poolInfo.queueFamilyIndex = queueFamily;

        VkCommandPool commandPool;
        VkResult result = vkCreateCommandPool(device, &poolInfo, nullptr, &commandPool);
        if (result != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to create thread command pool: {}", static_cast<int>(result));
            return nullptr;
        }

        // Create and store thread data
        auto threadData = std::make_unique<ThreadData>();
        threadData->CommandPool = commandPool;
        threadData->ThreadId = thisId;

        ThreadData* dataPtr = threadData.get();

        {
            std::lock_guard<std::mutex> lock(m_ThreadDataMutex);
            m_ThreadDataPool.push_back(std::move(threadData));
            m_ThreadDataMap[thisId] = dataPtr;
        }

        GG_CORE_TRACE("Created command pool for thread {}",
            std::hash<std::thread::id>{}(thisId));

        return dataPtr;
    }

    VkCommandBuffer ThreadedCommandBuffer::AllocateSecondary(uint32_t frameIndex)
    {
        if (!m_Initialized)
        {
            GG_CORE_ERROR("ThreadedCommandBuffer::AllocateSecondary called before Init()");
            return VK_NULL_HANDLE;
        }

        ThreadData* threadData = GetOrCreateThreadData();
        if (!threadData)
        {
            return VK_NULL_HANDLE;
        }

        auto& buffers = threadData->SecondaryBuffers[frameIndex];
        uint32_t& nextIndex = threadData->NextBufferIndex[frameIndex];

        // Check if we need to allocate more buffers
        if (nextIndex >= buffers.size())
        {
            VkDevice device = VulkanContext::Get().GetDevice();

            // Allocate a batch of secondary command buffers
            constexpr uint32_t BatchSize = 8;
            uint32_t allocCount = BatchSize;

            VkCommandBufferAllocateInfo allocInfo{};
            allocInfo.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
            allocInfo.commandPool = threadData->CommandPool;
            allocInfo.level = VK_COMMAND_BUFFER_LEVEL_SECONDARY;
            allocInfo.commandBufferCount = allocCount;

            std::vector<VkCommandBuffer> newBuffers(allocCount);
            VkResult result = vkAllocateCommandBuffers(device, &allocInfo, newBuffers.data());
            if (result != VK_SUCCESS)
            {
                GG_CORE_ERROR("Failed to allocate secondary command buffers: {}",
                    static_cast<int>(result));
                return VK_NULL_HANDLE;
            }

            buffers.insert(buffers.end(), newBuffers.begin(), newBuffers.end());
        }

        return buffers[nextIndex++];
    }

    void ThreadedCommandBuffer::BeginSecondary(VkCommandBuffer cmd,
                                                VkRenderPass renderPass,
                                                VkFramebuffer framebuffer,
                                                uint32_t subpass)
    {
        VkCommandBufferInheritanceInfo inheritanceInfo{};
        inheritanceInfo.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_INHERITANCE_INFO;
        inheritanceInfo.renderPass = renderPass;
        inheritanceInfo.subpass = subpass;
        inheritanceInfo.framebuffer = framebuffer;
        // occlusionQueryEnable, queryFlags, pipelineStatistics left as 0/default

        VkCommandBufferBeginInfo beginInfo{};
        beginInfo.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
        beginInfo.flags = VK_COMMAND_BUFFER_USAGE_RENDER_PASS_CONTINUE_BIT |
                          VK_COMMAND_BUFFER_USAGE_SIMULTANEOUS_USE_BIT;
        beginInfo.pInheritanceInfo = &inheritanceInfo;

        VkResult result = vkBeginCommandBuffer(cmd, &beginInfo);
        if (result != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to begin secondary command buffer: {}",
                static_cast<int>(result));
        }
    }

    void ThreadedCommandBuffer::EndSecondary(VkCommandBuffer cmd)
    {
        VkResult result = vkEndCommandBuffer(cmd);
        if (result != VK_SUCCESS)
        {
            GG_CORE_ERROR("Failed to end secondary command buffer: {}",
                static_cast<int>(result));
        }
    }

    void ThreadedCommandBuffer::SubmitSecondary(VkCommandBuffer secondary, uint32_t frameIndex)
    {
        std::lock_guard<std::mutex> lock(m_PendingMutex);
        m_PendingSecondaries[frameIndex].push_back(secondary);
    }

    void ThreadedCommandBuffer::ExecuteSecondaries(VkCommandBuffer primaryCmd, uint32_t frameIndex)
    {
        std::vector<VkCommandBuffer> secondaries;
        {
            std::lock_guard<std::mutex> lock(m_PendingMutex);
            std::swap(secondaries, m_PendingSecondaries[frameIndex]);
        }

        if (secondaries.empty())
            return;

        vkCmdExecuteCommands(primaryCmd,
            static_cast<uint32_t>(secondaries.size()),
            secondaries.data());

        GG_CORE_TRACE("Executed {} secondary command buffers", secondaries.size());
    }

    void ThreadedCommandBuffer::ResetPools(uint32_t frameIndex)
    {
        std::lock_guard<std::mutex> lock(m_ThreadDataMutex);

        for (auto& threadData : m_ThreadDataPool)
        {
            if (threadData)
            {
                // Reset the buffer index so buffers can be reused
                threadData->NextBufferIndex[frameIndex] = 0;

                // Note: We don't reset the command pool here because
                // VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT allows
                // individual buffer resets. The buffers will be reset
                // when vkBeginCommandBuffer is called on them.
            }
        }
    }

    size_t ThreadedCommandBuffer::GetPendingCount(uint32_t frameIndex) const
    {
        std::lock_guard<std::mutex> lock(m_PendingMutex);
        return m_PendingSecondaries[frameIndex].size();
    }

}
