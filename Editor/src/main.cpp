#include "GGEngine.h"
#include "GGEngine/Core/EntryPoint.h"
#include "GGEngine/Asset/AssetManager.h"
#include "EditorLayer.h"

class Editor : public GGEngine::Application {
public:
    Editor() {
        // Register Editor-specific asset directory
        GGEngine::AssetManager::Get().AddSearchPath("Editor/assets");

        PushLayer(new EditorLayer());
    }
    ~Editor() {
    }
};

GGEngine::Application* GGEngine::CreateApplication() {
    return new Editor();
}
