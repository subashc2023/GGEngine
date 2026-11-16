use std::f32::consts::FRAC_PI_2;

use gg_core::Error;
use gg_core::math::{Mat4, Quat, Vec2};

use super::draw::{LodView, scissor};
use super::registry::Added;
use super::streaming::{budget_behind, exceeds, room_for, waits};
use super::*;
use crate::frame::canvas_projection;
use crate::{Mipmaps, Projection, Uploads, Viewpoint, sprite, texture};

const TRIANGLE: [[f32; 3]; 3] = [[0.0, -0.5, 0.0], [0.5, 0.5, 0.0], [-0.5, 0.5, 0.0]];

/// Three triangles of `TRIANGLE`'s corners, for levels of detail to pick among.
const THREE: [u32; 9] = [0, 1, 2, 2, 1, 0, 1, 2, 0];

/// Levels of detail starting anywhere, with `errors`, in a sphere of `radius` round the origin.
fn lods(errors: &[f32], radius: f32) -> Lods {
    let level = |&error| Lod {
        start: 0,
        count: 3,
        error,
    };
    Lods {
        levels: errors.iter().map(level).collect(),
        center: Vec3::ZERO,
        radius,
    }
}

/// The meshes added or replaced, waiting, in order.
fn adds(meshes: &Meshes) -> Vec<&Added> {
    (meshes.changes.iter())
        .filter_map(|change| match change {
            Change::Added(added) => Some(added),
            Change::Removed(_) => None,
        })
        .collect()
}

#[test]
fn a_mesh_is_whole_triangles_of_its_own_vertices() {
    let mut meshes = Meshes::default();
    let bad = [&[][..], &[0, 1], &[0, 1, 2, 0], &[0, 1, 3]];
    for indices in bad {
        let error = meshes
            .add("broken", &TRIANGLE, indices)
            .expect_err("a broken mesh");
        assert_eq!(error.code(), "renderer.bad_mesh", "{indices:?}");
        assert!(error.to_string().contains("\"broken\""), "{error}");
    }
    let mut add = |indices: &[u32]| meshes.add("a", &TRIANGLE, indices).map(|mesh| mesh.index);
    // After the engine's quad.
    assert_eq!(add(&[0, 1, 2]).ok(), Some(1));
    assert_eq!(add(&[2, 1, 0]).ok(), Some(2));
}

/// A mesh's sphere is its levels' at once, through replacements; one without a finite sphere, or
/// removed, has none.
#[test]
fn a_mesh_s_sphere_is_the_one_its_levels_hold() {
    let mut meshes = Meshes::default();
    let draw = |index| MeshDraw::from_parts(index, 0, false, false, &[]).expect("no parameters");
    let sphere = |meshes: &Meshes, index| meshes.sphere_of(&draw(index));
    let mut round = lods(&[0.0], 2.0);
    round.center = Vec3::new(1.0, 0.0, 0.0);
    let a = meshes
        .add_lods("a", &TRIANGLE, &[0, 1, 2], round)
        .expect("a");
    let b = meshes.add("b", &TRIANGLE, &[0, 1, 2]).expect("b");
    let c = meshes
        .add_lods("c", &TRIANGLE, &[0, 1, 2], lods(&[0.0], 1.0))
        .expect("c");
    let quad = sprite::QUAD.index;
    assert_eq!(sphere(&meshes, a.index), Some((Vec3::X, 2.0)));
    assert_eq!(
        [quad, b.index].map(|index| sphere(&meshes, index)),
        [None, None],
        "infinite spheres"
    );
    // A replacement without levels has none; one with them has theirs.
    meshes.replace("a", a, &TRIANGLE, &[0, 1, 2]).expect("a");
    assert_eq!(sphere(&meshes, a.index), None);
    let replacement = lods(&[0.0], 3.0);
    let replaced = meshes.replace_lods("b", b, &TRIANGLE, &[0, 1, 2], replacement);
    replaced.expect("b");
    assert_eq!(sphere(&meshes, b.index), Some((Vec3::ZERO, 3.0)));
    // Removed, and its replacement dropped.
    assert!(meshes.remove(c));
    assert_eq!(sphere(&meshes, c.index), None);
    let again = meshes.replace_lods("c", c, &TRIANGLE, &[0, 1, 2], lods(&[0.0], 4.0));
    again.expect("c");
    assert_eq!(sphere(&meshes, c.index), None);
    assert_eq!(sphere(&meshes, 99), None, "a handle never handed out");
}

