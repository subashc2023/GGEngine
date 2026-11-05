//! Draws: what a frame draws besides UI, in order, each a mesh with a shader, the parameters it
//! takes, a transform and an ID, kept in runs that each render as one instanced draw.

use std::ops::Range;

use bytemuck::NoUninit;
use gg_core::math::{Mat3, Mat4, Vec4};
use gg_gpu::Faces;

use crate::{
    Circle, Glyph, Grid, Line, Mesh, MeshDraw, Meshes, Shader, Shaders, Sprite, halves, sprite,
};

/// What a frame draws besides UI, in order, each over those before it, but where an opaque one
/// before it is nearer: meshes, each with a shader for its vertex type, the parameters that shader
/// takes, and where it is in the world. Each run of consecutive draws of one mesh with one shader
/// that draw the same [`Faces`] renders as one instanced draw, whatever their transforms, IDs and
/// parameters, a texture among them, or, of a mesh with levels of detail, as one for each level its
/// instances draw at: every draw draws both faces of its mesh's triangles but a
/// material's that isn't double-sided. Each draw has an ID, a 64-bit number that the view writes
/// where it shows, which [picks](crate::Pick) read back: what the engine gives it, an entity's ID
/// for a draw of the world's; 0 for a draw of no entity, as [`push`](Self::push),
/// [`push_sprite`](Self::push_sprite) and [`push_circle`](Self::push_circle) give. Clearing it
/// keeps its memory, so that a frame that draws no more than the last allocates nothing.
///
/// The renderer's [`Scene`](crate::Scene) draws among them where [`draw_scene`](Self::draw_scene)
/// says, or else before them all.
#[derive(Debug, Default, PartialEq)]
pub struct Draws {
    /// The runs, in order.
    pub(crate) runs: Vec<Run>,
    /// The run the scene draws before, if [`draw_scene`](Self::draw_scene) says one.
    pub(crate) scene_at: Option<usize>,
    /// Every draw's transform, in order.
    pub(crate) transforms: Vec<Mat4>,
    /// Every draw's ID, in order, as its low and high halves, which shaders write.
    pub(crate) ids: Vec<[u32; 2]>,
    /// Every draw's parameters, one after another.
    pub(crate) params: Vec<u8>,
}

/// Consecutive draws of one mesh with one shader that draw the same faces: the instances of one
/// instanced draw.
#[derive(Debug, PartialEq)]
pub(crate) struct Run {
    pub(crate) mesh: u32,
    pub(crate) shader: u32,
    pub(crate) faces: Faces,
    /// Where its draws' transforms and IDs are in `Draws::transforms` and `Draws::ids`.
    pub(crate) instances: Range<usize>,
    /// Where their parameters are in `Draws::params`.
    pub(crate) params: Range<usize>,
}

impl Draws {
    /// Draws `mesh` with `shader`, which reads its vertex type and takes `params`, over the
    /// meshes drawn before it but where an opaque one is nearer. `transform` takes the mesh's own
    /// space, which its vertices' positions are in, to the world's. Its ID is 0, of no entity.
    pub fn push<V, P: NoUninit>(
        &mut self,
        mesh: Mesh<V>,
        shader: Shader<V, P>,
        transform: Mat4,
        params: P,
    ) {
        self.push_with_id(mesh, shader, transform, params, 0);
    }

    /// Draws as [`push`](Self::push) does, with the ID `id`.
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

    /// Draws `draw` over the meshes drawn before it but where an opaque one is nearer, as
    /// [`push`](Self::push) draws its mesh with its shader and parameters, with the ID `id`. A
    /// material's draw that isn't double-sided draws its mesh's fronts: the triangles whose corners
    /// wind counterclockwise, as glTF's fronts do, or clockwise where `transform` mirrors them, as
    /// glTF has it.
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

    /// Draws `sprite` over the meshes drawn before it but where an opaque one is nearer, as
    /// [`Sprite`] describes. `transform` takes the sprite's own space, where it's a unit square
    /// around the origin, to the world's. Its ID is 0, of no entity.
    pub fn push_sprite(&mut self, transform: Mat4, sprite: Sprite) {
        self.push(sprite::QUAD, sprite::SHADER, transform, sprite.params());
    }

    /// Draws each of `sprites`, a transform, a sprite and its ID, in turn, as
    /// [`push_sprite`](Self::push_sprite) does, but with its ID, making room for them and joining a
    /// run once for them all.
    pub fn extend_sprites(&mut self, sprites: impl ExactSizeIterator<Item = (Mat4, Sprite, u64)>) {
        let sprites = sprites.map(|(transform, sprite, id)| (transform, sprite.params(), id));
        self.extend_quads(sprite::SHADER, sprites);
    }

    /// Draws `circle` over the meshes drawn before it but where an opaque one is nearer, as
    /// [`Circle`] describes, with the engine's circle shader, which `shaders` adds the first time
    /// it's asked for. `transform` takes the circle's own space, where it's as large as fits a unit
    /// square around the origin, to the world's. Its ID is 0, of no entity.
    pub fn push_circle(&mut self, shaders: &mut Shaders, transform: Mat4, circle: Circle) {
        self.push(sprite::QUAD, shaders.circle(), transform, circle.params());
    }

    /// Draws each of `circles`, a transform, a circle and its ID, in turn, as
    /// [`push_circle`](Self::push_circle) does, but with its ID, making room for them and joining a
    /// run once for them all.
    pub fn extend_circles(
        &mut self,
        shaders: &mut Shaders,
        circles: impl ExactSizeIterator<Item = (Mat4, Circle, u64)>,
    ) {
        // No circles ask for no shader, which would have a renderer make its pipeline.
        if circles.len() == 0 {
            return;
        }
        let circles = circles.map(|(transform, circle, id)| (transform, circle.params(), id));
        self.extend_quads(shaders.circle(), circles);
    }

