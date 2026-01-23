#pragma once

#include "GGEngine/Core/Window.h"

#include <GLFW/glfw3.h>

namespace GGEngine {

    class WindowsWindow : public Window
    {
    public:
        WindowsWindow(const WindowProps& props);
        virtual ~WindowsWindow();

        void OnUpdate() override;

        inline unsigned int GetWidth() const override { return m_Data.Width; }
        inline unsigned int GetHeight() const override { return m_Data.Height; }
        void GetContentScale(float* xScale, float* yScale) const override;

        inline void SetEventCallback(const EventCallbackFn& callback) override { m_Data.EventCallback = callback; }
        void SetVSync(bool enabled) override;
        bool IsVSync() const override;

        inline void* GetNativeWindow() const override { return m_Window; }
    private:
        virtual void Init(const WindowProps& props);
        virtual void Shutdown();
    private:
        struct WindowData
        {
            std::string Title;
            unsigned int Width, Height;
            bool VSync;

            EventCallbackFn EventCallback;
        };

    private:
        WindowData m_Data;
        GLFWwindow* m_Window;

    };
    
}