//! A build without `dev`: the dock items games name, which draw nothing.
//!
//! Game code, layers' `ui` included, compiles unchanged. [`Ui`] has no values and `dev`'s
//! signatures; its methods never run, as the engine has no `Ui` to give
//! [`Layer::ui`](crate::Layer::ui). [`Docked`] is `dev`'s, and [`App::dock`](crate::App::dock)
//! ignores it.

use std::convert::Infallible;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;

use egui::{Align2, Response, Vec2};
use gg_renderer::Viewport;

#[path = "docked.rs"]
mod docked;

pub use docked::Docked;

/// What [`Layer::ui`](crate::Layer::ui) draws through. Here no value can exist.
pub struct Ui<'a> {
    never: Infallible,
    /// Stands for what `dev`'s holds for `'a`: egui's UI and the dock.
    _ui: PhantomData<&'a mut egui::Ui>,
}

impl Ui<'_> {
    pub(crate) fn default_ui(&mut self, _: &'static str) {
        match self.never {}
    }

    pub fn panel(&mut self, _: &str, _: impl FnOnce(&mut egui::Ui)) {
        match self.never {}
    }

    pub fn menu(&mut self, _: &str, _: impl FnOnce(&mut egui::Ui)) {
        match self.never {}
    }

    pub fn title_bar<R>(&mut self, _: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
        match self.never {}
    }

    pub fn settings(&mut self, _: impl FnOnce(&mut egui::Ui)) {
        match self.never {}
    }

    pub fn over_view<R>(
        &mut self,
        _: impl Hash + Debug,
        _: Align2,
        _: Vec2,
        _: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<R> {
        match self.never {}
    }

    pub fn cover<R>(&mut self, _: impl FnOnce(&mut egui::Ui) -> R) -> R {
        match self.never {}
    }

    pub fn view(&self) -> Option<Response> {
        match self.never {}
    }

    pub fn viewport(&self) -> Option<Viewport> {
        match self.never {}
    }

    pub fn egui(&self) -> &egui::Context {
        match self.never {}
    }

    pub fn focus_view(&mut self) {
        match self.never {}
    }

    pub fn view_focused(&self) -> bool {
        match self.never {}
    }
}
