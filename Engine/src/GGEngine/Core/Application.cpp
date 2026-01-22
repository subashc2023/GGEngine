#include "ggpch.h"
#include "Application.h"

#include "GGEngine/Events/ApplicationEvent.h"
#include "Window.h"
#include "Log.h"
#include "Timestep.h"
#include "GGEngine/ImGui/ImGuiLayer.h"
#include "GGEngine/Asset/ShaderLibrary.h"
#include "GGEngine/Asset/TextureLibrary.h"
#include "GGEngine/Asset/AssetManager.h"
#include "GGEngine/Renderer/MaterialLibrary.h"
#include "GGEngine/Renderer/Renderer2D.h"
#include "GGEngine/Core/Profiler.h"
#include "Platform/Vulkan/VulkanContext.h"

#include <GLFW/glfw3.h>

namespace GGEngine {

#define BIND_EVENT_FN(x) std::bind(&Application::x, this, std::placeholders::_1)

    Application* Application::s_Instance = nullptr;

    Application::Application()
    {
        GG_PROFILE_FUNCTION();
        s_Instance = this;

        m_Window = Scope<Window>(Window::Create());
        m_Window->SetEventCallback(BIND_EVENT_FN(OnEvent));

        VulkanContext::Get().Init(static_cast<GLFWwindow*>(m_Window->GetNativeWindow()));

        // Initialize asset libraries (requires VulkanContext to be ready)
        ShaderLibrary::Get().Init();
        TextureLibrary::Get().Init();

        // Initialize Renderer2D (requires ShaderLibrary to be ready)
        Renderer2D::Init();

        m_ImGuiLayer = new ImGuiLayer();
        PushOverlay(m_ImGuiLayer);
    }

    Application::~Application()
    {
        GG_PROFILE_FUNCTION();
        // Wait for GPU to finish before cleanup
        vkDeviceWaitIdle(VulkanContext::Get().GetDevice());

        // Manually call OnDetach() on all layers before VulkanContext shutdown.
        // This ensures layers can clean up their GPU resources (pipelines, buffers, etc.)
        // while the Vulkan device is still valid. LayerStack destructor will then delete
        // the layer objects, but their GPU resources are already released.
        for (Layer* layer : m_LayerStack)
        {
            layer->OnDetach();
        }

        // Shutdown Renderer2D before materials/shaders (it uses both)
        Renderer2D::Shutdown();

        // Shutdown asset system before Vulkan (assets may hold GPU resources)
        // Materials depend on shaders, so shut down materials first
        MaterialLibrary::Get().Shutdown();
        TextureLibrary::Get().Shutdown();
        ShaderLibrary::Get().Shutdown();
        AssetManager::Get().Shutdown();

        VulkanContext::Get().Shutdown();
    }

    void Application::PushLayer(Layer* layer)
    {
        GG_PROFILE_FUNCTION();
        m_LayerStack.PushLayer(layer);
        layer->OnAttach();
    }

    void Application::PushOverlay(Layer* layer)
    {
        GG_PROFILE_FUNCTION();
        m_LayerStack.PushOverlay(layer);
        layer->OnAttach();
    }

    void Application::OnEvent(Event& e)
    {
        GG_PROFILE_FUNCTION();
        EventDispatcher dispatcher(e);
        dispatcher.Dispatch<WindowCloseEvent>(BIND_EVENT_FN(OnWindowClose));
        dispatcher.Dispatch<WindowResizeEvent>(BIND_EVENT_FN(OnWindowResize));

        for (auto it = m_LayerStack.end(); it != m_LayerStack.begin(); )
        {
            (*--it)->OnEvent(e);
            if (e.Handled())
            {
                break;
            }
        }
    }

    void Application::Run()
    {
        while (m_Running)
        {
            GG_PROFILE_SCOPE("RunLoop");
            m_Window->OnUpdate();

            // Skip rendering when minimized to save resources
            if (m_Minimized)
                continue;

            GG_PROFILE_BEGIN_FRAME();

            VulkanContext::Get().BeginFrame();

            // Measure time AFTER BeginFrame to include VSync blocking time
            // (vkAcquireNextImageKHR blocks when VSync is enabled and CPU runs ahead)
            float time = static_cast<float>(glfwGetTime());
            Timestep timestep = time - m_LastFrameTime;
            m_LastFrameTime = time;

            // Offscreen rendering phase - layers render to their framebuffers
            for (Layer* layer : m_LayerStack)
            {
                layer->OnRenderOffscreen(timestep);
            }

            // Begin swapchain render pass for ImGui and direct swapchain rendering
            VulkanContext::Get().BeginSwapchainRenderPass();

            m_ImGuiLayer->Begin();
            for (Layer* layer : m_LayerStack)
            {
                layer->OnUpdate(timestep);
            }
            m_ImGuiLayer->End();

            VulkanContext::Get().EndFrame();
        }
    }

    bool Application::OnWindowClose(WindowCloseEvent& e)
    {
        GG_PROFILE_FUNCTION();
        m_Running = false;
        return true;
    }

    bool Application::OnWindowResize(WindowResizeEvent& e)
    {
        GG_PROFILE_FUNCTION();
        if (e.GetWidth() == 0 || e.GetHeight() == 0)
        {
            m_Minimized = true;
            return false;
        }

        m_Minimized = false;
        VulkanContext::Get().OnWindowResize(e.GetWidth(), e.GetHeight());

        // Notify all layers of the resize
        for (Layer* layer : m_LayerStack)
        {
            layer->OnWindowResize(e.GetWidth(), e.GetHeight());
        }

        return false;
    }
}
