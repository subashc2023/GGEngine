#include "ggpch.h"
#include "Application.h"

#include <chrono>

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
#include "GGEngine/Renderer/InstancedRenderer2D.h"
#include "GGEngine/Renderer/BindlessTextureManager.h"
#include "GGEngine/Renderer/TransferQueue.h"
#include "GGEngine/Renderer/ThreadedCommandBuffer.h"
#include "GGEngine/Core/Profiler.h"
#include "GGEngine/Core/JobSystem.h"
#include "GGEngine/Core/TaskGraph.h"
#include "GGEngine/RHI/RHIDevice.h"
#include "Platform/Vulkan/VulkanContext.h"

#include <GLFW/glfw3.h>

namespace GGEngine {

    Application* Application::s_Instance = nullptr;

    Application::Application()
    {
        GG_PROFILE_FUNCTION();
        s_Instance = this;

        m_Window = Scope<Window>(Window::Create());
        m_Window->SetEventCallback([this](Event& e) { OnEvent(e); });

        VulkanContext::Get().Init(static_cast<GLFWwindow*>(m_Window->GetNativeWindow()));

        // Initialize job system for async asset loading (legacy - being replaced by TaskGraph)
        JobSystem::Get().Init(1);  // Single worker thread for I/O-bound asset loading

        // Initialize TaskGraph with multiple workers for parallel task execution
        TaskGraph::Get().Init();  // Defaults to hardware_concurrency - 1 workers

        // Initialize ThreadedCommandBuffer for parallel command recording
        ThreadedCommandBuffer::Get().Init(TaskGraph::Get().GetWorkerCount());

        // Initialize RHI device (requires VulkanContext to be ready)
        RHIDevice::Get().Init(m_Window->GetNativeWindow());

        // Initialize bindless texture manager (requires VulkanContext to be ready)
        BindlessTextureManager::Get().Init();

        // Initialize asset libraries (requires VulkanContext to be ready)
        ShaderLibrary::Get().Init();
        TextureLibrary::Get().Init();

        // Initialize Renderer2D (requires ShaderLibrary and BindlessTextureManager to be ready)
        Renderer2D::Init();

        // Initialize InstancedRenderer2D (requires ShaderLibrary and BindlessTextureManager to be ready)
        InstancedRenderer2D::Init();

        m_ImGuiLayer = new ImGuiLayer();
        PushOverlay(m_ImGuiLayer);
    }

