#include "ggpch.h"
#include "GLFWWindow.h"

#include "GGEngine/Core/Profiler.h"
#include "GGEngine/Events/ApplicationEvent.h"
#include "GGEngine/Events/KeyEvent.h"
#include "GGEngine/Events/MouseEvent.h"
#include "Platform/Vulkan/VulkanContext.h"

namespace GGEngine {

    static bool s_GLFWInitialized = false;


    static void GLFWErrorCallback(int error, const char* description)
    {
        GG_CORE_ERROR("GLFW Error ({0}): {1}", error, description);
    }

    Window* Window::Create(const WindowProps& props)
    {
        return new GLFWWindow(props);
    }

    GLFWWindow::GLFWWindow(const WindowProps& props)
    {
        Init(props);
    }

    GLFWWindow::~GLFWWindow()
    {
        Shutdown();
    }

    void GLFWWindow::Init(const WindowProps& props)
    {
        GG_PROFILE_FUNCTION();
        m_Data.Title = props.Title;
        m_Data.Width = props.Width;
        m_Data.Height = props.Height;

        GG_CORE_INFO("Creating window {0} ({1}, {2})", props.Title, props.Width, props.Height);

        if (!s_GLFWInitialized)
        {
            int success = glfwInit();
            GG_CORE_ASSERT(success, "Failed to initialize GLFW");
            glfwSetErrorCallback(GLFWErrorCallback);
            s_GLFWInitialized = true;
        }

        // Disable OpenGL context creation for Vulkan
        glfwWindowHint(GLFW_CLIENT_API, GLFW_NO_API);

        m_Window = glfwCreateWindow(static_cast<int>(props.Width), static_cast<int>(props.Height), m_Data.Title.c_str(), nullptr, nullptr);
        GG_CORE_ASSERT(m_Window, "Failed to create GLFW window");
        glfwSetWindowUserPointer(m_Window, &m_Data);

        // Debug: Log actual window dimensions vs framebuffer vs content scale
        int actualW, actualH, fbW, fbH;
        float scaleX, scaleY;
        glfwGetWindowSize(m_Window, &actualW, &actualH);
        glfwGetFramebufferSize(m_Window, &fbW, &fbH);
        glfwGetWindowContentScale(m_Window, &scaleX, &scaleY);
        GG_CORE_INFO("Window dimensions: logical={}x{}, framebuffer={}x{}, contentScale={}x{}",
                     actualW, actualH, fbW, fbH, scaleX, scaleY);


        // GLFW CALLBACKS SETUP

        glfwSetWindowSizeCallback(m_Window, [](GLFWwindow* window, int width, int height)
        {
            WindowData& data = *static_cast<WindowData*>(glfwGetWindowUserPointer(window));
            data.Width = width;
            data.Height = height;
            WindowResizeEvent event(width, height);
            data.EventCallback(event);
        });

        glfwSetWindowCloseCallback(m_Window, [](GLFWwindow* window)
        {
            WindowData& data = *static_cast<WindowData*>(glfwGetWindowUserPointer(window));
            WindowCloseEvent event;
            data.EventCallback(event);
        });

        glfwSetKeyCallback(m_Window, [](GLFWwindow* window, int key, int scancode, int action, int mods)
        {
            WindowData& data = *static_cast<WindowData*>(glfwGetWindowUserPointer(window));
            switch (action)
            {
                case GLFW_PRESS:
                {
                    KeyPressedEvent event(static_cast<KeyCode>(key), 0);
                    data.EventCallback(event);
                    break;
                }
                case GLFW_RELEASE:
                {
                    KeyReleasedEvent event(static_cast<KeyCode>(key));
                    data.EventCallback(event);
                    break;
                }
                case GLFW_REPEAT:
                {
                    KeyPressedEvent event(static_cast<KeyCode>(key), 1);
                    data.EventCallback(event);
                    break;
                }
                default:
                    break;
            }
        });

        glfwSetCharCallback(m_Window, [](GLFWwindow* window, unsigned int keycode)
        {
            WindowData& data = *static_cast<WindowData*>(glfwGetWindowUserPointer(window));
            KeyTypedEvent event(static_cast<KeyCode>(keycode));
            data.EventCallback(event);
        });

        glfwSetMouseButtonCallback(m_Window, [](GLFWwindow* window, int button, int action, int mods)
        {
            WindowData& data = *static_cast<WindowData*>(glfwGetWindowUserPointer(window));
            switch (action)
            {
                case GLFW_PRESS:
                {
                    MouseButtonPressedEvent event(static_cast<MouseCode>(button));
                    data.EventCallback(event);
                    break;
                }
                case GLFW_RELEASE:
                {
                    MouseButtonReleasedEvent event(static_cast<MouseCode>(button));
                    data.EventCallback(event);
                    break;
                }
                default:
                    break;
            }
        });

        glfwSetScrollCallback(m_Window, [](GLFWwindow* window, double xoffset, double yoffset)
        {
            WindowData& data = *static_cast<WindowData*>(glfwGetWindowUserPointer(window));
            MouseScrolledEvent event(xoffset, yoffset);
            data.EventCallback(event);
        });

        glfwSetCursorPosCallback(m_Window, [](GLFWwindow* window, double xpos, double ypos)
        {
            WindowData& data = *static_cast<WindowData*>(glfwGetWindowUserPointer(window));
            MouseMovedEvent event(xpos, ypos);
            data.EventCallback(event);
        });
    }

    unsigned int GLFWWindow::GetWidth() const
    {
        int width, height;
        glfwGetWindowSize(m_Window, &width, &height);
        return static_cast<unsigned int>(width);
    }

    unsigned int GLFWWindow::GetHeight() const
    {
        int width, height;
        glfwGetWindowSize(m_Window, &width, &height);
        return static_cast<unsigned int>(height);
    }

    void GLFWWindow::GetContentScale(float* xScale, float* yScale) const
    {
        glfwGetWindowContentScale(m_Window, xScale, yScale);
    }

    void GLFWWindow::Shutdown()
    {
        GG_PROFILE_FUNCTION();
        glfwDestroyWindow(m_Window);
        glfwTerminate();
        s_GLFWInitialized = false;
    }

    void GLFWWindow::OnUpdate()
    {
        GG_PROFILE_FUNCTION();
        glfwPollEvents();
        // Note: Buffer swap handled by Vulkan swapchain presentation
    }

    void GLFWWindow::SetVSync(bool enabled)
    {
        VulkanContext::Get().SetVSync(enabled);
        m_Data.VSync = enabled;
    }

    bool GLFWWindow::IsVSync() const
    {
        return VulkanContext::Get().IsVSync();
    }

}
