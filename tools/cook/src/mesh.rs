//! A model primitive's triangles, optimized with meshoptimizer for drawing: identical vertices
//! merged, the triangles and the vertices ordered for the GPU's caches, and a chain of coarser
//! levels of detail over the same vertices, which a draw picks from by its size on screen.

use std::ops::Range;

use meshopt::{
    SimplifyOptions, VertexStream, generate_vertex_remap_multi, optimize_overdraw_in_place_decoder,
    optimize_vertex_cache_in_place, optimize_vertex_fetch_remap, remap_index_buffer,
    remap_vertex_buffer, simplify_decoder,
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

/// Optimizes a primitive's triangles in place, and returns its levels of detail, finest first.
///
/// Vertices identical in position, UV and color merge into one, and those no triangle uses go. Level
/// 0 has the input's triangles, with the winding they had, in the order and from the corners that
/// the vertex cache and overdraw optimizers choose. Each coarser level is the one before it
/// simplified toward half its triangles, by a simplifier that moves no vertex and keeps attribute
/// seams, the edges where vertices of one position differ in UV or color, then ordered for the
/// vertex cache. The chain ends at [`MAX_LEVELS`] levels, or before a level that would shed less
/// than [`MIN_SHED`] of the one before it, and no level is simplified toward fewer than
/// [`MIN_TRIANGLES`] triangles, though the simplifier may land a few short of its target.
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
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    keep_order: bool,
) -> Vec<Lod> {
    // meshoptimizer builds with its assertions off in release, and reads past the vertices of an
    // index that is.
    assert!(
        uvs.len() == positions.len()
            && colors.len() == positions.len()
            && indices.len().is_multiple_of(3)
            && indices
                .iter()
                .all(|&index| (index as usize) < positions.len()),
        "a primitive's vertex arrays are of one length, and its indices whole triangles within them"
    );

    // Its simplifier follows the mesh's topology, so identical vertices left apart would be seams
    // it can't collapse across (the README's basic simplification). All three attributes are
    // compared, bit for bit, as the shader reads them.
    let (count, remap) = generate_vertex_remap_multi(
        positions.len(),
        &[
            VertexStream::new(positions.as_ptr()),
            VertexStream::new(uvs.as_ptr()),
            VertexStream::new(colors.as_ptr()),
        ],
        Some(indices),
    );
    apply_remap(positions, uvs, colors, indices, count, &remap);

    if !keep_order {
        optimize_vertex_cache_in_place(indices, count);
        // After the vertex cache, which it takes the output of.
        optimize_overdraw_in_place_decoder(indices, positions, OVERDRAW_THRESHOLD);
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
            positions,
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
    apply_remap(positions, uvs, colors, indices, count, &remap);
    lods
}

/// Moves the vertex `i` of `positions`, `uvs` and `colors` to `remap[i]`, of `count` after the move,
/// and rewrites `indices` to match.
fn apply_remap(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    count: usize,
    remap: &[u32],
) {
    let vertices = positions.len();
    *indices = remap_index_buffer(Some(indices.as_slice()), vertices, remap);
    *positions = remap_vertex_buffer(positions, count, remap);
    *uvs = remap_vertex_buffer(uvs, count, remap);
    *colors = remap_vertex_buffer(colors, count, remap);
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
        indices: Vec<u32>,
    }

    /// A vertex as the bits of its fields, so that vertices compare and sort whatever order a mesh
    /// holds them in.
    type Key = [u32; 9];

    impl Mesh {
        fn optimize(&mut self) -> Vec<Lod> {
            self.optimize_keeping(false)
        }

        /// Optimizes it as `optimize` does, keeping its triangles' order if `keep_order`.
        fn optimize_keeping(&mut self, keep_order: bool) -> Vec<Lod> {
            optimize(
                &mut self.positions,
                &mut self.uvs,
                &mut self.colors,
                &mut self.indices,
                keep_order,
            )
        }

        fn key(&self, index: u32) -> Key {
            let i = index as usize;
            let [x, y, z] = self.positions[i].map(f32::to_bits);
            let [u, v] = self.uvs[i].map(f32::to_bits);
            let [r, g, b, a] = self.colors[i].map(f32::to_bits);
            [x, y, z, u, v, r, g, b, a]
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
        Mesh {
            positions,
            uvs,
            colors,
            indices,
        }
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
        Mesh {
            positions,
            uvs,
            colors,
            indices,
        }
    }

    /// `grid` but for its triangles whose first corner `weld` doesn't hold, which have vertices of
    /// their own and a UV of their own, so that each of their vertices is a seam.
    fn unwelded(grid: &Mesh, weld: impl Fn([f32; 3]) -> bool) -> Mesh {
        let mut mesh = Mesh {
            positions: grid.positions.clone(),
            uvs: grid.uvs.clone(),
            colors: grid.colors.clone(),
            indices: vec![],
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
            }
        }
        mesh
    }

    /// Checks what holds of any output: the arrays of one length; the levels' indices one after
    /// another to the end of `indices`, whole triangles within the vertices; level 0 using every
    /// vertex and each coarser level a range of them from 0, which is the order of first use from
    /// the coarsest level on, as each level's vertices are some of the level before's; and the
    /// errors finite, 0 for level 0 and not falling.
    fn check(name: &str, mesh: &Mesh, lods: &[Lod]) {
        let vertices = mesh.positions.len();
        assert_eq!(mesh.uvs.len(), vertices, "{name}");
        assert_eq!(mesh.colors.len(), vertices, "{name}");
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
        let mut mesh = Mesh {
            // A square of two triangles, whose corners 0 and 2 are held twice, and a vertex that
            // no triangle uses.
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [5.0, 5.0, 5.0],
            ],
            uvs: vec![
                [0.0, 0.0],
                [1.0, 0.0],
                [1.0, 1.0],
                [0.0, 1.0],
                [0.0, 0.0],
                [1.0, 1.0],
                [9.0, 9.0],
            ],
            colors: vec![[1.0; 4]; 7],
            indices: vec![0, 1, 2, 4, 5, 3],
        };
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
        let mut mesh = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            colors: vec![[1.0, 0.5, 0.25, 1.0]; 3],
            indices: vec![2, 1, 0],
        };
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
            )
        };
        assert!(run() == run());
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
