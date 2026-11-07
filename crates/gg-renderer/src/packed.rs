//! Models' meshes as the GPU holds them: each vertex packed into 8- and 16-bit fields, 24 bytes
//! where a [`ModelVertex`] takes 64, unpacked by the model shaders as they read it; and, as the
//! cooker cuts a mesh's levels, meshlets, each up to [`MAX_MESHLET_VERTICES`] of its vertices and
//! [`MAX_MESHLET_TRIANGLES`] of its triangles, with the sphere round them and the cone they face
//! within, which views cull them by.
//!
//! A mesh's buffer holds, in order: a [`Header`], in two vertices' room, which says how its
//! vertices unpack and where its meshlets' parts are; its vertices; and, if it has meshlets, what
//! draws each, as [`GpuMeshlet`] has it, their vertices' indices among the mesh's, and their
//! triangles' bytes, each part starting at a word. Shaders reach every part through the buffer's
//! address, as `shaders/model.slang` reads them.

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::math::{Vec2, Vec3, libm};

use crate::mesh::{bad_mesh, check, check_lods};
use crate::{Lod, Lods, ModelVertex};

/// The most vertices a meshlet has: as many as NVIDIA recommends a mesh shader's workgroup
/// output, and meshoptimizer's README suggests. `MAX_VERTICES` in `shaders/model.slang`.
pub const MAX_MESHLET_VERTICES: u32 = 64;

/// The most triangles a meshlet has: 124 rather than NVIDIA's 126, as meshoptimizer takes a
/// multiple of 4. `MAX_TRIANGLES` in `shaders/model.slang`, whose vertex path draws each meshlet
/// as this many triangles, those past its own drawing nothing.
pub const MAX_MESHLET_TRIANGLES: u32 = 124;

/// A vertex of a model's mesh as the GPU holds it, `PackedVertex` in `shaders/model.slang`, which
/// unpacks it to what a [`ModelVertex`] holds, as nearly as its fields keep it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, NoUninit)]
pub struct PackedVertex {
    /// Where it is, in steps of its mesh's [`Packing::step`] from its offset.
    pub position: [u16; 3],
    /// Its bitangent's sign, as its tangent's W has it: 1, or -1.
    pub sign: i16,
    /// Its normal and its tangent, each a unit vector folded onto an octahedron and unfolded onto
    /// a square, whose two coordinates are signed 16-bit fractions of 1 (Cigolle et al., "A Survey
    /// of Efficient Representations for Independent Unit Vectors", 2014): within a few
    /// thousandths of a degree of the vector.
    pub normal: [i16; 2],
    pub tangent: [i16; 2],
    /// Where it samples its material's textures, as unsigned 16-bit fractions of its mesh's box of
    /// them, which [`Packing`] gives.
    pub uv: [u16; 2],
    /// Its color, as unsigned 8-bit fractions of 1: its red, green and blue sRGB-encoded, which
    /// keeps dark colors apart, and its alpha as it is.
    pub color: [u8; 4],
}

/// How a mesh's [`PackedVertex`]es unpack: a vertex's position is `offset + step * position`, and
/// its texture coordinates `uv_offset + uv_scale * uv`.
///
/// `step` is a power of two, the least that spans the mesh's longest side in 65,534 steps, and
/// `offset` a multiple of it, so that meshes whose sizes are within a factor of two of each other
/// put their vertices on one grid, and an edge two of them share keeps its ends together, where
/// each fitted to its own box would round them apart.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub struct Packing {
    pub offset: Vec3,
    pub step: f32,
    pub uv_offset: Vec2,
    pub uv_scale: Vec2,
}

/// What a mesh's buffer starts with, `Header` in `shaders/model.slang`: its [`Packing`], and where
/// its meshlets' parts start, in words from the buffer's start, or 0 for a mesh without them. Two
/// vertices' room, so that its vertices follow it in place.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
struct Header {
    packing: Packing,
    meshlets: u32,
    meshlet_vertices: u32,
    triangles: u32,
    unused: u32,
}

/// How many vertices' room a mesh's [`Header`] takes at its buffer's start: `HEADER` in
/// `shaders/model.slang`.
const HEADER_VERTICES: usize = 2;

const _: () = assert!(size_of::<Header>() == HEADER_VERTICES * size_of::<PackedVertex>());

/// What draws a meshlet, as a mesh's buffer holds it: `Meshlet` in `shaders/model.slang`.
#[repr(C)]
#[derive(Clone, Copy, Debug, NoUninit)]
struct GpuMeshlet {
    vertices: u32,
    vertex_count: u32,
    /// In bytes from the start of the mesh's triangles.
    triangles: u32,
    triangle_count: u32,
}

