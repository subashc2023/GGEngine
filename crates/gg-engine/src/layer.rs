//! The layer stack: how an app's code, and the engine's tools, plug into the main loop.

use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytemuck::NoUninit;
use gg_core::Result;
use gg_core::math::Mat4;
use gg_renderer::{
    Blend, Camera, Draws, Mesh, Meshes, RenderData, Shader, Shaders, Texture, Textures, Viewport,
};

use crate::{Event, Input, Ui, image};

/// A part of an app that the main loop drives. Every method has a default that does nothing,
/// so a layer implements only what it uses.
pub trait Layer {
    /// Called once as the app starts, before the first update.
    fn attach(&mut self, _cx: &mut Context) {}

    /// Called once as the app ends.
    fn detach(&mut self, _cx: &mut Context) {}

    /// Called every frame, with the time since the last one: zero for the first, and for the
    /// first after the window is restored from being minimized, so that the pause doesn't count
    /// as one long frame.
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
}

impl Context {
    pub(crate) fn new(window: Option<Window>) -> Self {
        Self {
            window,
            input: Input::default(),
            clear_color: [0.02, 0.02, 0.02, 1.0],
            camera: Camera::default(),
            meshes: Meshes::default(),
            shaders: Shaders::default(),
            textures: Textures::default(),
            draws: Draws::default(),
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
    /// entry points `vertex_main` and `fragment_main`; push constants that hold the matrix from
    /// the mesh's space to clip space, the address of the vertices, which it reads as `V` is laid
    /// out, and the address of the draw's parameters, which it reads as `P` is; and textures
    /// sampled from the bindless set by the handles in its parameters.
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

    /// Loads the PNG file at `path` as a texture for layers' shaders to sample from then on,
    /// through the handle, which is a field of their parameters as it is. Any PNG loads: its
    /// colors are taken as sRGB-encoded and its alpha as not multiplied into them, and shaders
    /// sample its colors decoded to linear, with texture coordinates from (0, 0) at its top left to
    /// (1, 1) at its bottom right. `name` names the texture in errors and GPU captures. The GPU has
    /// it from the next frame that renders.
    ///
    /// Fails with `engine.bad_texture` if the file can't be read or doesn't decode as a PNG, with
    /// `renderer.bad_texture` if a side is more than 8,192 pixels, the most every Vulkan 1.4
    /// device takes, and with `renderer.too_many_textures` past the 61,440 a renderer holds.
    pub fn add_texture(&mut self, name: &str, path: impl AsRef<Path>) -> Result<Texture> {
        let image = image::load_png(name, path.as_ref())?;
        self.textures.add(name, image.size, &image.pixels)
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
    pub fn draw<V, P: NoUninit>(
        &mut self,
        mesh: Mesh<V>,
        shader: Shader<V, P>,
        transform: Mat4,
        params: P,
    ) {
        self.draws.push(mesh, shader, transform, params);
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
        for layer in &mut self.layers {
            layer.attach(cx);
        }
    }

    /// Detaches from the top down, the reverse of attaching.
    pub(crate) fn detach(&mut self, cx: &mut Context) {
        for layer in self.layers.iter_mut().rev() {
            layer.detach(cx);
        }
    }

    pub(crate) fn update(&mut self, cx: &mut Context, dt: Duration) {
        for layer in &mut self.layers {
            layer.update(cx, dt);
        }
    }

    /// Draws UI from the bottom up, so the layers above draw over those below.
    pub(crate) fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
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
    use crate::Key;

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
        stack.update(&mut cx, Duration::ZERO);
        stack.detach(&mut cx);
        assert_eq!(
            *calls.borrow(),
            [
                "attach a", "attach b", "attach o", "update a", "update b", "update o", "detach o",
                "detach b", "detach a"
            ]
        );
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
}
