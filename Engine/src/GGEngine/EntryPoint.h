#pragma once

#ifdef GG_PLATFORM_WINDOWS

extern GGEngine::Application* GGEngine::CreateApplication();

int main(int argc, char** argv)
{
    GGEngine::Log::Init();
    GG_CORE_INFO("GGEngine initialized");

    auto app = GGEngine::CreateApplication();
    app->Run();
    delete app;
    return 0;
}

#endif