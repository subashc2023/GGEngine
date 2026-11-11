//! Where an app docks one of its panels as it first shows, which [`App::dock`](crate::App::dock)
//! takes: in development builds, and in builds without `dev` too, which have no dock and ignore it,
//! so that an app's `main` builds in both.

/// Where an app docks one of its panels as it first shows, as [`App::dock`](crate::App::dock)
/// gives it: beside the panel named, taking a share of its place, from 0 to 1, or among its tabs,
/// while that panel is docked in the main window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Docked {
    /// Left of the panel named, taking the share of its width given.
    LeftOf(&'static str, f32),
    /// Right of the panel named, taking the share of its width given.
    RightOf(&'static str, f32),
    /// Above the panel named, taking the share of its height given.
    Above(&'static str, f32),
    /// Below the panel named, taking the share of its height given.
    Below(&'static str, f32),
    /// Among the tabs of the panel named, whose group goes on showing the tab it showed.
    With(&'static str),
}
