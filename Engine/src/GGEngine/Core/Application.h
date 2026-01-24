#pragma once

#include "Core.h"

#include "Window.h"
#include "LayerStack.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Events/ApplicationEvent.h"
#include "GGEngine/Renderer/MaterialLibrary.h"

namespace GGEngine {

    class ImGuiLayer;

    class GG_API Application
    {
    public:
        Application();
        virtual ~Application();

        void Run();

        void OnEvent(Event& e);

        void PushLayer(Layer* layer);
        void PushOverlay(Layer* layer);

        inline Window& GetWindow() { return *m_Window; }
        inline ImGuiLayer* GetImGuiLayer() { return m_ImGuiLayer; }
        inline MaterialLibrary& GetMaterialLibrary() { return m_MaterialLibrary; }

        // Fixed timestep configuration
        void SetFixedTimestep(float timestep) { m_FixedTimestep = timestep; }
        float GetFixedTimestep() const { return m_FixedTimestep; }
        void SetUseFixedTimestep(bool enabled) { m_UseFixedTimestep = enabled; }
        bool GetUseFixedTimestep() const { return m_UseFixedTimestep; }

        // Performance stats
        float GetFixedUpdateTime() const { return m_FixedUpdateTime; }
        int GetFixedUpdatesPerFrame() const { return m_FixedUpdatesThisFrame; }

        inline static Application& Get() { return *s_Instance; }
    private:
        bool OnWindowClose(WindowCloseEvent& e);
        bool OnWindowResize(WindowResizeEvent& e);

        Scope<Window> m_Window;
        ImGuiLayer* m_ImGuiLayer = nullptr;  // Raw ptr - ownership transferred to LayerStack
        bool m_Running = true;
        bool m_Minimized = false;
        LayerStack m_LayerStack;
        float m_LastFrameTime = 0.0f;

        // Fixed timestep accumulator
        bool m_UseFixedTimestep = false;         // Toggle between variable and fixed timestep
        float m_FixedTimestep = 1.0f / 60.0f;    // 60 Hz by default
        float m_Accumulator = 0.0f;              // Time accumulated for fixed updates
        float m_FixedUpdateTime = 0.0f;          // Time spent in fixed updates (for profiling)
        int m_FixedUpdatesThisFrame = 0;         // Number of fixed updates this frame

        // Libraries owned by Application (access via Get*Library() methods)
        MaterialLibrary m_MaterialLibrary;

        static Application* s_Instance;
    };

    Application* CreateApplication();
}