#[test]
fn a_mesh_replaced_keeps_its_handle_and_the_place_of_triangles_waiting() {
    let mut meshes = Meshes::default();
    let a = meshes.add("a", &TRIANGLE, &[0, 1, 2]).expect("a");
    let waiting = |meshes: &Meshes| -> Vec<(u32, bool, Vec<u32>)> {
        (adds(meshes).into_iter())
            .map(|added| (added.index, added.replacement, added.indices.clone()))
            .collect()
    };
    // Still waiting as added, so the replacement takes its place.
    let square = [TRIANGLE.as_slice(), &[[1.0; 3]]].concat();
    meshes
        .replace("a", a, &square, &[0, 1, 2, 2, 1, 3])
        .expect("others");
    let quad = (0, false, sprite::INDICES.to_vec());
    assert_eq!(waiting(&meshes), [quad, (1, false, vec![0, 1, 2, 2, 1, 3])]);
    // Once a frame has taken it, in place of the renderer's.
    meshes.changes.clear();
    meshes
        .replace("a", a, &TRIANGLE, &[2, 1, 0])
        .expect("others");
    assert_eq!(waiting(&meshes), [(1, true, vec![2, 1, 0])]);
    // Broken triangles leave it as it was.
    let error = (meshes.replace("a", a, &TRIANGLE, &[0, 1, 3])).expect_err("broken");
    assert_eq!(error.code(), "renderer.bad_mesh");
    assert_eq!(waiting(&meshes), [(1, true, vec![2, 1, 0])]);
}

/// A removal, by handle or index, follows the changes before it, once however often; its handle
/// isn't reused, and its replacement is checked, then dropped.
#[test]
fn a_mesh_removed_stays_removed_in_order() {
    let mut meshes = Meshes::default();
    let a = meshes.add("a", &TRIANGLE, &[0, 1, 2]).expect("a");
    meshes.changes.clear();
    meshes
        .replace("a", a, &TRIANGLE, &[2, 1, 0])
        .expect("others");
    assert!(meshes.remove(a), "removed");
    assert!(!meshes.remove_index(a.index()), "removed already");
    let b = meshes.add("b", &TRIANGLE, &[0, 1, 2]).expect("b");
    assert_eq!((a.index, b.index), (1, 2));
    let order: Vec<(u32, bool)> = (meshes.changes.iter())
        .map(|change| match change {
            Change::Added(added) => (added.index, false),
            Change::Removed(index) => (*index, true),
        })
        .collect();
    assert_eq!(order, [(1, false), (1, true), (2, false)]);
    meshes.changes.clear();
    meshes
        .replace("a", a, &TRIANGLE, &[0, 1, 2])
        .expect("checked");
    assert!(meshes.changes.is_empty(), "dropped");
    let error = (meshes.replace("a", a, &TRIANGLE, &[0, 1, 3])).expect_err("broken");
    assert_eq!(error.code(), "renderer.bad_mesh");
}

#[test]
#[should_panic(expected = "is one of another `Meshes`")]
fn another_meshes_mesh_cant_be_removed() {
    Meshes::default().remove(Mesh::<Vec2>::new(5));
}

/// An asset waits when its bytes, however large, would take usage past the budget while something
/// else copies; with nothing copying, the oldest waiting goes ahead alone, so nothing waits forever
/// on a GPU already past budget.
#[test]
fn an_asset_past_the_budget_waits_unless_nothing_else_is_copying() {
    assert!(!exceeds(10, 100, 90));
    assert!(exceeds(10, 100, 91));
    assert!(exceeds(0, 100, 101));
    assert!(!exceeds(0, 0, 0));
    assert!(exceeds(u64::MAX, 100, 1));
    assert!(!waits(10, 100, 90, true));
    assert!(waits(10, 100, 91, true));
    assert!(!waits(10, 100, 91, false));
    assert!(!waits(200, 100, 16, false), "past the budget already");
    assert!(waits(200, 100, 16, true));
}

/// A frame copies its budget while the transfer queue is at most a frame behind, and nothing at two
/// or more.
#[test]
fn a_frame_copies_nothing_once_the_transfer_queue_falls_behind() {
    assert_eq!(budget_behind(0, 1 << 20), 1 << 20);
    assert_eq!(budget_behind(1, 1 << 20), 1 << 20);
    assert_eq!(budget_behind(2, 1 << 20), 0);
    assert_eq!(budget_behind(7, 1 << 20), 0);
}

