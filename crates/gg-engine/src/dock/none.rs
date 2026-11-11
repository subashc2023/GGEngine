//! What a build without `dev`, which has no UI, has of the dock: the items games name, so that a
//! game's code, its layers' `ui` among it, compiles unchanged in both builds and draws nothing in
//! this one. [`Ui`] is a type no value of which exists, with a development build's methods and
//! their signatures, which never run: the engine never calls [`Layer::ui`](crate::Layer::ui), as
//! it has nothing to call it with. [`Docked`] is a development build's, which
//! [`App::dock`](crate::App::dock) takes and ignores.

use std::convert::Infallible;
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;

use egui::{Align2, Response, Vec2};
use gg_renderer::Viewport;

#[path = "docked.rs"]
mod docked;

pub use docked::Docked;

/// What layers draw their UI through, in [`Layer::ui`](crate::Layer::ui), in a development build;
/// in this one, which has no UI, a type no value of which can exist.
pub struct Ui<'a> {
    never: Infallible,
    /// What a development build's holds for `'a`: egui's UI and the dock.
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
}
