//! GPU memory through VMA, the Vulkan Memory Allocator: `build.rs` compiles its C++ and
//! `bindings.rs` declares it.
//!
//! VMA suballocates resources from large blocks of device memory, and keeps to the device's limit
//! on how many blocks there can be (4,096 on KosmicKrisp): past three quarters of it, it stops
//! giving large resources blocks of their own, and it refuses to go beyond it.

use std::ffi::{CStr, c_char, c_int};
use std::ptr;

use ash::vk;
use gg_core::Result;
use tracing::error;

use crate::vk_error;

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unsafe_op_in_unsafe_fn
)]
mod bindings;

use bindings::*;

/// Vulkan's types under the C names the bindings use. C's function pointers can be null, so
/// they're optional here; the `KHR` ones are the core functions they were promoted to.
#[allow(non_camel_case_types)]
mod vulkan {
    use ash::vk;

    pub type VkAllocationCallbacks = vk::AllocationCallbacks<'static>;
    pub type VkBool32 = vk::Bool32;
    pub type VkBuffer = vk::Buffer;
    pub type VkBufferCreateInfo = vk::BufferCreateInfo<'static>;
    pub type VkDevice = vk::Device;
    pub type VkDeviceMemory = vk::DeviceMemory;
    pub type VkDeviceSize = vk::DeviceSize;
    pub type VkExternalMemoryHandleTypeFlagsKHR = vk::ExternalMemoryHandleTypeFlags;
    pub type VkFlags = vk::Flags;
    pub type VkImage = vk::Image;
    pub type VkImageCreateInfo = vk::ImageCreateInfo<'static>;
    pub type VkInstance = vk::Instance;
    pub type VkMemoryPropertyFlags = vk::MemoryPropertyFlags;
    pub type VkMemoryRequirements = vk::MemoryRequirements;
    pub type VkPhysicalDevice = vk::PhysicalDevice;
    pub type VkPhysicalDeviceMemoryProperties = vk::PhysicalDeviceMemoryProperties;
    pub type VkPhysicalDeviceProperties = vk::PhysicalDeviceProperties;
    pub type VkResult = vk::Result;

    pub type PFN_vkAllocateMemory = Option<vk::PFN_vkAllocateMemory>;
    pub type PFN_vkBindBufferMemory = Option<vk::PFN_vkBindBufferMemory>;
    pub type PFN_vkBindBufferMemory2KHR = Option<vk::PFN_vkBindBufferMemory2>;
    pub type PFN_vkBindImageMemory = Option<vk::PFN_vkBindImageMemory>;
    pub type PFN_vkBindImageMemory2KHR = Option<vk::PFN_vkBindImageMemory2>;
    pub type PFN_vkCmdCopyBuffer = Option<vk::PFN_vkCmdCopyBuffer>;
    pub type PFN_vkCreateBuffer = Option<vk::PFN_vkCreateBuffer>;
    pub type PFN_vkCreateImage = Option<vk::PFN_vkCreateImage>;
    pub type PFN_vkDestroyBuffer = Option<vk::PFN_vkDestroyBuffer>;
    pub type PFN_vkDestroyImage = Option<vk::PFN_vkDestroyImage>;
    pub type PFN_vkFlushMappedMemoryRanges = Option<vk::PFN_vkFlushMappedMemoryRanges>;
    pub type PFN_vkFreeMemory = Option<vk::PFN_vkFreeMemory>;
    pub type PFN_vkGetBufferMemoryRequirements = Option<vk::PFN_vkGetBufferMemoryRequirements>;
    pub type PFN_vkGetBufferMemoryRequirements2KHR = Option<vk::PFN_vkGetBufferMemoryRequirements2>;
    pub type PFN_vkGetDeviceBufferMemoryRequirementsKHR =
        Option<vk::PFN_vkGetDeviceBufferMemoryRequirements>;
    pub type PFN_vkGetDeviceImageMemoryRequirementsKHR =
        Option<vk::PFN_vkGetDeviceImageMemoryRequirements>;
    pub type PFN_vkGetDeviceProcAddr = Option<vk::PFN_vkGetDeviceProcAddr>;
    pub type PFN_vkGetImageMemoryRequirements = Option<vk::PFN_vkGetImageMemoryRequirements>;
    pub type PFN_vkGetImageMemoryRequirements2KHR = Option<vk::PFN_vkGetImageMemoryRequirements2>;
    pub type PFN_vkGetInstanceProcAddr = Option<vk::PFN_vkGetInstanceProcAddr>;
    pub type PFN_vkGetPhysicalDeviceMemoryProperties =
        Option<vk::PFN_vkGetPhysicalDeviceMemoryProperties>;
    pub type PFN_vkGetPhysicalDeviceMemoryProperties2KHR =
        Option<vk::PFN_vkGetPhysicalDeviceMemoryProperties2>;
    pub type PFN_vkGetPhysicalDeviceProperties = Option<vk::PFN_vkGetPhysicalDeviceProperties>;
    pub type PFN_vkGetPhysicalDeviceProperties2KHR = Option<vk::PFN_vkGetPhysicalDeviceProperties2>;
    pub type PFN_vkInvalidateMappedMemoryRanges = Option<vk::PFN_vkInvalidateMappedMemoryRanges>;
    pub type PFN_vkMapMemory = Option<vk::PFN_vkMapMemory>;
    pub type PFN_vkUnmapMemory = Option<vk::PFN_vkUnmapMemory>;
}

