#pragma once

#include "GGEngine/Layer.h"
#include "GGEngine/Events/Event.h"

namespace GGEngine {

    class GG_API ImGuiLayer : public Layer
    {
    public:
        ImGuiLayer();
        ~ImGuiLayer() = default;

        void OnAttach() override;
        void OnDetach() override;
        void OnUpdate() override;
        void OnEvent(Event& e) override;

        void Begin();
        void End();

        void SetBlockEvents(bool block) { m_BlockEvents = block; }
        bool IsBlockingEvents() const { return m_BlockEvents; }

    private:
        bool m_BlockEvents = true;
    };

}
