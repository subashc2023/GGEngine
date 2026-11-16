//! What a frame draws besides UI, in runs that each render as one instanced draw.

use std::ops::Range;

use bytemuck::NoUninit;
use gg_core::math::{Mat3, Mat4, Vec2, Vec4};
use gg_gpu::Faces;

use crate::{
    Circle, Glyph, Grid, Line, Mesh, MeshDraw, Meshes, Particles, Shader, Shaders, Sprite, halves,
    sprite,
};

/// What a frame draws besides UI: meshes, each with a shader for its vertex type, that shader's
/// parameters, a transform and an ID.
///
/// - Opaque draws come first, in any order, each hiding what's behind it. Those that blend follow
///   in order, over those before and over what's opaque where it isn't nearer.
/// - Each run of consecutive draws of one mesh and shader, with the same [`Faces`] and
///   [`clip`](Self::clip), renders as one instanced draw, whatever their transforms, IDs and
///   parameters, textures among them. A mesh with levels of detail draws once per level its
///   instances draw at.
/// - Draws draw both faces, but a material's that isn't double-sided.
/// - An ID is a 64-bit number the view writes where the draw shows, which [picks](crate::Pick) read
///   back: an entity's for a draw of the world's, 0 for none, as [`push`](Self::push),
///   [`push_sprite`](Self::push_sprite) and [`push_circle`](Self::push_circle) give.
/// - The renderer's [`Scene`](crate::Scene) draws where [`draw_scene`](Self::draw_scene) says, or
///   else first. Skinned meshes draw where the frame's skins move them ([`skin`](Self::skin)).
///   Emitters' particles draw among those that blend, where
///   [`push_particles`](Self::push_particles) puts them.
///
/// Clearing keeps its memory, so a frame that draws no more than the last allocates nothing.
#[derive(Debug, Default, PartialEq)]
pub struct Draws {
    pub(crate) runs: Vec<Run>,
    /// The run the scene draws before, if any.
    pub(crate) scene_at: Option<usize>,
    pub(crate) transforms: Vec<Mat4>,
    /// Every draw's ID as its low and high halves, which shaders write.
    pub(crate) ids: Vec<[u32; 2]>,
    pub(crate) params: Vec<u8>,
    /// The skinned meshes the frame moves, each with its joints' range in `joints`.
    pub(crate) skins: Vec<(u32, Range<usize>)>,
    pub(crate) joints: Vec<Mat4>,
    /// Every emitter's particles, each drawn in its run's place.
    pub(crate) particles: Vec<Particles>,
    /// The clip of the draws pushed from here on ([`clip`](Self::clip)), and whether the next
    /// starts a run of its own.
    clip: Option<[Vec2; 2]>,
    split: bool,
}

/// Consecutive draws of one mesh and shader with the same faces and clip: one instanced draw's
/// instances.
#[derive(Debug, PartialEq)]
pub(crate) struct Run {
    pub(crate) mesh: u32,
    pub(crate) shader: u32,
    pub(crate) faces: Faces,
    /// Where its draws show ([`Draws::clip`]), if they're clipped.
    pub(crate) clip: Option<[Vec2; 2]>,
    /// Its draws' range in `Draws::transforms` and `Draws::ids`.
    pub(crate) instances: Range<usize>,
    /// Their range in `Draws::params`.
    pub(crate) params: Range<usize>,
    /// For an emitter's particles, drawn in place of the run's draws, of which it has none, of the
    /// engine's quad with the sprite shader: their index in `Draws::particles`.
    pub(crate) particles: Option<usize>,
}

impl Draws {
    /// Draws `mesh` with `shader`, which reads its vertex type and takes `params`: among the
    /// opaque, first, or in its turn if it blends. `transform` takes the mesh's space to the
    /// world's. Its ID is 0.
    pub fn push<V, P: NoUninit>(
        &mut self,
        mesh: Mesh<V>,
        shader: Shader<V, P>,
        transform: Mat4,
        params: P,
    ) {
        self.push_with_id(mesh, shader, transform, params, 0);
    }

    /// [`push`](Self::push), with the ID `id`.
    fn push_with_id<V, P: NoUninit>(
        &mut self,
        mesh: Mesh<V>,
        shader: Shader<V, P>,
        transform: Mat4,
        params: P,
        id: u64,
    ) {
        let (instance, start) = (self.transforms.len(), self.params.len());
        self.transforms.push(transform);
        self.ids.push(halves(id));
        self.params.extend_from_slice(bytemuck::bytes_of(&params));
        self.extend_run(mesh.index, shader.index, Faces::Both, instance, start);
    }

