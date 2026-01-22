#pragma once

#include <memory>

#ifdef GG_PLATFORM_WINDOWS
    #ifdef GG_BUILD_DLL
        #define GG_API __declspec(dllexport)
    #elif defined(GG_DLL)
        #define GG_API __declspec(dllimport)
    #else
        #define GG_API
    #endif
#else
    #error "GGEngine only supports Windows platform"
#endif

#ifdef GG_ENABLE_ASSERTS
    #define GG_ASSERT(x, ...) { if (!(x)) { GG_CORE_ERROR("Assertion Failed: {0}", __VA_ARGS__); __debugbreak(); } }
    #define GG_CORE_ASSERT(x, ...) { if (!(x)) { GG_CORE_ERROR("Assertion Failed: {0}", __VA_ARGS__); __debugbreak(); } }
#else
    #define GG_ASSERT(x, ...)
    #define GG_CORE_ASSERT(x, ...)
#endif

#define BIT(x) (1 << x)

// DEPRECATED: Use lambdas instead: [this](auto& e) { return OnEvent(e); }
#define GG_BIND_EVENT_FN(fn) std::bind(&fn, this, std::placeholders::_1)

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