/// Who reads and writes a resource's memory, which decides where it lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Memory {
    /// Only the GPU: render targets, textures, and buffers the GPU fills.
    Gpu,
    /// The CPU writes it, in order, and the GPU reads it: per-frame data. It stays mapped, and
    /// lives where VMA finds best for what it's used for, such as GPU memory the CPU can write
    /// directly.
    Upload,
    /// The CPU writes it once, in order, and the GPU copies from it: staging for uploads. It stays
    /// mapped, lives in the CPU's memory, and has memory of its own, so that freeing it gives the
    /// memory back.
    Staging,
    /// The GPU writes it and the CPU reads it back. It stays mapped.
    Readback,
}

/// VMA's allocator for one device.
pub(crate) struct Allocator {
    raw: VmaAllocator,
}

// SAFETY: VMA synchronizes the allocator itself, since it isn't made externally synchronized.
unsafe impl Send for Allocator {}
// SAFETY: as above.
unsafe impl Sync for Allocator {}

/// Memory VMA handed out for one buffer or image.
pub(crate) struct Allocation {
    raw: VmaAllocation,
    /// Where the CPU sees the memory, for memory that stays mapped; null otherwise.
    mapped: *mut u8,
}

// SAFETY: an allocation is a handle VMA lets any thread use, and its mapping is written only
// through `&mut` to the resource that owns it.
unsafe impl Send for Allocation {}
// SAFETY: as above.
unsafe impl Sync for Allocation {}

impl Allocation {
    pub(crate) fn mapped(&self) -> *mut u8 {
        self.mapped
    }
}

impl Allocator {
    /// # Safety
    ///
    /// The instance and device must be live and outlive the allocator, `physical` must be the
    /// device's, and the device must have buffer device addresses enabled.
    pub(crate) unsafe fn new(
        entry: &ash::Entry,
        instance: &ash::Instance,
        physical: vk::PhysicalDevice,
        device: &ash::Device,
    ) -> Result<Self> {
        // SAFETY: every field is a nullable pointer, for which zero is null; VMA fetches the
        // functions left null through the two it's given.
        let mut functions: VmaVulkanFunctions = unsafe { std::mem::zeroed() };
        functions.vkGetInstanceProcAddr = Some(entry.static_fn().get_instance_proc_addr);
        functions.vkGetDeviceProcAddr = Some(instance.fp_v1_0().get_device_proc_addr);
        let info = VmaAllocatorCreateInfo {
            flags: VmaAllocatorCreateFlagBits::VMA_ALLOCATOR_CREATE_BUFFER_DEVICE_ADDRESS_BIT.0
                as u32,
            physicalDevice: physical,
            device: device.handle(),
            preferredLargeHeapBlockSize: 0,
            pAllocationCallbacks: ptr::null(),
            pDeviceMemoryCallbacks: ptr::null(),
            pHeapSizeLimit: ptr::null(),
            pVulkanFunctions: &functions,
            instance: instance.handle(),
            // The instance's version; see `Instance::new`.
            vulkanApiVersion: vk::API_VERSION_1_3,
            pTypeExternalMemoryHandleTypes: ptr::null(),
        };
        let mut raw = ptr::null_mut();
        // SAFETY: the caller guarantees the handles; the info and functions outlive the call.
        let result = unsafe { vmaCreateAllocator(&info, &mut raw) };
        check("vmaCreateAllocator", result)?;
        Ok(Self { raw })
    }

