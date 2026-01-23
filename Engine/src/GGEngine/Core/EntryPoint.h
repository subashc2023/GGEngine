#pragma once

#if defined(GG_PLATFORM_WINDOWS) || defined(GG_PLATFORM_LINUX) || defined(GG_PLATFORM_MACOS)

extern GGEngine::Application* GGEngine::CreateApplication();

int main(int argc, char** argv)
{
    GG_PROFILE_BEGIN_SESSION("Startup", "GGProfile-Startup.json");
    GGEngine::Log::Init();
    GGEngine::AssetManager::Get().Init();
    GG_CORE_INFO("GGEngine initialized");
    auto app = GGEngine::CreateApplication();
    GG_PROFILE_END_SESSION();

    GG_PROFILE_BEGIN_SESSION("Runtime", "GGProfile-Runtime.json");
    app->Run();
    GG_PROFILE_END_SESSION();

    GG_PROFILE_BEGIN_SESSION("Shutdown", "GGProfile-Shutdown.json");
    delete app;
    GG_PROFILE_END_SESSION();

    return 0;
}

#else
    #error "Unsupported platform! GGEngine supports Windows, Linux, and macOS."
#endif
