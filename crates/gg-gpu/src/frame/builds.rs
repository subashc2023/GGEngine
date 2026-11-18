//! Acceleration structures built before a frame's passes.

use std::slice;

use ash::vk;
use gg_core::Result;

use super::Frame;
use crate::accel::{Blas, Built, Scratch, Structure, TlasInstance, Triangles};
use crate::{Device, GpuBuffer, Tlas};

/// What reads the structures frames build: compute shaders' ray queries.
const TRACERS: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::COMPUTE_SHADER;

impl Frame<'_> {
    /// Builds a BLAS of `triangles` before the frame's passes, whose shaders then find it.
    ///
    /// # Panics
    ///
    /// - The device has no ray queries, a forgotten pass is open, or a pass has begun.
    /// - Another device made the buffers, or frames haven't taken them for drawing.
    /// - There's no triangle, or the triangles reach past their buffers.
    /// - The first vertex isn't on a 2-byte boundary, or the first index on a 4-byte one.
    pub fn build_blas(&mut self, triangles: &Triangles<'_>, name: &str) -> Result<Blas> {
        let commands = self.before_passes();
        let device = self.frames.device.clone();
        let Triangles {
            vertices,
            first_vertex,
            vertex_stride,
            vertex_count,
            indices,
            first_index,
            triangles: count,
            opaque,
        } = *triangles;
        assert!(
            device.ray_query(),
            "only a device with ray queries builds acceleration structures"
        );
        device.check_made("a BLAS's vertices", vertices.device());
        device.check_made("a BLAS's indices", indices.device());
        assert!(
            vertices.written() && indices.written(),
            "a BLAS is built from buffers frames have taken for drawing"
        );
        assert!(vertex_count > 0 && count > 0, "a BLAS has a triangle");
        let vertices_end = u64::from(vertex_count - 1)
            .checked_mul(vertex_stride)
            .and_then(|last| last.checked_add(first_vertex + 8));
        assert!(
            vertices_end.is_some_and(|end| end <= vertices.size()),
            "{vertex_count} vertices of {vertex_stride} bytes from {first_vertex} overflow a \
             buffer of {}",
            vertices.size()
        );
        let indices_end = (u64::from(count) * 12).checked_add(first_index);
        assert!(
            indices_end.is_some_and(|end| end <= indices.size()),
            "{count} triangles' indices from {first_index} overflow a buffer of {}",
            indices.size()
        );
        assert!(
            first_vertex.is_multiple_of(2) && first_index.is_multiple_of(4),
            "vertices start on a 2-byte boundary and indices on a 4-byte one, not {first_vertex} \
             and {first_index}"
        );
        let triangles = vk::AccelerationStructureGeometryTrianglesDataKHR::default()
            .vertex_format(vk::Format::R16G16B16A16_UNORM)
            .vertex_data(vk::DeviceOrHostAddressConstKHR {
                device_address: vertices.address() + first_vertex,
            })
            .vertex_stride(vertex_stride)
            .max_vertex(vertex_count - 1)
            .index_type(vk::IndexType::UINT32)
            .index_data(vk::DeviceOrHostAddressConstKHR {
                device_address: indices.address() + first_index,
            });
        let flags = if opaque {
            vk::GeometryFlagsKHR::OPAQUE
        } else {
            vk::GeometryFlagsKHR::NO_DUPLICATE_ANY_HIT_INVOCATION
        };
        let geometry = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
            .geometry(vk::AccelerationStructureGeometryDataKHR { triangles })
            .flags(flags);
        let ty = vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL;
        let info = build_info(ty, &geometry);
        let sizes = build_sizes(&device, &info, count);
        let blas = Blas {
            structure: Structure::new(&device, ty, sizes.acceleration_structure_size, name)?,
        };
        // Dropped once recorded: it retires until the frame has finished.
        let scratch = Scratch::new(&device, sizes.build_scratch_size, name)?;
        let info = info
            .dst_acceleration_structure(blas.structure.raw)
            .scratch_data(vk::DeviceOrHostAddressKHR {
                device_address: scratch.address,
            });
        // SAFETY: the commands are recording outside a pass. The structure, scratch and buffers
        // live until the frame has finished, as they retire by frames; their addresses are
        // aligned and the ranges fit, as checked above. The structure and scratch are new, and
        // what wrote the buffers made its writes visible to builds, as `buffer_readers` has them.
        unsafe {
            device.begin_label(commands, c"build BLAS");
            record_build(&device, commands, &info, count);
            device.end_label(commands);
        }
        self.built = true;
        Ok(blas)
    }

    /// Builds `tlas` over the `count` [`TlasInstance`]s from byte `first` of `instances`, before
    /// the frame's passes, after what earlier frames traced of it. Grows its room as needed, to
    /// the next power of two instances.
    ///
    /// # Panics
    ///
    /// - The device has no ray queries, a forgotten pass is open, or a pass has begun.
    /// - Another device made `tlas` or `instances`, or frames haven't taken `instances` for
    ///   drawing.
    /// - `first` isn't a multiple of 16, or the instances reach past `instances`' end.
    pub fn build_tlas(
        &mut self,
        tlas: &mut Tlas,
        instances: &GpuBuffer,
        first: u64,
        count: u32,
    ) -> Result<()> {
        let commands = self.before_passes();
        let device = self.frames.device.clone();
        assert!(
            device.ray_query(),
            "only a device with ray queries builds acceleration structures"
        );
        device.check_made("a TLAS built", tlas.device.handle());
        device.check_made("a TLAS's instances", instances.device());
        assert!(
            instances.written(),
            "a TLAS is built from instances frames have taken for drawing"
        );
        assert!(
            first.is_multiple_of(16),
            "a TLAS's instances start at a multiple of 16, not {first}"
        );
        let end = (u64::from(count) * size_of::<TlasInstance>() as u64).checked_add(first);
        assert!(
            end.is_some_and(|end| end <= instances.size()),
            "{count} instances from {first} overflow a buffer of {}",
            instances.size()
        );
        let data = vk::AccelerationStructureGeometryInstancesDataKHR::default().data(
            vk::DeviceOrHostAddressConstKHR {
                device_address: instances.address() + first,
            },
        );
        let geometry = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::INSTANCES)
            .geometry(vk::AccelerationStructureGeometryDataKHR { instances: data });
        let ty = vk::AccelerationStructureTypeKHR::TOP_LEVEL;
        let info = build_info(ty, &geometry);
        if tlas.built.as_ref().is_none_or(|built| built.room < count) {
            let room = count.max(1).next_power_of_two();
            let sizes = build_sizes(&device, &info, room);
            let size = sizes.acceleration_structure_size;
            let structure = Structure::new(&device, ty, size, &tlas.name)?;
            let scratch = Scratch::new(&device, sizes.build_scratch_size, &tlas.name)?;
            // The old one retires until the frames that traced it have finished.
            tlas.built = Some(Built {
                structure,
                scratch,
                room,
            });
        }
        let built = tlas.built.as_ref().expect("just built");
        let info = info
            .dst_acceleration_structure(built.structure.raw)
            .scratch_data(vk::DeviceOrHostAddressKHR {
                device_address: built.scratch.address,
            });
        // After earlier frames' traces of it and builds into it and its scratch, and this frame's
        // builds of the BLASes it holds.
        let before = vk::MemoryBarrier2::default()
            .src_stage_mask(TRACERS | vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
            .src_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)
            .dst_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
            .dst_access_mask(
                vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR
                    | vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR,
            );
        // SAFETY: as in `build_blas`, the barrier ordering the build after what else used the
        // structure and its scratch.
        unsafe {
            device.begin_label(commands, c"build TLAS");
            device.raw().cmd_pipeline_barrier2(
                commands,
                &vk::DependencyInfo::default().memory_barriers(slice::from_ref(&before)),
            );
            record_build(&device, commands, &info, count);
            device.end_label(commands);
        }
        self.built = true;
        Ok(())
    }

    /// Makes what the frame built visible to the ray queries of its passes and later frames' once
    /// a build has been recorded: before the first pass, or as the frame is submitted without one.
    pub(super) fn finish_builds(&mut self) {
        if !self.built {
            return;
        }
        self.built = false;
        // A pass on the compute queue waits for the batch the builds are in.
        self.frames.buffers_written = true;
        let after = vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ACCELERATION_STRUCTURE_BUILD_KHR)
            .src_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_WRITE_KHR)
            .dst_stage_mask(TRACERS)
            .dst_access_mask(vk::AccessFlags2::ACCELERATION_STRUCTURE_READ_KHR);
        let device = &self.frames.device;
        // SAFETY: the frame's first batch is recording, outside a pass: no pass has begun.
        unsafe {
            device.raw().cmd_pipeline_barrier2(
                self.frames.batch_commands[0],
                &vk::DependencyInfo::default().memory_barriers(slice::from_ref(&after)),
            );
        }
    }
}