/// Buffers or an image that don't fit GPU memory leave their asset waiting, warned once; any other
/// failure ends the renderer.
#[test]
fn an_asset_that_doesnt_fit_waits_and_other_failures_propagate() {
    let mut warned = false;
    let no_room = || Error::new("gpu.out_of_memory", "vkCreateBuffer failed");
    assert_eq!(
        room_for(Err::<u32, _>(no_room()), "a", 16, &mut warned).ok(),
        Some(None)
    );
    assert!(warned);
    assert_eq!(room_for(Ok(5), "a", 16, &mut warned).ok(), Some(Some(5)));
    let lost = Error::new("gpu.device_lost", "vkCreateBuffer failed");
    let error = room_for(Err::<u32, _>(lost), "a", 16, &mut warned).expect_err("propagated");
    assert_eq!(error.code(), "gpu.device_lost");
}

#[test]
fn uploads_keep_what_a_frame_left_ahead_of_what_they_take_next() {
    let mut meshes = Meshes::default();
    let mut shaders = crate::Shaders::default();
    let mut textures = crate::Textures::default();
    let mut materials = crate::Materials::default();
    let mut uploads = Uploads::default();
    uploads.take(&mut meshes, &mut shaders, &mut textures, &mut materials);
    // As a frame that presented nothing leaves them, with more added before the next.
    meshes.add("a", &TRIANGLE, &[0, 1, 2]).unwrap();
    textures.add("a", [1, 1], &[255; 4], Mipmaps::None).unwrap();
    uploads.take(&mut meshes, &mut shaders, &mut textures, &mut materials);
    let meshes_held: Vec<u32> = (uploads.meshes.iter())
        .map(|change| match change {
            Change::Added(added) => added.index,
            Change::Removed(index) => *index,
        })
        .collect();
    // A texture's slot, then its image.
    let textures_held: Vec<u32> = (uploads.textures.iter())
        .map(|change| match change {
            texture::Change::Reserved { index, .. } => *index,
            texture::Change::Image(added) => added.index,
            texture::Change::Removed(index) => *index,
        })
        .collect();
    assert_eq!((meshes_held, textures_held), (vec![0, 1], vec![0, 0, 1, 1]));
    assert!(meshes.changes.is_empty() && textures.changes.is_empty());
}

#[test]
fn levels_of_detail_are_whole_triangles_among_the_indices_with_errors_that_grow() {
    let mut meshes = Meshes::default();
    let level = |start, count, error| Lod {
        start,
        count,
        error,
    };
    let sphere = |levels: Vec<Lod>, center: Vec3, radius: f32| Lods {
        levels,
        center,
        radius,
    };
    let levels = |levels: Vec<Lod>| sphere(levels, Vec3::ZERO, 1.0);
    let bad = [
        ("no levels of detail", levels(vec![])),
        (
            "9 levels of detail, past the 8",
            levels(vec![level(0, 3, 0.0); MAX_LEVELS + 1]),
        ),
        ("past its 9 indices", levels(vec![level(6, 6, 0.0)])),
        ("past its 9 indices", levels(vec![level(u32::MAX, 3, 0.0)])),
        ("without triangles", levels(vec![level(3, 0, 0.0)])),
        ("not three for each", levels(vec![level(0, 4, 0.0)])),
        ("not a finite distance", levels(vec![level(0, 3, -1.0)])),
        ("not a finite distance", levels(vec![level(0, 3, f32::NAN)])),
        (
            "not a finite distance",
            levels(vec![level(0, 3, f32::INFINITY)]),
        ),
        (
            "less than the 0.5",
            levels(vec![level(0, 6, 0.5), level(6, 3, 0.25)]),
        ),
        ("centered", sphere(vec![level(0, 3, 0.0)], Vec3::NAN, 1.0)),
        (
            "centered",
            sphere(vec![level(0, 3, 0.0)], Vec3::INFINITY, 1.0),
        ),
        (
            "radius -1",
            sphere(vec![level(0, 3, 0.0)], Vec3::ZERO, -1.0),
        ),
        (
            "radius NaN",
            sphere(vec![level(0, 3, 0.0)], Vec3::ZERO, f32::NAN),
        ),
        (
            "radius inf",
            sphere(vec![level(0, 3, 0.0)], Vec3::ZERO, f32::INFINITY),
        ),
    ];
    for (problem, lods) in bad {
        let error = meshes
            .add_lods("broken", &TRIANGLE, &THREE, lods.clone())
            .expect_err(problem);
        assert_eq!(error.code(), "renderer.bad_mesh", "{lods:?}");
        let message = error.to_string();
        assert!(message.contains("\"broken\""), "{message}");
        assert!(message.contains(problem), "{message} for {problem}");
    }
    // Indices no mesh has, though every level is among them.
    let error = (meshes.add_lods(
        "broken",
        &TRIANGLE,
        &[0, 1, 3],
        levels(vec![level(0, 3, 0.0)]),
    ))
    .expect_err("an index past the vertices");
    assert_eq!(error.code(), "renderer.bad_mesh");
    assert!(error.to_string().contains("past its 3 vertices"), "{error}");
    // Only the engine's quad was added; the next handle follows it.
    assert_eq!(meshes.count(), 1);
    assert_eq!(meshes.changes.len(), 1);

    // Levels may share indices, repeat an error, and not start at the first.
    let shared = levels(vec![level(0, 9, 0.0), level(3, 6, 0.1), level(3, 3, 0.1)]);
    let mesh = (meshes.add_lods("shared", &TRIANGLE, &THREE, shared.clone())).expect("a mesh");
    assert_eq!(mesh.index, 1);
    assert_eq!(adds(&meshes)[1].lods, shared);
    assert_eq!(adds(&meshes)[1].indices, THREE);
}

