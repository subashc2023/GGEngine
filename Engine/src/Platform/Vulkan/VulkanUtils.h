#pragma once

#include "GGEngine/Core/Log.h"
#include <vulkan/vulkan.h>

namespace GGEngine {

    // Log Vulkan error and continue execution
    #define VK_CHECK(result, msg) \
        do { \
            if ((result) != VK_SUCCESS) { \
                GG_CORE_ERROR("Vulkan error: {} (code: {})", msg, static_cast<int>(result)); \
            } \
        } while(0)

    // Log Vulkan error and return from function
    #define VK_CHECK_RETURN(result, msg) \
        do { \
            if ((result) != VK_SUCCESS) { \
                GG_CORE_ERROR("Vulkan error: {} (code: {})", msg, static_cast<int>(result)); \
                return; \
            } \
        } while(0)

    // Log Vulkan error and return a value from function
    #define VK_CHECK_RETURN_VAL(result, msg, retval) \
        do { \
            if ((result) != VK_SUCCESS) { \
                GG_CORE_ERROR("Vulkan error: {} (code: {})", msg, static_cast<int>(result)); \
                return retval; \
            } \
        } while(0)

}