    /// Draws `draw`'s mesh with its shader and parameters, as [`push`](Self::push) does, with the
    /// ID `id`. A material that isn't double-sided draws its mesh's fronts: triangles winding
    /// counterclockwise, as glTF's do, or clockwise where `transform` mirrors them.
    pub fn push_draw(&mut self, transform: Mat4, draw: &MeshDraw, id: u64) {
        let (instance, start) = (self.transforms.len(), self.params.len());
        self.transforms.push(transform);
        self.ids.push(halves(id));
        self.params.extend_from_slice(draw.params());
        let faces = if !draw.culls {
            Faces::Both
        } else if Mat3::from_mat4(transform).determinant() < 0.0 {
            Faces::Clockwise
        } else {
            Faces::CounterClockwise
        };
        self.extend_run(draw.mesh, draw.shader, faces, instance, start);
    }

    /// Draws `sprite` among those that blend ([`Sprite`]). `transform` takes its space, a unit
    /// square round the origin, to the world's. Its ID is 0.
    pub fn push_sprite(&mut self, transform: Mat4, sprite: Sprite) {
        self.push(sprite::QUAD, sprite::SHADER, transform, sprite.params());
    }

    /// Draws each of `sprites`, a transform, a sprite and an ID, as
    /// [`push_sprite`](Self::push_sprite) does, making room and joining a run once for all.
    pub fn extend_sprites(&mut self, sprites: impl ExactSizeIterator<Item = (Mat4, Sprite, u64)>) {
        let sprites = sprites.map(|(transform, sprite, id)| (transform, sprite.params(), id));
        self.extend_quads(sprite::SHADER, sprites);
    }

    /// Draws `circle` among those that blend ([`Circle`]), with the engine's circle shader, which
    /// `shaders` adds when first asked. `transform` takes its space, where it fills a unit square
    /// round the origin, to the world's. Its ID is 0.
    pub fn push_circle(&mut self, shaders: &mut Shaders, transform: Mat4, circle: Circle) {
        self.push(sprite::QUAD, shaders.circle(), transform, circle.params());
    }

    /// Draws each of `circles` as [`push_circle`](Self::push_circle) does, with its ID, making room
    /// and joining a run once for all.
    pub fn extend_circles(
        &mut self,
        shaders: &mut Shaders,
        circles: impl ExactSizeIterator<Item = (Mat4, Circle, u64)>,
    ) {
        // No circles, no shader: asking for it would have a renderer make its pipeline.
        if circles.len() == 0 {
            return;
        }
        let circles = circles.map(|(transform, circle, id)| (transform, circle.params(), id));
        self.extend_quads(shaders.circle(), circles);
    }

    /// Draws each of `glyphs`, a transform, a glyph and an ID, among those that blend ([`Glyph`]),
    /// with the engine's text shader, which `shaders` adds when first asked, making room and
    /// joining a run once for all. Each transform takes the glyph's unit square round the origin to
    /// the world.
    pub fn extend_glyphs(
        &mut self,
        shaders: &mut Shaders,
        glyphs: impl ExactSizeIterator<Item = (Mat4, Glyph, u64)>,
    ) {
        // No glyphs, no shader: asking for it would have a renderer make its pipeline.
        if glyphs.len() == 0 {
            return;
        }
        let glyphs = glyphs.map(|(transform, glyph, id)| (transform, glyph.params(), id));
        self.extend_quads(shaders.text(), glyphs);
    }

    /// Draws `line` among those that blend ([`Line`]), with the engine's line shader, which
    /// `shaders` adds when first asked, and the ID `id`: an entity's, or 0. `transform` takes its
    /// points' space to the world's; its width stays in the view's pixels.
    pub fn push_line(&mut self, shaders: &mut Shaders, transform: Mat4, line: Line, id: u64) {
        self.push_with_id(sprite::QUAD, shaders.line(), transform, line.params(), id);
    }

