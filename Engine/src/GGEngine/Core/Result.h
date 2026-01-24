#pragma once

#include "Core.h"
#include <string>
#include <utility>

namespace GGEngine {

    // ============================================================================
    // Result<T> - Lightweight error handling type
    // ============================================================================
    // A simple Result type for functions that can fail with an error message.
    // Inspired by Rust's Result<T, E> but simplified for C++ game engine use.
    //
    // Usage:
    //   Result<RHITextureHandle> CreateTexture(const Spec& spec) {
    //       if (failed)
    //           return Result<RHITextureHandle>::Err("vkCreateImage failed: ...");
    //       return Result<RHITextureHandle>::Ok(handle);
    //   }
    //
    //   auto result = CreateTexture(spec);
    //   if (result.IsErr()) {
    //       GG_CORE_ERROR("Texture creation failed: {}", result.Error());
    //       return;
    //   }
    //   auto handle = result.Value();
    //
    // Design decisions:
    // - Uses std::string for error messages (simple, debuggable)
    // - No exceptions (matches engine philosophy)
    // - Minimal overhead on success path
    // - Assertions on invalid access (catches bugs in debug)
    // ============================================================================

    template<typename T>
    class Result
    {
    public:
        // ========================================================================
        // Factory Methods
        // ========================================================================

        // Create a successful result with a value
        static Result Ok(T value)
        {
            return Result(std::move(value));
        }

        // Create a failed result with an error message
        static Result Err(std::string error)
        {
            return Result(std::move(error), ErrorTag{});
        }

        // ========================================================================
        // State Queries
        // ========================================================================

        bool IsOk() const { return m_HasValue; }
        bool IsErr() const { return !m_HasValue; }

        // Explicit bool conversion - true if Ok
        explicit operator bool() const { return m_HasValue; }

        // ========================================================================
        // Value Access
        // ========================================================================

        // Get the value (asserts if error)
        T& Value() &
        {
            GG_CORE_ASSERT(m_HasValue, "Result::Value() called on error result");
            return m_Value;
        }

        const T& Value() const &
        {
            GG_CORE_ASSERT(m_HasValue, "Result::Value() called on error result");
            return m_Value;
        }

        T&& Value() &&
        {
            GG_CORE_ASSERT(m_HasValue, "Result::Value() called on error result");
            return std::move(m_Value);
        }

        // Get value or return fallback (no assert)
        T ValueOr(T fallback) const &
        {
            return m_HasValue ? m_Value : std::move(fallback);
        }

        T ValueOr(T fallback) &&
        {
            return m_HasValue ? std::move(m_Value) : std::move(fallback);
        }

        // ========================================================================
        // Error Access
        // ========================================================================

        // Get the error message (asserts if success)
        const std::string& Error() const
        {
            GG_CORE_ASSERT(!m_HasValue, "Result::Error() called on success result");
            return m_Error;
        }

        // Get error or empty string (no assert)
        const std::string& ErrorOr(const std::string& fallback = "") const
        {
            return m_HasValue ? fallback : m_Error;
        }

        // ========================================================================
        // Monadic Operations
        // ========================================================================

        // Transform the value if Ok, propagate error if Err
        template<typename F>
        auto Map(F&& func) -> Result<decltype(func(std::declval<T>()))>
        {
            using U = decltype(func(std::declval<T>()));
            if (m_HasValue)
                return Result<U>::Ok(func(std::move(m_Value)));
            return Result<U>::Err(std::move(m_Error));
        }

        // Chain operations that return Result
        template<typename F>
        auto AndThen(F&& func) -> decltype(func(std::declval<T>()))
        {
            using ResultU = decltype(func(std::declval<T>()));
            if (m_HasValue)
                return func(std::move(m_Value));
            return ResultU::Err(std::move(m_Error));
        }

    private:
        // Tag type for error constructor disambiguation
        struct ErrorTag {};

        // Success constructor
        explicit Result(T value)
            : m_Value(std::move(value))
            , m_HasValue(true)
        {
        }

        // Error constructor
        explicit Result(std::string error, ErrorTag)
            : m_Error(std::move(error))
            , m_HasValue(false)
        {
        }

        T m_Value{};
        std::string m_Error;
        bool m_HasValue = false;
    };

    // ============================================================================
    // Result<void> Specialization
    // ============================================================================
    // For operations that succeed or fail but don't return a value.
    //
    // Usage:
    //   Result<void> Initialize() {
    //       if (failed)
    //           return Result<void>::Err("Init failed");
    //       return Result<void>::Ok();
    //   }
    // ============================================================================

    template<>
    class Result<void>
    {
    public:
        // ========================================================================
        // Factory Methods
        // ========================================================================

        static Result Ok()
        {
            return Result(true);
        }

        static Result Err(std::string error)
        {
            return Result(std::move(error));
        }

        // ========================================================================
        // State Queries
        // ========================================================================

        bool IsOk() const { return m_IsOk; }
        bool IsErr() const { return !m_IsOk; }

        explicit operator bool() const { return m_IsOk; }

        // ========================================================================
        // Error Access
        // ========================================================================

        const std::string& Error() const
        {
            GG_CORE_ASSERT(!m_IsOk, "Result::Error() called on success result");
            return m_Error;
        }

        const std::string& ErrorOr(const std::string& fallback = "") const
        {
            return m_IsOk ? fallback : m_Error;
        }

    private:
        explicit Result(bool ok) : m_IsOk(ok) {}
        explicit Result(std::string error) : m_Error(std::move(error)), m_IsOk(false) {}

        std::string m_Error;
        bool m_IsOk = false;
    };

    // ============================================================================
    // Convenience Macros
    // ============================================================================
    // These macros help reduce boilerplate when propagating errors.
    //
    // Usage:
    //   Result<Pipeline> CreatePipeline() {
    //       GG_TRY(shader, LoadShader("test.spv"));  // Propagates error if LoadShader fails
    //       return Result<Pipeline>::Ok(MakePipeline(shader));
    //   }
    // ============================================================================

    // Unwrap result or return error (for functions returning Result<T>)
    #define GG_TRY(var, expr) \
        auto _result_##var = (expr); \
        if (_result_##var.IsErr()) \
            return decltype(_result_##var)::Err(_result_##var.Error()); \
        auto var = std::move(_result_##var).Value()

    // Check void result or return error
    #define GG_TRY_VOID(expr) \
        do { \
            auto _result = (expr); \
            if (_result.IsErr()) \
                return _result; \
        } while(0)

}

