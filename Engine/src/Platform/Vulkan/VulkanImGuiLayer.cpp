#include "ggpch.h"
#include "GGEngine/ImGui/ImGuiLayer.h"

#include "GGEngine/Core/Application.h"
#include "Platform/Vulkan/VulkanContext.h"

#include <imgui.h>
#include <imgui_impl_glfw.h>
#include <imgui_impl_vulkan.h>

#include <GLFW/glfw3.h>

namespace GGEngine {

    ImGuiLayer::ImGuiLayer()
        : Layer("ImGuiLayer")
    {
    }

    void ImGuiLayer::OnAttach()
    {
        IMGUI_CHECKVERSION();
        ImGui::CreateContext();

        ImGuiIO& io = ImGui::GetIO();
        io.ConfigFlags |= ImGuiConfigFlags_NavEnableKeyboard;
        io.ConfigFlags |= ImGuiConfigFlags_DockingEnable;
        io.ConfigFlags |= ImGuiConfigFlags_ViewportsEnable;

        ImGui::StyleColorsDark();

        // When viewports are enabled, tweak WindowRounding/WindowBg so platform windows look identical to regular ones
        ImGuiStyle& style = ImGui::GetStyle();
        if (io.ConfigFlags & ImGuiConfigFlags_ViewportsEnable)
        {
            style.WindowRounding = 0.0f;
            style.Colors[ImGuiCol_WindowBg].w = 1.0f;
        }

        Application& app = Application::Get();
        GLFWwindow* window = static_cast<GLFWwindow*>(app.GetWindow().GetNativeWindow());

        // true = install GLFW callbacks for ImGui input handling
        // This is required for viewports to work properly (input on viewport windows)
        // ImGui handles input directly via native callbacks, not through our event system
        ImGui_ImplGlfw_InitForVulkan(window, true);

        VulkanContext& vkContext = VulkanContext::Get();

        ImGui_ImplVulkan_InitInfo initInfo = {};
        initInfo.ApiVersion = VK_API_VERSION_1_4;
        initInfo.Instance = vkContext.GetInstance();
        initInfo.PhysicalDevice = vkContext.GetPhysicalDevice();
        initInfo.Device = vkContext.GetDevice();
        initInfo.QueueFamily = vkContext.GetGraphicsQueueFamily();
        initInfo.Queue = vkContext.GetGraphicsQueue();
        initInfo.DescriptorPool = vkContext.GetDescriptorPool();
        initInfo.PipelineCache = VK_NULL_HANDLE;
        initInfo.MinImageCount = 2;
        initInfo.ImageCount = vkContext.GetSwapchainImageCount();
        initInfo.Allocator = nullptr;
        initInfo.CheckVkResultFn = nullptr;

        initInfo.PipelineInfoMain.RenderPass = vkContext.GetRenderPass();
        initInfo.PipelineInfoMain.Subpass = 0;
        initInfo.PipelineInfoMain.MSAASamples = VK_SAMPLE_COUNT_1_BIT;

        // For secondary viewports (multi-viewport support)
        initInfo.PipelineInfoForViewports.RenderPass = vkContext.GetRenderPass();
        initInfo.PipelineInfoForViewports.Subpass = 0;
        initInfo.PipelineInfoForViewports.MSAASamples = VK_SAMPLE_COUNT_1_BIT;

        ImGui_ImplVulkan_Init(&initInfo);

        GG_CORE_INFO("ImGui layer initialized with docking and viewports enabled");
    }

    void ImGuiLayer::OnDetach()
    {
        VulkanContext& vkContext = VulkanContext::Get();
        vkDeviceWaitIdle(vkContext.GetDevice());

        ImGui_ImplVulkan_Shutdown();
        ImGui_ImplGlfw_Shutdown();
        ImGui::DestroyContext();
    }

    void ImGuiLayer::OnUpdate(Timestep ts)
    {
        // Demo window disabled - enable for ImGui reference
        // ImGui::ShowDemoWindow();
    }

    void ImGuiLayer::OnEvent(Event& e)
    {
        // ImGui receives input directly via native GLFW callbacks (required for viewports)
        // Here we just block events from reaching other layers when ImGui captures them
        if (m_BlockEvents)
        {
            ImGuiIO& io = ImGui::GetIO();

            // Don't block scroll events - let them reach viewport camera controllers
            bool isScrollEvent = e.GetEventType() == EventType::MouseScrolled;
            if (!isScrollEvent)
            {
                e.m_Handled |= e.IsInCategory(EventCategoryMouse) && io.WantCaptureMouse;
            }
            e.m_Handled |= e.IsInCategory(EventCategoryKeyboard) && io.WantCaptureKeyboard;
        }
    }

    void ImGuiLayer::Begin()
    {
        ImGui_ImplVulkan_NewFrame();
        ImGui_ImplGlfw_NewFrame();
        ImGui::NewFrame();
    }

    void ImGuiLayer::End()
    {
        ImGui::Render();
        ImGui_ImplVulkan_RenderDrawData(ImGui::GetDrawData(), VulkanContext::Get().GetCurrentCommandBuffer());

        // Update and render additional platform windows (viewports)
        ImGuiIO& io = ImGui::GetIO();
        if (io.ConfigFlags & ImGuiConfigFlags_ViewportsEnable)
        {
            ImGui::UpdatePlatformWindows();
            ImGui::RenderPlatformWindowsDefault();
        }
    }

}
