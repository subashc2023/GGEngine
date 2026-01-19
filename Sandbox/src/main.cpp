#include "GGEngine.h"

class ExampleLayer : public GGEngine::Layer
{
public:
    ExampleLayer() : Layer("ExampleLayer") 
    {

    }
    void OnUpdate() override
    {
        GG_INFO("ExampleLayer::OnUpdate");
    }
    void OnEvent(GGEngine::Event& event) override
    {
        GG_INFO("{0}", event);
    }
};

class Sandbox : public GGEngine::Application 
{
public:
    Sandbox() 
    {
        PushLayer(new ExampleLayer());
    }
    ~Sandbox() 
    {
    }
};

GGEngine::Application* GGEngine::CreateApplication() {
    return new Sandbox();
}