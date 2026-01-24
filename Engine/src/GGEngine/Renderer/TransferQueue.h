#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/RHI/RHITypes.h"

#include <vector>
#include <functional>
#include <mutex>

namespace GGEngine {

    // Deferred GPU transfer queue for async asset loading.
    // Batches texture/buffer uploads and executes them at frame boundaries
    // to avoid blocking with ImmediateSubmit.
    class GG_API TransferQueue
    {
    public:
        static TransferQueue& Get();

        using UploadCompleteCallback = std::function<void()>;

        // Queue a texture upload (thread-safe)
        // Data is copied to a staging buffer immediately
        // Transfer is recorded at next FlushUploads() call
        void QueueTextureUpload(
            RHITextureHandle texture,
            const void* data,
            uint64_t size,
            uint32_t width,
            uint32_t height,
            UploadCompleteCallback callback = nullptr);

        // Queue a buffer upload (thread-safe)
        void QueueBufferUpload(
            RHIBufferHandle buffer,
            const void* data,
            uint64_t size,
            uint64_t offset = 0,
            UploadCompleteCallback callback = nullptr);

        // Record all pending transfers to the command buffer
        // Called from main loop before swapchain render pass
        // After GPU completes the frame, staging buffers are recycled
        void FlushUploads(RHICommandBufferHandle cmd);

        // Called at end of frame to cleanup staging buffers from completed frames
        void EndFrame(uint32_t frameIndex);

        // Get number of pending uploads
        uint32_t GetPendingCount() const;

        // Shutdown - cleanup all resources
        void Shutdown();

    private:
        TransferQueue() = default;
        ~TransferQueue() = default;
        TransferQueue(const TransferQueue&) = delete;
        TransferQueue& operator=(const TransferQueue&) = delete;

        struct TextureUploadRequest
        {
            RHITextureHandle texture;
            RHIBufferHandle stagingBuffer;
            uint32_t width;
            uint32_t height;
            UploadCompleteCallback callback;
        };

        struct BufferUploadRequest
        {
            RHIBufferHandle target;
            RHIBufferHandle stagingBuffer;
            uint64_t size;
            uint64_t offset;
            UploadCompleteCallback callback;
        };

        // Pending uploads (protected by mutex for thread-safe queuing)
        std::vector<TextureUploadRequest> m_PendingTextureUploads;
        std::vector<BufferUploadRequest> m_PendingBufferUploads;
        mutable std::mutex m_Mutex;

        // Staging buffers waiting for GPU completion (per-frame)
        static constexpr uint32_t MaxFramesInFlight = 2;
        std::vector<RHIBufferHandle> m_StagingBuffersInFlight[MaxFramesInFlight];
        std::vector<UploadCompleteCallback> m_PendingCallbacks[MaxFramesInFlight];
    };

}
