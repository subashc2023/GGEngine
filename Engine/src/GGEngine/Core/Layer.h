#pragma once

#include "Core.h"
#include "Timestep.h"
#include "GGEngine/Events/Event.h"

namespace GGEngine {

    class GG_API Layer
    {
    public:
        Layer(const std::string& name = "Layer");
        virtual ~Layer();

        virtual void OnAttach() {}
        virtual void OnDetach() {}
        virtual void OnUpdate(Timestep ts) {}
        virtual void OnRenderOffscreen(Timestep ts) {}
        virtual void OnEvent(Event& event) {}
        virtual void OnWindowResize(uint32_t width, uint32_t height) {}

        inline const std::string& GetName() const { return m_DebugName; }
    protected:
        std::string m_DebugName;
    };

}