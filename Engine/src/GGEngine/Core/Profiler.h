#pragma once

#include "GGEngine/Core/Core.h"
#include "GGEngine/Debug/Instrumentor.h"
#include <chrono>
#include <vector>

namespace GGEngine {

    // Frame-based profiling result for ImGui display
    struct FrameProfileResult
    {
        const char* Name;
        float DurationMs;
    };

    // Frame-based profiler for ImGui display (clears each frame)
    class GG_API Profiler
    {
    public:
        static void BeginFrame();
        static void SubmitResult(const FrameProfileResult& result);
        static const std::vector<FrameProfileResult>& GetResults();

    private:
        static std::vector<FrameProfileResult> s_Results;
    };

    // Template-based Timer class for frame-based profiling
    // Also writes to Instrumentor if a session is active
    template<typename Func>
    class Timer
    {
    public:
        Timer(const char* name, Func&& callback)
            : m_Name(name)
            , m_Callback(std::forward<Func>(callback))
            , m_Stopped(false)
        {
            m_StartTimepoint = std::chrono::steady_clock::now();
        }

        ~Timer()
        {
            if (!m_Stopped)
                Stop();
        }

        void Stop()
        {
            auto endTimepoint = std::chrono::steady_clock::now();

            auto startUs = std::chrono::time_point_cast<std::chrono::microseconds>(m_StartTimepoint).time_since_epoch();
            auto endUs = std::chrono::time_point_cast<std::chrono::microseconds>(endTimepoint).time_since_epoch();

            auto elapsedTime = endUs - startUs;
            float durationMs = static_cast<float>(elapsedTime.count()) * 0.001f;

            // Submit to frame-based profiler for ImGui display
            m_Callback({ m_Name, durationMs });

            // Also submit to Instrumentor for file output
            FloatingPointMicroseconds highResStart{ m_StartTimepoint.time_since_epoch() };
            Instrumentor::Get().WriteProfile({ m_Name, highResStart, elapsedTime, std::this_thread::get_id() });

            m_Stopped = true;
        }

    private:
        const char* m_Name;
        Func m_Callback;
        std::chrono::time_point<std::chrono::steady_clock> m_StartTimepoint;
        bool m_Stopped;
    };

} // namespace GGEngine

// ============================================================================
// Profiling Macros - stripped in GG_DIST builds
// ============================================================================

// Helper macro to concatenate tokens with line number for unique variable names
#define GG_PROFILE_CONCAT_INNER(a, b) a ## b
#define GG_PROFILE_CONCAT(a, b) GG_PROFILE_CONCAT_INNER(a, b)

#ifdef GG_DIST
    // Strip profiling in distribution builds
    #define GG_PROFILE_BEGIN_SESSION(name, filepath)
    #define GG_PROFILE_END_SESSION()
    #define GG_PROFILE_SCOPE(name)
    #define GG_PROFILE_FUNCTION()
    #define GG_PROFILE_BEGIN_FRAME()
#else
    // Session management for file output
    #define GG_PROFILE_BEGIN_SESSION(name, filepath) \
        ::GGEngine::Instrumentor::Get().BeginSession(name, filepath)
    #define GG_PROFILE_END_SESSION() \
        ::GGEngine::Instrumentor::Get().EndSession()

    // Profile a named scope - writes to both ImGui display and file output
    #define GG_PROFILE_SCOPE(name) \
        auto GG_PROFILE_CONCAT(ggTimer, __LINE__) = ::GGEngine::Timer(name, \
            [](const ::GGEngine::FrameProfileResult& result) { \
                ::GGEngine::Profiler::SubmitResult(result); \
            })

    // Profile the current function using compiler-provided function signature
    #ifdef _MSC_VER
        #define GG_PROFILE_FUNCTION() GG_PROFILE_SCOPE(__FUNCSIG__)
    #else
        #define GG_PROFILE_FUNCTION() GG_PROFILE_SCOPE(__PRETTY_FUNCTION__)
    #endif

    // Begin a new profiling frame (clears previous results for ImGui display)
    #define GG_PROFILE_BEGIN_FRAME() ::GGEngine::Profiler::BeginFrame()
#endif
