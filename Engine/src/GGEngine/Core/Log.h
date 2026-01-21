#pragma once

#include <memory>

#include "Core.h"
#include <spdlog/spdlog.h>
#include <spdlog/fmt/ostr.h>

namespace GGEngine {
    class GG_API Log {
    public:
        static void Init();

        inline static std::shared_ptr<spdlog::logger>& GetCoreLogger() { return m_CoreLogger; }
        inline static std::shared_ptr<spdlog::logger>& GetClientLogger() { return m_ClientLogger; }

    private:
        static std::shared_ptr<spdlog::logger> m_CoreLogger;
        static std::shared_ptr<spdlog::logger> m_ClientLogger; 

    };
}

// Strip logging in Dist builds
#ifdef GG_DIST
    #define GG_CORE_TRACE(...)
    #define GG_CORE_INFO(...)
    #define GG_CORE_WARN(...)
    #define GG_CORE_ERROR(...)
    #define GG_CORE_CRITICAL(...)

    #define GG_TRACE(...)
    #define GG_INFO(...)
    #define GG_WARN(...)
    #define GG_ERROR(...)
    #define GG_CRITICAL(...)
#else
    #define GG_CORE_TRACE(...) GGEngine::Log::GetCoreLogger()->trace(__VA_ARGS__)
    #define GG_CORE_INFO(...) GGEngine::Log::GetCoreLogger()->info(__VA_ARGS__)
    #define GG_CORE_WARN(...) GGEngine::Log::GetCoreLogger()->warn(__VA_ARGS__)
    #define GG_CORE_ERROR(...) GGEngine::Log::GetCoreLogger()->error(__VA_ARGS__)
    #define GG_CORE_CRITICAL(...) GGEngine::Log::GetCoreLogger()->critical(__VA_ARGS__)

    #define GG_TRACE(...) GGEngine::Log::GetClientLogger()->trace(__VA_ARGS__)
    #define GG_INFO(...) GGEngine::Log::GetClientLogger()->info(__VA_ARGS__)
    #define GG_WARN(...) GGEngine::Log::GetClientLogger()->warn(__VA_ARGS__)
    #define GG_ERROR(...) GGEngine::Log::GetClientLogger()->error(__VA_ARGS__)
    #define GG_CRITICAL(...) GGEngine::Log::GetClientLogger()->critical(__VA_ARGS__)
#endif