    Application::~Application()
    {
        GG_PROFILE_FUNCTION();
        // Wait for GPU to finish before cleanup
        RHIDevice::Get().WaitIdle();

        // Manually call OnDetach() on all layers before VulkanContext shutdown.
        // This ensures layers can clean up their GPU resources (pipelines, buffers, etc.)
        // while the Vulkan device is still valid. LayerStack destructor will then delete
        // the layer objects, but their GPU resources are already released.
        for (Layer* layer : m_LayerStack)
        {
            layer->OnDetach();
        }

        // Shutdown Renderer2D and InstancedRenderer2D before materials/shaders (they use both)
        InstancedRenderer2D::Shutdown();
        Renderer2D::Shutdown();

        // Shutdown transfer queue before asset system
        TransferQueue::Get().Shutdown();

        // Shutdown asset system before Vulkan (assets may hold GPU resources)
        // Materials depend on shaders, so shut down materials first
        m_MaterialLibrary.Shutdown();
        TextureLibrary::Get().Shutdown();
        ShaderLibrary::Get().Shutdown();
        AssetManager::Get().Shutdown();

        // Shutdown bindless texture manager before Vulkan
        BindlessTextureManager::Get().Shutdown();

        // Shutdown ThreadedCommandBuffer before TaskGraph
        ThreadedCommandBuffer::Get().Shutdown();

        // Shutdown TaskGraph (waits for pending tasks to complete)
        TaskGraph::Get().Shutdown();

        // Shutdown job system (waits for pending jobs to complete)
        JobSystem::Get().Shutdown();

        // Shutdown RHI device before Vulkan context
        RHIDevice::Get().Shutdown();

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
        dispatcher.Dispatch<WindowCloseEvent>([this](WindowCloseEvent& e) { return OnWindowClose(e); });
        dispatcher.Dispatch<WindowResizeEvent>([this](WindowResizeEvent& e) { return OnWindowResize(e); });

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

            // When minimized, block on events instead of busy-polling
            if (m_Minimized)
            {
                glfwWaitEvents();
                // Reset frame time to avoid huge timestep spike on restore
                m_LastFrameTime = static_cast<float>(glfwGetTime());
                continue;
            }

            GG_PROFILE_BEGIN_FRAME();

            RHIDevice::Get().BeginFrame();

            // Measure time AFTER BeginFrame to include VSync blocking time
            // (vkAcquireNextImageKHR blocks when VSync is enabled and CPU runs ahead)
            float time = static_cast<float>(glfwGetTime());
            float frameTime = time - m_LastFrameTime;
            m_LastFrameTime = time;

            // Clamp frame time to avoid spiral of death (e.g., after breakpoint or long pause)
            const float maxFrameTime = 0.25f;  // Max 250ms per frame
            if (frameTime > maxFrameTime)
                frameTime = maxFrameTime;

            // Reset thread command pools for this frame (safe since fence waited)
            ThreadedCommandBuffer::Get().ResetPools(RHIDevice::Get().GetCurrentFrameIndex());

            // Cleanup staging buffers from completed frame (safe now that fence waited)
            TransferQueue::Get().EndFrame(RHIDevice::Get().GetCurrentFrameIndex());

            // Process async asset loading - uploads pending textures and fires callbacks
            AssetManager::Get().Update();
            JobSystem::Get().ProcessCompletedCallbacks();
            TaskGraph::Get().ProcessCompletedCallbacks();

            // Flush pending GPU uploads before rendering
            TransferQueue::Get().FlushUploads(RHIDevice::Get().GetCurrentCommandBuffer());

            // Fixed timestep accumulator pattern
            float alpha = 1.0f;  // Interpolation factor for rendering
            m_FixedUpdatesThisFrame = 0;

            if (m_UseFixedTimestep)
            {
                GG_PROFILE_SCOPE("FixedUpdate Loop");

                auto fixedStart = std::chrono::high_resolution_clock::now();

                m_Accumulator += frameTime;

                // Run fixed updates until we've caught up
                while (m_Accumulator >= m_FixedTimestep)
                {
                    GG_PROFILE_SCOPE("OnFixedUpdate");
                    for (Layer* layer : m_LayerStack)
                    {
                        layer->OnFixedUpdate(m_FixedTimestep);
                    }
                    m_Accumulator -= m_FixedTimestep;
                    m_FixedUpdatesThisFrame++;
                }

                // Calculate interpolation alpha (how far between previous and current state)
                alpha = m_Accumulator / m_FixedTimestep;

                auto fixedEnd = std::chrono::high_resolution_clock::now();
                m_FixedUpdateTime = std::chrono::duration<float, std::milli>(fixedEnd - fixedStart).count();
            }

            // Create timestep with interpolation alpha
            Timestep timestep(frameTime, alpha);

            // Offscreen rendering phase - layers render to their framebuffers
            for (Layer* layer : m_LayerStack)
            {
                layer->OnRenderOffscreen(timestep);
            }

            // Begin swapchain render pass for ImGui and direct swapchain rendering
            RHIDevice::Get().BeginSwapchainRenderPass();

            m_ImGuiLayer->Begin();
            for (Layer* layer : m_LayerStack)
            {
                layer->OnUpdate(timestep);
            }
            m_ImGuiLayer->End();

            RHIDevice::Get().EndFrame();
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
        RHIDevice::Get().OnWindowResize(e.GetWidth(), e.GetHeight());

        // Notify all layers of the resize
        for (Layer* layer : m_LayerStack)
        {
            layer->OnWindowResize(e.GetWidth(), e.GetHeight());
        }

        return false;
    }
}
