//! The layer stack: how an app's code, and the engine's tools, plug into the main loop.

use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_core::math::Mat4;
use gg_core::{Error, Result};
use gg_renderer::{
    Blend, Camera, Draws, Mesh, Meshes, RenderData, Shader, Shaders, Sprite, SpriteSheet, Texture,
    Textures, Viewport,
};

use crate::draw::{self, Scratch};
use crate::image::{self, Image};
use crate::perf::Perf;
use crate::{Event, Input, MeshDraw, Transform, Ui, World};

/// A part of an app that the main loop drives. Every method has a default that does nothing,
/// so a layer implements only what it uses.
///
/// The world changes only as the layers attach and detach, and at their ticks: the rest of a
/// layer reads it, and queues commands, data of its own, for its next tick to apply, so that the
/// simulation goes on at its own rate whatever the frame rate.
pub trait Layer {
    /// Called once as the app starts, before the first tick and update: it builds the world the
    /// simulation starts from.
    fn attach(&mut self, _cx: &mut Context) {}

    /// Called once as the app ends, after the last tick.
    fn detach(&mut self, _cx: &mut Context) {}

    /// Called at each tick of the simulation, [64 a second](Tick::DT) whatever the frame rate,
    /// before the frame's updates: the layer applies the commands it queued since its last tick,
    /// in the order it queued them, then runs its systems, in the order it calls them. It gets the
    /// world alone, so that a tick goes by the world, the tick, its commands and what the layer set
    /// up as it attached, never by the frame's time, input or drawing.
    fn tick(&mut self, _world: &mut World, _tick: Tick) {}

    /// Called every frame, after the frame's ticks, with the time since the last frame: zero for
    /// the first, and for the first after the window is restored from being minimized, so that
    /// the pause doesn't count as one long frame. For what's presented, such as the camera, rather
    /// than the world, which changes at ticks.
    fn update(&mut self, _cx: &mut Context, _dt: Duration) {}

    /// Called for each event that the layers above didn't handle. `Break` handles it, so the
    /// layers below don't see it.
    fn event(&mut self, _cx: &mut Context, _event: &Event<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    /// Called every frame, after the updates, to draw UI: dockable panels, and anything else
    /// egui draws.
    fn ui(&mut self, _cx: &mut Context, _ui: &mut Ui<'_>) {}
}

/// A step of the simulation, which runs 64 of them a second whatever the frame rate: what a
/// layer's [`tick`](Layer::tick) gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    /// The ticks before it.
    pub number: u64,
}

impl Tick {
    /// How long a tick simulates: a 64th of a second, exact in `f32`, `f64` and nanoseconds.
    pub const DT: Duration = Duration::from_nanos(15_625_000);

    /// The simulation's time once this tick has run: `DT` for the first.
    pub fn time(self) -> Duration {
        Duration::from_nanos(Self::DT.as_nanos() as u64 * (self.number + 1))
    }
}

/// What layers can see and change of the running app.
pub struct Context {
    window: Option<Window>,
    input: Input,
    clear_color: [f32; 4],
    camera: Camera,
    meshes: Meshes,
    shaders: Shaders,
    textures: Textures,
    /// What to draw this frame.
    draws: Draws,
    world: World,
    /// What drawing the world sorts and computes, kept between frames.
    scratch: Scratch,
    /// Set from when the layers have attached until they detach: the world then changes only at
    /// ticks.
    attached: bool,
    /// How long frames take, which the main loop measures and
    /// [`PerfPanel`](crate::PerfPanel) shows.
    pub(crate) perf: Perf,
}

impl Context {
    pub(crate) fn new(window: Option<Window>) -> Self {
        // The engine's names first, so that a game's types can't take them, listed before any
        // entity has one.
        let mut world = World::default();
        let registered = world.register::<Transform>().and_then(|()| {
            world.register::<Sprite>()?;
            world.register::<MeshDraw>()
        });
        registered.expect("the engine's components describe themselves");
        Self {
            window,
            input: Input::default(),
            clear_color: [0.02, 0.02, 0.02, 1.0],
            camera: Camera::default(),
            meshes: Meshes::default(),
            shaders: Shaders::default(),
            textures: Textures::default(),
            draws: Draws::default(),
            world,
            scratch: Scratch::default(),
            attached: false,
            perf: Perf::default(),
        }
    }

