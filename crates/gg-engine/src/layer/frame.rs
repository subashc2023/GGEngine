//! What a frame draws: the view's settings, the layers' draws and picks, and its render data.

use std::mem;

use bytemuck::NoUninit;
use gg_core::math::{DVec3, Mat4, Vec3, Vec4};
use gg_renderer::{
    Antialiasing, Circle, Grade, Line, MAX_PICKS, Mesh, Shader, Sprite, Tonemapper, Viewpoint,
    Viewport,
};
use tracing::warn;

#[cfg(feature = "dev")]
use super::Outlining;
use super::{Context, Pick, Seer};
#[cfg(doc)]
use crate::audio::Place;
use crate::camera;
#[cfg(doc)]
use crate::camera::Camera;
use crate::canvas::Canvas;
#[cfg(feature = "dev")]
use crate::cells::CellState;
use crate::draw::{self, Poses};
use crate::environment;
#[cfg(doc)]
use crate::environment::Environment;
use crate::font::{Align, TextStyle};
use crate::game_ui;
#[cfg(doc)]
use crate::game_ui::GameUi;
use crate::grid::{self, Plane};
use crate::navigation;
#[cfg(doc)]
use crate::navigation::{NavAgent, NavMeshVolume};
use crate::physics;
#[cfg(doc)]
use crate::physics::{
    BoxCollider3D, CapsuleCollider3D, CircleCollider2D, RigidBody3D, SphereCollider3D,
};
use crate::render_thread::Extracted;
#[cfg(doc)]
use crate::{BoxCollider2D, Input, RigidBody2D, Transform};
use crate::{EntityId, MeshDraw};

impl Context {
    /// The color each frame clears to, in linear RGBA.
    pub fn clear_color(&self) -> [f32; 4] {
        self.clear_color
    }

    /// Sets the color each frame clears to, in linear RGBA.
    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    /// The antialiasing a layer set for the game's view, if any: see
    /// [`set_antialiasing`](Self::set_antialiasing).
    pub fn antialiasing(&self) -> Option<Antialiasing> {
        self.antialiasing
    }

    /// Sets the game's view's antialiasing from this frame on, over the world's [`Environment`] or
    /// without one. `None` returns to the environment's. Like a player's display settings, it's
    /// the run's own: no hash, scene, snapshot or recording holds it, so lockstep peers choosing
    /// differently, and their replays, are unaffected.
    pub fn set_antialiasing(&mut self, antialiasing: Option<Antialiasing>) {
        self.antialiasing = antialiasing;
    }

    /// The viewpoint a layer set for this frame, if any.
    pub fn viewpoint(&self) -> Option<Viewpoint> {
        self.viewpoint
    }

    /// Sets where the game's view sees the world from this frame. Frames where no layer sets one
    /// use the primary entity with a [`Camera`] and a [`Transform`], else [`Viewpoint::default`].
    /// A projection that can't project (see [`Projection`](crate::Projection)) is drawn as the
    /// last that could, or the default's, and warns once (`engine.bad_projection`).
    pub fn set_viewpoint(&mut self, viewpoint: Viewpoint) {
        self.viewpoint = Some(viewpoint);
    }

    /// What every matrix and position a frame draws is relative to, in `f32`: see
    /// [`RenderData::origin`](gg_renderer::RenderData::origin). The world and the layers' draws
    /// ([`draw`](Self::draw) and the rest) are drawn from it, so what's near the view is as precise
    /// 1,000 km from the world's origin as at it, within a layer's own `f32`s.
    ///
    /// A multiple of 1,024 on each axis, near the previous frame's view or, in a development build,
    /// the look's viewpoint. It moves to the nearest multiple once
    /// the view is more than 1,024 from it on an axis, so never while the view stays within 1,024
    /// of the world's origin.
    pub fn origin(&self) -> DVec3 {
        self.origin
    }

    /// Moves the [origin](Self::origin) to the look's viewpoint before the layers draw, unless the
    /// frame is bare, so a tool's far-off look is as precise as later frames.
    #[cfg(feature = "dev")]
    pub(crate) fn follow_look(&mut self) {
        if let Some(looked) = self.dev.look.viewpoint.filter(|_| !self.dev.look.bare) {
            self.origin = followed(self.origin, looked.position);
        }
    }

