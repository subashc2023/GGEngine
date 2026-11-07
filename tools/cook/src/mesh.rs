//! A model primitive's triangles, optimized with meshoptimizer for drawing: identical vertices
//! merged, the triangles and the vertices ordered for the GPU's caches, and a chain of coarser
//! levels of detail over the same vertices, which a draw picks from by its size on screen; and,
//! once its vertices are packed, each level cut into meshlets, which views cull and draw apart.

use std::ops::Range;

use gg_engine::cook::{MAX_MESHLET_TRIANGLES, MAX_MESHLET_VERTICES, Meshlet, Meshlets, ModelMesh};
use gg_engine::math::Vec3;
use meshopt::{
    SimplifyOptions, VertexDataAdapter, VertexStream, build_meshlets, compute_meshlet_bounds,
    generate_vertex_remap_multi, optimize_overdraw_in_place_decoder,
    optimize_vertex_cache_in_place, optimize_vertex_fetch_remap, remap_index_buffer,
    remap_vertex_buffer, simplify_decoder, typed_to_bytes,
};

/// How many levels a chain holds at most, level 0 among them. Each level has about half the
/// triangles of the one before, so eight take the coarsest to a 128th of level 0's, a million
/// triangles to some eight thousand: past that, what an instance costs is its fixed cost, not its
/// triangles, and each level more is one more for the renderer to sort instances into.
const MAX_LEVELS: usize = 8;

/// The fewest triangles a level is simplified toward, and the most that one can have and not be
/// simplified further. A GPU shades vertices in warps or waves of 32 or 64, so a level whose
/// vertices don't fill one draws no faster than a level of 32 triangles, which has some 16 to 32.
const MIN_TRIANGLES: usize = 32;

/// The share of the level before it that a coarser level must shed, or the chain ends before it:
/// the simplifier stops short of its target at what its topology rules allow, as at a mesh whose
/// every vertex is a seam, and a level that keeps nine tenths of the triangles draws almost as
/// slowly as the one before it, for the index memory of another level.
const MIN_SHED: f32 = 0.1;

/// How much of the vertex cache's hits the overdraw optimizer may cost level 0, to cut its overdraw:
/// 1.05 is up to 5%, the example meshoptimizer.h gives.
const OVERDRAW_THRESHOLD: f32 = 1.05;

/// A level of detail: some of a primitive's indices, whole triangles over its vertices, and how far
/// the surface they make may stray from level 0's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Lod {
    /// Where its indices begin in the primitive's.
    pub(crate) start: u32,
    /// How many indices it has, three for each triangle.
    pub(crate) count: u32,
    /// How far its surface may stray from level 0's, absolute, in the mesh's own units, as the
    /// simplifier measures it: 0 for level 0, and no less than the level before's. It adds up the
    /// simplifier's error for each step from level 0, as a level's surface may stray that far from
    /// the one before it, which may stray from level 0's.
    pub(crate) error: f32,
}

impl Lod {
    /// The level whose indices are the `count` from `start`, `error` away from level 0.
    ///
    /// Panics if its indices pass what 32 bits count, which no primitive the engine reads has.
    fn new(start: usize, count: usize, error: f32) -> Self {
        let bits =
            |n: usize| u32::try_from(n).expect("a primitive's indices are counted in 32 bits");
        Self {
            start: bits(start),
            count: bits(count),
            error,
        }
    }

    /// Where its indices are in the primitive's.
    fn range(&self) -> Range<usize> {
        let start = self.start as usize;
        start..start + self.count as usize
    }
}

/// A primitive's vertices, an array for each of their attributes, as `optimize` takes them.
pub(crate) struct Vertices<'a> {
    pub(crate) positions: &'a mut Vec<[f32; 3]>,
    pub(crate) uvs: &'a mut Vec<[f32; 2]>,
    pub(crate) colors: &'a mut Vec<[f32; 4]>,
    pub(crate) normals: &'a mut Vec<[f32; 3]>,
    pub(crate) tangents: &'a mut Vec<[f32; 4]>,
}

impl Vertices<'_> {
    /// Moves vertex `i` to `remap[i]`, of `count` after the move.
    fn remap(&mut self, count: usize, remap: &[u32]) {
        *self.positions = remap_vertex_buffer(self.positions, count, remap);
        *self.uvs = remap_vertex_buffer(self.uvs, count, remap);
        *self.colors = remap_vertex_buffer(self.colors, count, remap);
        *self.normals = remap_vertex_buffer(self.normals, count, remap);
        *self.tangents = remap_vertex_buffer(self.tangents, count, remap);
    }
}

