#pragma once

#include "GGEngine/Core/Log.h"

#ifdef __OBJC__
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>
#endif

#include <string_view>

namespace GGEngine {

    // ============================================================================
    // NSError to String Helper
    // ============================================================================

#ifdef __OBJC__
    inline std::string NSErrorToString(NSError* error)
    {
        if (!error) return "Unknown error";
        return [[error localizedDescription] UTF8String];
    }
#endif

    // ============================================================================
    // Metal Error Checking Macros
    // ============================================================================

    // Log Metal error and continue execution
    #define MTL_CHECK(condition, msg) \
        do { \
            if (!(condition)) { \
                GG_CORE_ERROR("Metal error: {}", msg); \
            } \
        } while(0)

    // Log Metal error and return from function
    #define MTL_CHECK_RETURN(condition, msg) \
        do { \
            if (!(condition)) { \
                GG_CORE_ERROR("Metal error: {}", msg); \
                return; \
            } \
        } while(0)

    // Log Metal error and return a value from function
    #define MTL_CHECK_RETURN_VAL(condition, msg, retval) \
        do { \
            if (!(condition)) { \
                GG_CORE_ERROR("Metal error: {}", msg); \
                return retval; \
            } \
        } while(0)

    // Check NSError and log
    #define MTL_CHECK_ERROR(error, msg) \
        do { \
            if (error) { \
                GG_CORE_ERROR("Metal error: {} - {}", msg, NSErrorToString(error)); \
            } \
        } while(0)

    // Check NSError, log, and return
    #define MTL_CHECK_ERROR_RETURN(error, msg) \
        do { \
            if (error) { \
                GG_CORE_ERROR("Metal error: {} - {}", msg, NSErrorToString(error)); \
                return; \
            } \
        } while(0)

    // Check NSError, log, and return value
    #define MTL_CHECK_ERROR_RETURN_VAL(error, msg, retval) \
        do { \
            if (error) { \
                GG_CORE_ERROR("Metal error: {} - {}", msg, NSErrorToString(error)); \
                return retval; \
            } \
        } while(0)

}
