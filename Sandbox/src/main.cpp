#include "GGEngine.h"
#include "GGEngine/Core/EntryPoint.h"
#include "GGEngine/Asset/AssetManager.h"
#include "ExamplesLayer.h"

class Sandbox : public GGEngine::Application
{
public:
    Sandbox()
    {
        // Register Sandbox-specific asset directory
        GGEngine::AssetManager::Get().AddSearchPath("Sandbox/assets");

        PushLayer(new ExamplesLayer());
    }
    ~Sandbox()
    {
    }
};

GGEngine::Application* GGEngine::CreateApplication() {
    return new Sandbox();
}
