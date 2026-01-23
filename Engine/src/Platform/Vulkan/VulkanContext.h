#pragma once

#include "GGEngine/Core/Core.h"

#include <vulkan/vulkan.h>
#include <vk_mem_alloc.h>
#include <vector>
#include <functional>

struct GLFWwindow;

namespace GGEngine {

    class GG_API VulkanContext
    {
    public:
        static VulkanContext& Get();

        void Init(GLFWwindow* window);
        void Shutdown();

        void BeginFrame();
        void BeginSwapchainRenderPass();
        void EndFrame();

        void OnWindowResize(uint32_t width, uint32_t height);

        void SetVSync(bool enabled);
        bool IsVSync() const { return m_VSync; }

        VkInstance GetInstance() const { return m_Instance; }
        VkPhysicalDevice GetPhysicalDevice() const { return m_PhysicalDevice; }
        VkDevice GetDevice() const { return m_Device; }
        VkQueue GetGraphicsQueue() const { return m_GraphicsQueue; }
        uint32_t GetGraphicsQueueFamily() const { return m_GraphicsQueueFamily; }
        VkRenderPass GetRenderPass() const { return m_RenderPass; }
        VkCommandBuffer GetCurrentCommandBuffer() const { return m_CommandBuffers[m_CurrentFrameIndex]; }
        VkDescriptorPool GetDescriptorPool() const { return m_DescriptorPool; }
        VkCommandPool GetCommandPool() const { return m_CommandPool; }
        uint32_t GetSwapchainImageCount() const { return static_cast<uint32_t>(m_SwapchainImages.size()); }
        VkExtent2D GetSwapchainExtent() const { return m_SwapchainExtent; }
        uint32_t GetCurrentFrameIndex() const { return m_CurrentFrameIndex; }
        static constexpr uint32_t GetMaxFramesInFlight() { return MAX_FRAMES_IN_FLIGHT; }

        VmaAllocator GetAllocator() const { return m_Allocator; }

        // Bindless rendering limits
        struct BindlessLimits {
            uint32_t maxSampledImages = 0;
            uint32_t maxPerStageDescriptorSampledImages = 0;
            uint32_t maxSamplers = 0;
            uint32_t maxPerStageDescriptorSamplers = 0;
        };
        const BindlessLimits& GetBindlessLimits() const { return m_BindlessLimits; }

        // Execute a one-time command buffer synchronously (blocks until complete)
        void ImmediateSubmit(const std::function<void(VkCommandBuffer)>& func);

    private:
        VulkanContext() = default;
        ~VulkanContext() = default;

        void CreateInstance();
        void SetupDebugMessenger();
        void PopulateDebugMessengerCreateInfo(VkDebugUtilsMessengerCreateInfoEXT& createInfo);
        void CreateSurface();
        void PickPhysicalDevice();
        void CreateLogicalDevice();
        void CreateSwapchain();
        void CreateImageViews();
        void CreateRenderPass();
        void CreateFramebuffers();
        void CreateCommandPool();
        void CreateCommandBuffers();
        void CreateSyncObjects();
        void CreateAllocator();
        void DestroyAllocator();
        void CreateDescriptorPool();

        void CleanupSwapchain();
        void RecreateSwapchain();

        bool CheckValidationLayerSupport();
        std::vector<const char*> GetRequiredExtensions();
        bool IsDeviceSuitable(VkPhysicalDevice device);
        bool CheckDeviceExtensionSupport(VkPhysicalDevice device);

        struct QueueFamilyIndices {
            uint32_t graphicsFamily = UINT32_MAX;
            uint32_t presentFamily = UINT32_MAX;
            bool IsComplete() const { return graphicsFamily != UINT32_MAX && presentFamily != UINT32_MAX; }
        };
        QueueFamilyIndices FindQueueFamilies(VkPhysicalDevice device);

        struct SwapChainSupportDetails {
            VkSurfaceCapabilitiesKHR capabilities;
            std::vector<VkSurfaceFormatKHR> formats;
            std::vector<VkPresentModeKHR> presentModes;
        };
        SwapChainSupportDetails QuerySwapChainSupport(VkPhysicalDevice device);

        VkSurfaceFormatKHR ChooseSwapSurfaceFormat(const std::vector<VkSurfaceFormatKHR>& availableFormats);
        VkPresentModeKHR ChooseSwapPresentMode(const std::vector<VkPresentModeKHR>& availablePresentModes);
        VkExtent2D ChooseSwapExtent(const VkSurfaceCapabilitiesKHR& capabilities);

    private:
        GLFWwindow* m_Window = nullptr;

        VkInstance m_Instance = VK_NULL_HANDLE;
        VkDebugUtilsMessengerEXT m_DebugMessenger = VK_NULL_HANDLE;
        VkSurfaceKHR m_Surface = VK_NULL_HANDLE;

        VkPhysicalDevice m_PhysicalDevice = VK_NULL_HANDLE;
        VkDevice m_Device = VK_NULL_HANDLE;

        VkQueue m_GraphicsQueue = VK_NULL_HANDLE;
        VkQueue m_PresentQueue = VK_NULL_HANDLE;
        uint32_t m_GraphicsQueueFamily = 0;

        VkSwapchainKHR m_Swapchain = VK_NULL_HANDLE;
        std::vector<VkImage> m_SwapchainImages;
        std::vector<VkImageView> m_SwapchainImageViews;
        VkFormat m_SwapchainImageFormat;
        VkExtent2D m_SwapchainExtent;

        VkRenderPass m_RenderPass = VK_NULL_HANDLE;
        std::vector<VkFramebuffer> m_SwapchainFramebuffers;

        VkCommandPool m_CommandPool = VK_NULL_HANDLE;
        std::vector<VkCommandBuffer> m_CommandBuffers;

        std::vector<VkSemaphore> m_ImageAvailableSemaphores;
        std::vector<VkSemaphore> m_RenderFinishedSemaphores;
        std::vector<VkFence> m_InFlightFences;
        std::vector<VkFence> m_ImagesInFlight;

        VkDescriptorPool m_DescriptorPool = VK_NULL_HANDLE;
        VmaAllocator m_Allocator = VK_NULL_HANDLE;

        BindlessLimits m_BindlessLimits;

        uint32_t m_CurrentFrameIndex = 0;
        uint32_t m_CurrentImageIndex = 0;
        bool m_FramebufferResized = false;
        bool m_FrameStarted = false;
        bool m_VSync = true;

        static constexpr int MAX_FRAMES_IN_FLIGHT = 2;

#ifdef NDEBUG
        const bool m_EnableValidationLayers = false;
#else
        const bool m_EnableValidationLayers = true;
#endif

        const std::vector<const char*> m_ValidationLayers = {
            "VK_LAYER_KHRONOS_validation"
        };

        const std::vector<const char*> m_DeviceExtensions = {
            VK_KHR_SWAPCHAIN_EXTENSION_NAME,
#ifdef __APPLE__
            "VK_KHR_portability_subset"
#endif
        };
    };

}