/// Optimizes a primitive's triangles in place, and returns its levels of detail, finest first.
///
/// Vertices identical in every attribute merge into one, and those no triangle uses go. Level 0 has
/// the input's triangles, with the winding they had, in the order and from the corners that the
/// vertex cache and overdraw optimizers choose. Each coarser level is the one before it simplified
/// toward half its triangles, by a simplifier that moves no vertex and keeps attribute seams, the
/// edges where vertices of one position differ in another attribute, as hard edges' normals do,
/// then ordered for the vertex cache. The chain ends at [`MAX_LEVELS`] levels, or before a level
/// that would shed less than [`MIN_SHED`] of the one before it, and no level is simplified toward
/// fewer than [`MIN_TRIANGLES`] triangles, though the simplifier may land a few short of its target.
///
/// Afterwards `indices` holds level 0's triangles, then each coarser level's, over the same
/// vertices, which are in the order that one pass through the levels, from the coarsest to the
/// finest, first uses them, so that a coarser level reads a shorter range of them. Every vertex is
/// used by level 0, and a coarser level uses only some of them.
///
/// If `keep_order`, as a primitive whose material blends needs, whose triangles cover each other in
/// the order they draw in, no level's triangles are ordered for the vertex cache or overdraw: level
/// 0's are the input's in its order, and a coarser level's in the order the simplifier gives them.
///
/// Panics if the vertex arrays differ in length, if `indices` aren't whole triangles, or if one is
/// past the vertices, as the glTF reader leaves none; and if the levels' indices together pass
/// what 32 bits count.
pub(crate) fn optimize(
    mut vertices: Vertices<'_>,
    indices: &mut Vec<u32>,
    keep_order: bool,
) -> Vec<Lod> {
    let len = vertices.positions.len();
    // meshoptimizer builds with its assertions off in release, and reads past the vertices of an
    // index that is.
    assert!(
        [
            vertices.uvs.len(),
            vertices.colors.len(),
            vertices.normals.len(),
            vertices.tangents.len()
        ] == [len; 4]
            && indices.len().is_multiple_of(3)
            && indices.iter().all(|&index| (index as usize) < len),
        "a primitive's vertex arrays are of one length, and its indices whole triangles within them"
    );

    // Its simplifier follows the mesh's topology, so identical vertices left apart would be seams
    // it can't collapse across (the README's basic simplification). Every attribute is compared,
    // bit for bit, as the shader reads them.
    let (count, remap) = generate_vertex_remap_multi(
        len,
        &[
            VertexStream::new(vertices.positions.as_ptr()),
            VertexStream::new(vertices.uvs.as_ptr()),
            VertexStream::new(vertices.colors.as_ptr()),
            VertexStream::new(vertices.normals.as_ptr()),
            VertexStream::new(vertices.tangents.as_ptr()),
        ],
        Some(indices),
    );
    *indices = remap_index_buffer(Some(indices.as_slice()), len, &remap);
    vertices.remap(count, &remap);

    if !keep_order {
        optimize_vertex_cache_in_place(indices, count);
        // After the vertex cache, which it takes the output of.
        optimize_overdraw_in_place_decoder(indices, vertices.positions, OVERDRAW_THRESHOLD);
    }

    let mut finer = Lod::new(0, indices.len(), 0.0);
    let mut lods = vec![finer];
    while lods.len() < MAX_LEVELS {
        let triangles = finer.count as usize / 3;
        if triangles <= MIN_TRIANGLES {
            break;
        }
        // From the level before, which the README says is faster than from level 0 and keeps
        // attributes better, with the errors added up to choose among levels by. Sparse, which it
        // suggests for deep chains, makes relative errors relative to the level's own extents,
        // hence the absolute ones it advises with it. With no limit on the error, the simplifier
        // is likelier to reach the target, and the error it returns is what a draw chooses the
        // level by, as the README has it. Not Prune, which it recommends for a mesh simplified
        // whole: with no limit on the error, its cleanup removes whole parts that topology holds,
        // all of a mesh stuck above the target, where without it one stuck in part sheds the rest.
        let mut step = 0.0;
        let mut level = simplify_decoder(
            &indices[finer.range()],
            vertices.positions,
            (triangles / 2).max(MIN_TRIANGLES) * 3,
            f32::MAX,
            SimplifyOptions::ErrorAbsolute | SimplifyOptions::Sparse,
            Some(&mut step),
        );
        let error = finer.error + step;
        let shed = triangles.saturating_sub(level.len() / 3) as f32;
        // An error that isn't finite, as of opposite infinities, is one no draw's size on screen
        // is within; and a level of no triangles is none that a run takes.
        if shed < MIN_SHED * triangles as f32 || !error.is_finite() || level.is_empty() {
            break;
        }
        if !keep_order {
            optimize_vertex_cache_in_place(&mut level, count);
        }
        finer = Lod::new(indices.len(), level.len(), error);
        lods.push(finer);
        indices.extend(level);
    }

    // Levels that share vertices: the README has each ordered for the vertex cache alone, then
    // one vertex fetch pass over all their indices from the coarsest level on, so that a coarser
    // level's vertices are a short range at the start. GPUs that can only transform a sequential
    // range of vertices need that, and on any a distant instance's draw reads less memory.
    let mut by_use = Vec::with_capacity(indices.len());
    for lod in lods.iter().rev() {
        by_use.extend_from_slice(&indices[lod.range()]);
    }
    let remap = optimize_vertex_fetch_remap(&by_use, count);
    // meshopt cuts the table to the vertices used, which are all of them, as level 0 uses each.
    assert_eq!(remap.len(), count, "level 0 uses every vertex");
    *indices = remap_index_buffer(Some(indices.as_slice()), count, &remap);
    vertices.remap(count, &remap);
    lods
}

