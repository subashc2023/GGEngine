//! Physics, through rapier: bodies that it moves at the world's ticks, as their entities'
//! components describe them, in the XY plane ([`dim2`]) or in space ([`dim3`]), with characters
//! and casts through the colliders in space; the world's resources that hold rapier's bodies and
//! contacts, which snapshots copy and snapshot files hold; and the colliders' outlines, drawn where
//! their entities draw.

use std::f32::consts::TAU;

use gg_core::math::{Mat4, Vec3, Vec4, libm};
use gg_renderer::{Draws, Line, Shaders};
use tracing::warn;

use crate::draw::Poses;
use crate::{Component, Entity, Reflect, Transform, World};

mod dim2;
mod dim3;

pub use dim2::{BoxCollider2D, CircleCollider2D, Physics2D, RigidBody2D};
pub use dim3::{
    BoxCollider3D, CapsuleCollider3D, Cast, CharacterController3D, Hit3D, MeshCollider3D,
    Physics3D, RigidBody3D, Shape3D, SphereCollider3D,
};

/// How a [`RigidBody2D`] or a [`RigidBody3D`] moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum BodyType {
    /// By its velocity, which gravity and what it hits change.
    #[default]
    Dynamic,
    /// By its velocity alone, which nothing it hits changes, pushing what's dynamic aside, as a
    /// moving platform does.
    Kinematic,
    /// Not at all: it stays where its transform puts it, as a floor or a wall does.
    Static,
}

/// Steps `world`'s physics a tick, [`Tick::DT`](crate::Tick::DT), as a system a layer's tick runs,
/// after what sets its bodies' velocities, such as scripts: 2D physics, then 3D. Each takes in what
/// changed since its last step, making a body for each entity that has come to have a
/// [`RigidBody2D`] or a [`RigidBody3D`] and a [`Transform`], in the order queries find them, and
/// taking out each whose entity has gone or lost either; and takes a change to a body's components
/// that the last step didn't make, a game's or an edit's, as where the body is, how fast it goes,
/// how it moves or its colliders. Then it moves the bodies, and writes where each went and how fast
/// into its components: a 2D body's translation across and up, its z as it was, and its rotation
/// turned about Z as far as the body turned, so that what turns it otherwise stays; a 3D body's
/// translation and rotation whole; and their velocities. A world is given a [`Physics2D`] or a
/// [`Physics3D`] the first time it steps with a body of its kind and without one.
pub fn step_physics(world: &mut World) {
    let _zone = gg_core::zone!();
    dim2::step(world);
    dim3::step(world);
}

/// No link, in [`Links::by_slot`].
const NONE: u32 = u32::MAX;

/// Which of a world's entities a physics resource has bodies for, as 2D and 3D physics keep them
/// alike: a link for each entity with a body's component and a [`Transform`], but those under a
/// parent, whose transforms are in their parents' space, which it warns of once each and leaves
/// where they are.
struct Links<L> {
    /// A link to each body, in the order their entities came.
    linked: Vec<L>,
    /// Each link's index in `linked`, by its entity's slot; `NONE` for none.
    by_slot: Vec<u32>,
    /// The entities with bodies, as a step finds them, in memory kept from one step to the next.
    found: Vec<Entity>,
    /// The bodies under a parent, which aren't moved, warned of once each as they're found there.
    children: Vec<Entity>,
}

/// A link of [`Links`], to its entity's body.
trait Link {
    fn entity(&self) -> Entity;
}

impl<L> Default for Links<L> {
    fn default() -> Self {
        Self {
            linked: Vec::new(),
            by_slot: Vec::new(),
            found: Vec::new(),
            children: Vec::new(),
        }
    }
}

impl<L: Clone> Clone for Links<L> {
    /// A copy, without `found`'s memory, as no step carries anything in it over.
    fn clone(&self) -> Self {
        Self {
            linked: self.linked.clone(),
            by_slot: self.by_slot.clone(),
            found: Vec::new(),
            children: self.children.clone(),
        }
    }

    /// Copies `source` into the memory this one holds, keeping its own `found`.
    fn clone_from(&mut self, source: &Self) {
        self.linked.clone_from(&source.linked);
        self.by_slot.clone_from(&source.by_slot);
        self.children.clone_from(&source.children);
    }
}