    /// Makes a buffer and binds memory to it.
    pub(crate) fn create_buffer(
        &self,
        info: &vk::BufferCreateInfo<'_>,
        memory: Memory,
    ) -> Result<(vk::Buffer, Allocation)> {
        let mut buffer = vk::Buffer::null();
        let mut raw = ptr::null_mut();
        // SAFETY: zero is valid for every field of this plain-data struct, which VMA fills.
        let mut allocation_info: VmaAllocationInfo = unsafe { std::mem::zeroed() };
        // SAFETY: the allocator is live, and the infos outlive the call; the buffer info's
        // lifetime only constrains its pointers, which VMA reads during the call.
        let result = unsafe {
            vmaCreateBuffer(
                self.raw,
                ptr::from_ref(info).cast(),
                &creation(memory),
                &mut buffer,
                &mut raw,
                &mut allocation_info,
            )
        };
        check("vmaCreateBuffer", result)?;
        Ok((buffer, allocation(raw, &allocation_info)))
    }

    /// Makes an image and binds memory to it.
    pub(crate) fn create_image(
        &self,
        info: &vk::ImageCreateInfo<'_>,
        memory: Memory,
    ) -> Result<(vk::Image, Allocation)> {
        let mut image = vk::Image::null();
        let mut raw = ptr::null_mut();
        // SAFETY: as in `create_buffer`.
        let mut allocation_info: VmaAllocationInfo = unsafe { std::mem::zeroed() };
        // SAFETY: as in `create_buffer`.
        let result = unsafe {
            vmaCreateImage(
                self.raw,
                ptr::from_ref(info).cast(),
                &creation(memory),
                &mut image,
                &mut raw,
                &mut allocation_info,
            )
        };
        check("vmaCreateImage", result)?;
        Ok((image, allocation(raw, &allocation_info)))
    }

    /// # Safety
    ///
    /// The buffer and allocation must have come from `create_buffer` together, and the GPU
    /// must be done with the buffer.
    pub(crate) unsafe fn destroy_buffer(&self, buffer: vk::Buffer, allocation: Allocation) {
        // SAFETY: the caller guarantees it.
        unsafe { vmaDestroyBuffer(self.raw, buffer, allocation.raw) };
    }

    /// # Safety
    ///
    /// The image and allocation must have come from `create_image` together, and the GPU must
    /// be done with the image.
    pub(crate) unsafe fn destroy_image(&self, image: vk::Image, allocation: Allocation) {
        // SAFETY: the caller guarantees it.
        unsafe { vmaDestroyImage(self.raw, image, allocation.raw) };
    }

    /// Makes the CPU's writes to the allocation visible to the GPU. Memory the CPU writes may not
    /// be coherent, and then this is needed before the GPU reads it.
    pub(crate) fn flush(&self, allocation: &Allocation) -> Result<()> {
        // SAFETY: the allocator and allocation are live, and the range is the whole allocation:
        // from 0, and to its end for `WHOLE_SIZE`.
        let result = unsafe { vmaFlushAllocation(self.raw, allocation.raw, 0, vk::WHOLE_SIZE) };
        check("vmaFlushAllocation", result)
    }

