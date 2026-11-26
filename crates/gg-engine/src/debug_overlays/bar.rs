//! The overlays' bar: a menu for each group of overlays across the top of the game's view.

use egui::containers::menu::{MenuButton, MenuConfig};
use egui::{Align2, Area, Frame, Id, Order, PopupCloseBehavior};
use gg_api::{OverlayGroup, Pass};

use super::state::{CHANNELS, Overlay, PASSES, State};

/// Shows the bar over the top of the view, its menus changing `state`. Returns how far down from
/// the window's top it reaches, in points.
pub(super) fn show(state: &mut State, egui: &egui::Context) -> f32 {
    let area = Area::new(Id::unique("gg debug overlays bar"))
        .anchor(Align2::CENTER_TOP, [0.0, 6.0])
        .order(Order::Foreground);
    let shown = area.show(egui, |ui| {
        Frame::popup(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                menu(ui, "Stats", |ui| group(ui, state, OverlayGroup::Stats));
                menu(ui, "Inspect", |ui| group(ui, state, OverlayGroup::Inspect));
                menu(ui, "World", |ui| group(ui, state, OverlayGroup::World));
                menu(ui, "Render", |ui| render(ui, state));
                menu(ui, "Game", |ui| game(ui, state));
                ui.separator();
                if ui.button("All off").clicked() {
                    state.all_off();
                }
                ui.weak("` closes");
            });
        });
    });
    shown.response.rect.bottom()
}

/// A menu of the bar's, which stays open as its boxes are ticked.
fn menu(ui: &mut egui::Ui, name: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let config = MenuConfig::new()
        .style(crate::menu_style)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside);
    MenuButton::new(name).config(config).ui(ui, add_contents);
}

/// A box for each of the engine's overlays in `group`.
fn group(ui: &mut egui::Ui, state: &mut State, group: OverlayGroup) {
    for overlay in Overlay::ALL.into_iter().filter(|o| o.group() == group) {
        let mut on = state.on(overlay);
        if ui
            .checkbox(&mut on, overlay.name())
            .on_hover_text(overlay.tip())
            .changed()
        {
            state.set(overlay, on);
        }
    }
}

/// The view's light or a part of shading in its place, then a box for each pass.
fn render(ui: &mut egui::Ui, state: &mut State) {
    let mut channel = state.channel;
    let tip = "What the view shows in place of its light; the look's `show` wins over it";
    ui.label("Show").on_hover_text(tip);
    ui.radio_value(&mut channel, None, "Light");
    for (each, name) in CHANNELS {
        ui.radio_value(&mut channel, Some(each), name);
    }
    state.set_channel(channel);
    ui.separator();
    let tip = "The passes the view runs; a pass it doesn't run shows nothing either way";
    ui.label("Passes").on_hover_text(tip);
    for (pass, name) in Pass::ALL.into_iter().zip(PASSES) {
        let mut runs = !state.skip.contains(&pass);
        if ui.checkbox(&mut runs, name).changed() {
            state.set_skipped(pass, !runs);
        }
    }
}

/// A box for each of the game's overlays, in the order it first asked for each.
fn game(ui: &mut egui::Ui, state: &mut State) {
    if state.game().is_empty() {
        let tip =
            "A game lists its own as it asks `Context::debug_overlay(name)` whether each is on";
        ui.weak("None yet").on_hover_text(tip);
        return;
    }
    for index in 0..state.game().len() {
        let (name, mut on) = (state.game()[index].0.clone(), state.game()[index].1);
        if ui.checkbox(&mut on, name).changed() {
            state.set_game(index, on);
        }
    }
}