impl<L: Link> Links<L> {
    /// The links of a resource as a snapshot file held it: `linked`, an entity gone since its last
    /// step [`Entity::DEAD`]'s, and `children`, the bodies it warned of. Fails with an entity linked
    /// twice.
    fn new(linked: Vec<L>, children: Vec<Entity>) -> Result<Self, Entity> {
        let mut by_slot = Vec::new();
        for (index, link) in (0..).zip(&linked) {
            let entity = link.entity();
            if entity == Entity::DEAD {
                continue;
            }
            if by_slot.len() <= entity.slot() {
                by_slot.resize(entity.slot() + 1, NONE);
            }
            if by_slot[entity.slot()] != NONE {
                return Err(entity);
            }
            by_slot[entity.slot()] = index;
        }
        Ok(Self {
            linked,
            by_slot,
            found: Vec::new(),
            children,
        })
    }

    /// Keeps the links that `keep` keeps, in their order.
    fn retain(&mut self, keep: impl FnMut(&L) -> bool) {
        let before = self.linked.len();
        self.linked.retain(keep);
        if self.linked.len() != before {
            self.by_slot.fill(NONE);
            for (index, link) in (0..).zip(&self.linked) {
                self.by_slot[link.entity().slot()] = index;
            }
        }
    }

    /// Gives `link` each entity with a `B` and a [`Transform`], in the order queries find them,
    /// with these links, to make its body or take in what changed of it: but those under a parent,
    /// which it warns of once each, naming `B` as `name`.
    fn find<B: Component>(
        &mut self,
        world: &World,
        name: &str,
        mut link: impl FnMut(&mut Self, Entity),
    ) {
        let mut found = std::mem::take(&mut self.found);
        found.clear();
        let bodies = world.query::<(Entity, &B, &Transform)>();
        found.extend(bodies.map(|(entity, ..)| entity));
        self.children.retain(|&child| {
            let body = world.get::<B>(child).is_some();
            body && matches!(world.parent(child), Ok(Some(_)))
        });
        for &entity in &found {
            if matches!(world.parent(entity), Ok(Some(_))) {
                if !self.children.contains(&entity) {
                    let id = world.id(entity).expect("found alive");
                    warn!(
                        code = "physics.child_body",
                        entity = %id,
                        "Entity {id}'s {name} isn't moved: it's under a parent, whose space its \
                         transform is in"
                    );
                    self.children.push(entity);
                }
                continue;
            }
            link(self, entity);
        }
        self.found = found;
    }

    /// The link to `entity`'s body, if it has one.
    fn get(&self, entity: Entity) -> Option<&L> {
        let index = self.by_slot.get(entity.slot()).copied().unwrap_or(NONE);
        let link = self.linked.get(index as usize);
        link.filter(|link| link.entity() == entity)
    }

    /// The link to `entity`'s body, if it has one, to change.
    fn get_mut(&mut self, entity: Entity) -> Option<&mut L> {
        let index = self.by_slot.get(entity.slot()).copied().unwrap_or(NONE);
        let link = self.linked.get_mut(index as usize);
        link.filter(|link| link.entity() == entity)
    }

    /// Adds `link`, whose entity has none.
    fn push(&mut self, link: L) {
        let index = u32::try_from(self.linked.len()).expect("fewer than 2^32 bodies");
        let slot = link.entity().slot();
        if self.by_slot.len() <= slot {
            self.by_slot.resize(slot + 1, NONE);
        }
        self.by_slot[slot] = index;
        self.linked.push(link);
    }
}

/// The color colliders' outlines draw in, linear: a light green, as physics debuggers draw them,
/// apart from the grid's darker green axis.
const OUTLINE: Vec4 = Vec4::new(0.3, 1.0, 0.3, 1.0);

/// How wide colliders' outlines draw, in points: pixels of a window at a scale of 1.
const OUTLINE_WIDTH: f32 = 2.0;

/// How many lines a circle's outline draws with, and each ring of a sphere's or a capsule's: round
/// to within a pixel up to some 800 pixels across.
const SEGMENTS: usize = 64;

/// A circle a unit in radius around the origin, in the XY plane, from +X counterclockwise and back,
/// as the ends of the lines its outline draws with.
type Ring = [Vec3; SEGMENTS + 1];

fn ring() -> Ring {
    std::array::from_fn(|i| {
        let (sin, cos) = libm::sincosf(TAU * i as f32 / SEGMENTS as f32);
        Vec3::new(cos, sin, 0.0)
    })
}

/// A collider's outline, as what places its shape.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Outline {
    /// The square a unit across around the origin, in its XY plane, a [`BoxCollider2D`]'s.
    Rect(Mat4),
    /// The circle a unit in radius around the origin, in its XY plane, a [`CircleCollider2D`]'s.
    Circle(Mat4),
    /// The cube a unit across around the origin, a [`BoxCollider3D`]'s.
    Cuboid(Mat4),
    /// The sphere a unit in radius around the origin, a [`SphereCollider3D`]'s.
    Sphere(Mat4),
    /// A [`CapsuleCollider3D`]'s: what's within `radius` of the segment along Y from `-half` to
    /// `half`, which `placed` turns and moves.
    Capsule {
        placed: Mat4,
        radius: f32,
        half: f32,
    },
}

