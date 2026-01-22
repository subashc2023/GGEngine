#include "GGEngine.h"
#include "GGEngine/Core/EntryPoint.h"
#include "TriangleLayer.h"

class Sandbox : public GGEngine::Application
{
public:
    Sandbox()
    {
        PushLayer(new TriangleLayer());
    }
    ~Sandbox()
    {
    }
};

GGEngine::Application* GGEngine::CreateApplication() {
    return new Sandbox();
}
