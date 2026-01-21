#pragma once

#include "GGEngine/Core/Core.h"

#include <string>
#include <functional>
#include <ostream>
#include <spdlog/fmt/ostr.h>

namespace GGEngine {
    
    enum class EventType {
        None = 0,
        WindowClose, WindowResize, WindowFocus, WindowLostFocus, WindowMoved,
        AppTick, AppUpdate, AppRender,
        KeyPressed, KeyReleased, KeyTyped,
        MouseButtonPressed, MouseButtonReleased, MouseMoved, MouseScrolled
    };

    enum EventCategory {
        None = 0,
        EventCategoryApplication = BIT(0),
        EventCategoryInput =       BIT(1),
        EventCategoryKeyboard =    BIT(2),
        EventCategoryMouse =       BIT(3),
        EventCategoryMouseButton = BIT(4),
    };

#define EVENT_CLASS_TYPE(type) static EventType GetStaticType() { return EventType::type; } \
                                virtual EventType GetEventType() const override { return GetStaticType(); } \
                                virtual const char* GetName() const override { return #type; }

#define EVENT_CLASS_CATEGORY(category) virtual int GetCategoryFlags() const override { return category; }

    class ImGuiLayer;

    class GG_API Event
    {
        friend class EventDispatcher;
        friend class ImGuiLayer;
    public:
        virtual ~Event() = default;
        bool Handled() const { return m_Handled; }
        
        virtual EventType GetEventType() const = 0;
        virtual const char* GetName() const = 0;
        virtual int GetCategoryFlags() const = 0;
        virtual std::string ToString() const { return GetName(); }

        inline bool IsInCategory(EventCategory category) const
        {
            return GetCategoryFlags() & category;
        }

        friend std::ostream& operator<<(std::ostream& os, const Event& e)
        {
            return os << e.ToString();
        }
    protected:
        bool m_Handled = false;
    };

    class EventDispatcher
    {
        template<typename T>
        using EventFn = std::function<bool(T&)>;
    public:
        EventDispatcher(Event& event)
        : m_Event(event) 
        {

        }

        template<typename T>
        bool Dispatch(EventFn<T> func)
        {
            if (m_Event.GetEventType() == T::GetStaticType())
            {
                m_Event.m_Handled |= func(static_cast<T&>(m_Event));
                return true;
            }
            return false;
        }
    private:
        Event& m_Event;
    };

}

template <typename T>
struct fmt::formatter<T, std::enable_if_t<std::is_base_of<GGEngine::Event, T>::value, char>> : fmt::ostream_formatter {};