/// What culls a meshlet, as the renderer's table of them holds it: `MeshletBounds` in
/// `shaders/cull.slang`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, NoUninit)]
pub(crate) struct MeshletBounds {
    center: Vec3,
    radius: f32,
    /// [`Meshlet::cone`]'s bytes, its axis's X lowest.
    cone: u32,
    triangles: u32,
}

/// A meshlet of a mesh: some of its triangles, of few vertices, which views cull and draw apart
/// from the rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meshlet {
    /// Where its vertices start in [`Meshlets::vertices`], and how many it has, from 1 to
    /// [`MAX_MESHLET_VERTICES`].
    pub vertices: u32,
    pub vertex_count: u32,
    /// Where its triangles start in [`Meshlets::triangles`], three bytes each, each byte a corner's
    /// place among its vertices, winding as the mesh's; and how many it has, from 1 to
    /// [`MAX_MESHLET_TRIANGLES`].
    pub triangles: u32,
    pub triangle_count: u32,
    /// The center of a sphere round its triangles, in the mesh's space, and its radius.
    pub center: Vec3,
    pub radius: f32,
    /// The cone its triangles face within, as meshoptimizer's `meshopt_Bounds` gives it in bytes:
    /// its axis's X, Y and Z, then its cutoff, each a signed fraction of 127, rounded so that the
    /// cone holds every triangle's normal. Where the way from the camera to the sphere's center,
    /// `d`, has `dot(d, axis) >= cutoff * length(d) + radius`, the camera sees only those
    /// triangles' backs; a cutoff of 127 holds for no camera.
    pub cone: [i8; 4],
}

/// A mesh's meshlets, its levels' each cut into theirs: none for a mesh drawn whole, as a
/// development run draws a model's primitives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meshlets {
    pub meshlets: Vec<Meshlet>,
    /// The meshlets' vertices, each an index among the mesh's.
    pub vertices: Vec<u32>,
    /// The meshlets' triangles, a byte for each corner.
    pub triangles: Vec<u8>,
    /// Each level's meshlets, in the mesh's levels' order: the first, and how many.
    pub levels: Vec<[u32; 2]>,
}

/// A model's mesh as the GPU draws it: its vertices packed, its triangles' indices, its levels of
/// detail among them with the sphere round its vertices, and its meshlets, if the cooker cut it
/// into them.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelMesh {
    pub packing: Packing,
    pub vertices: Vec<PackedVertex>,
    /// Three for each triangle, indexing `vertices`.
    pub indices: Vec<u32>,
    pub lods: Lods,
    pub meshlets: Meshlets,
}

impl ModelMesh {
    /// `vertices` packed, mesh `name`'s, with `indices`, of `levels` among them, or of one level,
    /// all of them, if none is given; with the sphere round them as they unpack, which views cull
    /// it by, and without meshlets, which the cooker cuts. It isn't checked otherwise:
    /// [`Meshes::add_model`] checks it.
    ///
    /// A coordinate that isn't finite packs as the least of its axis's that is, and a normal or
    /// tangent that isn't a finite vector of some length as +Z or +X: no mesh the engine reads has
    /// them, and such a vertex draws nothing sensible either way. A tangent along its normal packs
    /// as a vector square to it, which the model shaders' tangent space needs.
    ///
    /// Fails with `renderer.bad_mesh` for vertices that its grid can't reach with what an f32
    /// holds, so that a vertex would unpack to a coordinate that isn't finite: as one at -f32::MAX
    /// beside one at 0 would, from an offset of -2^128, or one at f32::MAX beside one at 0, at
    /// 2^128.
    ///
    /// [`Meshes::add_model`]: crate::Meshes::add_model
    pub fn pack(
        name: &str,
        vertices: &[ModelVertex],
        indices: Vec<u32>,
        levels: Vec<Lod>,
    ) -> Result<Self> {
        let (offset, step) = grid(bounds(vertices.iter().map(|v| v.position.to_array())));
        let (uv_offset, uv_scale) = uv_box(bounds(vertices.iter().map(|v| v.uv.to_array())));
        let packing = Packing {
            offset,
            step,
            uv_offset,
            uv_scale,
        };
        let packed: Vec<PackedVertex> = (vertices.iter())
            .map(|vertex| {
                let position = vertex.position.to_array();
                let uv = vertex.uv.to_array();
                let color = vertex.color.to_array();
                let (normal, tangent) =
                    normal_and_tangent(vertex.normal, vertex.tangent.truncate());
                PackedVertex {
                    position: [0, 1, 2].map(|i| quantize(position[i], offset[i], step)),
                    sign: if vertex.tangent.w < 0.0 { -1 } else { 1 },
                    normal: octahedral(normal),
                    tangent: octahedral(tangent),
                    uv: [0, 1].map(|i| quantize(uv[i], uv_offset[i], uv_scale[i])),
                    color: [0, 1, 2, 3]
                        .map(|i| unorm8(if i < 3 { srgb(color[i]) } else { color[i] })),
                }
            })
            .collect();
        let levels = if levels.is_empty() {
            vec![Lod {
                start: 0,
                count: indices.len() as u32,
                error: 0.0,
            }]
        } else {
            levels
        };
        let mut mesh = Self {
            packing,
            vertices: packed,
            indices,
            lods: Lods {
                levels,
                center: Vec3::ZERO,
                radius: 0.0,
            },
            meshlets: Meshlets::default(),
        };
        // Checked as they unpack rather than by the grid alone, whose offset and step can be finite
        // while a vertex unpacks past what an f32 holds.
        let positions = mesh.positions();
        if let Some(i) = positions.iter().position(|position| !position.is_finite()) {
            return Err(bad_mesh(
                name,
                format!(
                    "has vertex {i} at {}, which unpacks to {} from {:?}: its vertices span more \
                     than packed vertices reach with what an f32 holds",
                    vertices[i].position, positions[i], mesh.packing
                ),
            ));
        }
        (mesh.lods.center, mesh.lods.radius) = sphere(&positions);
        Ok(mesh)
    }