    /// Nothing: a build without `dev` has no look.
    #[cfg(not(feature = "dev"))]
    pub(crate) fn follow_look(&mut self) {}

    /// Where the game's view was in the main window as the last frame was handed off: physical
    /// pixels from the window's top left, like [`Input::cursor`]. The view is the main window's
    /// central area or the [`App::view_panel`](crate::App::view_panel). `None` while it has no
    /// area (a dockspace's first frame, another tab over it) and in replays. For what's presented,
    /// such as a camera keeping the point under the cursor still, not the world.
    pub fn view(&self) -> Option<Viewport> {
        self.view
    }

    /// Shows `size` pixels in the game's view instead of what's drawn, from this frame until the
    /// next call or [`hide_pixels`](Self::hide_pixels): another process's game, as the editor
    /// shows it. `fill` writes them: RGBA8 rows, sRGB, tightly packed, top row first, one per view
    /// pixel from the top left; the rest is the clear color. Meanwhile nothing drawn shows and
    /// picks find nothing.
    ///
    /// # Panics
    ///
    /// If a side is 0 or more than [`MAX_TEXTURE_SIZE`](gg_renderer::MAX_TEXTURE_SIZE).
    pub fn show_pixels(&mut self, size: [u32; 2], fill: impl FnOnce(&mut [u8])) {
        let sides = 1..=gg_renderer::MAX_TEXTURE_SIZE;
        assert!(
            size.iter().all(|side| sides.contains(side)),
            "{size:?} pixels; each side is 1 to {} pixels",
            gg_renderer::MAX_TEXTURE_SIZE
        );
        let bytes = size[0] as usize * size[1] as usize * 4;
        self.shown_bytes.resize(bytes, 0);
        fill(&mut self.shown_bytes);
        self.shown = Some(size);
        self.shown_new = true;
    }

    /// Undoes [`show_pixels`](Self::show_pixels) from this frame.
    pub fn hide_pixels(&mut self) {
        self.shown = None;
        self.shown_new = false;
    }

    /// Asks which entity the game's view shows at `at` (main-window physical pixels, like
    /// [`Input::cursor`]) in this frame: the one whose sprite, circle, mesh or collider outline
    /// ([`set_colliders_shown`](Self::set_colliders_shown)) is on top there.
    ///
    /// Returns the ticket for [`picked`](Self::picked), or `None` after 64 in a frame, as many as
    /// a frame reads back. The answer arrives four frames later, about 67 ms at 60 Hz: the main
    /// thread runs a frame ahead of the render thread, whose readback the GPU finishes two frames
    /// after.
    ///
    /// The answer is `None` where a layer's own draw ([`draw`](Self::draw) and the like) is on
    /// top, over the clear color, and, sooner, outside the view. A fully transparent sprite,
    /// circle, line or blended model, which its shader discards, finds what's behind it. A frame
    /// with nothing to render into (no swapchain image), or made bare by a tool's capture, leaves
    /// its picks to the next that renders. Replays answer none.
    pub fn pick(&mut self, at: [f64; 2]) -> Option<Pick> {
        if self.picks.len() == MAX_PICKS {
            return None;
        }
        let pick = Pick(self.tickets);
        self.tickets += 1;
        self.picks.push((pick, at));
        Some(pick)
    }

    /// Answers from the frame last taken back from the render thread: each a [`pick`](Self::pick)
    /// ticket with the entity shown at its pixel, or `None`. They last one frame, replaced after
    /// the next frame's updates and UI. The entity may have gone since.
    pub fn picked(&self) -> &[(Pick, Option<EntityId>)] {
        &self.picked
    }

    /// Draws `mesh` with `shader` and its `params` this frame, placed by `transform`, in the game's
    /// view.
    ///
    /// An opaque `shader` draws with the opaque, first, hiding what's behind. Otherwise it draws
    /// over earlier draws and the opaque, but under a nearer opaque draw. Consecutive draws of one
    /// mesh and shader render as one instanced draw, whatever their transforms and parameters.
    /// It's of no entity: a [pick](Self::pick) finds none there.
    pub fn draw<V, P: NoUninit>(
        &mut self,
        mesh: Mesh<V>,
        shader: Shader<V, P>,
        transform: Mat4,
        params: P,
    ) {
        let transform = relative(transform, self.origin);
        self.draws.push(mesh, shader, transform, params);
    }