    /// Draws `grid` among those that blend ([`Grid`]), with the engine's grid mesh and shader,
    /// which `meshes` and `shaders` add when first asked. `transform` takes the grid's unit square
    /// round the origin in its XY plane to the world's: its first two columns are the square's
    /// sides, its translation the square's center. It has no ID and leaves what's under it, so
    /// picks see through it.
    pub fn push_grid(
        &mut self,
        meshes: &mut Meshes,
        shaders: &mut Shaders,
        transform: Mat4,
        grid: Grid,
    ) {
        self.push(meshes.grid(), shaders.grid(), transform, grid.params());
    }

    /// Draws the outline of a sprite's unit square that `transform` places: four lines meeting at
    /// its corners, in `color`, `width` pixels wide, drawn as [`push_line`](Self::push_line) does
    /// with the ID `id`.
    pub fn push_rect(
        &mut self,
        shaders: &mut Shaders,
        transform: Mat4,
        color: Vec4,
        width: f32,
        id: u64,
    ) {
        let corners = sprite::CORNERS.map(|corner| corner.position);
        for (i, &from) in corners.iter().enumerate() {
            let to = corners[(i + 1) % corners.len()];
            let line = Line {
                from,
                to,
                color,
                width,
            };
            self.push_line(shaders, transform, line, id);
        }
    }

    /// Draws each of `quads`, a transform, parameters and an ID, on the engine's quad with
    /// `shader`, making room and joining a run once for all.
    fn extend_quads<P: NoUninit>(
        &mut self,
        shader: Shader<sprite::Corner, P>,
        quads: impl ExactSizeIterator<Item = (Mat4, P, u64)>,
    ) {
        let (instance, start) = (self.transforms.len(), self.params.len());
        self.params.reserve(quads.len() * size_of::<P>());
        self.ids.reserve(quads.len());
        let (ids, params) = (&mut self.ids, &mut self.params);
        let transforms = quads.map(|(transform, quad, id)| {
            ids.push(halves(id));
            params.extend_from_slice(bytemuck::bytes_of(&quad));
            transform
        });
        self.transforms.extend(transforms);
        if self.transforms.len() > instance {
            let quad = sprite::QUAD.index;
            self.extend_run(quad, shader.index, Faces::Both, instance, start);
        }
    }

    /// Draws an emitter's `particles` among those that blend ([`Particles`]): each facing the view,
    /// alive at the frame's time, in its ring's order from the oldest, testing depth without
    /// writing it, and writing the emitter's ID. A frame that doesn't draw an emitter, or doesn't
    /// render the view, lets go of its particles. A second draw of one emitter in a frame draws
    /// nothing. The canvas draws no particles.
    pub fn push_particles(&mut self, particles: Particles) {
        self.particles.push(particles);
        let (instance, start) = (self.transforms.len(), self.params.len());
        self.runs.push(Run {
            mesh: sprite::QUAD.index,
            shader: sprite::SHADER.index,
            faces: Faces::Both,
            clip: self.clip,
            instances: instance..instance,
            params: start..start,
            particles: Some(self.particles.len() - 1),
        });
        self.split = false;
    }

    /// The emitters' particles the frame draws, in order, for tests.
    #[doc(hidden)]
    pub fn particles(&self) -> &[Particles] {
        &self.particles
    }

    /// Has the frame draw skinned `mesh`, packed by
    /// [`pack_skinned`](crate::ModelMesh::pack_skinned), wherever it draws, with its vertices where
    /// `joints` carry them. In skin order, each joint's matrix takes the mesh's space as the joint
    /// was bound to its space as the joint is drawn now; each vertex blends them by its weights, as
    /// glTF's skins do. A frame that doesn't, or gives a mesh not skinned or not yet drawable,
    /// draws it as bound. A joint past those given counts as the last.
    pub fn skin<V>(&mut self, mesh: Mesh<V>, joints: impl IntoIterator<Item = Mat4>) {
        let start = self.joints.len();
        self.joints.extend(joints);
        if self.joints.len() > start {
            self.skins.push((mesh.index, start..self.joints.len()));
        }
    }

    /// The skinned meshes the frame moves, by handle, with their joints' matrices, for tests.
    #[doc(hidden)]
    pub fn skins(&self) -> impl Iterator<Item = (u32, &[Mat4])> {
        (self.skins.iter()).map(|(mesh, joints)| (*mesh, &self.joints[joints.clone()]))
    }