#[test]
fn a_mesh_has_one_level_of_all_its_indices_unless_added_with_more_and_again_once_replaced() {
    let mut meshes = Meshes::default();
    let whole = |indices| Lods {
        levels: vec![Lod {
            start: 0,
            count: indices,
            error: 0.0,
        }],
        // No sphere, as `add` can't read vertex positions.
        center: Vec3::ZERO,
        radius: f32::INFINITY,
    };
    assert_eq!(adds(&meshes)[0].lods, whole(6), "the quad");
    let plain = meshes.add("plain", &TRIANGLE, &THREE).expect("a mesh");
    assert_eq!(adds(&meshes)[1].lods, whole(9));
    let finer = Lods {
        levels: vec![
            Lod {
                start: 0,
                count: 6,
                error: 0.0,
            },
            Lod {
                start: 6,
                count: 3,
                error: 0.5,
            },
        ],
        center: Vec3::X,
        radius: 2.0,
    };
    let with_levels = (meshes.add_lods("levels", &TRIANGLE, &THREE, finer.clone())).expect("a");
    assert_eq!(adds(&meshes)[2].lods, finer);
    // Replaced while it waits, and once a frame has taken it.
    meshes
        .replace("levels", with_levels, &TRIANGLE, &[0, 1, 2])
        .expect("others");
    assert_eq!(adds(&meshes)[2].lods, whole(3));
    meshes.changes.clear();
    meshes
        .replace("plain", plain, &TRIANGLE, &[0, 1, 2, 2, 1, 0])
        .expect("others");
    assert_eq!(adds(&meshes)[0].lods, whole(6));
    // Replaced with levels, taken and checked as `add_lods` does; refused ones leave what waits as
    // it was.
    meshes
        .replace_lods("plain", plain, &TRIANGLE, &THREE, finer.clone())
        .expect("levels");
    assert_eq!(adds(&meshes)[0].lods, finer);
    let unbounded = Lods {
        radius: f32::INFINITY,
        ..finer.clone()
    };
    let refused = meshes.replace_lods("plain", plain, &TRIANGLE, &THREE, unbounded);
    assert_eq!(refused.expect_err("no sphere").code(), "renderer.bad_mesh");
    assert_eq!(adds(&meshes)[0].lods, finer);
}

/// What a viewpoint at the origin facing -Z, or as `viewpoint` has it, sees of a `size`-pixel view.
fn lod_view(viewpoint: Viewpoint, size: [u32; 2]) -> LodView {
    let aspect = size[0] as f32 / size[1] as f32;
    LodView::new(&viewpoint.view_projection(aspect), size)
}

fn looking(projection: Projection) -> Viewpoint {
    Viewpoint {
        projection,
        ..Viewpoint::default()
    }
}