    /// Its vertices' positions, unpacked as the model shaders unpack them.
    pub fn positions(&self) -> Vec<Vec3> {
        let Packing { offset, step, .. } = self.packing;
        (self.vertices.iter())
            .map(|vertex| offset + Vec3::from_array(vertex.position.map(f32::from)) * step)
            .collect()
    }

    /// Checks that it holds together, mesh `name`'s: its indices whole triangles of its vertices
    /// and its levels among them, as [`Meshes::add_lods`](crate::Meshes::add_lods) checks them;
    /// its packing finite, its step more than 0; and its meshlets, if any, a run of each level's,
    /// each of its own vertices, within its sizes, among the mesh's vertices, and with a finite
    /// sphere. Shaders read what it says where it says, so a mesh that doesn't hold together could
    /// read past its buffer.
    ///
    /// Fails with `renderer.bad_mesh`, saying where.
    pub fn check(&self, name: &str) -> Result<()> {
        check(name, &self.vertices, &self.indices)?;
        check_lods(name, &self.indices, &self.lods)?;
        let bad = |problem: String| Err(bad_mesh(name, problem));
        let Packing {
            offset,
            step,
            uv_offset,
            uv_scale,
        } = self.packing;
        if !(offset.is_finite() && uv_offset.is_finite() && uv_scale.is_finite())
            || !(step.is_finite() && step > 0.0)
        {
            return bad(format!("packs its vertices by {:?}", self.packing));
        }
        let Meshlets {
            meshlets,
            vertices,
            triangles,
            levels,
        } = &self.meshlets;
        if meshlets.is_empty() && levels.is_empty() {
            return Ok(());
        }
        if levels.len() != self.lods.levels.len() {
            return bad(format!(
                "has meshlets for {} levels, of its {}",
                levels.len(),
                self.lods.levels.len()
            ));
        }
        for (i, &[first, count]) in levels.iter().enumerate() {
            let end = u64::from(first) + u64::from(count);
            if count == 0 || end > meshlets.len() as u64 {
                return bad(format!(
                    "has level {i} of meshlets {first} on, {count} of them, of its {}",
                    meshlets.len()
                ));
            }
        }
        if let Some(index) = vertices
            .iter()
            .find(|&&i| i as usize >= self.vertices.len())
        {
            return bad(format!(
                "has a meshlet's vertex {index}, past its {} vertices",
                self.vertices.len()
            ));
        }
        for (i, meshlet) in meshlets.iter().enumerate() {
            let Meshlet {
                vertices: first,
                vertex_count,
                triangles: start,
                triangle_count,
                center,
                radius,
                ..
            } = *meshlet;
            let ends = [
                (u64::from(first) + u64::from(vertex_count), vertices.len()),
                (
                    u64::from(start) + 3 * u64::from(triangle_count),
                    triangles.len(),
                ),
            ];
            let sized = (1..=MAX_MESHLET_VERTICES).contains(&vertex_count)
                && (1..=MAX_MESHLET_TRIANGLES).contains(&triangle_count);
            if !sized || ends.iter().any(|&(end, len)| end > len as u64) {
                return bad(format!("has meshlet {i}, {meshlet:?}, past what it holds"));
            }
            let corners = &triangles[start as usize..(start + 3 * triangle_count) as usize];
            if let Some(corner) = corners.iter().find(|&&c| u32::from(c) >= vertex_count) {
                return bad(format!(
                    "has meshlet {i} with a corner of its vertex {corner}, of its {vertex_count}"
                ));
            }
            if !(center.is_finite() && radius.is_finite() && radius >= 0.0) {
                return bad(format!(
                    "has meshlet {i} in a sphere at {center} of radius {radius}"
                ));
            }
        }
        Ok(())
    }