    /// Moves `other`'s draws after these, in order, as though drawn here, leaving it empty with its
    /// memory.
    pub fn append(&mut self, other: &mut Draws) {
        let joint = self.joints.len();
        self.joints.append(&mut other.joints);
        (self.skins).extend(
            (other.skins.drain(..))
                .map(|(mesh, joints)| (mesh, joint + joints.start..joint + joints.end)),
        );
        let emitter = self.particles.len();
        self.particles.append(&mut other.particles);
        let (instance, start) = (self.transforms.len(), self.params.len());
        self.transforms.append(&mut other.transforms);
        self.ids.append(&mut other.ids);
        self.params.append(&mut other.params);
        let (scene_at, count) = (other.scene_at.take(), other.runs.len());
        for (i, run) in other.runs.drain(..).enumerate() {
            if scene_at == Some(i) {
                self.draw_scene();
            }
            let instances = instance + run.instances.start..instance + run.instances.end;
            let params = start + run.params.start..start + run.params.end;
            let particles = run.particles.map(|index| emitter + index);
            let scene_last = self.scene_at == Some(self.runs.len());
            match self.runs.last_mut() {
                // Its first run goes on from these' last if it's of the same mesh, shader, faces
                // and clip, neither is particles, the scene doesn't draw between them, and no clip
                // was set since.
                Some(last)
                    if (last.mesh, last.shader, last.faces, last.clip)
                        == (run.mesh, run.shader, run.faces, run.clip)
                        && (last.particles, particles) == (None, None)
                        && !scene_last
                        && !self.split =>
                {
                    last.instances.end = instances.end;
                    last.params.end = params.end;
                }
                _ => self.runs.push(Run {
                    instances,
                    params,
                    particles,
                    ..run
                }),
            }
            self.split = false;
        }
        if scene_at == Some(count) {
            self.draw_scene();
        }
        (other.clip, other.split) = (None, false);
    }

    /// Adds the draws from `instance` on, their parameters from `start`, of `mesh` with `shader`
    /// drawing `faces`: to the last run if it's of them and of the clip set before it, or else as a
    /// run of their own.
    fn extend_run(&mut self, mesh: u32, shader: u32, faces: Faces, instance: usize, start: usize) {
        let scene_last = self.scene_at == Some(self.runs.len());
        match self.runs.last_mut() {
            Some(run)
                if (run.mesh, run.shader, run.faces, run.clip)
                    == (mesh, shader, faces, self.clip)
                    && run.particles.is_none()
                    && !scene_last
                    && !self.split =>
            {
                run.instances.end = self.transforms.len();
                run.params.end = self.params.len();
            }
            _ => self.runs.push(Run {
                mesh,
                shader,
                faces,
                clip: self.clip,
                instances: instance..self.transforms.len(),
                params: start..self.params.len(),
                particles: None,
            }),
        }
        self.split = false;
    }

    /// Has the draws pushed from here on show only within `clip` until the next call: a rectangle
    /// in the XY plane their transforms take them to, least corner then greatest. What falls
    /// outside the box round where the view shows it isn't drawn, as a scrolling list hides what's
    /// scrolled out. `None` clips nothing.
    ///
    /// Each call starts a run, so [`place`](Self::place) can place the clip with its draws. It's a
    /// scissor on the run, not a rectangle in each instance that every canvas shader would test, so
    /// a UI's list or text field is one scissored instanced draw per mesh and shader. The view's
    /// culling ignores clips.
    pub fn clip(&mut self, clip: Option<[Vec2; 2]>) {
        (self.clip, self.split) = (clip, true);
    }

    /// Has the renderer's [`Scene`](crate::Scene) draw here: over the draws so far and under those
    /// after, but where what's opaque is nearer. The last place called is the scene's.
    pub fn draw_scene(&mut self) {
        self.scene_at = Some(self.runs.len());
    }

    /// How many draws there are: one per mesh drawn.
    pub fn len(&self) -> usize {
        self.transforms.len()
    }

