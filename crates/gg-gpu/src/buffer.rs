//! Buffers in memory that VMA allocates.

use std::sync::Arc;

use ash::vk;
use gg_core::Result;

use crate::Device;
use crate::device::Retired;
use crate::vma::{Allocation, Memory};

/// A buffer and its memory. Dropping it retires both until the GPU is done with them.
pub(crate) struct Buffer {
    device: Arc<Device>,
    raw: vk::Buffer,
    /// Taken when the buffer is dropped.
    allocation: Option<Allocation>,
    size: u64,
    /// Where shaders find it, if its usage includes `SHADER_DEVICE_ADDRESS`; 0 otherwise.
    address: vk::DeviceAddress,
}

impl Buffer {
    /// A buffer of `size` bytes, with `name` as its debug name.
    pub(crate) fn new(
        device: &Arc<Device>,
        size: u64,
        usage: vk::BufferUsageFlags,
        memory: Memory,
        name: &str,
    ) -> Result<Self> {
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let (raw, allocation) = device.allocator().create_buffer(&info, memory)?;
        let address = if usage.contains(vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS) {
            let info = vk::BufferDeviceAddressInfo::default().buffer(raw);
            // SAFETY: the buffer is live, bound to memory, and made for device addresses.
            unsafe { device.raw().get_buffer_device_address(&info) }
        } else {
            0
        };
        // SAFETY: the buffer is live.
        unsafe { device.set_name(raw, name) };
        Ok(Self {
            device: device.clone(),
            raw,
            allocation: Some(allocation),
            size,
            address,
        })
    }

    pub(crate) fn raw(&self) -> vk::Buffer {
        self.raw
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn address(&self) -> vk::DeviceAddress {
        self.address
    }

    fn allocation(&self) -> &Allocation {
        self.allocation
            .as_ref()
            .expect("a live buffer has its allocation")
    }

    /// Copies `bytes` into the buffer at `offset`, for memory the CPU writes. The GPU sees
    /// them once [`Buffer::flush`] has covered them.
    ///
    /// # Panics
    ///
    /// If the buffer isn't mapped, or the bytes don't fit.
    pub(crate) fn write(&mut self, offset: u64, bytes: &[u8]) {
        let mapped = self.allocation().mapped();
        assert!(!mapped.is_null(), "only mapped buffers can be written");
        let end = offset.checked_add(bytes.len() as u64);
        assert!(
            end.is_some_and(|end| end <= self.size),
            "{} bytes at {offset} overflow a buffer of {}",
            bytes.len(),
            self.size
        );
        // SAFETY: the mapping covers the whole buffer, and the range was just checked; `&mut`
        // keeps other writes out, and the caller's frame keeps the GPU off memory it may still
        // be reading.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.add(offset as usize), bytes.len());
        }
    }

    /// Makes the CPU's writes to `size` bytes at `offset` visible to the GPU.
    pub(crate) fn flush(&self, offset: u64, size: u64) -> Result<()> {
        self.device
            .allocator()
            .flush(self.allocation(), offset, size)
    }

    /// Copies the whole buffer out, once the GPU has finished writing it.
    ///
    /// # Panics
    ///
    /// If the buffer isn't mapped.
    pub(crate) fn read(&self) -> Result<Vec<u8>> {
        let mapped = self.allocation().mapped();
        assert!(!mapped.is_null(), "only mapped buffers can be read");
        self.device
            .allocator()
            .invalidate(self.allocation(), 0, self.size)?;
        // SAFETY: the mapping covers the whole buffer, and the GPU's writes are visible.
        Ok(unsafe { std::slice::from_raw_parts(mapped, self.size as usize) }.to_vec())
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            self.device.retire(Retired::Buffer(self.raw, allocation));
        }
    }
}

/// A buffer in the GPU's own memory, which frames fill ([`Frame::write_buffer`]): vertices and
/// other data that shaders read through its address, and the indices that draws take. Dropping
/// it retires it until the GPU is done with it.
///
/// [`Frame::write_buffer`]: crate::Frame::write_buffer
pub struct GpuBuffer {
    buffer: Buffer,
    /// Whether a frame has written it, so that a later write waits for the frames reading it.
    written: bool,
}

impl GpuBuffer {
    /// A buffer of `size` bytes, with `name` as its debug name. What it holds is undefined until
    /// a frame writes it.
    ///
    /// # Panics
    ///
    /// If `size` is 0, which Vulkan doesn't allow.
    pub fn new(device: &Arc<Device>, size: u64, name: &str) -> Result<Self> {
        assert!(size > 0, "a buffer holds at least a byte");
        let usage = vk::BufferUsageFlags::INDEX_BUFFER
            | vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
            | vk::BufferUsageFlags::TRANSFER_DST;
        Ok(Self {
            buffer: Buffer::new(device, size, usage, Memory::Gpu, name)?,
            written: false,
        })
    }

    pub fn size(&self) -> u64 {
        self.buffer.size()
    }

    /// Where shaders read the buffer, through a buffer pointer.
    pub fn address(&self) -> u64 {
        self.buffer.address()
    }

    pub(crate) fn raw(&self) -> vk::Buffer {
        self.buffer.raw()
    }

    pub(crate) fn written(&self) -> bool {
        self.written
    }

    pub(crate) fn set_written(&mut self) {
        self.written = true;
    }
}