    /// What its buffer on the GPU holds, as the [module's](self) docs lay it out.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let Meshlets {
            meshlets,
            vertices,
            triangles,
            ..
        } = &self.meshlets;
        let words = |bytes: usize| (bytes / 4) as u32;
        let header_bytes = size_of::<Header>();
        let start = header_bytes + size_of_val(self.vertices.as_slice());
        let (gpu_meshlets, meshlet_vertices) = if meshlets.is_empty() {
            (0, 0)
        } else {
            let at = start + meshlets.len() * size_of::<GpuMeshlet>();
            (words(start), words(at))
        };
        let triangles_at = if meshlets.is_empty() {
            0
        } else {
            meshlet_vertices + vertices.len() as u32
        };
        let header = Header {
            packing: self.packing,
            meshlets: gpu_meshlets,
            meshlet_vertices,
            triangles: triangles_at,
            unused: 0,
        };
        let mut bytes = Vec::with_capacity(start + self.meshlets_bytes());
        bytes.extend_from_slice(bytemuck::bytes_of(&header));
        bytes.extend_from_slice(bytemuck::cast_slice(&self.vertices));
        for meshlet in meshlets {
            let gpu = GpuMeshlet {
                vertices: meshlet.vertices,
                vertex_count: meshlet.vertex_count,
                triangles: meshlet.triangles,
                triangle_count: meshlet.triangle_count,
            };
            bytes.extend_from_slice(bytemuck::bytes_of(&gpu));
        }
        bytes.extend_from_slice(bytemuck::cast_slice(vertices));
        bytes.extend_from_slice(triangles);
        // Whole words, as shaders read them.
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        bytes
    }

    /// The bytes its meshlets take in its buffer.
    fn meshlets_bytes(&self) -> usize {
        let Meshlets {
            meshlets,
            vertices,
            triangles,
            ..
        } = &self.meshlets;
        meshlets.len() * size_of::<GpuMeshlet>()
            + size_of_val(vertices.as_slice())
            + triangles.len().next_multiple_of(4)
    }

    /// What culls each of its meshlets, in their order.
    pub(crate) fn meshlet_bounds(&self) -> Vec<MeshletBounds> {
        (self.meshlets.meshlets.iter())
            .map(|meshlet| MeshletBounds {
                center: meshlet.center,
                radius: meshlet.radius,
                cone: u32::from_le_bytes(meshlet.cone.map(|byte| byte as u8)),
                triangles: meshlet.triangle_count,
            })
            .collect()
    }
}

/// The sphere round `positions`, all finite, as they unpack: round their box's center, as far as
/// the farthest. In f64, so that the farthest of positions near the edges of what an f32 holds
/// doesn't overflow; its radius at most the most an f32 holds, which every view keeps whole.
fn sphere(positions: &[Vec3]) -> (Vec3, f32) {
    let wide = |p: Vec3| p.to_array().map(f64::from);
    let mut box_ = [(f64::INFINITY, f64::NEG_INFINITY); 3];
    for &p in positions {
        for (axis, c) in box_.iter_mut().zip(wide(p)) {
            *axis = (axis.0.min(c), axis.1.max(c));
        }
    }
    if positions.is_empty() {
        return (Vec3::ZERO, 0.0);
    }
    // As an f32 holds it, which the radius is measured from.
    let center = Vec3::from_array(box_.map(|(low, high)| (low / 2.0 + high / 2.0) as f32));
    let farthest = (positions.iter()).fold(0.0, |farthest: f64, &p| {
        let apart = (wide(p).into_iter().zip(wide(center))).map(|(c, m)| (c - m) * (c - m));
        farthest.max(apart.sum::<f64>().sqrt())
    });
    // Rounded up, so that it holds the farthest.
    let farthest = farthest.min(f64::from(f32::MAX));
    let mut radius = farthest as f32;
    if f64::from(radius) < farthest {
        radius = f32::from_bits(radius.to_bits() + 1);
    }
    (center, radius)
}