    /// The app's window; `None` in a headless run.
    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }

    /// What's held down and where the cursor is, for layers that poll input rather than
    /// follow events.
    pub fn input(&self) -> &Input {
        &self.input
    }

    /// The color each frame clears to, in linear RGBA.
    pub fn clear_color(&self) -> [f32; 4] {
        self.clear_color
    }

    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    /// What the game's view sees the world from; at first, [`Camera::default`].
    pub fn camera(&self) -> Camera {
        self.camera
    }

    /// Sets what the game's view sees the world from, for this frame and those after it. A
    /// projection that can't project panics as the frame renders, as
    /// [`Projection`](crate::Projection) says.
    pub fn set_camera(&mut self, camera: Camera) {
        self.camera = camera;
    }

    /// Compiles the Slang shader at `path` for layers to draw meshes of vertex type `V` with from
    /// then on, which takes parameters of type `P` with each draw, such as a color or a texture:
    /// `()` for none. `blend` says how its color goes over what's drawn before it, such as
    /// [`Blend::Alpha`] for straight alpha, as PNGs store it. `name` names it in errors and GPU
    /// captures. The shader keeps to the contract that [`gg_renderer::Shaders::add`] describes:
    /// entry points `vertex_main` and `fragment_main`; instanced draws, each run of consecutive
    /// draws of one mesh with the shader being one, with push constants that hold the matrix from
    /// the world to clip space and the addresses of the vertices, which it reads as `V` is laid
    /// out, of the instances' transforms, and of their parameters, which it reads as `P` is; and
    /// textures sampled from the bindless set by the handles in its parameters.
    ///
    /// Fails with `renderer.bad_shader` and slangc's diagnostics if it doesn't compile, and with
    /// `renderer.no_slangc` if slangc can't be run.
    pub fn add_shader<V: NoUninit, P: NoUninit>(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
        blend: Blend,
    ) -> Result<Shader<V, P>> {
        self.shaders.add(name, path.as_ref(), blend)
    }

