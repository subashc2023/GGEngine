// VMA Implementation file
// This file provides the implementation for Vulkan Memory Allocator
// VMA is a single-header library that requires VMA_IMPLEMENTATION to be defined
// exactly once before including the header

#define VMA_IMPLEMENTATION
#define VMA_STATIC_VULKAN_FUNCTIONS 0
#define VMA_DYNAMIC_VULKAN_FUNCTIONS 1

// Suppress nullability warnings from VMA (third-party code)
#ifdef __clang__
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wnullability-completeness"
#endif

#include <vk_mem_alloc.h>

#ifdef __clang__
#pragma clang diagnostic pop
#endif