    /// Draws `sprite` this frame, placed by `transform`, layered like a non-opaque
    /// [`draw`](Self::draw), of no entity. Unscaled, it's a unit square around the translation
    /// ([`Sprite`]).
    pub fn draw_sprite(&mut self, transform: Mat4, sprite: Sprite) {
        (self.draws).push_sprite(relative(transform, self.origin), sprite);
    }

    /// Draws `circle` this frame, placed by `transform`, layered like a non-opaque
    /// [`draw`](Self::draw), of no entity. Unscaled, it's a unit across around the translation
    /// ([`Circle`]).
    pub fn draw_circle(&mut self, transform: Mat4, circle: Circle) {
        let transform = relative(transform, self.origin);
        self.draws.push_circle(&mut self.shaders, transform, circle);
    }

    /// Draws `text` this frame in `style`, layered like a non-opaque [`draw`](Self::draw), of no
    /// entity.
    ///
    /// - `transform` places the origin, in its XY plane, +Y up, facing +Z, as for a sprite.
    /// - Lines end at `\n` or `\r\n`, laid out left to right, kerned, without glyph-replacing
    ///   features such as ligatures.
    /// - The em is `style.size` of the transform's units; the origin is the point of the text's
    ///   box `style.align` says.
    /// - Glyphs are crisp at any size ([`Glyph`](gg_renderer::Glyph)), by straight alpha.
    ///
    /// A style with no font draws in Inter, as the UI does. A run bakes it on the job system's
    /// threads as it starts; text drawn in it before that's done bakes it here: about 50 ms in
    /// release, 140 ms in debug. Once a text as long has been drawn, drawing text allocates
    /// nothing.
    ///
    /// # Panics
    ///
    /// If `style.font` is another context's.
    pub fn draw_text(&mut self, transform: Mat4, text: &str, style: &TextStyle) {
        let transform = relative(transform, self.origin);
        let glyphs = self.fonts.lay_out(transform, text, style, 0);
        self.draws.extend_glyphs(&mut self.shaders, glyphs);
    }

    /// Draws a line from `from` to `to` in the world this frame, `width` view pixels wide at any
    /// distance; thinner than a pixel, a pixel wide and fainter by as much. Its ends are squared
    /// off half its width past its points. `color` is linear RGBA, alpha not multiplied in.
    ///
    /// It draws over everything else but a nearer opaque mesh, as debugging wants, and is of no
    /// entity: [picks](Self::pick) find none there.
    pub fn draw_line(&mut self, from: Vec3, to: Vec3, color: Vec4, width: f32) {
        let origin = self.origin.as_vec3();
        let line = Line {
            from: from - origin,
            to: to - origin,
            color,
            width,
        };
        self.over
            .push_line(&mut self.shaders, Mat4::IDENTITY, line, 0);
    }

    /// Draws the outline of the unit square `transform` places, like a sprite's, this frame: four
    /// [`draw_line`](Self::draw_line) sides meeting at the corners.
    pub fn draw_rect(&mut self, transform: Mat4, color: Vec4, width: f32) {
        let transform = relative(transform, self.origin);
        self.over
            .push_rect(&mut self.shaders, transform, color, width, 0);
    }