    /// Loads the PNG file at `path` as a texture for sprites and layers' shaders to sample from
    /// then on, through the handle: a sprite's `texture`, or a field of a shader's parameters as it
    /// is. Any PNG loads: its colors are taken as sRGB-encoded and its alpha as not multiplied
    /// into them, and shaders sample its colors decoded to linear, with texture coordinates from
    /// (0, 0) at its top left to (1, 1) at its bottom right. `name` names the texture in errors
    /// and GPU captures. The GPU has it from the next frame that renders.
    ///
    /// Fails with `engine.bad_texture` if the file can't be read or doesn't decode as a PNG, with
    /// `renderer.bad_texture` if a side is more than 8,192 pixels, the most every Vulkan 1.4
    /// device takes, and with `renderer.too_many_textures` past the 61,440 a renderer holds, the
    /// engine's white one among them.
    pub fn add_texture(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Texture> {
        let image = image::load_png(name, path.as_ref())?;
        self.textures.add(name, image.size, &image.pixels)
    }

    /// Loads the PNG file at `path` as a sprite sheet: a texture, as
    /// [`add_texture`](Self::add_texture) loads one, cut into cells `cell` pixels across and down,
    /// packed edge to edge from its top left, which [`SpriteSheet::sprite`] draws.
    ///
    /// Fails as `add_texture` does, and with `engine.bad_sprite_sheet` if a side of `cell` is 0
    /// or more than the image's.
    pub fn add_sprite_sheet(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
        cell: [u32; 2],
    ) -> Result<SpriteSheet> {
        let path = path.as_ref();
        let image = image::load_png(name, path)?;
        add_sheet(&mut self.textures, name, path, image, cell)
    }

    /// Adds a mesh for layers to draw from then on: triangles, three of `indices` each, which
    /// index `vertices`. A vertex is any `#[repr(C)]` type without padding, which derives
    /// bytemuck's `NoUninit`. `name` names the mesh in errors and GPU captures. The GPU has it
    /// from the next frame that renders.
    ///
    /// Fails with `renderer.bad_mesh` if it has no triangles, indices left over from whole
    /// triangles, or an index past its vertices.
    pub fn add_mesh<V: NoUninit>(
        &mut self,
        name: &str,
        vertices: &[V],
        indices: &[u32],
    ) -> Result<Mesh<V>> {
        self.meshes.add(name, vertices, indices)
    }

    /// Draws `mesh` with `shader` this frame, where `transform` places it in the world, over the
    /// meshes drawn before it, in the game's view as the camera sees it; the shader gets
    /// `params`. The view is the main window's central area, which the panels dock around.
    /// Consecutive draws of one mesh with one shader render as one instanced draw, whatever their
    /// transforms and parameters, so a frame of many takes few draws on the GPU.
    pub fn draw<V, P: NoUninit>(
        &mut self,
        mesh: Mesh<V>,
        shader: Shader<V, P>,
        transform: Mat4,
        params: P,
    ) {
        self.draws.push(mesh, shader, transform, params);
    }

    /// Draws `sprite` this frame, where `transform` places it in the world, over the meshes drawn
    /// before it, in the game's view as the camera sees it. Unscaled, a sprite is a unit square
    /// around the transform's translation, as [`Sprite`] says.
    pub fn draw_sprite(&mut self, transform: Mat4, sprite: Sprite) {
        self.draws.push_sprite(transform, sprite);
    }

    /// The world: its entities and their components, as the frame's ticks left it. After the
    /// layers' updates each frame, the engine draws those with a [`Transform`](crate::Transform)
    /// and a [`Sprite`] or a [`MeshDraw`](crate::MeshDraw), back to front by their z's in the
    /// world, and at one z sprites under meshes, each in the order of their IDs, children where
    /// their parents carry them, over what the layers drew themselves.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The world, to change as a layer attaches or detaches. At any other time it changes only at
    /// ticks, which [`Layer::tick`] is given it for, and this panics: a layer queues a command for
    /// its next tick instead.
    pub fn world_mut(&mut self) -> &mut World {
        assert!(
            !self.attached,
            "a layer changes the world only as it attaches or detaches, and at its ticks: queue a \
             command for its next tick"
        );
        &mut self.world
    }

    /// Draws the world's entities, after what the layers drew this frame.
    pub(crate) fn draw_world(&mut self) {
        draw::draw_world(&self.world, &mut self.draws, &mut self.scratch);
    }

    /// The frame's render data, with the game's view at `viewport`.
    pub(crate) fn render_data(&mut self, viewport: Viewport) -> RenderData<'_> {
        RenderData {
            clear_color: self.clear_color,
            viewport,
            camera: self.camera,
            meshes: &mut self.meshes,
            shaders: &mut self.shaders,
            textures: &mut self.textures,
            draws: &self.draws,
        }
    }

    /// Forgets the frame's draws, once the renderer has had them.
    pub(crate) fn clear_draws(&mut self) {
        self.draws.clear();
    }
}

/// Adds `image`, sprite sheet `name`'s from `path`, to `textures` as a sheet of cells `cell` pixels
/// across and down, if they fit it; one that doesn't takes no texture.
fn add_sheet(
    textures: &mut Textures,
    name: &str,
    path: &Path,
    image: Image,
    cell: [u32; 2],
) -> Result<SpriteSheet> {
    let [width, height] = image.size;
    if !(1..=width).contains(&cell[0]) || !(1..=height).contains(&cell[1]) {
        return Err(Error::new(
            "engine.bad_sprite_sheet",
            format!(
                "sprite sheet {name:?} at {} is {width}x{height} pixels, but its cells are {}x{}, \
                 where each side is 1 pixel up to the image's",
                path.display(),
                cell[0],
                cell[1]
            ),
        ));
    }
    let texture = textures.add(name, image.size, &image.pixels)?;
    Ok(SpriteSheet {
        texture,
        size: image.size,
        cell,
    })
}

/// The app's window, as layers see it.
pub struct Window {
    raw: Arc<winit::window::Window>,
}

impl Window {
    pub(crate) fn new(raw: Arc<winit::window::Window>) -> Self {
        Self { raw }
    }

    /// The drawable area's width and height in physical pixels.
    pub fn size(&self) -> [u32; 2] {
        let size = self.raw.inner_size();
        [size.width, size.height]
    }

    /// Physical pixels per logical pixel, such as 2 on a high-density display.
    pub fn scale(&self) -> f64 {
        self.raw.scale_factor()
    }
}