/// A build of a structure of `ty` from `geometry`, without its destination and scratch.
fn build_info<'a>(
    ty: vk::AccelerationStructureTypeKHR,
    geometry: &'a vk::AccelerationStructureGeometryKHR<'a>,
) -> vk::AccelerationStructureBuildGeometryInfoKHR<'a> {
    vk::AccelerationStructureBuildGeometryInfoKHR::default()
        .ty(ty)
        .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
        .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
        .geometries(slice::from_ref(geometry))
}

/// The sizes of the structure and scratch `info` builds of up to `count` primitives need.
fn build_sizes(
    device: &Device,
    info: &vk::AccelerationStructureBuildGeometryInfoKHR<'_>,
    count: u32,
) -> vk::AccelerationStructureBuildSizesInfoKHR<'static> {
    let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
    // SAFETY: the device has acceleration structures, and `info` has one geometry, which a count
    // is given for; the query reads no addresses.
    unsafe {
        device
            .acceleration_structure_fn()
            .get_acceleration_structure_build_sizes(
                vk::AccelerationStructureBuildTypeKHR::DEVICE,
                info,
                &[count],
                &mut sizes,
            );
    }
    sizes
}

/// Records the build `info` makes of its one geometry's `count` primitives.
///
/// # Safety
///
/// `commands` must be recording outside a pass, and `info` must be a valid build whose memory
/// lives until the commands have finished.
unsafe fn record_build(
    device: &Device,
    commands: vk::CommandBuffer,
    info: &vk::AccelerationStructureBuildGeometryInfoKHR<'_>,
    count: u32,
) {
    let range = vk::AccelerationStructureBuildRangeInfoKHR {
        primitive_count: count,
        ..Default::default()
    };
    let ranges = [&raw const range];
    // SAFETY: the caller guarantees the build; one info, with one geometry and its one range.
    // ash's wrapper would allocate.
    unsafe {
        (device
            .acceleration_structure_fn()
            .fp()
            .cmd_build_acceleration_structures_khr)(commands, 1, info, ranges.as_ptr());
    }
}