/// Draws into `draws` the outline of each 2D and 3D collider in `world` whose entity has a
/// [`Transform`], 2 points wide in a window of `scale` pixels a point, where its entity draws this
/// frame as `poses` has it, with the entity's ID, so that a pick of an outline finds its entity:
/// the 2D colliders' first. Lines draw with the engine's line shader, which `shaders` adds the
/// first time any do.
pub(crate) fn draw_colliders(
    world: &World,
    poses: &Poses<'_>,
    scale: f32,
    draws: &mut Draws,
    shaders: &mut Shaders,
) {
    let _zone = gg_core::zone!();
    let width = OUTLINE_WIDTH * scale;
    let ring = ring();
    let mut each = |outline, id| match outline {
        Outline::Rect(placed) => draws.push_rect(shaders, placed, OUTLINE, width, id),
        outline => lines(outline, &ring, |placed, from, to| {
            let line = Line {
                from,
                to,
                color: OUTLINE,
                width,
            };
            draws.push_line(shaders, placed, line, id);
        }),
    };
    dim2::outlines(world, poses, &mut each);
    dim3::outlines(world, poses, &mut each);
}

/// Gives `each` the lines `outline` draws with, each as the matrix that places it and its ends in
/// that matrix's space: a rect's none, as it draws as one. A sphere draws as its rings about each
/// axis; a capsule as its rings where its caps meet its side, its side's lines along X and Z, and
/// its caps' arcs across them.
fn lines(outline: Outline, ring: &Ring, mut each: impl FnMut(Mat4, Vec3, Vec3)) {
    // A line from each of `points` to the next, each where `at` puts it.
    let mut join = |placed: Mat4, points: &[Vec3], at: &dyn Fn(Vec3) -> Vec3| {
        for pair in points.windows(2) {
            each(placed, at(pair[0]), at(pair[1]));
        }
    };
    let same = |point| point;
    match outline {
        Outline::Rect(_) => {}
        Outline::Circle(placed) => join(placed, ring, &same),
        Outline::Cuboid(placed) => {
            // Each corner of the cube by its bits, X's the lowest; each edge joins two corners
            // whose bits differ in one.
            let corner = |bits: usize| {
                let half = |bit: usize| if bits & bit == 0 { -0.5 } else { 0.5 };
                Vec3::new(half(1), half(2), half(4))
            };
            for bit in [1, 2, 4] {
                for bits in (0..8).filter(|bits| bits & bit == 0) {
                    join(placed, &[corner(bits), corner(bits | bit)], &same);
                }
            }
        }
        Outline::Sphere(placed) => {
            join(placed, ring, &same);
            join(placed, ring, &|p| Vec3::new(0.0, p.x, p.y));
            join(placed, ring, &|p| Vec3::new(p.y, 0.0, p.x));
        }
        Outline::Capsule {
            placed,
            radius,
            half,
        } => {
            for y in [-half, half] {
                join(placed, ring, &|p| Vec3::new(p.x * radius, y, p.y * radius));
            }
            for side in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
                let side = side * radius;
                join(
                    placed,
                    &[side - Vec3::Y * half, side + Vec3::Y * half],
                    &same,
                );
            }
            // The ring's upper half, from +X round to -X, arcs over the top; its lower the bottom.
            let middle = SEGMENTS / 2;
            for (arc, y) in [(&ring[..=middle], half), (&ring[middle..], -half)] {
                join(placed, arc, &|p| {
                    Vec3::new(p.x * radius, y + p.y * radius, 0.0)
                });
                join(placed, arc, &|p| {
                    Vec3::new(0.0, y + p.y * radius, p.x * radius)
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gg_renderer::{Circle, Sprite, Viewport};

    use super::*;
    use crate::Context;
    use crate::render_thread::Extracted;

    /// Lines and squares' outlines a layer draws go over the world's entities, though it draws them
    /// before the world, with no entity's ID, and the colliders' outlines over them while they're
    /// shown, 2 points wide in a window at a scale of 1, with their entities' IDs; and not while
    /// they're hidden. A circle a layer draws goes before the world's entities, as its sprites do,
    /// with no entity's ID either.
    #[test]
    fn lines_and_outlines_draw_over_the_world() {
        let mut cx = Context::seeded(3);
        let sprite = (
            Transform::IDENTITY,
            Sprite::default(),
            BoxCollider2D::default(),
        );
        let entity = cx.world_mut().spawn(sprite);
        let id = cx.world().id(entity).expect("alive").get();
        let line = Line {
            from: Vec3::ZERO,
            to: Vec3::X,
            color: Vec4::ONE,
            width: 3.0,
        };
        let square = Mat4::from_translation(Vec3::Y);
        let viewport = Viewport {
            origin: [0, 0],
            size: [64, 64],
        };
        let mut frame = Extracted::default();
        assert!(!cx.colliders_shown(), "hidden at first");
        for shown in [false, true] {
            cx.set_colliders_shown(shown);
            // The circle first on both sides, since the engine's shaders take their indices in the
            // order they're first asked for.
            cx.draw_circle(Mat4::IDENTITY, Circle::default());
            cx.draw_line(line.from, line.to, line.color, line.width);
            cx.draw_rect(square, line.color, 1.0);
            cx.draw_world(1.0, 0.0, 0);
            cx.extract(&mut frame, viewport);
            let mut shaders = Shaders::default();
            let mut want = Draws::default();
            want.push_circle(&mut shaders, Mat4::IDENTITY, Circle::default());
            want.draw_scene();
            let drawn = (Mat4::IDENTITY, Sprite::default(), id);
            want.extend_sprites(std::iter::once(drawn));
            want.push_line(&mut shaders, Mat4::IDENTITY, line, 0);
            want.push_rect(&mut shaders, square, line.color, 1.0, 0);
            if shown {
                want.push_rect(&mut shaders, Mat4::IDENTITY, OUTLINE, 2.0, id);
            }
            assert_eq!(frame.draws, want, "shown: {shown}");
        }
    }

    /// The lines each shape's outline draws with, in the space its matrix places them in.
    fn drawn(outline: Outline) -> Vec<(Vec3, Vec3)> {
        let mut found = Vec::new();
        lines(outline, &ring(), |placed, from, to| {
            assert_eq!(placed, Mat4::IDENTITY);
            found.push((from, to));
        });
        found
    }

    /// A cube's outline is its 12 edges, each a unit long between two of its corners; a sphere's,
    /// its three rings about its axes, each point of them a unit from its middle; and a capsule's,
    /// its two rings, its side's four lines and its caps' four arcs, each point of them as far from
    /// its segment as its radius, and every side of it reached; a rect's draws as one.
    #[test]
    fn outlines_draw_as_their_shapes_lines() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        let cube = drawn(Outline::Cuboid(Mat4::IDENTITY));
        assert_eq!(cube.len(), 12);
        for (from, to) in &cube {
            assert!(near(from.distance(*to), 1.0), "{from} {to}");
            for end in [from, to] {
                assert_eq!(end.abs(), Vec3::splat(0.5), "a corner: {end}");
            }
        }
        let sphere = drawn(Outline::Sphere(Mat4::IDENTITY));
        assert_eq!(sphere.len(), 3 * SEGMENTS);
        for end in sphere.iter().flat_map(|(from, to)| [from, to]) {
            assert!(near(end.length(), 1.0), "{end}");
        }
        let (radius, half) = (0.5, 0.75);
        let capsule = Outline::Capsule {
            placed: Mat4::IDENTITY,
            radius,
            half,
        };
        let capsule = drawn(capsule);
        assert_eq!(capsule.len(), 2 * SEGMENTS + 4 + 4 * SEGMENTS / 2);
        let mut reach = [f32::MAX, f32::MIN];
        for end in capsule.iter().flat_map(|(from, to)| [from, to]) {
            let on_segment = Vec3::new(0.0, end.y.clamp(-half, half), 0.0);
            assert!(near(end.distance(on_segment), radius), "{end}");
            reach = [reach[0].min(end.y), reach[1].max(end.y)];
        }
        assert!(near(reach[0], -1.25) && near(reach[1], 1.25), "{reach:?}");
        assert!(drawn(Outline::Rect(Mat4::IDENTITY)).is_empty());
    }

    /// A world is given each kind of physics' resource the first time it steps with a body of that
    /// kind, and not before.
    #[test]
    fn worlds_get_physics_of_the_kinds_of_bodies_they_have() {
        let mut world = World::default();
        step_physics(&mut world);
        assert!(world.resource::<Physics2D>().is_none());
        assert!(world.resource::<Physics3D>().is_none());
        world.spawn((Transform::IDENTITY, RigidBody3D::default()));
        step_physics(&mut world);
        assert!(world.resource::<Physics2D>().is_none());
        assert!(world.resource::<Physics3D>().is_some());
        world.spawn((Transform::IDENTITY, RigidBody2D::default()));
        step_physics(&mut world);
        assert!(world.resource::<Physics2D>().is_some());
    }
}