    /// Draws on the game's view's canvas this frame, from `anchor`: a game's own UI, HUDs and
    /// menus, laid out by a layer each update in the view's own space, apart from the development
    /// UI.
    ///
    /// `anchor` picks a view edge, corner or middle, across then down, like text alignment:
    /// `[Align::End, Align::Start]` is the top right. The canvas's origin is there, +Y up. It
    /// scales with the view, [`CANVAS_HEIGHT`](crate::CANVAS_HEIGHT) units to its height as handed
    /// off, whatever the pixels.
    ///
    /// It draws in order over everything else, nearest entities and lines too, so captures and the
    /// editor's view show it, but under the development UI. Its state, such as the focused button,
    /// is the layer's, so each lockstep peer has its own. A choice that changes the game queues a
    /// command, like input.
    pub fn canvas(&mut self, anchor: [Align; 2]) -> Canvas<'_> {
        let (view, cursor) = (self.view, self.input.cursor());
        let shaders = &mut self.shaders;
        Canvas::new(
            &mut self.canvas,
            &mut self.fonts,
            shaders,
            anchor,
            view,
            cursor,
        )
    }

    /// Whether the cursor is over something a pointer works on in any input-taking UI drawn last
    /// frame ([`GameUi::pointer_over`]). A game reading the mouse itself, to aim, pick or drag,
    /// leaves it alone while this is true.
    pub fn pointer_over_ui(&mut self) -> bool {
        let (view, cursor) = (self.view, self.input.cursor());
        self.uis.pointer_over(&mut self.fonts, view, cursor)
    }

    /// What the layers' UIs lay out, draw and take input with, lent apart: the UIs, fonts, canvas,
    /// shaders, the view as the last frame handed it off, and the input as layers are sent it.
    pub(crate) fn ui_parts(&mut self) -> game_ui::Parts<'_> {
        game_ui::Parts {
            uis: &mut self.uis,
            fonts: &mut self.fonts,
            drawn: &mut self.canvas,
            shaders: &mut self.shaders,
            view: self.view,
            input: &self.input,
        }
    }

    /// Each UI the layers added, laid out for the view as the last frame handed it off, as the
    /// tool API's `ui` lists them.
    #[cfg(feature = "dev")]
    pub(crate) fn ui_list(&mut self) -> gg_api::UiList {
        self.uis.listed(&mut self.fonts, self.view)
    }

    /// Draws a grid this frame as `viewpoint` sees it (the frame's, from
    /// [`set_viewpoint`](Self::set_viewpoint)), fitted to the last handed-off view
    /// ([`view`](Self::view)).
    ///
    /// Lines are a power of ten apart on axis plane `plane`, or the one an axis-aligned
    /// orthographic viewpoint faces ([`grid::shown`]). The plane's two axes
    /// are in [`grid::RED`], [`grid::GREEN`] and [`grid::BLUE`]. Lines show once they're 8 pixels
    /// apart, are a crisp pixel each through an orthographic viewpoint that's [`grid::square`],
    /// and fade toward grazing angles.
    ///
    /// It draws by straight alpha, layered like a non-opaque [`draw`](Self::draw), before the world
    /// and of no entity, so picks go to what's on top. Nothing draws while the view has no area (a
    /// dockspace's first frame) or the plane is too near edge-on.
    pub fn draw_grid(&mut self, viewpoint: &Viewpoint, plane: Plane) {
        let Some(view) = self.view else {
            return;
        };
        let shown = grid::shown(viewpoint, plane);
        if let Some((transform, grid)) = grid::quad(viewpoint, view, shown, self.origin) {
            (self.draws).push_grid(&mut self.meshes, &mut self.shaders, transform, grid);
        }
    }

    /// Outlines what the game's view shows of entity `id` and its subtree this frame, as the
    /// editor outlines what's picked: a model's nodes and a prefab's entities included.
    ///
    /// The outline runs round the shape their pixels make together where their draws, and their
    /// colliders' outlines, are on top, even across something in front, but not round what's
    /// hidden. It's over what the view shows and under the UI: a solid band `width` view pixels
    /// wide, at most 8, its outer edge smoothed, then a glow at up to half its alpha fading over
    /// as far again. `color` is linear RGBA, alpha not multiplied in.
    ///
    /// An entity that's gone outlines nothing. One outline a frame: a later call replaces an
    /// earlier one.
    pub fn outline(&mut self, id: EntityId, color: Vec4, width: f32) {
        self.outline_root.clear();
        self.outline_root.push(id);
        self.outline = Some((color, width));
    }

    /// What a layer has [outlined](Self::outline) this frame, if anything: the entity, the color
    /// and the width.
    pub fn outlined(&self) -> Option<(EntityId, Vec4, f32)> {
        let (color, width) = self.outline?;
        Some((*self.outline_root.first()?, color, width))
    }

    /// Whether the world's colliders' outlines draw: see
    /// [`set_colliders_shown`](Self::set_colliders_shown). Not at first.
    pub fn colliders_shown(&self) -> bool {
        self.colliders
    }

    /// Draws the world's colliders' outlines each frame from now on, or not. Each
    /// [`BoxCollider2D`], [`CircleCollider2D`], [`BoxCollider3D`], [`SphereCollider3D`] and
    /// [`CapsuleCollider3D`] whose entity has a [`Transform`] is outlined as physics shapes it,
    /// 2 points wide in light green, where its entity draws, like lines. A [pick](Self::pick) of an
    /// outline finds its entity. They draw whether or not physics moves the entity (it doesn't
    /// without a [`RigidBody2D`] or [`RigidBody3D`], or under a parent).
    pub fn set_colliders_shown(&mut self, shown: bool) {
        self.colliders = shown;
    }

    /// Whether the world's navmesh and its agents' paths draw: see
    /// [`set_navmesh_shown`](Self::set_navmesh_shown). Not at first.
    pub fn navmesh_shown(&self) -> bool {
        self.navmesh
    }

    /// Has the world's navmesh and its agents' paths draw each frame from then on, or not, as the
    /// last [`step_navigation`](crate::step_navigation) left them:
    /// - each navmesh polygon outlined in sky blue along its cells' floors, with its
    ///   [`NavMeshVolume`]'s entity's ID;
    /// - each [`NavAgent`]'s path in amber from where its entity draws through the corners it has
    ///   left, its destination marked, with the agent's ID.
    ///
    /// A [pick](Self::pick) of a line finds its entity. Lines are 2 points wide, a little above
    /// the floors, drawn as lines are.
    pub fn set_navmesh_shown(&mut self, shown: bool) {
        self.navmesh = shown;
    }

    /// Copies where the world's entities are, as a tick starts, for the frames after it to draw
    /// them from, and moves the world's change tick on, so what the tick changes is marked as
    /// changed since it started.
    pub(crate) fn capture(&mut self) {
        self.past.capture(&mut self.world);
        self.tick_started = self.world.change_tick();
    }

    /// Draws the world's entities, after what the layers drew this frame: each `alpha` of the way
    /// from where it was as the latest of the `ticks` ticks run started to where the tick left it,
    /// at `time`, the simulation's in seconds. Emitters' particles draw too, and colliders'
    /// outlines over the rest while shown or the look shows them. It also finds the world's
    /// camera, environment and lights. Before the entities, a development build draws the look's
    /// grid, unless the frame is bare, as the viewpoint the frame sees from so far sees it.
    pub(crate) fn draw_world(&mut self, alpha: f32, time: f32, ticks: u64) {
        self.past.resolve(&self.world);
        let held = self.world.component::<MeshDraw>();
        self.placed.place(&self.world, held);
        (self.placed).draw(&self.world, &self.past, alpha, self.tick_started, held);
        let (made, meshes, draws) = (&self.loaded.made, &mut self.meshes, &mut self.draws);
        (self.skins).find(&self.world, &self.placed, made, meshes, draws);
        let origin = self.origin;
        (self.opaque).hold(
            &self.world,
            &self.placed,
            &self.skins,
            &mut self.scene,
            origin,
        );
        let (world, past, placed) = (&self.world, &self.past, &self.placed);
        self.seen = camera::seen(world, past, placed, alpha, &mut self.tie);
        self.environment = environment::seen(world, &mut self.environment_tie);
        (self.lit).find(world, past, placed, (alpha, self.origin), self.tick_started);
        self.time = time;
        #[cfg(feature = "dev")]
        if let Some(plane) = self.dev.look.grid.filter(|_| !self.dev.look.bare) {
            let seen = self.seen.map(|(_, seen)| seen);
            let viewpoint = self.dev.look.viewpoint.or(self.viewpoint).or(seen);
            self.draw_grid(&viewpoint.unwrap_or_default(), plane);
            self.draw_cells();
        }
        let (world, past, placed) = (&self.world, &self.past, &self.placed);
        let (draws, shaders, fonts) = (&mut self.draws, &mut self.shaders, &mut self.fonts);
        let scratch = &mut self.scratch;
        let drawn = (alpha, self.origin);
        draw::draw_world(
            world, past, placed, drawn, ticks, draws, shaders, fonts, scratch,
        );
        let (colliders, navmesh) = (self.colliders_drawn(), self.navmesh_drawn());
        if (colliders || navmesh)
            && let Some(poses) = Poses::new(world, past, placed, alpha, self.origin)
        {
            let scale = self.scale() as f32;
            let (over, shaders) = (&mut self.over, &mut self.shaders);
            if colliders {
                physics::draw_colliders(world, &poses, scale, over, shaders);
            }
            if navmesh {
                navigation::draw_navigation(world, &poses, scale, over, shaders);
            }
        }
        // What changes after, until the next frame draws, is marked past what this one saw.
        self.world.advance();
    }

    /// Draws the world's cells' cubes as twelve lines each, [`CELL_IN`] for those in and
    /// [`CELL_REFUSED`] for those refused, as the look's grid draws, so the editor's view shows
    /// where the cells are.
    #[cfg(feature = "dev")]
    fn draw_cells(&mut self) {
        let Some((_, cells, _)) = self.world.world_cells() else {
            return;
        };
        let size = cells.size;
        let state = self.world.resource::<CellState>();
        let Some(state) = state.filter(|_| size.is_finite() && size > 0.0) else {
            return;
        };
        let cells_in = state.cells_in.iter().map(|&cell| (cell, CELL_IN));
        let refused = state.refused.iter().map(|&cell| (cell, CELL_REFUSED));
        for (cell, color) in cells_in.chain(refused) {
            let low = (cell.0.as_dvec3() * f64::from(size) - self.origin).as_vec3();
            // Along each axis, the four edges from the corners low on it.
            for (axis, corner) in (0..3).flat_map(|axis| (0..4).map(move |corner| (axis, corner))) {
                let mut from = low;
                let across = [(axis + 1) % 3, (axis + 2) % 3];
                for (bit, across) in across.into_iter().enumerate() {
                    if corner & (1 << bit) != 0 {
                        from[across] += size;
                    }
                }
                let mut to = from;
                to[axis] += size;
                let line = Line {
                    from,
                    to,
                    color,
                    width: 1.5,
                };
                (self.over).push_line(&mut self.shaders, Mat4::IDENTITY, line, 0);
            }
        }
    }

    /// Whether the frame draws the colliders' outlines: as the look says, in a frame that isn't
    /// bare, or else as the layers set.
    fn colliders_drawn(&self) -> bool {
        #[cfg(feature = "dev")]
        if !self.dev.look.bare
            && let Some(shown) = self.dev.look.colliders
        {
            return shown;
        }
        self.colliders
    }

    /// Whether the frame draws the navmesh and the agents' paths, as the layers set.
    fn navmesh_drawn(&self) -> bool {
        self.navmesh
    }

    /// Fills in `frame` with this frame's render data, with the game's view at `viewport`
    /// ([`view`](Self::view) gives it from then on):
    /// - the world's environment's settings and lights as last drawn, and the time. Without an
    ///   environment there's no tonemapper, antialiasing (unless a layer set one), adaptation,
    ///   sky, ambient occlusion, fog, reflections or camera effects, and the default's exposure;
    /// - the draws, those over the rest last, and the canvas placed from its anchors in the
    ///   view's pixels, forgetting the last frame's but keeping their memory;
    /// - the viewpoint a layer set this frame, else the world's camera's, else the default. The
    ///   frame is cut to it if it comes from elsewhere than the last frame's, as [`Seer`] says,
    ///   and audio hears from it, as [`Place`] says. The [origin](Self::origin) then follows it;
    /// - what a layer outlined this frame;
    /// - the pixels the view shows, if any, swapped for those `frame` had;
    /// - what's been added since a frame last took it, after what `frame` holds, including images
    ///   the loader has decoded and how many it's still decoding;
    /// - the picks asked this frame, each at the view's pixel under it (the floor of where it was
    ///   asked, less the view's origin). Those outside the view are answered `None` with the next
    ///   answers to come back.
    ///
    /// The viewpoint, outline and picks are forgotten once taken. In a development build the
    /// look's viewpoint and outline replace the layers', the passes it leaves out are left out,
    /// and the frame carries the look's revision and whether it's seen from the look's viewpoint,
    /// whose exposure adapts apart from the game's own. A bare frame leaves the look out, and
    /// leaves all picks, asked this frame or held by the renderer, to the next frame that isn't
    /// bare, so none reads a view a client isn't shown.
    pub(crate) fn extract(&mut self, frame: &mut Extracted, viewport: Viewport) {
        frame.clear_color = self.clear_color;
        // A world without an environment isn't tonemapped or antialiased, but as a layer sets, and
        // its light is exposed as the default environment's is.
        frame.tonemapper = (self.environment).map_or(Tonemapper::None, |seen| seen.tonemapper);
        frame.antialiasing = self.antialiasing.unwrap_or_else(|| {
            (self.environment).map_or(Antialiasing::None, |seen| seen.antialiasing)
        });
        frame.exposure = self.environment.unwrap_or_default().light_scale();
        frame.adaptation = (self.environment)
            .and_then(|seen| seen.adaptation)
            .map(|adaptation| adaptation.rendered());
        frame.sky = self.environment.and_then(|seen| seen.sky());
        // Nor bloomed or graded.
        frame.bloom = self.environment.map_or(0.0, |seen| seen.bloom);
        frame.grade = self.environment.map_or(Grade::IDENTITY, |seen| seen.grade);
        frame.ambient_occlusion = self.environment.and_then(|seen| seen.ambient_occlusion);
        let fog = self.environment.and_then(|seen| seen.fog);
        let broken = fog.and_then(|fog| Some((fog, fog.non_finite()?)));
        if let Some((fog, field)) = broken
            && !mem::replace(&mut self.fog_warned, true)
        {
            warn!(
                code = "engine.bad_fog",
                field,
                "the world's fog has a {field} that isn't finite, {fog:?}: the view draws without \
                 fog"
            );
        }
        // The next streak of broken fog warns anew.
        self.fog_warned &= broken.is_some();
        frame.fog = fog.filter(|_| broken.is_none());
        frame.reflections = self.environment.and_then(|seen| seen.reflections);
        frame.atmosphere = self.environment.and_then(|seen| seen.atmosphere());
        frame.depth_of_field = (self.environment)
            .and_then(|seen| seen.depth_of_field)
            .map(|focus| focus.rendered());
        frame.motion_blur = (self.environment)
            .and_then(|seen| seen.motion_blur)
            .map(|blur| blur.rendered());
        frame.lens = self
            .environment
            .map_or(gg_renderer::Lens::NONE, |seen| seen.lens.rendered());
        frame.lights.clear();
        frame.lights.extend_from_slice(self.lit.lights());
        frame.time = self.time;
        frame.viewport = viewport;
        self.view = (viewport.size[0] > 0 && viewport.size[1] > 0).then_some(viewport);
        let (viewpoint, outline) = (self.viewpoint.take(), self.outline.take());
        let seen = match (viewpoint, self.seen) {
            (Some(viewpoint), _) => (Seer::Layer, viewpoint),
            (None, Some((camera, viewpoint))) => (Seer::Camera(camera), viewpoint),
            (None, None) => (Seer::Default, Viewpoint::default()),
        };
        let layers = outline.map(|(color, width)| (&self.outline_root[..], color, width));
        #[cfg(feature = "dev")]
        let (seen, outline) = if self.dev.look.bare {
            (seen, layers)
        } else {
            let looked = (self.dev.look.viewpoint).map(|looked| (Seer::Look(looked), looked));
            let wanted = (self.dev.look.outline.as_ref())
                .map(|Outlining { ids, color, width }| (&ids[..], *color, *width));
            (looked.unwrap_or(seen), wanted.or(layers))
        };
        #[cfg(not(feature = "dev"))]
        let outline = layers;
        let (seer, viewpoint) = seen;
        let viewpoint = camera::checked(viewpoint, seer, &mut self.projection);
        frame.viewpoint = viewpoint;
        frame.origin = self.origin;
        self.origin = followed(self.origin, viewpoint.position);
        let perspective = matches!(viewpoint.projection, crate::Projection::Perspective { .. });
        frame.cut = self.seer != Some((seer, perspective));
        self.seer = Some((seer, perspective));
        (self.audio).hear_from(&frame.viewpoint, viewport.size, &self.world);
        frame.outline = outline.map(|(roots, color, width)| {
            let (walk, ids) = (&mut self.outline_walk, &mut frame.outline_ids);
            self.world.fill_subtree_ids(roots, walk, ids);
            (color, width)
        });
        #[cfg(feature = "dev")]
        {
            self.dev.look.seen = Some(frame.viewpoint);
            frame.look = self.dev.look.revision;
            frame.bare = self.dev.look.bare;
            frame.show = self.dev.look.shown();
            self.dev.look.leave_out(frame);
            // As the look's viewpoint shows, so that a frame that turns to or from it is cut to.
            frame.looked = matches!(seer, Seer::Look(_));
        }
        frame.pixels = self.shown;
        if mem::take(&mut self.shown_new) {
            mem::swap(&mut frame.pixel_bytes, &mut self.shown_bytes);
            frame.pixels_new = true;
        }
        self.draws.append(&mut self.over);
        mem::swap(&mut frame.draws, &mut self.draws);
        self.draws.clear();
        self.canvas.place(viewport.size, &mut frame.canvas);
        self.uis.handed_off();
        self.loader.take(&mut self.textures);
        frame.loading = self.loader.pending();
        self.fonts.place(&mut self.textures);
        (frame.uploads).take(
            &mut self.meshes,
            &mut self.shaders,
            &mut self.textures,
            &mut self.materials,
        );
        frame.uploads.take_scene(&mut self.scene);
        frame.picks.clear();
        #[cfg(feature = "dev")]
        if self.dev.look.bare {
            return;
        }
        let [left, top] = viewport.origin.map(f64::from);
        let [width, height] = viewport.size.map(f64::from);
        for (pick, [x, y]) in self.picks.drain(..) {
            // Not a NaN, which no range contains.
            let [x, y] = [x.floor() - left, y.floor() - top];
            if (0.0..width).contains(&x) && (0.0..height).contains(&y) {
                let pixel = [x as u32, y as u32];
                frame.picks.push(gg_renderer::Pick { tag: pick.0, pixel });
            } else {
                self.unseen.push(pick);
            }
        }
    }

    /// Takes `answers`, which came back with the frame taken back from the render thread, each a
    /// pick's ticket number with the ID the view showed at its pixel, replacing the earlier ones.
    /// Picks the last frame handed off outside the view are answered `None`.
    pub(crate) fn take_answers(&mut self, answers: &[(u64, u64)]) {
        self.picked.clear();
        for &(tag, id) in answers {
            self.picked.push((Pick(tag), EntityId::new(id)));
        }
        for pick in self.unseen.drain(..) {
            self.picked.push((pick, None));
        }
    }
}