#[test]
fn perspective_picks_the_coarsest_level_whose_error_spans_a_pixel_where_the_sphere_is_nearest() {
    // A quarter turn bottom to top over 200 pixels: 100 a unit where W is 1, across too, over 320.
    let perspective = Projection::Perspective {
        fov: FRAC_PI_2,
        near: 0.1,
    };
    let view = lod_view(looking(perspective), [320, 200]);
    // The sphere's nearest point is a unit nearer than its center, so level 1's error spans 0.05 *
    // 100 / (depth - 1) pixels, and level 2's 0.4 * 100 / (depth - 1).
    let lods = lods(&[0.0, 0.05, 0.4], 1.0);
    let level = |at: Vec3| view.level(&lods, &Mat4::from_translation(at));
    for (depth, expected) in [(1.5, 0), (5.9, 0), (6.1, 1), (40.9, 1), (41.1, 2), (1e6, 2)] {
        assert_eq!(level(Vec3::new(0.0, 0.0, -depth)), expected, "{depth} away");
        // Off to the side as far as it is ahead, it's as far.
        let aside = Vec3::new(depth, -depth / 2.0, -depth);
        assert_eq!(level(aside), expected, "{depth} away and aside");
    }
    // Behind the camera, and so close its sphere reaches the camera's plane.
    assert_eq!(level(Vec3::new(0.0, 0.0, 1e3)), 0, "behind");
    assert_eq!(
        level(Vec3::new(0.0, 0.0, -1.0)),
        0,
        "the sphere reaches the plane"
    );

    // Turned and moved, the camera sees the same: distance ahead is what counts.
    let rotation = Quat::from_rotation_y(1.0) * Quat::from_rotation_x(-0.4);
    let position = Vec3::new(3.0, -2.0, 5.0);
    let viewpoint = Viewpoint {
        position: position.as_dvec3(),
        rotation,
        projection: perspective,
    };
    let view = lod_view(viewpoint, [320, 200]);
    for (depth, expected) in [(5.9, 0), (6.1, 1), (40.9, 1), (41.1, 2)] {
        let at = position + rotation * Vec3::new(0.0, 0.0, -depth);
        let placed = view.level(&lods, &Mat4::from_translation(at));
        assert_eq!(placed, expected, "{depth} away from a turned camera");
    }
}

#[test]
fn a_transform_scales_the_error_and_the_sphere_by_its_longest_axis() {
    let perspective = Projection::Perspective {
        fov: FRAC_PI_2,
        near: 0.1,
    };
    let view = lod_view(looking(perspective), [320, 200]);
    let lods = lods(&[0.0, 0.05], 1.0);
    let level = |scale: Vec3, depth: f32| {
        // Turned, so the axes aren't the world's.
        let turn = Quat::from_rotation_z(0.7) * Quat::from_rotation_y(0.3);
        let at = Vec3::new(1.0, 2.0, -depth);
        let placed = Mat4::from_scale_rotation_translation(scale, turn, at);
        view.level(&lods, &placed)
    };
    // Doubled: the error spans 0.05 * 2 * 100 / (depth - 2) pixels.
    assert_eq!(level(Vec3::splat(2.0), 11.9), 0);
    assert_eq!(level(Vec3::splat(2.0), 12.1), 1);
    // Three times as long on one axis alone: the same by 3 and (depth - 3), whichever axis.
    for scale in [
        Vec3::new(3.0, 1.0, 1.0),
        Vec3::new(1.0, 3.0, 2.0),
        Vec3::new(0.5, 1.0, 3.0),
    ] {
        assert_eq!(level(scale, 17.9), 0, "{scale}");
        assert_eq!(level(scale, 18.1), 1, "{scale}");
    }
}

#[test]
fn the_sphere_is_round_its_center_not_the_meshs_origin() {
    let view = lod_view(
        looking(Projection::Perspective {
            fov: FRAC_PI_2,
            near: 0.1,
        }),
        [320, 200],
    );
    // 5 farther than the mesh's origin, so its sphere's nearest point is 4 past the origin's depth,
    // and level 1's error spans 0.05 * 100 / (depth + 4) pixels: one at 1.
    let behind = Lods {
        center: Vec3::new(0.0, 0.0, -5.0),
        ..lods(&[0.0, 0.05], 1.0)
    };
    let level = |depth: f32| {
        let at = Mat4::from_translation(Vec3::new(0.0, 0.0, -depth));
        view.level(&behind, &at)
    };
    assert_eq!(level(0.9), 0);
    assert_eq!(level(1.1), 1);
    // Turned a half about Y, the center is 5 nearer than the origin instead, the nearest point 6:
    // 0.05 * 100 / (depth - 6) is a pixel at 11.
    let turned = |depth: f32| {
        let at = Mat4::from_translation(Vec3::new(0.0, 0.0, -depth));
        view.level(&behind, &(at * Mat4::from_rotation_y(std::f32::consts::PI)))
    };
    assert_eq!(turned(10.9), 0);
    assert_eq!(turned(11.1), 1);
}

