#pragma once

#include "GGEngine/Core/Window.h"

#include <GLFW/glfw3.h>

namespace GGEngine {

    class GLFWWindow : public Window
    {
    public:
        GLFWWindow(const WindowProps& props);
        virtual ~GLFWWindow();

        void OnUpdate() override;

        unsigned int GetWidth() const override;
        unsigned int GetHeight() const override;
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
