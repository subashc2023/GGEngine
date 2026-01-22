#pragma once

#include "GGEngine/Core/Timestep.h"
#include "GGEngine/Events/Event.h"
#include "GGEngine/Renderer/Camera.h"

#include <string>

// Base class for all examples
// Each example demonstrates a specific engine feature
class Example
{
public:
    Example(const std::string& name, const std::string& description)
        : m_Name(name), m_Description(description) {}
    virtual ~Example() = default;

    virtual void OnAttach() {}
    virtual void OnDetach() {}
    virtual void OnUpdate(GGEngine::Timestep ts) {}
    virtual void OnRender(const GGEngine::Camera& camera) {}
    virtual void OnImGuiRender() {}
    virtual void OnEvent(GGEngine::Event& event) {}

    const std::string& GetName() const { return m_Name; }
    const std::string& GetDescription() const { return m_Description; }

protected:
    std::string m_Name;
    std::string m_Description;
};
