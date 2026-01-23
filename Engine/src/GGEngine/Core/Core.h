#pragma once

#include <memory>

// =============================================================================
// Platform Detection
// =============================================================================
// CMake should define these, but provide fallback auto-detection
#if !defined(GG_PLATFORM_WINDOWS) && !defined(GG_PLATFORM_LINUX) && !defined(GG_PLATFORM_MACOS)
    #if defined(_WIN32) || defined(_WIN64)
        #define GG_PLATFORM_WINDOWS
    #elif defined(__linux__)
        #define GG_PLATFORM_LINUX
    #elif defined(__APPLE__) && defined(__MACH__)
        #include <TargetConditionals.h>
        #if TARGET_OS_MAC
            #define GG_PLATFORM_MACOS
        #endif
    #endif
#endif

// Verify we have a supported platform
#if !defined(GG_PLATFORM_WINDOWS) && !defined(GG_PLATFORM_LINUX) && !defined(GG_PLATFORM_MACOS)
    #error "Unsupported platform! GGEngine supports Windows, Linux, and macOS."
#endif

// =============================================================================
// DLL/Shared Library Export Macros
// =============================================================================
#ifdef GG_PLATFORM_WINDOWS
    #ifdef GG_BUILD_DLL
        #define GG_API __declspec(dllexport)
    #elif defined(GG_DLL)
        #define GG_API __declspec(dllimport)
    #else
        #define GG_API
    #endif
#else
    // Linux/macOS: Use visibility attributes for shared libraries
    #ifdef GG_BUILD_DLL
        #define GG_API __attribute__((visibility("default")))
    #else
        #define GG_API
    #endif
#endif

// =============================================================================
// Debug Break
// =============================================================================
#ifdef GG_PLATFORM_WINDOWS
    #define GG_DEBUGBREAK() __debugbreak()
#elif defined(GG_PLATFORM_LINUX) || defined(GG_PLATFORM_MACOS)
    #include <signal.h>
    #define GG_DEBUGBREAK() raise(SIGTRAP)
#else
    #define GG_DEBUGBREAK()
#endif

// =============================================================================
// Assertions
// =============================================================================
#ifdef GG_ENABLE_ASSERTS
    #define GG_ASSERT(x, ...) { if (!(x)) { GG_CORE_ERROR("Assertion Failed: {0}", __VA_ARGS__); GG_DEBUGBREAK(); } }
    #define GG_CORE_ASSERT(x, ...) { if (!(x)) { GG_CORE_ERROR("Assertion Failed: {0}", __VA_ARGS__); GG_DEBUGBREAK(); } }
#else
    #define GG_ASSERT(x, ...)
    #define GG_CORE_ASSERT(x, ...)
#endif

// =============================================================================
// Deprecation Macro
// =============================================================================
#if defined(__GNUC__) || defined(__clang__)
    #define GG_DEPRECATED(msg) __attribute__((deprecated(msg)))
#elif defined(_MSC_VER)
    #define GG_DEPRECATED(msg) __declspec(deprecated(msg))
#else
    #define GG_DEPRECATED(msg)
#endif

// =============================================================================
// Utility Macros
// =============================================================================
#define BIT(x) (1 << x)

// DEPRECATED: Use lambdas instead: [this](auto& e) { return OnEvent(e); }
#define GG_BIND_EVENT_FN(fn) std::bind(&fn, this, std::placeholders::_1)

// =============================================================================
// Smart Pointer Aliases
// =============================================================================
namespace GGEngine {

    template<typename T>
    using Scope = std::unique_ptr<T>;

    template<typename T, typename ... Args>
    constexpr Scope<T> CreateScope(Args&& ... args)
    {
        return std::make_unique<T>(std::forward<Args>(args)...);
    }

    template<typename T>
    using Ref = std::shared_ptr<T>;

    template<typename T, typename ... Args>
    constexpr Ref<T> CreateRef(Args&& ... args)
    {
        return std::make_shared<T>(std::forward<Args>(args)...);
    }

}
