#include "ggpch.h"
#include "RenderCommand.h"
#include "GGEngine/RHI/RHICommandBuffer.h"

namespace GGEngine {

    void RenderCommand::SetViewport(RHICommandBufferHandle cmd, float x, float y, float width, float height,
                                    float minDepth, float maxDepth)
    {
        RHICmd::SetViewport(cmd, x, y, width, height, minDepth, maxDepth);
    }

    void RenderCommand::SetViewport(RHICommandBufferHandle cmd, float width, float height)
    {
        RHICmd::SetViewport(cmd, width, height);
    }

    void RenderCommand::SetViewport(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        RHICmd::SetViewport(cmd, width, height);
    }

    void RenderCommand::SetScissor(RHICommandBufferHandle cmd, int32_t x, int32_t y, uint32_t width, uint32_t height)
    {
        RHICmd::SetScissor(cmd, x, y, width, height);
    }

    void RenderCommand::SetScissor(RHICommandBufferHandle cmd, uint32_t width, uint32_t height)
    {
        RHICmd::SetScissor(cmd, width, height);
    }

    void RenderCommand::Draw(RHICommandBufferHandle cmd, uint32_t vertexCount, uint32_t instanceCount,
                             uint32_t firstVertex, uint32_t firstInstance)
    {
        RHICmd::Draw(cmd, vertexCount, instanceCount, firstVertex, firstInstance);
    }

    void RenderCommand::DrawIndexed(RHICommandBufferHandle cmd, uint32_t indexCount, uint32_t instanceCount,
                                    uint32_t firstIndex, int32_t vertexOffset, uint32_t firstInstance)
    {
        RHICmd::DrawIndexed(cmd, indexCount, instanceCount, firstIndex, vertexOffset, firstInstance);
    }

    void RenderCommand::PushConstants(RHICommandBufferHandle cmd, RHIPipelineLayoutHandle layout, ShaderStage stageFlags,
                                      uint32_t offset, uint32_t size, const void* data)
    {
        RHICmd::PushConstants(cmd, layout, stageFlags, offset, size, data);
    }

}