/// How much meshoptimizer's meshlet builder weighs keeping each meshlet's triangles facing one way,
/// which narrows the cones views cull them by, against keeping meshlets full: the README's
/// suggestion for meshlets culled by their cones.
const CONE_WEIGHT: f32 = 0.25;

/// Cuts each of `mesh`'s levels into meshlets, as [`ModelMesh`]'s holds them: each level's
/// triangles, by meshoptimizer's builder, into meshlets of at most [`MAX_MESHLET_VERTICES`] vertices
/// and [`MAX_MESHLET_TRIANGLES`] triangles, each with the sphere round its vertices as they unpack,
/// which is where they draw, and the cone its triangles face within; one level's after another's,
/// in the levels' order, each triangle in one meshlet of its level.
pub(crate) fn cut(mesh: &mut ModelMesh) {
    let positions: Vec<[f32; 3]> = (mesh.positions().iter())
        .map(|position| position.to_array())
        .collect();
    let adapter = VertexDataAdapter::new(typed_to_bytes(&positions), size_of::<[f32; 3]>(), 0)
        .expect("positions are whole vertices of 12 bytes from 0");
    let mut meshlets = Meshlets::default();
    for lod in &mesh.lods.levels {
        let start = lod.start as usize;
        let indices = &mesh.indices[start..start + lod.count as usize];
        let built = build_meshlets(
            indices,
            &adapter,
            MAX_MESHLET_VERTICES as usize,
            MAX_MESHLET_TRIANGLES as usize,
            CONE_WEIGHT,
        );
        let first = meshlets.meshlets.len() as u32;
        let (vertices, triangles) = (meshlets.vertices.len(), meshlets.triangles.len());
        for (meshlet, cut) in built.meshlets.iter().zip(built.iter()) {
            let bounds = compute_meshlet_bounds(cut, &adapter);
            let [x, y, z] = bounds.cone_axis_s8;
            meshlets.meshlets.push(Meshlet {
                vertices: vertices as u32 + meshlet.vertex_offset,
                vertex_count: meshlet.vertex_count,
                triangles: triangles as u32 + meshlet.triangle_offset,
                triangle_count: meshlet.triangle_count,
                center: Vec3::from_array(bounds.center),
                radius: bounds.radius,
                cone: [x, y, z, bounds.cone_cutoff_s8],
            });
        }
        meshlets.vertices.extend_from_slice(&built.vertices);
        // Each meshlet's triangles start at a word, the builder's padding kept.
        meshlets.triangles.extend_from_slice(&built.triangles);
        let count = meshlets.meshlets.len() as u32 - first;
        meshlets.levels.push([first, count]);
    }
    mesh.meshlets = meshlets;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A primitive's vertices and indices, as `optimize` takes them.
    #[derive(Clone, PartialEq)]
    struct Mesh {
        positions: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        colors: Vec<[f32; 4]>,
        normals: Vec<[f32; 3]>,
        tangents: Vec<[f32; 4]>,
        indices: Vec<u32>,
    }

    /// A vertex as the bits of its fields, so that vertices compare and sort whatever order a mesh
    /// holds them in.
    type Key = [u32; 16];

    /// A normal and a tangent facing +Z, as a vertex of a flat mesh has them.
    const FACING: ([f32; 3], [f32; 4]) = ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0]);

    impl Mesh {
        /// A mesh of `positions`, `uvs` and `colors`, facing +Z, and `indices`.
        fn facing(
            positions: Vec<[f32; 3]>,
            uvs: Vec<[f32; 2]>,
            colors: Vec<[f32; 4]>,
            indices: Vec<u32>,
        ) -> Self {
            let count = positions.len();
            Self {
                positions,
                uvs,
                colors,
                normals: vec![FACING.0; count],
                tangents: vec![FACING.1; count],
                indices,
            }
        }

        fn optimize(&mut self) -> Vec<Lod> {
            self.optimize_keeping(false)
        }

        /// Optimizes it as `optimize` does, keeping its triangles' order if `keep_order`.
        fn optimize_keeping(&mut self, keep_order: bool) -> Vec<Lod> {
            let vertices = Vertices {
                positions: &mut self.positions,
                uvs: &mut self.uvs,
                colors: &mut self.colors,
                normals: &mut self.normals,
                tangents: &mut self.tangents,
            };
            optimize(vertices, &mut self.indices, keep_order)
        }

        fn key(&self, index: u32) -> Key {
            let i = index as usize;
            let bits = [
                &self.positions[i][..],
                &self.uvs[i],
                &self.colors[i],
                &self.normals[i],
                &self.tangents[i],
            ];
            let mut key = [0; 16];
            for (key, value) in key.iter_mut().zip(bits.into_iter().flatten()) {
                *key = value.to_bits();
            }
            key
        }

        /// The triangles of `indices`, each as its vertices' keys from its least on, in the order
        /// they wind in, and sorted: what a set of triangles is whatever order they and their
        /// corners come in.
        fn triangles(&self, indices: &[u32]) -> Vec<[Key; 3]> {
            let mut triangles: Vec<[Key; 3]> = (indices.as_chunks::<3>().0.iter())
                .map(|corners| {
                    let keys = corners.map(|corner| self.key(corner));
                    let least = (0..3).min_by_key(|&i| keys[i]).unwrap();
                    [0, 1, 2].map(|i| keys[(least + i) % 3])
                })
                .collect();
            triangles.sort_unstable();
            triangles
        }
    }

    /// A grid of `n` by `n` quads, two triangles each, over a hill with a ripple, so that no
    /// collapse is free at any level.
    fn grid(n: u32) -> Mesh {
        let (mut positions, mut uvs, mut colors, mut indices) = (vec![], vec![], vec![], vec![]);
        for y in 0..=n {
            for x in 0..=n {
                let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
                let ripple = ((x * 73) ^ (y * 151)) % 7;
                let height = 4.0 * u * (1.0 - u) * v * (1.0 - v) + ripple as f32 * 0.002;
                positions.push([u, v, height]);
                uvs.push([u, v]);
                colors.push([u, v, 0.5, 1.0]);
            }
        }
        let at = |x, y| y * (n + 1) + x;
        for y in 0..n {
            for x in 0..n {
                let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
                indices.extend([a, b, c, c, b, d]);
            }
        }
        Mesh::facing(positions, uvs, colors, indices)
    }

    /// A cube whose six faces are each `n` by `n` quads with UVs of their own, `f` to `f + 0.5`
    /// across face `f`, so that its edges are seams: vertices of one position, in other faces.
    fn cube(n: u32) -> Mesh {
        let (mut positions, mut uvs, mut colors, mut indices) = (vec![], vec![], vec![], vec![]);
        for face in 0..6u32 {
            let (axis, far) = (face as usize / 2, (face % 2) as f32);
            let (s, t) = ((axis + 1) % 3, (axis + 2) % 3);
            let first = positions.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
                    let mut position = [0.0; 3];
                    (position[axis], position[s], position[t]) = (far, u, v);
                    positions.push(position);
                    uvs.push([face as f32 + 0.5 * u, v]);
                    colors.push([1.0; 4]);
                }
            }
            let at = |i, j| first + j * (n + 1) + i;
            for j in 0..n {
                for i in 0..n {
                    let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
                    indices.extend(if face % 2 == 0 {
                        [a, c, b, b, c, d]
                    } else {
                        [a, b, c, c, b, d]
                    });
                }
            }
        }
        Mesh::facing(positions, uvs, colors, indices)
    }

    /// `grid` but for its triangles whose first corner `weld` doesn't hold, which have vertices of
    /// their own and a UV of their own, so that each of their vertices is a seam.
    fn unwelded(grid: &Mesh, weld: impl Fn([f32; 3]) -> bool) -> Mesh {
        let mut mesh = Mesh {
            indices: vec![],
            ..grid.clone()
        };
        for (triangle, corners) in grid.indices.as_chunks::<3>().0.iter().enumerate() {
            if weld(grid.positions[corners[0] as usize]) {
                mesh.indices.extend(corners);
                continue;
            }
            for &corner in corners {
                mesh.indices.push(mesh.positions.len() as u32);
                mesh.positions.push(grid.positions[corner as usize]);
                // Past the grid's own UVs, which are within 0 to 1.
                mesh.uvs.push([2.0 + triangle as f32, 0.0]);
                mesh.colors.push([1.0; 4]);
                mesh.normals.push(FACING.0);
                mesh.tangents.push(FACING.1);
            }
        }
        mesh
    }

    /// A grid whose quads each tilt their normals their own way, as a faceted mesh's hard edges
    /// do: every vertex of one position stays apart from those of another normal, as a seam, which
    /// no level collapses across.
    #[test]
    fn hard_edges_keep_their_normals_apart() {
        let input = grid(16);
        let mut faceted = Mesh {
            indices: vec![],
            positions: vec![],
            uvs: vec![],
            colors: vec![],
            normals: vec![],
            tangents: vec![],
        };
        for (triangle, corners) in input.indices.as_chunks::<3>().0.iter().enumerate() {
            // Each quad's two triangles share a normal of their own.
            let normal = [(triangle / 2 % 3) as f32, 0.0, 1.0];
            for &corner in corners {
                let i = corner as usize;
                faceted.indices.push(faceted.positions.len() as u32);
                faceted.positions.push(input.positions[i]);
                faceted.uvs.push(input.uvs[i]);
                faceted.colors.push(input.colors[i]);
                faceted.normals.push(normal);
                faceted.tangents.push(FACING.1);
            }
        }
        let triangles = faceted.triangles(&faceted.indices);
        let lods = faceted.optimize();
        check("faceted by normals", &faceted, &lods);
        assert_eq!(
            faceted.triangles(&faceted.indices[lods[0].range()]),
            triangles
        );
        for lod in &lods {
            for triangle in faceted.indices[lod.range()].as_chunks::<3>().0 {
                let normal = |i: u32| faceted.normals[i as usize];
                assert!(
                    normal(triangle[0]) == normal(triangle[1])
                        && normal(triangle[1]) == normal(triangle[2]),
                    "{lod:?} has a triangle across a hard edge"
                );
            }
        }
    }

    /// Checks what holds of any output: the arrays of one length; the levels' indices one after
    /// another to the end of `indices`, whole triangles within the vertices; level 0 using every
    /// vertex and each coarser level a range of them from 0, which is the order of first use from
    /// the coarsest level on, as each level's vertices are some of the level before's; and the
    /// errors finite, 0 for level 0 and not falling.
    fn check(name: &str, mesh: &Mesh, lods: &[Lod]) {
        let vertices = mesh.positions.len();
        let others = [
            mesh.uvs.len(),
            mesh.colors.len(),
            mesh.normals.len(),
            mesh.tangents.len(),
        ];
        assert_eq!(others, [vertices; 4], "{name}");
        assert!(
            mesh.indices
                .iter()
                .all(|&index| (index as usize) < vertices),
            "{name}"
        );
        let mut next = 0;
        for (level, lod) in lods.iter().enumerate() {
            assert_eq!(lod.start, next, "{name}: levels' indices one after another");
            assert_eq!(lod.count % 3, 0, "{name}: whole triangles");
            next += lod.count;
            let mut used = vec![false; vertices];
            for &index in &mesh.indices[lod.range()] {
                used[index as usize] = true;
            }
            let distinct = used.iter().filter(|&&used| used).count();
            assert!(
                used[..distinct].iter().all(|&used| used),
                "{name}: level {level} uses a range of the vertices from 0"
            );
            if level == 0 {
                assert_eq!(distinct, vertices, "{name}: level 0 uses every vertex");
            }
        }
        assert_eq!(next as usize, mesh.indices.len(), "{name}: to the end");
        assert_eq!(lods[0].error, 0.0, "{name}");
        for pair in lods.windows(2) {
            assert!(pair[0].error <= pair[1].error, "{name}: {lods:?}");
        }
        assert!(lods.iter().all(|lod| lod.error.is_finite()), "{name}");
    }

    /// A grid of 64 by 64 quads gets a chain of levels at the cap, each with fewer triangles than
    /// the one before it, more error than it, and some; its level 0 has the input's triangles.
    #[test]
    fn a_grid_gets_a_chain_of_levels() {
        let mut mesh = grid(64);
        let input = mesh.triangles(&mesh.indices);
        let lods = mesh.optimize();
        check("grid", &mesh, &lods);
        assert_eq!(lods.len(), MAX_LEVELS);
        assert_eq!(lods[0].count as usize, 64 * 64 * 6);
        for pair in lods.windows(2) {
            assert!(pair[1].count < pair[0].count, "{lods:?}");
            assert!(pair[1].error > pair[0].error, "{lods:?}");
        }
        assert_eq!(mesh.triangles(&mesh.indices[lods[0].range()]), input);
    }

    /// A grid small enough that its chain ends before the cap ends at the floor: its last level is
    /// the first to have no more than the floor's triangles, and is simplified toward the floor
    /// rather than past it, which the simplifier lands within a tenth of.
    #[test]
    fn a_small_mesh_stops_at_the_floor() {
        for n in [8, 5] {
            let mut mesh = grid(n);
            let lods = mesh.optimize();
            check("small grid", &mesh, &lods);
            assert!(lods.len() > 1 && lods.len() < MAX_LEVELS, "{lods:?}");
            let triangles: Vec<usize> = lods.iter().map(|lod| lod.count as usize / 3).collect();
            let (last, finer) = triangles.split_last().unwrap();
            assert!(*last <= MIN_TRIANGLES, "{triangles:?}");
            assert!(*last >= MIN_TRIANGLES * 9 / 10, "{triangles:?}");
            assert!(finer.iter().all(|&n| n > MIN_TRIANGLES), "{triangles:?}");
        }
    }

    /// Errors are in the mesh's own units: a mesh eight times as large has levels of the same
    /// triangles and eight times the errors.
    #[test]
    fn errors_are_in_the_meshs_own_units() {
        let mut small = grid(32);
        let mut large = small.clone();
        large.positions.iter_mut().flatten().for_each(|c| *c *= 8.0);
        let (small_lods, large_lods) = (small.optimize(), large.optimize());
        assert!(small_lods.len() > 2, "{small_lods:?}");
        assert_eq!(small_lods.len(), large_lods.len());
        for (small, large) in small_lods.iter().zip(&large_lods) {
            assert_eq!(small.count, large.count);
            assert!(
                (large.error - 8.0 * small.error).abs() <= 1e-5 * large.error,
                "{small:?} {large:?}"
            );
        }
    }

    /// Identical vertices merge into one, those no triangle uses go, and the triangles stay what
    /// they were, as vertices' attributes.
    #[test]
    fn identical_vertices_merge_and_unused_ones_go() {
        // A square of two triangles, whose corners 0 and 2 are held twice, and a vertex that no
        // triangle uses.
        let mut mesh = Mesh::facing(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [5.0, 5.0, 5.0],
            ],
            vec![
                [0.0, 0.0],
                [1.0, 0.0],
                [1.0, 1.0],
                [0.0, 1.0],
                [0.0, 0.0],
                [1.0, 1.0],
                [9.0, 9.0],
            ],
            vec![[1.0; 4]; 7],
            vec![0, 1, 2, 4, 5, 3],
        );
        let input = mesh.triangles(&mesh.indices);
        let lods = mesh.optimize();
        check("square", &mesh, &lods);
        assert_eq!(mesh.positions.len(), 4);
        assert!(!mesh.positions.contains(&[5.0, 5.0, 5.0]));
        assert_eq!(mesh.triangles(&mesh.indices), input);
    }

    /// A triangle has one level, all of it, with no error.
    /// A primitive whose material blends keeps level 0's triangles in the order it had them, each
    /// from the corner it started at, as they cover each other in that order; and still gets
    /// coarser levels.
    #[test]
    fn a_primitive_that_blends_keeps_its_triangles_order() {
        let input = grid(16);
        let mut mesh = input.clone();
        let lods = mesh.optimize_keeping(true);
        check("blended grid", &mesh, &lods);
        let keys = |mesh: &Mesh, indices: &[u32]| -> Vec<Key> {
            indices.iter().map(|&index| mesh.key(index)).collect()
        };
        let finest = &mesh.indices[lods[0].range()];
        assert_eq!(keys(&mesh, finest), keys(&input, &input.indices));
        assert!(lods.len() > 1, "{lods:?}");
        // Ordered for the GPU's caches, its order changes.
        let mut ordered = input.clone();
        let lods = ordered.optimize();
        let finest = &ordered.indices[lods[0].range()];
        assert_ne!(keys(&ordered, finest), keys(&input, &input.indices));
    }

    #[test]
    fn a_triangle_has_one_level() {
        let mut mesh = Mesh::facing(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            vec![[1.0, 0.5, 0.25, 1.0]; 3],
            vec![2, 1, 0],
        );
        let input = mesh.triangles(&mesh.indices);
        let lods = mesh.optimize();
        check("triangle", &mesh, &lods);
        assert_eq!(
            lods,
            [Lod {
                start: 0,
                count: 3,
                error: 0.0
            }]
        );
        assert_eq!(mesh.triangles(&mesh.indices), input);
    }

    /// A cube's edges are seams: its faces' vertices at one position stay apart, as their UVs
    /// differ, and no level of a finer cube has a triangle across one, which would be a collapse
    /// across a seam.
    #[test]
    fn a_cube_keeps_its_seams() {
        let mut mesh = cube(1);
        let lods = mesh.optimize();
        check("cube", &mesh, &lods);
        assert_eq!(mesh.positions.len(), 24, "four to a face, none shared");
        assert_eq!(lods.len(), 1, "12 triangles are under the floor");

        let mut mesh = cube(8);
        let vertices = mesh.positions.len();
        let lods = mesh.optimize();
        check("fine cube", &mesh, &lods);
        assert_eq!(mesh.positions.len(), vertices, "no seam welded");
        assert!(lods.len() > 1, "{lods:?}");
        for lod in &lods {
            for triangle in mesh.indices[lod.range()].as_chunks::<3>().0 {
                let face = |i: u32| mesh.uvs[i as usize][0].floor();
                assert!(
                    face(triangle[0]) == face(triangle[1])
                        && face(triangle[1]) == face(triangle[2]),
                    "{lod:?} has a triangle of more than one face"
                );
            }
        }
    }

    /// A mesh every vertex of which is a seam, as a faceted one's, can't be simplified, and has
    /// one level.
    #[test]
    fn a_mesh_that_cant_be_simplified_has_one_level() {
        let mut mesh = unwelded(&grid(16), |_| false);
        let lods = mesh.optimize();
        check("faceted", &mesh, &lods);
        assert_eq!(lods.len(), 1, "{lods:?}");
    }

    /// A mesh stuck in part, as a grid with half of it faceted is, still has the levels that the
    /// simplifier can make of the rest.
    #[test]
    fn a_mesh_stuck_in_part_still_gets_levels() {
        let mut mesh = unwelded(&grid(32), |corner| corner[0] < 0.5);
        let lods = mesh.optimize();
        check("half faceted", &mesh, &lods);
        assert!(lods.len() > 1, "{lods:?}");
    }

    /// The same input gives the same output, down to the bit.
    #[test]
    fn the_same_input_gives_the_same_output() {
        let run = || {
            let mut mesh = grid(48);
            let lods = mesh.optimize();
            let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
            let error_bits: Vec<u32> = lods.iter().map(|lod| lod.error.to_bits()).collect();
            (
                lods.iter()
                    .map(|lod| (lod.start, lod.count))
                    .collect::<Vec<_>>(),
                error_bits,
                mesh.indices,
                bits(&mesh.positions.concat()),
                bits(&mesh.uvs.concat()),
                bits(&mesh.colors.concat()),
                bits(&mesh.normals.concat()),
                bits(&mesh.tangents.concat()),
            )
        };
        assert!(run() == run());
    }

    /// `mesh` optimized into levels, packed, and cut into meshlets.
    fn cut_mesh(mut mesh: Mesh) -> ModelMesh {
        let lods = mesh.optimize();
        let vertices: Vec<_> = (0..mesh.positions.len())
            .map(|i| gg_engine::cook::ModelVertex {
                position: mesh.positions[i].into(),
                uv: mesh.uvs[i].into(),
                color: mesh.colors[i].into(),
                normal: mesh.normals[i].into(),
                tangent: mesh.tangents[i].into(),
            })
            .collect();
        let levels = (lods.iter())
            .map(|lod| gg_engine::cook::Lod {
                start: lod.start,
                count: lod.count,
                error: lod.error,
            })
            .collect();
        let mut packed = ModelMesh::pack("cut", &vertices, mesh.indices, levels).expect("a mesh");
        cut(&mut packed);
        packed
    }

    /// `corners` of a triangle, from its least on, keeping its winding.
    fn rotated(corners: [u32; 3]) -> [u32; 3] {
        let least = (0..3).min_by_key(|&i| corners[i]).unwrap();
        [0, 1, 2].map(|i| corners[(least + i) % 3])
    }

    /// Each level's meshlets hold its triangles, each once, winding as they did, within a
    /// meshlet's limits, each in a sphere round its vertices as they unpack; and the mesh holds
    /// together.
    #[test]
    fn each_levels_meshlets_hold_its_triangles() {
        for (name, input) in [("grid", grid(48)), ("cube", cube(12))] {
            let mesh = cut_mesh(input);
            mesh.check(name).unwrap_or_else(|error| panic!("{error:#}"));
            let positions = mesh.positions();
            let meshlets = &mesh.meshlets;
            assert_eq!(meshlets.levels.len(), mesh.lods.levels.len(), "{name}");
            for (lod, &[first, count]) in mesh.lods.levels.iter().zip(&meshlets.levels) {
                let start = lod.start as usize;
                let indices = &mesh.indices[start..start + lod.count as usize];
                let mut level: Vec<_> = (indices.as_chunks::<3>().0.iter())
                    .map(|&corners| rotated(corners))
                    .collect();
                let mut held = Vec::new();
                for meshlet in &meshlets.meshlets[first as usize..(first + count) as usize] {
                    let vertex = |corner: u8| {
                        meshlets.vertices[(meshlet.vertices + u32::from(corner)) as usize]
                    };
                    let start = meshlet.triangles as usize;
                    let bytes =
                        &meshlets.triangles[start..start + 3 * meshlet.triangle_count as usize];
                    for corners in bytes.as_chunks::<3>().0 {
                        held.push(rotated(corners.map(vertex)));
                    }
                    for i in meshlet.vertices..meshlet.vertices + meshlet.vertex_count {
                        let p = positions[meshlets.vertices[i as usize] as usize];
                        let apart = p.distance(meshlet.center);
                        assert!(
                            apart <= meshlet.radius * 1.0001 + 1e-6,
                            "{name}: {meshlet:?}"
                        );
                    }
                }
                level.sort_unstable();
                held.sort_unstable();
                assert_eq!(held, level, "{name}: a level's triangles, each once");
            }
        }
    }

    /// Wherever a meshlet's cone says a camera sees only its triangles' backs, as the culling tests
    /// it, it does: no triangle faces a camera on the cone's axis, behind its sphere, at any
    /// distance.
    #[test]
    fn a_meshlets_cone_hides_only_triangles_facing_away() {
        let mesh = cut_mesh(cube(12));
        let positions = mesh.positions();
        let meshlets = &mesh.meshlets;
        let mut tested = 0;
        for meshlet in &meshlets.meshlets {
            let [x, y, z, cutoff] = meshlet.cone.map(|c| f32::from(c) / 127.0);
            let axis = Vec3::new(x, y, z);
            // A cone of no axis, of a meshlet facing every way, hides it from no camera.
            let Some(way) = axis.try_normalize() else {
                continue;
            };
            for distance in [1.5, 4.0, 40.0] {
                let camera = meshlet.center - way * meshlet.radius * distance;
                let d = meshlet.center - camera;
                if d.dot(axis) < cutoff * d.length() + meshlet.radius {
                    continue;
                }
                tested += 1;
                let start = meshlet.triangles as usize;
                let bytes = &meshlets.triangles[start..start + 3 * meshlet.triangle_count as usize];
                for corners in bytes.as_chunks::<3>().0 {
                    let [a, b, c] = corners.map(|corner| {
                        let vertex =
                            meshlets.vertices[(meshlet.vertices + u32::from(corner)) as usize];
                        positions[vertex as usize]
                    });
                    let normal = (b - a).cross(c - a);
                    assert!(
                        normal.dot(a - camera) >= -1e-4 * normal.length(),
                        "{meshlet:?} shows a triangle's front to {camera}"
                    );
                }
            }
        }
        assert!(
            tested > 0,
            "some cones hide their meshlets from some cameras"
        );
    }

    /// Inputs that make no sense don't panic or hang, and give levels that hold what any do.
    #[test]
    fn meshes_that_make_no_sense_dont_panic() {
        type Edit = fn(&mut Mesh);
        let cases: [(&str, Edit); 10] = [
            ("some not a number", |mesh| {
                mesh.positions
                    .iter_mut()
                    .step_by(7)
                    .for_each(|p| p[1] = f32::NAN);
            }),
            ("all not a number", |mesh| {
                (mesh.positions.iter_mut()).for_each(|p| *p = [f32::NAN; 3]);
            }),
            ("some infinite", |mesh| {
                mesh.positions
                    .iter_mut()
                    .step_by(5)
                    .for_each(|p| p[0] = f32::INFINITY);
            }),
            // The simplifier's error for these is not a number: the chain ends before it.
            ("opposite infinities", |mesh| {
                for (i, p) in mesh.positions.iter_mut().enumerate() {
                    p[2] = [f32::INFINITY, f32::NEG_INFINITY][i % 2];
                }
            }),
            ("extent past a float", |mesh| {
                for (i, p) in mesh.positions.iter_mut().enumerate() {
                    p[0] = [3e38, -3e38][i % 2];
                }
            }),
            ("enormous", |mesh| {
                mesh.positions.iter_mut().flatten().for_each(|c| *c *= 1e38);
            }),
            ("tiny", |mesh| {
                mesh.positions
                    .iter_mut()
                    .flatten()
                    .for_each(|c| *c *= 1e-40);
            }),
            ("one point", |mesh| {
                mesh.positions.iter_mut().for_each(|p| *p = [1.0; 3]);
            }),
            ("degenerate triangles", |mesh| {
                (mesh.indices.as_chunks_mut::<3>().0.iter_mut().step_by(2))
                    .for_each(|t| t[1] = t[0]);
            }),
            ("no triangles", |mesh| mesh.indices.clear()),
        ];
        for (name, edit) in cases {
            let mut mesh = grid(16);
            edit(&mut mesh);
            let lods = mesh.optimize();
            check(name, &mesh, &lods);
        }
    }
}