/// Layers from the bottom up, with the overlays above the layers.
#[derive(Default)]
pub(crate) struct Stack {
    layers: Vec<Box<dyn Layer>>,
    /// Where the overlays start.
    overlays: usize,
}

impl Stack {
    /// Adds a layer on top of the other layers, under the overlays.
    pub(crate) fn push_layer(&mut self, layer: Box<dyn Layer>) {
        self.layers.insert(self.overlays, layer);
        self.overlays += 1;
    }

    /// Adds an overlay on top of everything.
    pub(crate) fn push_overlay(&mut self, overlay: Box<dyn Layer>) {
        self.layers.push(overlay);
    }

    pub(crate) fn attach(&mut self, cx: &mut Context) {
        let _zone = gg_core::zone!();
        for layer in &mut self.layers {
            layer.attach(cx);
        }
        cx.attached = true;
    }

    /// Detaches from the top down, the reverse of attaching.
    pub(crate) fn detach(&mut self, cx: &mut Context) {
        let _zone = gg_core::zone!();
        cx.attached = false;
        for layer in self.layers.iter_mut().rev() {
            layer.detach(cx);
        }
    }

    /// Runs a tick of the simulation: each layer's, from the bottom up.
    pub(crate) fn tick(&mut self, cx: &mut Context, tick: Tick) {
        let _zone = gg_core::zone!();
        for layer in &mut self.layers {
            layer.tick(&mut cx.world, tick);
        }
    }

    pub(crate) fn update(&mut self, cx: &mut Context, dt: Duration) {
        let _zone = gg_core::zone!();
        for layer in &mut self.layers {
            layer.update(cx, dt);
        }
    }

