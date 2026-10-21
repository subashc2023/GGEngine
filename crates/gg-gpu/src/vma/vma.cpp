// VMA's implementation, which build.rs compiles. The macros match those bindgen read the header
// with; see bindings.rs.

#define VMA_STATIC_VULKAN_FUNCTIONS 0
#define VMA_DYNAMIC_VULKAN_FUNCTIONS 1

// A failed assert is a broken invariant: Rust logs it, then aborts. On in every build, since the
// checks are cheap and continuing past one corrupts memory.
extern "C" {
[[noreturn]] void gg_vma_assert_failed(const char* expression, const char* file, int line);
}
#define VMA_ASSERT(expression) \
    ((expression) ? (void)0 : gg_vma_assert_failed(#expression, __FILE__, __LINE__))

#define VMA_IMPLEMENTATION
#include "vk_mem_alloc.h"