    /// Places draws `draws`, counted from the first, in the space `transform` takes theirs to: each
    /// transform becomes `transform` times its own, and the clip of each run starting among them
    /// the box round its corners, transformed. Runs don't change otherwise, so a mirroring
    /// `transform` leaves a culling draw culling the faces it did.
    ///
    /// # Panics
    ///
    /// - If `draws` reaches past them.
    pub fn place(&mut self, draws: Range<usize>, transform: Mat4) {
        for run in &mut self.runs {
            if let Some([least, most]) = &mut run.clip
                && draws.contains(&run.instances.start)
            {
                let [a, b] = [*least, *most].map(|at| transform.transform_point3(at.extend(0.0)));
                (*least, *most) = (a.min(b).truncate(), a.max(b).truncate());
            }
        }
        for at in &mut self.transforms[draws] {
            *at = transform * *at;
        }
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Forgets every draw.
    pub fn clear(&mut self) {
        self.runs.clear();
        self.scene_at = None;
        self.transforms.clear();
        self.ids.clear();
        self.params.clear();
        self.skins.clear();
        self.joints.clear();
        self.particles.clear();
        (self.clip, self.split) = (None, false);
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use gg_core::math::{Vec3, Vec4};

    use super::*;
    use crate::{GridLines, Material, Materials, ModelVertex, Texture, circle, grid, line};

    /// Each run's mesh, shader and draws.
    fn runs(draws: &Draws) -> Vec<(u32, u32, Range<usize>)> {
        let run = |run: &Run| (run.mesh, run.shader, run.instances.clone());
        draws.runs.iter().map(run).collect()
    }

    #[test]
    fn consecutive_draws_of_one_mesh_with_one_shader_are_one_run() {
        let square = Mesh::<[f32; 3]>::new(1);
        let triangle = Mesh::<[f32; 3]>::new(2);
        let flat = Shader::<[f32; 3], Vec4>::new(1, true);
        let other = Shader::<[f32; 3], Vec4>::new(2, true);
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let (red, green, blue) = (Vec4::X, Vec4::Y, Vec4::Z);
        let mut draws = Draws::default();
        // Sprites of three textures, which share a run.
        for (x, index) in [0, 5, 6].into_iter().enumerate() {
            let texture = Texture { index };
            let sprite = Sprite {
                texture: Some(texture),
                ..Sprite::default()
            };
            draws.push_sprite(at(x as f32), sprite);
        }
        draws.push(square, flat, at(3.0), red);
        draws.push(square, flat, at(4.0), green);
        // Another mesh, then another shader.
        draws.push(triangle, flat, at(5.0), blue);
        draws.push(triangle, other, at(6.0), red);
        // Back to sprites, after the rest, then more of them at once, which join their run.
        draws.push_sprite(at(7.0), Sprite::default());
        let more = [(8.0, 8), (9.0, 9 | 1 << 32)].map(|(x, id)| (at(x), Sprite::default(), id));
        draws.extend_sprites(more.into_iter());
        // A draw without its types joins a run of its mesh and shader, as one with them does.
        draws.push(triangle, other, at(10.0), green);
        draws.push_draw(at(11.0), &MeshDraw::new(triangle, other, blue), u64::MAX);
        draws.push(triangle, other, at(12.0), red);
        assert_eq!(
            runs(&draws),
            [
                (0, 0, 0..3),
                (1, 1, 3..5),
                (2, 1, 5..6),
                (2, 2, 6..7),
                (0, 0, 7..10),
                (2, 2, 10..13)
            ]
        );
        // A run's transforms and parameters are its own draws'.
        let squares = &draws.runs[1];
        assert_eq!(
            draws.transforms[squares.instances.clone()],
            [at(3.0), at(4.0)]
        );
        let colors = [red, green];
        let colors: &[u8] = bytemuck::cast_slice(&colors);
        assert_eq!(&draws.params[squares.params.clone()], colors);
        let sprites = draws.runs[0].params.clone();
        assert_eq!(sprites, 0..3 * size_of::<sprite::Params>());
        let triangles = &draws.runs[5];
        let colors = [green, blue, red];
        let colors: &[u8] = bytemuck::cast_slice(&colors);
        assert_eq!(&draws.params[triangles.params.clone()], colors);
        let sprites = draws.runs[4].params.clone();
        assert_eq!(sprites.len(), 3 * size_of::<sprite::Params>());
        // Each draw's ID, as its low and high halves: 0 for those of no entity.
        let mut ids = [[0; 2]; 13];
        ids[8] = [8, 0];
        ids[9] = [9, 1];
        ids[11] = [u32::MAX; 2];
        assert_eq!(draws.ids, ids);

        draws.clear();
        assert!(draws.is_empty());
        // No sprites make no run.
        draws.extend_sprites(iter::empty());
        assert!(draws.is_empty());
        draws.push(square, flat, at(0.0), red);
        assert_eq!(runs(&draws), [(1, 1, 0..1)]);
        assert_eq!(draws.params.len(), size_of::<Vec4>());
        assert_eq!(draws.ids, [[0; 2]]);
    }

    /// Circles and lines are runs of the engine's quad with their own shaders, which `Shaders` adds
    /// when first asked, and no circles ask for none. Appended draws go on in order with their IDs,
    /// the first joining the last run of the same mesh, shader and faces; those appended from keep
    /// their memory alone.
    #[test]
    fn appended_draws_go_on_after_the_rest() {
        let mut shaders = Shaders::default();
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let line = Line {
            from: Vec3::ZERO,
            to: Vec3::X,
            color: Vec4::ONE,
            width: 2.0,
        };
        let mut draws = Draws::default();
        draws.extend_circles(&mut shaders, iter::empty());
        assert_eq!(shaders.changes.len(), 1, "the sprite shader alone");
        draws.push_sprite(at(0.0), Sprite::default());
        draws.push_circle(&mut shaders, at(1.0), Circle::default());
        let mut over = Draws::default();
        over.push_circle(&mut shaders, at(2.0), Circle::default());
        over.push_line(&mut shaders, at(3.0), line, 7);
        over.push_line(&mut shaders, at(4.0), line, 8 | 1 << 32);
        let held = [over.transforms.capacity(), over.ids.capacity()];
        draws.append(&mut over);
        // The circle shader after the sprite's, then the line's.
        assert_eq!(runs(&draws), [(0, 0, 0..1), (0, 1, 1..3), (0, 2, 3..5)]);
        assert_eq!(draws.transforms, [0.0, 1.0, 2.0, 3.0, 4.0].map(at));
        assert_eq!(draws.ids, [[0, 0], [0, 0], [0, 0], [7, 0], [8, 1]]);
        let sizes = [
            size_of::<sprite::Params>(),
            size_of::<circle::Params>(),
            size_of::<line::Params>(),
        ];
        let ends = [sizes[0], sizes[0] + 2 * sizes[1], draws.params.len()];
        let params: Vec<_> = draws.runs.iter().map(|run| run.params.end).collect();
        assert_eq!(params, ends);
        assert_eq!(ends[2] - ends[1], 2 * sizes[2]);
        assert!(over.is_empty());
        assert_eq!([over.transforms.capacity(), over.ids.capacity()], held);
    }

    /// Placing draws moves those alone, across their runs, which stay as they were.
    #[test]
    fn placed_draws_go_where_a_transform_takes_them() {
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let mut draws = Draws::default();
        let sprites = [0.0, 1.0, 2.0].map(|x| (at(x), Sprite::default(), 0));
        draws.extend_sprites(sprites.into_iter());
        draws.push_circle(&mut Shaders::default(), at(3.0), Circle::default());
        assert_eq!(draws.len(), 4);
        let before = runs(&draws);
        let scaled = Mat4::from_scale(Vec3::splat(2.0));
        draws.place(1..4, scaled);
        assert_eq!(runs(&draws), before);
        let placed = [
            at(0.0),
            scaled * at(1.0),
            scaled * at(2.0),
            scaled * at(3.0),
        ];
        assert_eq!(draws.transforms, placed);
    }

    /// A clip starts a run for the draws after it, even of the last run's mesh and shader and the
    /// same clip. Appended draws keep their runs' clips. Placing places the clips of the runs
    /// starting among the placed draws, as their corners' box, and no other's. Clearing forgets the
    /// clip.
    #[test]
    fn a_clip_starts_a_run_and_is_placed_with_its_draws() {
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let clip = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 5.0)];
        let mut draws = Draws::default();
        draws.push_sprite(at(0.0), Sprite::default());
        draws.clip(Some(clip));
        draws.push_sprite(at(1.0), Sprite::default());
        draws.push_sprite(at(2.0), Sprite::default());
        draws.clip(Some(clip));
        draws.push_sprite(at(3.0), Sprite::default());
        draws.clip(None);
        draws.push_sprite(at(4.0), Sprite::default());
        assert_eq!(
            runs(&draws),
            [(0, 0, 0..1), (0, 0, 1..3), (0, 0, 3..4), (0, 0, 4..5)]
        );
        let clips: Vec<_> = draws.runs.iter().map(|run| run.clip).collect();
        assert_eq!(clips, [None, Some(clip), Some(clip), None]);
        let mut over = Draws::default();
        over.clip(Some(clip));
        over.push_sprite(at(5.0), Sprite::default());
        draws.append(&mut over);
        assert_eq!(draws.runs[4].clip, Some(clip), "appended with its clip");
        // Mirrored and doubled, from the second draw to the third: the second run's clip alone.
        let mirrored = Mat4::from_scale(Vec3::new(-2.0, 2.0, 1.0));
        draws.place(1..3, mirrored);
        let placed = [Vec2::new(-20.0, 0.0), Vec2::new(0.0, 10.0)];
        let clips: Vec<_> = draws.runs.iter().map(|run| run.clip).collect();
        assert_eq!(clips, [None, Some(placed), Some(clip), None, Some(clip)]);
        draws.clear();
        draws.push_sprite(at(0.0), Sprite::default());
        assert_eq!(draws.runs[0].clip, None);
    }