/// The least and the most of each axis of `points` that's finite, or 0 for an axis without one.
fn bounds<const N: usize>(points: impl Iterator<Item = [f32; N]>) -> [(f32, f32); N] {
    let mut axes = [(f32::INFINITY, f32::NEG_INFINITY); N];
    for point in points {
        for (axis, c) in axes.iter_mut().zip(point) {
            if c.is_finite() {
                *axis = (axis.0.min(c), axis.1.max(c));
            }
        }
    }
    axes.map(|(low, high)| if low <= high { (low, high) } else { (0.0, 0.0) })
}

/// The grid positions within `axes` pack onto, as [`Packing`] says: its offset, and its step.
fn grid(axes: [(f32, f32); 3]) -> (Vec3, f32) {
    // In f64, which holds the extent of any two finite f32s, and whose rounding is the same on
    // every platform, as no transcendental is taken.
    let extent = (axes.iter()).fold(0.0, |most: f64, &(low, high)| {
        most.max(f64::from(high) - f64::from(low))
    });
    let step = power_of_two_at_least(extent / 65_534.0);
    let offset = axes.map(|(low, _)| ((f64::from(low) / step).floor() * step) as f32);
    (Vec3::from_array(offset), step as f32)
}

/// The least power of two that's at least `x`, a finite number of 0 or more, and at least the
/// least normal f32, so that it's a step that packs; 1 for 0, a mesh of one point.
fn power_of_two_at_least(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let mut step = f64::from(f32::MIN_POSITIVE);
    while step < x {
        step *= 2.0;
    }
    step
}

/// The box texture coordinates within `axes` pack into, as [`Packing`] says: its offset, and its
/// scale.
fn uv_box(axes: [(f32, f32); 2]) -> (Vec2, Vec2) {
    let scale = axes.map(|(low, high)| ((f64::from(high) - f64::from(low)) / 65_535.0) as f32);
    (Vec2::new(axes[0].0, axes[1].0), Vec2::from_array(scale))
}

/// `value` as a 16-bit count of `step`s past `offset`, rounded to the nearest and clamped to what
/// 16 bits hold: 0 for a value that isn't finite, or a step of 0, as of an axis whose values are
/// all one.
fn quantize(value: f32, offset: f32, step: f32) -> u16 {
    if !(value.is_finite() && step > 0.0) {
        return 0;
    }
    let steps = (f64::from(value) - f64::from(offset)) / f64::from(step);
    steps.round().clamp(0.0, 65_535.0) as u16
}

/// The unit normal and tangent a vertex of `normal` and `tangent` packs: its normal, or +Z for one
/// that isn't a finite vector of some length; and its tangent, or +X for one that isn't, or a
/// vector square to the normal for one along it, within about a twentieth of a degree, as a tool
/// can leave one where texture coordinates have no area. The model shaders keep the tangent's part
/// square to the normal, which such a tangent, packed, would leave none of, to normalize; packed,
/// each is within a few thousandths of a degree of what it was.
fn normal_and_tangent(normal: Vec3, tangent: Vec3) -> (Vec3, Vec3) {
    let normal = normal.try_normalize().unwrap_or(Vec3::Z);
    let tangent = tangent.try_normalize().unwrap_or(Vec3::X);
    if normal.cross(tangent).length() < 1e-3 {
        (normal, normal.any_orthonormal_vector())
    } else {
        (normal, tangent)
    }
}

/// Unit vector `v`, folded onto an octahedron and unfolded onto a square, as [`PackedVertex`]
/// says.
fn octahedral(v: Vec3) -> [i16; 2] {
    let p = v / (v.x.abs() + v.y.abs() + v.z.abs());
    let sign = |c: f32| if c >= 0.0 { 1.0 } else { -1.0 };
    let folded = if p.z < 0.0 {
        Vec2::new((1.0 - p.y.abs()) * sign(p.x), (1.0 - p.x.abs()) * sign(p.y))
    } else {
        p.truncate()
    };
    folded
        .to_array()
        .map(|c| (c.clamp(-1.0, 1.0) * 32_767.0).round() as i16)
}

/// Linear `value`, of 0 to 1, sRGB-encoded.
fn srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * libm::powf(value, 1.0 / 2.4) - 0.055
    }
}

