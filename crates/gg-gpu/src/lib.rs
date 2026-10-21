//! The Vulkan layer, built on `ash`: instance and device setup, memory, buffers and images,
//! bindless descriptors, pipelines, swapchain, synchronization and queue submission.
//!
//! `gg-renderer` is its only client; the engine reaches the GPU through the renderer.

// Vulkan through ash is an unsafe API; wrapping it in safe types is this crate's job.
#![allow(unsafe_code)]
