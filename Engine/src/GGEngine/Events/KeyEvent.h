#pragma once

#include "Event.h"
#include "GGEngine/Core/KeyCodes.h"

#include <sstream>

namespace GGEngine {

    class GG_API KeyEvent : public Event
    {
    public:
        inline KeyCode GetKeyCode() const { return m_KeyCode; }

        EVENT_CLASS_CATEGORY(EventCategoryKeyboard | EventCategoryInput)
    protected:
        KeyEvent(KeyCode keycode)
            : m_KeyCode(keycode) {}

        KeyCode m_KeyCode;
    };

    class GG_API KeyPressedEvent : public KeyEvent
    {
    public:
        KeyPressedEvent(KeyCode keycode, int repeatCount)
        : KeyEvent(keycode), m_RepeatCount(repeatCount) {}

        int GetRepeatCount() const { return m_RepeatCount; }

        std::string ToString() const override
        {
            std::stringstream ss;
            ss << "KeyPressedEvent: " << static_cast<int>(m_KeyCode) << " (" << m_RepeatCount << " repeats)";
            return ss.str();
        }

        EVENT_CLASS_TYPE(KeyPressed)
    private:
        int m_RepeatCount;
    };

    class GG_API KeyReleasedEvent : public KeyEvent
    {
    public:
        KeyReleasedEvent(KeyCode keycode)
        : KeyEvent(keycode) {}

        std::string ToString() const override
        {
            std::stringstream ss;
            ss << "KeyReleasedEvent: " << static_cast<int>(m_KeyCode);
            return ss.str();
        }

        EVENT_CLASS_TYPE(KeyReleased)
    };

    class GG_API KeyTypedEvent : public KeyEvent
    {
    public:
        KeyTypedEvent(KeyCode keycode)
        : KeyEvent(keycode) {}

        std::string ToString() const override
        {
            std::stringstream ss;
            ss << "KeyTypedEvent: " << static_cast<int>(m_KeyCode);
            return ss.str();
        }

        EVENT_CLASS_TYPE(KeyTyped)
    };

}