    /// Grids are runs of the engine's own mesh and shader, which `Meshes` and `Shaders` add when
    /// first asked, and reuse after.
    #[test]
    fn grids_draw_with_a_mesh_and_a_shader_added_once() {
        let mut meshes = Meshes::default();
        let mut shaders = Shaders::default();
        let at = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let grid = Grid {
            first: Vec3::X,
            first_center: 0.0,
            second: Vec3::Y,
            second_center: 0.0,
            eye: Vec3::NEG_Z,
            coarsest: 3.0,
            normal: Vec3::Z,
            lines: GridLines::Crisp,
        };
        let mut draws = Draws::default();
        draws.push_grid(&mut meshes, &mut shaders, at(0.0), grid);
        draws.push_sprite(at(1.0), Sprite::default());
        draws.push_grid(&mut meshes, &mut shaders, at(2.0), grid);
        draws.push_grid(&mut meshes, &mut shaders, at(3.0), grid);
        // After the engine's quad and sprite shader, which come first.
        assert_eq!((meshes.count(), shaders.count()), (2, 2));
        assert_eq!(runs(&draws), [(1, 1, 0..1), (0, 0, 1..2), (1, 1, 2..4)]);
        let grids = draws.runs[2].params.clone();
        assert_eq!(grids.len(), 2 * size_of::<grid::Params>());
    }

