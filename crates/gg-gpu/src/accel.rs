//! Acceleration structures, which compute shaders' ray queries traverse. Frames build them
//! ([`Frame::build_blas`], [`Frame::build_tlas`]) before their passes, on a device with
//! [ray queries](Device::ray_query).
//!
//! [`Frame::build_blas`]: crate::Frame::build_blas
//! [`Frame::build_tlas`]: crate::Frame::build_tlas

use std::sync::Arc;

use ash::vk;
use bytemuck::{Pod, Zeroable};
use gg_core::Result;

use crate::buffer::Buffer;
use crate::device::Retired;
use crate::vma::Memory;
use crate::{Device, GpuBuffer, vk_failed};

/// A mesh's triangles as rays find them: a bottom-level acceleration structure, built once.
/// Dropping it retires it until the GPU is done with it.
pub struct Blas {
    pub(crate) structure: Structure,
}

impl Blas {
    /// Where instances find it: [`TlasInstance::blas`].
    pub fn address(&self) -> u64 {
        self.structure.address
    }
}

/// What a [`Blas`] is built from: triangles of packed vertices, read in place.
///
/// A position is four 16-bit unsigned counts, and the BLAS holds the first three read as UNORM,
/// each 0 to 1: its instances' transforms take them into the world. The BLAS has no transform of
/// its own, as lavapipe (Mesa 25.0) multiplies one's offset by the fourth count, which models
/// fill with the bitangent's sign.
pub struct Triangles<'a> {
    /// The buffer holding the vertices, where the first one's position starts in bytes, the bytes
    /// from one vertex to the next, and how many there are.
    pub vertices: &'a GpuBuffer,
    pub first_vertex: u64,
    pub vertex_stride: u64,
    pub vertex_count: u32,
    /// The buffer holding the indices (32-bit), where they start in bytes, and how many triangles.
    pub indices: &'a GpuBuffer,
    pub first_index: u64,
    pub triangles: u32,
    /// Whether rays take every triangle as opaque. Otherwise each is a candidate that the shader
    /// accepts or not, once, as masked materials need.
    pub opaque: bool,
}

/// The instances rays find: a top-level acceleration structure over [`Blas`]es, rebuilt as they
/// move. Dropping it retires it until the GPU is done with it.
pub struct Tlas {
    pub(crate) device: Arc<Device>,
    /// Its debug name, which its structure and scratch take.
    pub(crate) name: String,
    /// What the last build made, if any.
    pub(crate) built: Option<Built>,
}

/// A [`Tlas`]'s structure and scratch, with room for `room` instances.
pub(crate) struct Built {
    pub(crate) structure: Structure,
    pub(crate) scratch: Scratch,
    pub(crate) room: u32,
}

impl Tlas {
    /// A TLAS named `name`, which holds nothing until a frame builds it.
    ///
    /// # Panics
    ///
    /// If the device has no ray queries.
    pub fn new(device: &Arc<Device>, name: &str) -> Result<Self> {
        assert!(
            device.ray_query(),
            "only a device with ray queries has acceleration structures"
        );
        Ok(Self {
            device: device.clone(),
            name: name.to_owned(),
            built: None,
        })
    }

    /// Where shaders find it, for `RaytracingAccelerationStructure(address)`; 0 before a build.
    pub fn address(&self) -> u64 {
        self.built
            .as_ref()
            .map_or(0, |built| built.structure.address)
    }
}

/// An instance of a [`Blas`]: `VkAccelerationStructureInstanceKHR`, field for field.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TlasInstance {
    /// Row-major 3x4: the instance's space to the TLAS's.
    pub transform: [[f32; 4]; 3],
    /// Low 24 bits: what shaders read as `CommittedInstanceID`. High 8: the mask; 0 hides it.
    pub index_and_mask: u32,
    /// Low 24 bits: unused here, 0. High 8: `VkGeometryInstanceFlagsKHR`.
    pub offset_and_flags: u32,
    /// The [`Blas::address`].
    pub blas: u64,
}

impl TlasInstance {
    /// In `offset_and_flags`: rays that cull back faces find this instance's too
    /// (`VK_GEOMETRY_INSTANCE_TRIANGLE_FACING_CULL_DISABLE_BIT_KHR`). Without it, its fronts are the
    /// triangles wound counterclockwise in its BLAS's right-handed space, as glTF winds them,
    /// whatever its transform: the facing is its BLAS's, which rays find, as the RTX 4090 shows.
    pub const BOTH_FACES: u32 = 0x1 << 24;
    /// In `offset_and_flags`: its triangles are candidates that shaders accept or not, whatever its
    /// BLAS says (`VK_GEOMETRY_INSTANCE_FORCE_NO_OPAQUE_BIT_KHR`), as masked materials need.
    pub const NOT_OPAQUE: u32 = 0x8 << 24;
}

/// An acceleration structure and the buffer it lives in.
pub(crate) struct Structure {
    device: Arc<Device>,
    pub(crate) raw: vk::AccelerationStructureKHR,
    pub(crate) address: u64,
    /// Retired after the structure, which its drop retires first, so it goes after.
    _buffer: Buffer,
}

impl Structure {
    /// A structure of `ty`, `size` bytes, named `name`.
    pub(crate) fn new(
        device: &Arc<Device>,
        ty: vk::AccelerationStructureTypeKHR,
        size: u64,
        name: &str,
    ) -> Result<Self> {
        let functions = device.acceleration_structure_fn();
        let usage = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
        let buffer = Buffer::new(device, size, usage, Memory::Gpu, name)?;
        let info = vk::AccelerationStructureCreateInfoKHR::default()
            .buffer(buffer.raw())
            .size(size)
            .ty(ty);
        // SAFETY: the device has acceleration structures, and the buffer is live, made for their
        // storage, and holds `size` bytes from offset 0, which is a multiple of 256.
        let raw =
            unsafe { functions.create_acceleration_structure(&info, None) }.map_err(vk_failed(
                "vkCreateAccelerationStructureKHR",
                format_args!("acceleration structure {name:?} of {size} bytes"),
            ))?;
        let info =
            vk::AccelerationStructureDeviceAddressInfoKHR::default().acceleration_structure(raw);
        // SAFETY: the structure is live; the device has its features, and the name is a debugging
        // aid.
        let address = unsafe {
            device.set_name(raw, name);
            functions.get_acceleration_structure_device_address(&info)
        };
        Ok(Self {
            device: device.clone(),
            raw,
            address,
            _buffer: buffer,
        })
    }
}

impl Drop for Structure {
    fn drop(&mut self) {
        self.device.retire(Retired::AccelerationStructure(self.raw));
    }
}

/// The memory a build works in, at `address`: a multiple of the device's scratch alignment.
pub(crate) struct Scratch {
    _buffer: Buffer,
    pub(crate) address: u64,
}

impl Scratch {
    /// Scratch of `size` bytes, named `name`. Its buffer holds an alignment more, as VMA aligns
    /// buffers to less than some GPUs' builds need (128 bytes on NVIDIA's).
    pub(crate) fn new(device: &Arc<Device>, size: u64, name: &str) -> Result<Self> {
        let alignment = device.scratch_alignment();
        let usage =
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
        let buffer = Buffer::new(device, size + alignment, usage, Memory::Gpu, name)?;
        let address = buffer.address().next_multiple_of(alignment);
        Ok(Self {
            _buffer: buffer,
            address,
        })
    }
}
