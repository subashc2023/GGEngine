//! Where an app docks one of its panels as it first shows, which [`App::dock`](crate::App::dock)
//! takes: in development builds, and in builds without `dev` too, which have no dock and ignore it,
//! so that an app's `main` builds in both.

/// Where an app docks one of its panels as it first shows, as [`App::dock`](crate::App::dock)
/// gives it: beside the panel named, or among its tabs, while that panel is docked in the main
/// window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Docked {
    /// Left of the panel named, taking a quarter of the view's panel or half of another's.
    LeftOf(&'static str),
    /// Right of the panel named, taking a quarter of the view's panel or half of another's.
    RightOf(&'static str),
    /// Above the panel named, taking a quarter of the view's panel or half of another's.
    Above(&'static str),
    /// Below the panel named, taking a quarter of the view's panel or half of another's.
    Below(&'static str),
    /// Among the tabs of the panel named, whose group goes on showing the tab it showed.
    With(&'static str),
}