    #[test]
    fn draws_that_cull_run_apart_where_their_transforms_mirror_them() {
        let mut meshes = Meshes::default();
        let mut shaders = Shaders::default();
        let mut materials = Materials::default();
        let corner = ModelVertex {
            position: Vec3::ZERO,
            uv: gg_core::math::Vec2::ZERO,
            color: Vec4::ONE,
            normal: Vec3::Z,
            tangent: Vec4::X,
        };
        let packed = crate::ModelMesh::pack("a", &[corner; 3], vec![0, 1, 2], Vec::new()).unwrap();
        let mesh = meshes.add_model("a", &packed).unwrap();
        let mut draw = |double_sided| {
            let material = Material {
                double_sided,
                ..Material::default()
            };
            let material = materials.add(&material);
            MeshDraw::with_material(mesh, material, &materials, &mut shaders)
        };
        let (single, double) = (draw(false), draw(true));
        let at = Mat4::from_translation(Vec3::X);
        let mirrored = Mat4::from_scale(Vec3::new(-1.0, 2.0, 1.0));
        let faces = |draws: &Draws| draws.runs.iter().map(|run| run.faces).collect::<Vec<_>>();
        let mut draws = Draws::default();
        for transform in [at, mirrored, at] {
            draws.push_draw(transform, &single, 0);
        }
        let expected = [
            Faces::CounterClockwise,
            Faces::Clockwise,
            Faces::CounterClockwise,
        ];
        assert_eq!(faces(&draws), expected);
        // Mirroring twice turns it back.
        draws.clear();
        draws.push_draw(mirrored * mirrored, &single, 0);
        assert_eq!(faces(&draws), [Faces::CounterClockwise]);
        // Draws that draw both faces, and sprites, never split for it.
        draws.clear();
        for transform in [at, mirrored, at] {
            draws.push_draw(transform, &double, 0);
        }
        for transform in [at, mirrored, at] {
            draws.push_sprite(transform, Sprite::default());
        }
        assert_eq!(faces(&draws), [Faces::Both, Faces::Both]);
        assert_eq!(runs(&draws), [(1, 1, 0..3), (0, 0, 3..6)]);
    }
}
