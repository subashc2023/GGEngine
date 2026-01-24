#include "ggpch.h"
#include "VulkanResourceRegistry.h"
#include "VulkanContext.h"
#include "VulkanUtils.h"
#include "GGEngine/RHI/RHIDevice.h"

namespace GGEngine {

    // ============================================================================
    // Shader Management
    // ============================================================================

    Result<RHIShaderModuleHandle> RHIDevice::TryCreateShaderModule(ShaderStage stage, const std::vector<char>& spirvCode)
    {
        return TryCreateShaderModule(stage, spirvCode.data(), spirvCode.size());
    }

    Result<RHIShaderModuleHandle> RHIDevice::TryCreateShaderModule(ShaderStage stage, const void* spirvData, size_t spirvSize)
    {
        if (!spirvData || spirvSize == 0)
            return Result<RHIShaderModuleHandle>::Err("SPIR-V data is null or empty");

        auto device = VulkanContext::Get().GetDevice();

        VkShaderModuleCreateInfo createInfo{};
        createInfo.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO;
        createInfo.codeSize = spirvSize;
        createInfo.pCode = reinterpret_cast<const uint32_t*>(spirvData);

        VkShaderModule module = VK_NULL_HANDLE;
        VkResult vkResult = vkCreateShaderModule(device, &createInfo, nullptr, &module);
        if (vkResult != VK_SUCCESS)
        {
            return Result<RHIShaderModuleHandle>::Err(
                std::string("vkCreateShaderModule failed: ") + std::string(VkResultToString(vkResult)));
        }

        return Result<RHIShaderModuleHandle>::Ok(
            VulkanResourceRegistry::Get().RegisterShaderModule(module, stage));
    }

    RHIShaderModuleHandle RHIDevice::CreateShaderModule(ShaderStage stage, const std::vector<char>& spirvCode)
    {
        auto result = TryCreateShaderModule(stage, spirvCode);
        if (result.IsErr())
        {
            GG_CORE_ERROR("RHIDevice::CreateShaderModule: {}", result.Error());
            return NullShaderModule;
        }
        return result.Value();
    }

    RHIShaderModuleHandle RHIDevice::CreateShaderModule(ShaderStage stage, const void* spirvData, size_t spirvSize)
    {
        auto result = TryCreateShaderModule(stage, spirvData, spirvSize);
        if (result.IsErr())
        {
            GG_CORE_ERROR("RHIDevice::CreateShaderModule: {}", result.Error());
            return NullShaderModule;
        }
        return result.Value();
    }

    void RHIDevice::DestroyShaderModule(RHIShaderModuleHandle handle)
    {
        if (!handle.IsValid()) return;

        auto& registry = VulkanResourceRegistry::Get();
        auto moduleData = registry.GetShaderModuleData(handle);
        auto device = VulkanContext::Get().GetDevice();

        if (moduleData.module != VK_NULL_HANDLE)
            vkDestroyShaderModule(device, moduleData.module, nullptr);

        registry.UnregisterShaderModule(handle);
    }

}
