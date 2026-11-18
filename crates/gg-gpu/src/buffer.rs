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
    /// Its debug name, which its errors use.
    name: String,
}

impl Buffer {
    /// A buffer of `size` bytes, with `name` as its debug name, shared by every queue the device
    /// has: one queue reads what another wrote once it has waited for it, as frames' semaphores
    /// have it, without a handover. Sharing can cost an image its compression; buffers have none.
    pub(crate) fn new(
        device: &Arc<Device>,
        size: u64,
        usage: vk::BufferUsageFlags,
        memory: Memory,
        name: &str,
    ) -> Result<Self> {
        let families = device.families();
        let info = vk::BufferCreateInfo::default().size(size).usage(usage);
        // Sharing takes two families or more.
        let info = if families.len() > 1 {
            info.sharing_mode(vk::SharingMode::CONCURRENT)
                .queue_family_indices(families)
        } else {
            info.sharing_mode(vk::SharingMode::EXCLUSIVE)
        };
        let (raw, allocation) = device.allocator().create_buffer(&info, memory, name)?;
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
            name: name.to_owned(),
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

    /// Copies `bytes` into the buffer at `offset`, for memory the CPU writes. The GPU sees them
    /// once [`Buffer::flush`] has covered them.
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

    /// Makes the CPU's writes visible to the GPU.
    pub(crate) fn flush(&self) -> Result<()> {
        self.device.allocator().flush(self.allocation(), &self.name)
    }

    /// Copies the whole buffer out, once the GPU has finished writing it.
    ///
    /// # Panics
    ///
    /// If the buffer isn't mapped.
    pub(crate) fn read(&self) -> Result<Vec<u8>> {
        let mut bytes = vec![0; self.size as usize];
        self.read_into(0, &mut bytes)?;
        Ok(bytes)
    }

    /// Copies the buffer's bytes from `offset` into `out`, once the GPU has finished writing them,
    /// without allocating.
    ///
    /// # Panics
    ///
    /// If the buffer isn't mapped, or the bytes run past its end.
    pub(crate) fn read_into(&self, offset: u64, out: &mut [u8]) -> Result<()> {
        let mapped = self.allocation().mapped();
        assert!(!mapped.is_null(), "only mapped buffers can be read");
        let end = offset.checked_add(out.len() as u64);
        assert!(
            end.is_some_and(|end| end <= self.size),
            "{} bytes at {offset} overflow a buffer of {}",
            out.len(),
            self.size
        );
        self.device
            .allocator()
            .invalidate(self.allocation(), &self.name)?;
        // SAFETY: the mapping covers the whole buffer, the range was just checked, and the GPU's
        // writes are visible; `out` is the caller's own, so it doesn't overlap the mapping.
        unsafe {
            std::ptr::copy_nonoverlapping(mapped.add(offset as usize), out.as_mut_ptr(), out.len());
        }
        Ok(())
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            self.device.retire(Retired::Buffer(self.raw, allocation));
        }
    }
}

/// A buffer in GPU memory. Dropping it retires it until the GPU is done.
///
/// Frames either fill it a part at a time ([`Frame::stream_buffer`]) and then take it for drawing
/// ([`Frame::finish_buffer`]), for vertices, other data shaders read through its address, and
/// draws' indices; or write into it as its data changes ([`Frame::write_buffer`]), as materials'
/// do.
///
/// [`Frame::stream_buffer`]: crate::Frame::stream_buffer
/// [`Frame::finish_buffer`]: crate::Frame::finish_buffer
/// [`Frame::write_buffer`]: crate::Frame::write_buffer
pub struct GpuBuffer {
    buffer: Buffer,
    /// How many of its bytes frames have copied, from the first.
    copied: u64,
    /// The transfer-queue submission of copies that hands it over to the graphics queue once its
    /// last part is recorded; 0 before.
    released: u64,
    /// Whether frames have filled it and taken it for drawing.
    written: bool,
}

impl GpuBuffer {
    /// A buffer of `size` bytes, with `name` as its debug name. Undefined until frames fill it.
    ///
    /// # Panics
    ///
    /// If `size` is 0, which Vulkan doesn't allow.
    pub fn new(device: &Arc<Device>, size: u64, name: &str) -> Result<Self> {
        assert!(size > 0, "a buffer holds at least a byte");
        let mut usage = vk::BufferUsageFlags::INDEX_BUFFER
            | vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
            | vk::BufferUsageFlags::TRANSFER_DST;
        // Meshes' vertices and indices, and TLASes' instances, are builds' input.
        if device.ray_query() {
            usage |= vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR;
        }
        Ok(Self {
            buffer: Buffer::new(device, size, usage, Memory::Gpu, name)?,
            copied: 0,
            released: 0,
            written: false,
        })
    }

    pub(crate) fn size(&self) -> u64 {
        self.buffer.size()
    }

    /// Where shaders read the buffer, through a buffer pointer.
    pub fn address(&self) -> u64 {
        self.buffer.address()
    }

    /// How many of its bytes frames have copied, from the first.
    pub fn streamed(&self) -> u64 {
        self.copied
    }

    pub(crate) fn raw(&self) -> vk::Buffer {
        self.buffer.raw()
    }

    pub(crate) fn device(&self) -> vk::Device {
        self.buffer.device.handle()
    }

    pub(crate) fn written(&self) -> bool {
        self.written
    }

    pub(crate) fn set_written(&mut self) {
        self.written = true;
    }

    /// Mutable `copied` and `released`.
    pub(crate) fn progress(&mut self) -> (&mut u64, &mut u64) {
        (&mut self.copied, &mut self.released)
    }
}
