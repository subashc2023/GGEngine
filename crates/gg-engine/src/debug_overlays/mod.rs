//! Debug overlays: what a development build shows over a game's view as the key below Esc's bar,
//! `--overlays` and the tool API's `overlays` turn each on.
//!
//! [`DebugOverlays`] is the layer an app adds ([`App::debug_overlays`]). It shows the bar, the
//! Stats menu's numbers in corners of the view, and the Inspect menu's panels, the Inspector's
//! through an [`Inspector`] it holds. The World menu's lines draw with the world, as colliders'
//! outlines do, so a capture shows them; the Render menu's choices go with the look's. Which are
//! on is the context's [`State`], kept between runs beside the dock's layout. A build without
//! `dev` has `none.rs` in its place.
//!
//! [`App::debug_overlays`]: crate::App::debug_overlays

mod bar;
pub(crate) mod state;
mod stats;
#[cfg(test)]
mod tests;
pub(crate) mod world;

use std::ops::ControlFlow;
use std::time::Duration;

use gg_core::math::Vec4;

pub(crate) use state::{Overlay, State, keep, start};

use crate::inspector::Showing;
use crate::{
    Commands, Context, Event, Inspector, Key, Layer, MouseButton, PerfPanel, Pick, Tick, Ui, World,
};

/// The outline round what the Inspector has picked: linear RGBA and view pixels.
const PICKED: Vec4 = Vec4::new(1.0, 0.55, 0.1, 1.0);
const PICKED_WIDTH: f32 = 2.0;

/// A development build's debug overlays over a game's view, which an app adds as its top overlay
/// ([`App::debug_overlays`](crate::App::debug_overlays)).
///
/// - The key below Esc opens and closes a bar of menus: Stats, Inspect, World, Render and Game,
///   each a list of overlays to turn on and off, then All off. The key doesn't reach the layers
///   below, and does nothing while a text field has the keys.
/// - Stats: numbers at the view's top left, below the bar while it shows, and the log's at its
///   bottom left. Clicks pass through them.
/// - Inspect: the Performance, Entities and Inspector panels, floating over the view. With the bar
///   open and Entities or Inspector on, a left click in the view picks what's drawn there for the
///   Inspector (Ctrl, Cmd on macOS, adds or removes it), which outlines it. The click doesn't reach
///   the layers below.
/// - World: lines and labels drawn with the world, which `gg capture` shows.
/// - Render: a part of shading in place of the view's light, and passes to leave out.
/// - Game: the game's own, which [`Context::debug_overlay`] names as it asks whether each is on.
///
/// Which are on is kept for the app's next runs, unless `--overlays` names them. The tool API's
/// `overlays` reads and sets them. A build without `dev` has a layer that does nothing.
#[derive(Default)]
pub struct DebugOverlays {
    inspector: Inspector,
    /// A click's pick, until its answer comes, and whether it adds or removes rather than picks.
    picking: Option<(Pick, bool)>,
    stats: stats::Stats,
}

impl DebugOverlays {
    /// Whether a click in the view picks for the Inspector: with the bar open and Entities or
    /// Inspector on.
    fn picks(cx: &Context) -> bool {
        let state = &cx.dev.overlays;
        state.bar && (state.on(Overlay::Entities) || state.on(Overlay::Inspector))
    }

    /// Hands the Inspector the answer to a click's pick, once it comes.
    fn take_pick(&mut self, cx: &Context) {
        let Some((ticket, toggles)) = self.picking else {
            return;
        };
        let Some(&(_, id)) = cx.picked().iter().find(|(pick, _)| *pick == ticket) else {
            return;
        };
        self.picking = None;
        let link = self.inspector.link();
        match id {
            Some(id) if toggles => link.toggle(id),
            id => link.pick(id),
        }
    }
}

impl Layer for DebugOverlays {
    fn attach(&mut self, cx: &mut Context) {
        cx.dev.overlays.engine = true;
        self.inspector.attach(cx);
    }

    fn restored(&mut self, cx: &mut Context) {
        cx.dev.overlays.engine = true;
        self.inspector.restored(cx);
    }

    fn swapped(&mut self, cx: &mut Context) {
        self.inspector.swapped(cx);
    }

    fn detach(&mut self, cx: &mut Context) {
        self.inspector.detach(cx);
    }

    fn tick(&mut self, world: &mut World, tick: Tick) {
        self.inspector.tick(world, tick);
    }

    /// The Inspector's: a recording keeps them under this layer.
    fn commands(&mut self) -> Option<&mut dyn Commands> {
        self.inspector.commands()
    }

    fn update(&mut self, cx: &mut Context, dt: Duration) {
        self.take_pick(cx);
        let state = &cx.dev.overlays;
        if (state.on(Overlay::Entities) || state.on(Overlay::Inspector))
            && let Some(id) = self.inspector.link().picked()
        {
            cx.outline(id, PICKED, PICKED_WIDTH);
        }
        self.stats.update(cx, dt);
    }

    fn event(&mut self, cx: &mut Context, event: &Event<'_>) -> ControlFlow<()> {
        self.stats.event(event);
        match *event {
            Event::Key {
                key: Key::Backquote,
                pressed: true,
                repeat,
            } => {
                if !repeat {
                    cx.dev.overlays.bar = !cx.dev.overlays.bar;
                }
                ControlFlow::Break(())
            }
            Event::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            } if Self::picks(cx) => {
                let toggles = cx.input().command_down();
                if let Some(at) = cx.input().cursor()
                    && let Some(ticket) = cx.pick(at)
                {
                    self.picking = Some((ticket, toggles));
                }
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        }
    }

    fn ui(&mut self, cx: &mut Context, ui: &mut Ui<'_>) {
        let top = match cx.dev.overlays.bar {
            true => bar::show(&mut cx.dev.overlays, ui.egui()),
            false => 0.0,
        };
        self.stats.show(cx, ui.egui(), top);
        let state = &cx.dev.overlays;
        if state.on(Overlay::Performance) {
            PerfPanel.ui(cx, ui);
        }
        let state = &cx.dev.overlays;
        let (entities, inspector) = (state.on(Overlay::Entities), state.on(Overlay::Inspector));
        if entities || inspector {
            self.inspector.showing = Showing {
                entities,
                inspector,
                menus: false,
            };
            self.inspector.ui(cx, ui);
        }
        keep(cx);
    }
}
