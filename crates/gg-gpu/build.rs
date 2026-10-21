//! Compiles VMA, from the sources vendored in `vendor/`. Its Rust declarations are generated once
//! and checked in, as `src/vma/bindings.rs` says.

fn main() {
    println!("cargo::rerun-if-changed=src/vma/vma.cpp");
    println!("cargo::rerun-if-changed=vendor");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("src/vma/vma.cpp")
        .include("vendor/VulkanMemoryAllocator/include")
        .include("vendor/Vulkan-Headers/include")
        // VMA's own warnings aren't ours to fix.
        .warnings(false)
        .compile("vma");
}