/// How far apart on each axis a frame's [origin](Context::origin) can be, and how far on one axis
/// the view goes from it before it moves. A power of two, so the origin is exact in `f32` and
/// positions within a step of it differ by less than 0.25 mm in `f32`.
const ORIGIN_STEP: f64 = 1024.0;

/// The origin the frame after one drawn from `origin` draws from, the view having seen from `at`.
/// It's `origin` while `at` is within [`ORIGIN_STEP`] of it on every axis, or isn't finite; else
/// `at` rounded to a multiple of it on each axis. So it moves at most once per half step the
/// view travels, and never for a view within a step of the world's origin.
pub(super) fn followed(origin: DVec3, at: DVec3) -> DVec3 {
    let near = (at - origin).abs().cmple(DVec3::splat(ORIGIN_STEP)).all();
    if near || !at.is_finite() {
        return origin;
    }
    (at / ORIGIN_STEP).round() * ORIGIN_STEP
}

/// `transform`, which places something in the world, placed relative to `origin` instead.
fn relative(transform: Mat4, origin: DVec3) -> Mat4 {
    let mut relative = transform;
    relative.w_axis -= origin.as_vec3().extend(0.0);
    relative
}

/// The color of the edges of a cell in, as [`Context::draw_cells`] draws them: a light blue, apart
/// from the grid's axes' colors.
#[cfg(feature = "dev")]
const CELL_IN: Vec4 = Vec4::new(0.3, 0.7, 1.0, 0.6);

/// The color of the edges of a cell refused, as [`Context::draw_cells`] draws them.
#[cfg(feature = "dev")]
const CELL_REFUSED: Vec4 = Vec4::new(1.0, 0.25, 0.2, 0.9);
