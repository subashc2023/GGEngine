//! The engine's own system: drawing the world's entities.

use std::ops::Range;

use gg_renderer::{Draws, MeshDraw, Sprite};

use crate::{Component, Transform, World};

impl Component for Sprite {}

impl Component for MeshDraw {}

/// Rows of one table at one z, drawn together.
pub(crate) struct Segment {
    z: f32,
    /// Which segment it was found as: tables in the order they were made, then a table's sprites
    /// before its meshes, then its rows in order.
    order: u32,
    kind: Kind,
    table: u32,
    rows: Range<u32>,
}

#[derive(Clone, Copy)]
enum Kind {
    Sprite,
    Mesh,
}

/// Draws into `draws` every entity of `world` with a [`Transform`] and a [`Sprite`] or a
/// [`MeshDraw`], back to front by its translation's z; at one z, in the order queries find them,
/// and an entity with both draws its sprite under its mesh. `segments` is the scratch it sorts,
/// kept from frame to frame so that it allocates nothing once it's grown.
///
/// It sorts runs of consecutive rows at one z, so it's quick while each table's rows at one z stay
/// together. Spawning entities of a table at mixed z's, or despawning, which moves a table's last
/// row into the gap, splits those runs; at worst, as when sprites each have their own z, as sorting
/// by y gives them, it sorts every sprite every frame, until the renderer sorts by depth, with
/// Phase 4.
pub(crate) fn draw_world(world: &World, draws: &mut Draws, segments: &mut Vec<Segment>) {
    let _zone = gg_core::zone!();
    segments.clear();
    let Some(transform) = world.component::<Transform>() else {
        return;
    };
    let sprite = world.component::<Sprite>();
    let mesh = world.component::<MeshDraw>();
    for (table, rows) in world.tables().iter().enumerate() {
        let Some(transforms) = rows.column::<Transform>(transform) else {
            continue;
        };
        for (kind, id) in [(Kind::Sprite, sprite), (Kind::Mesh, mesh)] {
            if id.is_some_and(|id| rows.has(id)) {
                cut(segments, transforms, kind, table as u32);
            }
        }
    }
    segments.sort_unstable_by(|a, b| a.z.total_cmp(&b.z).then(a.order.cmp(&b.order)));
    let tables = world.tables();
    for segment in segments.iter() {
        let table = &tables[segment.table as usize];
        let rows = segment.rows.start as usize..segment.rows.end as usize;
        let transforms = &table
            .column::<Transform>(transform)
            .expect("it has transforms")[rows.clone()];
        match segment.kind {
            Kind::Sprite => {
                let sprites = sprite.and_then(|id| table.column::<Sprite>(id));
                let sprites = &sprites.expect("it has sprites")[rows];
                let both = transforms.iter().zip(sprites);
                draws.extend_sprites(both.map(|(transform, sprite)| (transform.matrix(), *sprite)));
            }
            Kind::Mesh => {
                let meshes = mesh.and_then(|id| table.column::<MeshDraw>(id));
                let meshes = &meshes.expect("it has meshes")[rows];
                for (transform, mesh) in transforms.iter().zip(meshes) {
                    draws.push_draw(transform.matrix(), mesh);
                }
            }
        }
    }
}

/// Adds to `segments` the runs of rows at one z among `transforms`, table `table`'s, of `kind`.
fn cut(segments: &mut Vec<Segment>, transforms: &[Transform], kind: Kind, table: u32) {
    let z = |row: usize| transforms[row].translation.z;
    let mut start = 0;
    for row in 1..=transforms.len() {
        if row == transforms.len() || z(row).total_cmp(&z(start)).is_ne() {
            segments.push(Segment {
                z: z(start),
                order: segments.len() as u32,
                kind,
                table,
                rows: start as u32..row as u32,
            });
            start = row;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gg_core::math::{Vec3, Vec4};
    use gg_renderer::{Blend, Meshes, Shaders};

    use super::*;

    #[derive(Clone, Copy)]
    struct Marked;
    impl Component for Marked {}

    enum Expected {
        Sprite(Transform, Sprite),
        Mesh(Transform, MeshDraw),
    }

    /// Spawns sprites and meshes at several z's across tables, and checks that they're drawn back
    /// to front, and at one z in the order queries find them: tables in the order they were made,
    /// a table's sprites before its meshes, then its rows in order. Two tables' rows alternate
    /// between two z's, so that dozens of runs share each, enough for a sort that loses that
    /// order to show it.
    #[test]
    fn the_world_draws_back_to_front_by_z() {
        let mut shaders = Shaders::default();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/flat_color.slang");
        let flat = shaders
            .add::<[f32; 3], Vec4>("flat color", &fixture, Blend::Opaque)
            .expect("the shader compiles");
        let corners = [[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]];
        let triangle = Meshes::default().add("triangle", &corners, &[0, 1, 2]);
        let triangle = triangle.expect("a mesh");
        // Each draw is told apart by its x and its color.
        let mut next = 0.0;
        let mut new = |z: f32| {
            next += 1.0;
            let at = Transform {
                translation: Vec3::new(next, 0.0, z),
                ..Transform::IDENTITY
            };
            let sprite = Sprite {
                color: Vec4::splat(next),
                ..Sprite::default()
            };
            (at, sprite, MeshDraw::new(triangle, flat, Vec4::splat(next)))
        };
        let mut world = World::default();
        // In the order of their tables, each table's sprites before its meshes, then its rows.
        let mut expected = Vec::new();
        let alternating = |row: usize| if row.is_multiple_of(2) { 0.5 } else { 0.6 };
        for z in [0.3, 0.1, 0.1].into_iter().chain((0..20).map(alternating)) {
            let (at, sprite, _) = new(z);
            world.spawn((at, sprite));
            expected.push(Expected::Sprite(at, sprite));
        }
        for z in (0..20).map(alternating).chain([0.1]) {
            let (at, sprite, _) = new(z);
            world.spawn((Marked, sprite, at));
            expected.push(Expected::Sprite(at, sprite));
        }
        for z in [0.2, 0.1] {
            let (at, _, mesh) = new(z);
            world.spawn((at, mesh));
            expected.push(Expected::Mesh(at, mesh));
        }
        let mut meshes = Vec::new();
        for z in [0.2, 0.0] {
            let (at, sprite, mesh) = new(z);
            world.spawn((at, sprite, mesh));
            expected.push(Expected::Sprite(at, sprite));
            meshes.push(Expected::Mesh(at, mesh));
        }
        expected.append(&mut meshes);
        // Nothing to draw, or nowhere to draw it.
        let (at, sprite, _) = new(0.0);
        world.spawn(at);
        world.spawn(sprite);

        // A stable sort, which keeps that order among each z's.
        expected.sort_by(|a, b| {
            let z = |expected: &Expected| match expected {
                Expected::Sprite(at, _) | Expected::Mesh(at, _) => at.translation.z,
            };
            z(a).total_cmp(&z(b))
        });
        let mut want = Draws::default();
        for expected in &expected {
            match expected {
                Expected::Sprite(at, sprite) => want.push_sprite(at.matrix(), *sprite),
                Expected::Mesh(at, mesh) => want.push_draw(at.matrix(), mesh),
            }
        }
        let mut draws = Draws::default();
        draw_world(&world, &mut draws, &mut Vec::new());
        assert_eq!(draws, want);
    }
}