#[test]
fn orthographic_levels_do_not_depend_on_distance() {
    // 4 units over 200 pixels: 50 a unit, across too. The error spans 50 * error pixels anywhere,
    // and the sphere's radius reaches no nearer.
    let orthographic = Projection::Orthographic {
        height: 4.0,
        near: -10.0,
        far: 10.0,
    };
    let view = lod_view(looking(orthographic), [200, 200]);
    let lods = lods(&[0.0, 0.01, 0.03], 5.0);
    for z in [-9.0, -1.0, 0.0, 3.0, 9.0] {
        let at = Mat4::from_translation(Vec3::new(1.0, -1.0, z));
        assert_eq!(view.level(&lods, &at), 1, "at {z}");
    }
    // Scaled, 0.01 spans a pixel at 2 and 0.03 at 0.66.
    let scaled = Mat4::from_scale(Vec3::splat(0.6));
    assert_eq!(view.level(&lods, &scaled), 2);
    let scaled = Mat4::from_scale(Vec3::splat(2.1));
    assert_eq!(view.level(&lods, &scaled), 0);

    // The canvas spans a pixel a unit.
    let canvas = LodView::new(&canvas_projection([400, 300]), [400, 300]);
    let lods = self::lods(&[0.0, 0.5, 2.0], 10.0);
    let level = |scale: f32| canvas.level(&lods, &Mat4::from_scale(Vec3::splat(scale)));
    assert_eq!([level(1.0), level(0.4), level(3.0)], [1, 2, 0]);
}

/// A canvas clip, in view pixels from the bottom left, scissors those pixels from the top left,
/// edges at the nearest pixel's, kept within the view; none outside it.
#[test]
fn a_clip_on_the_canvas_scissors_its_pixels() {
    let canvas = canvas_projection([400, 300]);
    let clip = |least: [f32; 2], most: [f32; 2]| {
        scissor([least, most].map(Vec2::from), canvas, [400, 300])
    };
    assert_eq!(clip([10.0, 20.0], [110.0, 70.0]), ([10, 230], [100, 50]));
    assert_eq!(clip([10.4, 19.6], [110.6, 70.4]), ([10, 230], [101, 50]));
    assert_eq!(clip([-50.0, 250.0], [450.0, 400.0]), ([0, 0], [400, 50]));
    assert_eq!(clip([500.0, 0.0], [600.0, 10.0]).1, [0, 10]);
}

#[test]
fn a_sphere_that_reaches_the_plane_of_the_camera_has_its_finest_level() {
    let view = lod_view(
        looking(Projection::Perspective {
            fov: FRAC_PI_2,
            near: 0.1,
        }),
        [320, 200],
    );
    // An error far under a pixel anywhere ahead of the sphere's nearest point.
    let lods = lods(&[0.0, 0.0005], 3.0);
    let level = |depth: f32| {
        let at = Mat4::from_translation(Vec3::new(0.0, 0.0, -depth));
        view.level(&lods, &at)
    };
    assert_eq!(level(-5.0), 0, "behind it");
    assert_eq!(level(0.0), 0, "in its plane");
    assert_eq!(level(2.9), 0, "across it");
    assert_eq!(level(3.0), 0, "touching it");
    assert_eq!(level(3.1), 1, "just ahead of it");
}

#[test]
fn a_mesh_of_one_level_draws_its_indices_however_far() {
    let view = lod_view(
        looking(Projection::Perspective {
            fov: FRAC_PI_2,
            near: 0.1,
        }),
        [320, 200],
    );
    let whole = Lods::whole(9);
    for depth in [-1.0, 0.5, 10.0, 1e9] {
        let at = Mat4::from_translation(Vec3::new(0.0, 0.0, -depth));
        assert_eq!(view.level(&whole, &at), 0, "{depth} away");
    }
    // A NaN transform also leaves the finest level.
    let broken = Mat4::from_scale(Vec3::NAN);
    assert_eq!(view.level(&lods(&[0.0, 1.0], 1.0), &broken), 0);
}