/// `value`, of 0 to 1, as an 8-bit fraction of 1, rounded to the nearest: 0 for one that isn't a
/// number.
fn unorm8(value: f32) -> u8 {
    if value.is_nan() {
        return 0;
    }
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use gg_core::math::Vec4;

    fn vertex(position: Vec3) -> ModelVertex {
        ModelVertex {
            position,
            uv: Vec2::ZERO,
            color: Vec4::ONE,
            normal: Vec3::Z,
            tangent: Vec4::new(1.0, 0.0, 0.0, 1.0),
        }
    }

    /// The vector [`PackedVertex`]'s two coordinates stand for, as `model.slang` unfolds them.
    fn unfolded(packed: [i16; 2]) -> Vec3 {
        let [x, y] = packed.map(|c| f32::from(c) / 32_767.0);
        let mut v = Vec3::new(x, y, 1.0 - x.abs() - y.abs());
        let t = (-v.z).max(0.0);
        v.x += if v.x >= 0.0 { -t } else { t };
        v.y += if v.y >= 0.0 { -t } else { t };
        v.normalize()
    }

    /// Positions unpack within half a step of where they were, on a step that's a power of two,
    /// from an offset that's a multiple of it, the longest side in 65,534 steps or fewer.
    #[test]
    fn positions_unpack_within_half_a_step() {
        let points = [
            Vec3::new(-3.25, 0.5, 10.0),
            Vec3::new(7.0, 0.1, 10.0),
            Vec3::new(1.0, 2.0, 10.25),
            Vec3::new(0.3, 1.7, 10.1),
        ];
        let vertices: Vec<_> = points.iter().map(|&p| vertex(p)).collect();
        let mesh = ModelMesh::pack("square", &vertices, vec![0, 1, 2, 1, 3, 2], vec![]).unwrap();
        let step = mesh.packing.step;
        assert_eq!(step.to_bits() & 0x007f_ffff, 0, "a power of two: {step}");
        assert!(
            10.25 / step <= 65_534.0 && 10.25 / step > 32_767.0,
            "{step}"
        );
        let offset = mesh.packing.offset / step;
        assert_eq!(offset, offset.round(), "a multiple of the step");
        for (unpacked, point) in mesh.positions().iter().zip(points) {
            assert!(
                (*unpacked - point).abs().max_element() <= step / 2.0,
                "{unpacked} {point}"
            );
        }
        // The sphere round them holds them as they unpack, which is what draws.
        let Lods { center, radius, .. } = mesh.lods;
        let apart = |p: &Vec3| p.as_dvec3().distance(center.as_dvec3());
        assert!(
            mesh.positions()
                .iter()
                .all(|p| apart(p) <= f64::from(radius))
        );
        // One level, of every index.
        assert_eq!(
            mesh.lods.levels,
            [Lod {
                start: 0,
                count: 6,
                error: 0.0
            }]
        );
        assert!(mesh.check("square").is_ok());
    }

    /// Two meshes of like sizes share their grid, so an edge both have keeps its ends together.
    #[test]
    fn meshes_of_like_sizes_put_shared_corners_alike() {
        let edge = [
            Vec3::new(1.0, 0.123_456, 0.0),
            Vec3::new(1.0, 0.987_654, 0.0),
        ];
        let left = [edge[0], edge[1], Vec3::new(-0.6, 0.5, 0.0)];
        let right = [edge[0], edge[1], Vec3::new(2.5, 0.25, 0.0)];
        let pack = |points: [Vec3; 3]| {
            let vertices: Vec<_> = points.iter().map(|&p| vertex(p)).collect();
            ModelMesh::pack("half", &vertices, vec![0, 1, 2], vec![]).unwrap()
        };
        let (left, right) = (pack(left), pack(right));
        assert_eq!(left.packing.step, right.packing.step);
        assert_eq!(left.positions()[..2], right.positions()[..2]);
    }

    /// Normals and tangents unfold to within a few thousandths of a degree, keeping the sign;
    /// colors to within an 8-bit step of their sRGB encoding; texture coordinates to within a
    /// 16-bit step of their box.
    #[test]
    fn attributes_unpack_nearly_as_they_were() {
        let normals = [
            Vec3::new(0.3, -0.5, 0.81),
            Vec3::new(-0.2, 0.1, -0.97),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(1.0, 0.0, 0.0),
        ];
        for normal in normals {
            let normal = normal.normalize();
            let back = unfolded(octahedral(normal));
            let (a, b) = (normal.as_dvec3(), back.as_dvec3());
            let radians = libm::atan2(a.cross(b).length(), a.dot(b));
            let degrees = radians * 180.0 / std::f64::consts::PI;
            assert!(degrees < 5e-3, "{normal} {back} {degrees}");
        }
        // What isn't a finite vector of some length packs as +Z for a normal, and +X for a tangent.
        let (normal, tangent) = normal_and_tangent(Vec3::NAN, Vec3::ZERO);
        let unpacked = [normal, tangent].map(|v| unfolded(octahedral(v)));
        assert_eq!(unpacked, [Vec3::Z, Vec3::X]);
        let mut corner = vertex(Vec3::ZERO);
        corner.tangent = Vec4::new(0.0, 1.0, 0.0, -1.0);
        corner.color = Vec4::new(1.0, 0.5, 0.0, 0.25);
        corner.uv = Vec2::new(-2.0, 0.75);
        let mut other = vertex(Vec3::ONE);
        other.uv = Vec2::new(3.0, 0.25);
        let mesh = ModelMesh::pack("corner", &[corner, other, other], vec![0, 1, 2], vec![]);
        let mesh = mesh.unwrap();
        let packed = mesh.vertices[0];
        assert_eq!(packed.sign, -1);
        assert_eq!(mesh.vertices[1].sign, 1);
        assert_eq!(packed.color, [255, 188, 0, 64]);
        let Packing {
            uv_offset,
            uv_scale,
            ..
        } = mesh.packing;
        let uv = uv_offset + Vec2::from_array(packed.uv.map(f32::from)) * uv_scale;
        assert!(
            (uv - corner.uv).abs().max_element() <= uv_scale.max_element(),
            "{uv}"
        );
    }

    /// A tangent along its normal, either way, nearly, or as +X stands in for one along a normal of
    /// +X, packs square to the normal: unpacked, its part square to the normal, which the model
    /// shaders normalize, is about as long as it is. One that's off its normal packs as it was.
    #[test]
    fn tangents_along_their_normals_pack_square_to_them() {
        let unpacked = |normal: Vec3, tangent: Vec3| {
            let mut corner = vertex(Vec3::ZERO);
            corner.normal = normal;
            corner.tangent = tangent.extend(1.0);
            let corners = [corner; 3];
            let mesh = ModelMesh::pack("along", &corners, vec![0, 1, 2], vec![]).unwrap();
            let packed = mesh.vertices[0];
            (unfolded(packed.normal), unfolded(packed.tangent))
        };
        let tilted = Vec3::new(0.3, -0.5, 0.81).normalize();
        let along = [
            (tilted, tilted),
            (Vec3::Y, Vec3::NEG_Y),
            (Vec3::Z, Vec3::new(1e-5, 0.0, 1.0)),
            (Vec3::X, Vec3::ZERO),
        ];
        for (normal, tangent) in along {
            let (n, t) = unpacked(normal, tangent);
            let square = (t - n * n.dot(t)).length();
            assert!(square > 0.99, "{normal} {tangent}: {square}");
            assert!(n.dot(normal) > 0.9999, "{normal}: {n}");
        }
        let off = Vec3::new(0.2, 0.0, 1.0).normalize();
        let (_, t) = unpacked(Vec3::Z, off);
        assert!(t.dot(off) > 0.9999, "{off}: {t}");
    }

    /// A buffer holds its header, its vertices after it at their place, and its meshlets' parts
    /// where the header says, each a run of words.
    #[test]
    fn a_buffer_holds_its_header_then_its_vertices_then_its_meshlets() {
        let vertices: Vec<_> = (0..3).map(|i| vertex(Vec3::splat(i as f32))).collect();
        let mut mesh = ModelMesh::pack("one meshlet", &vertices, vec![0, 1, 2], vec![]).unwrap();
        let plain = mesh.bytes();
        assert_eq!(
            plain.len(),
            (HEADER_VERTICES + 3) * size_of::<PackedVertex>()
        );
        let word = |bytes: &[u8], i: usize| {
            u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
        };
        assert_eq!([word(&plain, 8), word(&plain, 9), word(&plain, 10)], [0; 3]);
        mesh.meshlets = Meshlets {
            meshlets: vec![Meshlet {
                vertices: 0,
                vertex_count: 3,
                triangles: 0,
                triangle_count: 1,
                center: Vec3::ONE,
                radius: 2.0,
                cone: [0, 0, 127, 127],
            }],
            vertices: vec![2, 1, 0],
            triangles: vec![0, 1, 2],
            levels: vec![[0, 1]],
        };
        assert!(mesh.check("one meshlet").is_ok());
        let bytes = mesh.bytes();
        let at = (HEADER_VERTICES + 3) * size_of::<PackedVertex>() / 4;
        assert_eq!(
            [word(&bytes, 8), word(&bytes, 9), word(&bytes, 10)],
            [at as u32, at as u32 + 4, at as u32 + 7]
        );
        assert_eq!(
            [word(&bytes, at), word(&bytes, at + 1), word(&bytes, at + 3)],
            [0, 3, 1]
        );
        assert_eq!([word(&bytes, at + 4), word(&bytes, at + 6)], [2, 0]);
        assert_eq!(word(&bytes, at + 7), 0x0002_0100);
        assert_eq!(bytes.len(), (at + 8) * 4);
        let bounds = mesh.meshlet_bounds();
        assert_eq!(bounds[0].cone, 0x7f7f_0000);
    }

    /// A mesh whose meshlets would read past what it holds is refused.
    #[test]
    fn meshlets_that_dont_hold_together_are_refused() {
        let vertices: Vec<_> = (0..3).map(|i| vertex(Vec3::splat(i as f32))).collect();
        let mut good = ModelMesh::pack("good", &vertices, vec![0, 1, 2], vec![]).unwrap();
        good.meshlets = Meshlets {
            meshlets: vec![Meshlet {
                vertices: 0,
                vertex_count: 3,
                triangles: 0,
                triangle_count: 1,
                center: Vec3::ONE,
                radius: 2.0,
                cone: [0; 4],
            }],
            vertices: vec![0, 1, 2],
            triangles: vec![0, 1, 2],
            levels: vec![[0, 1]],
        };
        assert!(good.check("good").is_ok());
        type Edit = fn(&mut ModelMesh);
        let cases: [(&str, Edit); 9] = [
            ("a vertex past the mesh's", |m| m.meshlets.vertices[1] = 3),
            ("a corner past its vertices", |m| {
                m.meshlets.triangles[2] = 3
            }),
            ("vertices past the array", |m| {
                m.meshlets.meshlets[0].vertices = 1
            }),
            ("triangles past the array", |m| {
                m.meshlets.meshlets[0].triangle_count = 2
            }),
            ("too many vertices", |m| {
                m.meshlets.meshlets[0].vertex_count = 65
            }),
            ("no triangles", |m| {
                m.meshlets.meshlets[0].triangle_count = 0
            }),
            ("a level past its meshlets", |m| {
                m.meshlets.levels[0] = [1, 1]
            }),
            ("levels other than the mesh's", |m| {
                m.meshlets.levels.push([0, 1])
            }),
            ("a sphere not finite", |m| {
                m.meshlets.meshlets[0].radius = f32::NAN
            }),
        ];
        for (case, edit) in cases {
            let mut mesh = good.clone();
            edit(&mut mesh);
            let error = mesh.check("m").expect_err(case);
            assert_eq!(error.code(), "renderer.bad_mesh", "{case}");
        }
        let mut unpackable = good.clone();
        unpackable.packing.step = 0.0;
        assert!(unpackable.check("m").is_err());
    }

    /// What isn't finite packs without panicking, and the rest still packs within its box.
    #[test]
    fn what_isnt_finite_packs() {
        let mut odd = vertex(Vec3::new(f32::NAN, f32::INFINITY, 1.0));
        odd.normal = Vec3::NAN;
        odd.color = Vec4::NAN;
        let far = vertex(Vec3::new(-3e38, 3e38, 2.0));
        let vertices = [odd, far, vertex(Vec3::ZERO)];
        let mesh = ModelMesh::pack("odd", &vertices, vec![0, 1, 2], vec![]).unwrap();
        assert!(mesh.packing.step.is_finite() && mesh.packing.offset.is_finite());
        assert!(mesh.lods.radius.is_finite(), "{:?}", mesh.lods);
        assert_eq!(mesh.vertices[0].color, [0; 4]);
        assert!(mesh.check("odd").is_ok());
    }

    /// Vertices farther apart than the grid reaches with what an f32 holds are refused, rather
    /// than packed to unpack where a coordinate isn't finite: from an offset of -2^128, or at
    /// 2^128 from an offset of 0.
    #[test]
    fn vertices_past_what_the_grid_reaches_are_refused() {
        for far in [-f32::MAX, f32::MAX] {
            let vertices = [vertex(Vec3::new(far, 0.0, 0.0)), vertex(Vec3::ZERO)];
            let packed = ModelMesh::pack("far", &vertices, vec![0, 1, 1], vec![]);
            let error = packed.expect_err("past the grid");
            assert_eq!(error.code(), "renderer.bad_mesh", "{far}");
            assert!(
                error.to_string().contains("\"far\" has vertex 0"),
                "{error}"
            );
        }
    }
}
