#pragma once

#include "GGEngine/Core/Input.h"

namespace GGEngine {

    class WindowsInput : public Input
    {
    protected:
        virtual bool IsKeyPressedImpl(KeyCode keycode) override;
        virtual bool IsMouseButtonPressedImpl(MouseCode button) override;
        virtual float GetMouseXImpl() override;
        virtual float GetMouseYImpl() override;
        virtual std::pair<float, float> GetMousePositionImpl() override;
    };

}
