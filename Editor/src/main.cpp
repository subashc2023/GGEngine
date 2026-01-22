#include "GGEngine.h"
#include "GGEngine/Core/EntryPoint.h"
#include "EditorLayer.h"

class Editor : public GGEngine::Application {
public:
    Editor() {
        PushLayer(new EditorLayer());
    }
    ~Editor() {
    }
};

GGEngine::Application* GGEngine::CreateApplication() {
    return new Editor();
}