    /// Draws UI from the bottom up, so the layers above draw over those below.
    pub(crate) fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let _zone = gg_core::zone!();
        for layer in &mut self.layers {
            layer.ui(cx, ui);
        }
    }

    /// Records `event` in the input state that layers poll, then hands it down from the top
    /// until a layer handles it. `Break` means one did.
    pub(crate) fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        cx.input.record(event);
        for layer in self.layers.iter_mut().rev() {
            layer.event(cx, event)?;
        }
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::{Key, Transform};

    /// Records each call it gets, by layer name, and handles events if `handles` is set.
    struct Probe {
        name: &'static str,
        handles: bool,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl Layer for Probe {
        fn attach(&mut self, _: &mut Context) {
            self.calls
                .borrow_mut()
                .push(format!("attach {}", self.name));
        }
        fn detach(&mut self, _: &mut Context) {
            self.calls
                .borrow_mut()
                .push(format!("detach {}", self.name));
        }
        fn tick(&mut self, _: &mut World, _: Tick) {
            self.calls.borrow_mut().push(format!("tick {}", self.name));
        }
        fn update(&mut self, _: &mut Context, _: Duration) {
            self.calls
                .borrow_mut()
                .push(format!("update {}", self.name));
        }
        fn event(&mut self, _: &mut Context, _: &Event<'_>) -> ControlFlow<()> {
            self.calls.borrow_mut().push(format!("event {}", self.name));
            if self.handles {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }
    }

    /// Spawns an entity as it attaches, at each tick, and as it detaches; and as it updates, if
    /// `rogue`.
    struct Spawner {
        rogue: bool,
    }

    impl Layer for Spawner {
        fn attach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Transform::IDENTITY);
        }
        fn detach(&mut self, cx: &mut Context) {
            cx.world_mut().spawn(Transform::IDENTITY);
        }
        fn tick(&mut self, world: &mut World, _: Tick) {
            world.spawn(Transform::IDENTITY);
        }
        fn update(&mut self, cx: &mut Context, _: Duration) {
            if self.rogue {
                cx.world_mut().spawn(Transform::IDENTITY);
            }
        }
    }

    /// Notes, for each key event it's sent, whether the input state shows that key held.
    struct Poll(Rc<RefCell<Vec<bool>>>);

    impl Layer for Poll {
        fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
            if let Event::Key { key, .. } = *event {
                self.0.borrow_mut().push(cx.input().key_down(key));
            }
            ControlFlow::Continue(())
        }
    }

    /// Pushes, in this order: overlay `o`, layer `a`, layer `b` (which handles events if
    /// `b_handles`), and returns the stack, bottom to top `a b o`, with its call record.
    fn probes(b_handles: bool) -> (Stack, Rc<RefCell<Vec<String>>>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let probe = |name, handles| {
            Box::new(Probe {
                name,
                handles,
                calls: calls.clone(),
            })
        };
        let mut stack = Stack::default();
        stack.push_overlay(probe("o", false));
        stack.push_layer(probe("a", false));
        stack.push_layer(probe("b", b_handles));
        (stack, calls)
    }

    #[test]
    fn lifecycle_goes_up_the_stack_and_detach_comes_back_down() {
        let (mut stack, calls) = probes(false);
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        stack.tick(&mut cx, Tick { number: 0 });
        stack.update(&mut cx, Duration::ZERO);
        stack.detach(&mut cx);
        assert_eq!(
            *calls.borrow(),
            [
                "attach a", "attach b", "attach o", "tick a", "tick b", "tick o", "update a",
                "update b", "update o", "detach o", "detach b", "detach a"
            ]
        );
    }

    #[test]
    fn layers_change_the_world_as_they_attach_and_detach_and_at_their_ticks() {
        let mut stack = Stack::default();
        stack.push_layer(Box::new(Spawner { rogue: false }));
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        stack.tick(&mut cx, Tick { number: 0 });
        stack.update(&mut cx, Duration::ZERO);
        stack.detach(&mut cx);
        assert_eq!(cx.world().query::<&Transform>().count(), 3);
    }

    #[test]
    #[should_panic(expected = "only as it attaches or detaches")]
    fn a_layer_changing_the_world_as_it_updates_is_a_bug() {
        let mut stack = Stack::default();
        stack.push_layer(Box::new(Spawner { rogue: true }));
        let mut cx = Context::new(None);
        stack.attach(&mut cx);
        stack.update(&mut cx, Duration::ZERO);
    }

    #[test]
    fn events_go_down_the_stack_until_a_layer_handles_them() {
        let mut cx = Context::new(None);
        let (mut stack, calls) = probes(false);
        assert_eq!(
            stack.event(&mut cx, &Event::CursorEntered),
            ControlFlow::Continue(())
        );
        assert_eq!(*calls.borrow(), ["event o", "event b", "event a"]);

        let (mut stack, calls) = probes(true);
        assert_eq!(
            stack.event(&mut cx, &Event::CursorEntered),
            ControlFlow::Break(())
        );
        assert_eq!(*calls.borrow(), ["event o", "event b"]);
    }

    #[test]
    fn the_input_state_takes_each_event_before_the_layers() {
        let held = Rc::new(RefCell::new(Vec::new()));
        let mut stack = Stack::default();
        stack.push_layer(Box::new(Poll(held.clone())));
        let mut cx = Context::new(None);
        for pressed in [true, false] {
            let key = Event::Key {
                key: Key::Space,
                pressed,
                repeat: false,
            };
            let _ = stack.event(&mut cx, &key);
        }
        assert_eq!(*held.borrow(), [true, false]);
    }

    #[test]
    fn a_sprite_sheet_loads_only_if_its_cells_fit_its_image() {
        let mut textures = Textures::default();
        let path = Path::new("assets/tiles.png");
        let image = || Image {
            size: [64, 32],
            pixels: vec![0; 64 * 32 * 4],
        };
        for cell in [[0, 16], [16, 0], [65, 16], [16, 33]] {
            let error = add_sheet(&mut textures, "tiles", path, image(), cell)
                .expect_err("cells that don't fit");
            assert_eq!(error.code(), "engine.bad_sprite_sheet", "{cell:?}");
            let message = error.to_string();
            assert!(
                message.contains("\"tiles\"")
                    && message.contains(&path.display().to_string())
                    && message.contains("64x32"),
                "{message}"
            );
        }
        let sheet = add_sheet(&mut textures, "tiles", path, image(), [64, 32]).expect("a sheet");
        assert_eq!((sheet.size, sheet.cell), ([64, 32], [64, 32]));
        // The sheets that didn't load took no texture, so this one's comes first after the
        // engine's white one.
        let first = Textures::default().add("first", [1, 1], &[0; 4]);
        assert_eq!(sheet.texture, first.expect("a texture"));
    }
}