    /// Draws each of `glyphs`, a transform, a glyph and its ID, in turn, over the meshes drawn
    /// before it but where an opaque one is nearer, as [`Glyph`] describes, with the engine's text
    /// shader, which `shaders` adds the first time it's asked for, making room for them and joining
    /// a run once for them all. Each transform takes the glyph's own space, where it's a unit square
    /// around the origin, to the world's.
    pub fn extend_glyphs(
        &mut self,
        shaders: &mut Shaders,
        glyphs: impl ExactSizeIterator<Item = (Mat4, Glyph, u64)>,
    ) {
        // No glyphs ask for no shader, which would have a renderer make its pipeline.
        if glyphs.len() == 0 {
            return;
        }
        let glyphs = glyphs.map(|(transform, glyph, id)| (transform, glyph.params(), id));
        self.extend_quads(shaders.text(), glyphs);
    }

    /// Draws `line` over the meshes drawn before it but where an opaque one is nearer, as [`Line`]
    /// describes, with the engine's line shader, which `shaders` adds the first time it's asked
    /// for, and with the ID `id`: an entity's, or 0 for none. `transform` takes the space its
    /// points are in to the world's, and leaves its width as it is, in the view's pixels.
    pub fn push_line(&mut self, shaders: &mut Shaders, transform: Mat4, line: Line, id: u64) {
        self.push_with_id(sprite::QUAD, shaders.line(), transform, line.params(), id);
    }

    /// Draws `grid` over the meshes drawn before it but where an opaque one is nearer, as [`Grid`]
    /// describes, with the engine's grid mesh and shader, which `meshes` and `shaders` add the
    /// first time they're asked for. `transform` takes the grid's own space, where it's a unit
    /// square around the origin in its XY plane, to the world's: its first two columns are the
    /// square's sides in the grid's plane, and its translation the square's center. Its ID is 0, of
    /// no entity.
    pub fn push_grid(
        &mut self,
        meshes: &mut Meshes,
        shaders: &mut Shaders,
        transform: Mat4,
        grid: Grid,
    ) {
        self.push(meshes.grid(), shaders.grid(), transform, grid.params());
    }

    /// Draws the outline of the square a unit across around the origin, in its XY plane, as a
    /// sprite is, that `transform` places in the world: its four sides, each a line in `color`,
    /// `width` pixels wide, as [`push_line`](Self::push_line) draws them with the ID `id`, which
    /// meet at its corners.
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
    /// `shader`, making room for them and joining a run once for them all.
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

    /// Moves `other`'s draws after these, in their order, leaving it empty but with its memory, as
    /// though they'd been drawn here.
    pub fn append(&mut self, other: &mut Draws) {
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
            let scene_last = self.scene_at == Some(self.runs.len());
            match self.runs.last_mut() {
                // Its first run goes on from these' last, if it's of the same mesh, shader and
                // faces, and the scene doesn't draw between them: the last ends where it begins.
                Some(last)
                    if (last.mesh, last.shader, last.faces)
                        == (run.mesh, run.shader, run.faces)
                        && !scene_last =>
                {
                    last.instances.end = instances.end;
                    last.params.end = params.end;
                }
                _ => self.runs.push(Run {
                    instances,
                    params,
                    ..run
                }),
            }
        }
        if scene_at == Some(count) {
            self.draw_scene();
        }
    }

    /// Adds the draws from `instance` on, whose parameters start at `start`, of `mesh` with
    /// `shader`, drawing `faces`, to the last run if it's of them too, or as a run of their own.
    fn extend_run(&mut self, mesh: u32, shader: u32, faces: Faces, instance: usize, start: usize) {
        let scene_last = self.scene_at == Some(self.runs.len());
        match self.runs.last_mut() {
            Some(run)
                if (run.mesh, run.shader, run.faces) == (mesh, shader, faces) && !scene_last =>
            {
                run.instances.end = self.transforms.len();
                run.params.end = self.params.len();
            }
            _ => self.runs.push(Run {
                mesh,
                shader,
                faces,
                instances: instance..self.transforms.len(),
                params: start..self.params.len(),
            }),
        }
    }

    /// Has the renderer's [`Scene`](crate::Scene) draw here, after the draws so far and before
    /// those after: over what's drawn so far, and under what's drawn after, but where what's
    /// opaque is nearer. The last place it says is the scene's.
    pub fn draw_scene(&mut self) {
        self.scene_at = Some(self.runs.len());
    }

    /// How many draws there are: one for each mesh drawn, of whichever run.
    pub fn len(&self) -> usize {
        self.transforms.len()
    }

    /// Places draws `draws`, as they're counted from the first, in the space `transform` takes
    /// theirs to: each one's transform becomes `transform` times its own. It changes no run, so a
    /// `transform` that mirrors leaves a draw that culls culling the faces it culled before.
    ///
    /// # Panics
    ///
    /// If `draws` reaches past them.
    pub fn place(&mut self, draws: Range<usize>, transform: Mat4) {
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

    /// Circles and lines are runs of the engine's quad with shaders of their own, which `Shaders`
    /// adds as each is first asked for, and no circles ask for none. Draws appended after others go
    /// on in their order, with their IDs, the first joining the last run, of the same mesh, shader
    /// and faces; those appended keep their memory alone.
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

    /// Placing draws counted from the first moves those alone into the space a transform takes
    /// them to, across their runs, which stay as they were.
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

    /// Grids are runs of a mesh and a shader of the engine's own, which `Meshes` and `Shaders` add
    /// the first time they're asked for, and then the same.
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
        let mesh = meshes.add("a", &[corner; 3], &[0, 1, 2]).unwrap();
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