    /// Makes the GPU's writes to the allocation visible to the CPU's reads, for memory that
    /// isn't coherent.
    pub(crate) fn invalidate(&self, allocation: &Allocation) -> Result<()> {
        // SAFETY: as in `flush`.
        let result =
            unsafe { vmaInvalidateAllocation(self.raw, allocation.raw, 0, vk::WHOLE_SIZE) };
        check("vmaInvalidateAllocation", result)
    }
}

impl Drop for Allocator {
    fn drop(&mut self) {
        // SAFETY: the device destroys the allocator last, after every resource; VMA asserts,
        // through `gg_vma_assert_failed`, that none was leaked.
        unsafe { vmaDestroyAllocator(self.raw) };
    }
}

/// How VMA should allocate for `memory`.
fn creation(memory: Memory) -> VmaAllocationCreateInfo {
    let (usage, flags) = match memory {
        Memory::Gpu => (
            VmaMemoryUsage::VMA_MEMORY_USAGE_AUTO_PREFER_DEVICE,
            VmaAllocationCreateFlagBits(0),
        ),
        Memory::Upload => (
            VmaMemoryUsage::VMA_MEMORY_USAGE_AUTO,
            VmaAllocationCreateFlagBits::VMA_ALLOCATION_CREATE_HOST_ACCESS_SEQUENTIAL_WRITE_BIT
                | VmaAllocationCreateFlagBits::VMA_ALLOCATION_CREATE_MAPPED_BIT,
        ),
        // Dedicated, since VMA keeps an emptied block for later, which would hold on to the
        // largest upload's memory for good.
        Memory::Staging => (
            VmaMemoryUsage::VMA_MEMORY_USAGE_AUTO_PREFER_HOST,
            VmaAllocationCreateFlagBits::VMA_ALLOCATION_CREATE_HOST_ACCESS_SEQUENTIAL_WRITE_BIT
                | VmaAllocationCreateFlagBits::VMA_ALLOCATION_CREATE_MAPPED_BIT
                | VmaAllocationCreateFlagBits::VMA_ALLOCATION_CREATE_DEDICATED_MEMORY_BIT,
        ),
        Memory::Readback => (
            VmaMemoryUsage::VMA_MEMORY_USAGE_AUTO,
            VmaAllocationCreateFlagBits::VMA_ALLOCATION_CREATE_HOST_ACCESS_RANDOM_BIT
                | VmaAllocationCreateFlagBits::VMA_ALLOCATION_CREATE_MAPPED_BIT,
        ),
    };
    VmaAllocationCreateInfo {
        flags: flags.0 as u32,
        usage,
        requiredFlags: vk::MemoryPropertyFlags::empty(),
        preferredFlags: vk::MemoryPropertyFlags::empty(),
        memoryTypeBits: 0,
        pool: ptr::null_mut(),
        pUserData: ptr::null_mut(),
        priority: 0.0,
        minAlignment: 0,
    }
}

fn allocation(raw: VmaAllocation, info: &VmaAllocationInfo) -> Allocation {
    Allocation {
        raw,
        mapped: info.pMappedData.cast(),
    }
}

fn check(call: &str, result: vk::Result) -> Result<()> {
    if result == vk::Result::SUCCESS {
        Ok(())
    } else {
        Err(vk_error(call, result))
    }
}

/// Where VMA reports a failed assert, as `vma.cpp` arranges.
#[unsafe(no_mangle)]
extern "C" fn gg_vma_assert_failed(
    expression: *const c_char,
    file: *const c_char,
    line: c_int,
) -> ! {
    // SAFETY: VMA passes string literals, which end in a nul and live forever.
    let (expression, file) = unsafe { (literal(expression), literal(file)) };
    error!(
        code = "gpu.vma_assert",
        %expression,
        %file,
        line,
        "VMA found a broken invariant; aborting"
    );
    std::process::abort()
}

/// A C string VMA passes, as text; empty for null.
///
/// # Safety
///
/// `text` must be null, or point to a nul-terminated string that lives through the call.
unsafe fn literal(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: the caller guarantees the string.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}
