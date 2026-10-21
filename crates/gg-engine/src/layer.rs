//! The layer stack: how an app's code, and the engine's tools, plug into the main loop.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use crate::Event;

/// A part of an app that the main loop drives. Every method has a default that does nothing,
/// so a layer implements only what it uses.
pub trait Layer {
    /// Called once as the app starts, before the first update.
    fn attach(&mut self, _cx: &mut Context) {}

    /// Called once as the app ends.
    fn detach(&mut self, _cx: &mut Context) {}

    /// Called every frame, with the time since the last one (zero for the first).
    fn update(&mut self, _cx: &mut Context, _dt: Duration) {}

    /// Called for each event that the layers above didn't handle. `Break` handles it, so the
    /// layers below don't see it.
    fn event(&mut self, _cx: &mut Context, _event: &Event<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    /// Called every frame, after the updates, to draw UI with egui.
    fn ui(&mut self, _cx: &mut Context, _ui: &egui::Context) {}
}

/// What layers can see and change of the running app.
pub struct Context {
    window: Option<Window>,
    clear_color: [f32; 4],
}

impl Context {
    pub(crate) fn new(window: Option<Window>) -> Self {
        Self {
            window,
            clear_color: [0.02, 0.02, 0.02, 1.0],
        }
    }

    /// The app's window; `None` in a headless run.
    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }

    /// The color each frame clears to, in linear RGBA.
    pub fn clear_color(&self) -> [f32; 4] {
        self.clear_color
    }

    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
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
    pub(crate) fn ui(&mut self, cx: &mut Context, ui: &egui::Context) {
        for layer in &mut self.layers {
            layer.ui(cx, ui);
        }
    }

    /// Hands `event` down from the top until a layer handles it. `Break` means one did.
    pub(crate) fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
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
}
