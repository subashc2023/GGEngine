//! Where a panel first docks, for [`App::dock`](crate::App::dock). Builds without `dev` ignore it,
//! so an app's `main` builds in both.

/// Where a panel first docks, from [`App::dock`](crate::App::dock): beside the named panel, taking
/// a share from 0 to 1 of its place, or among its tabs. Applies while that panel is docked in the
/// main window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Docked {
    /// Left of the named panel, taking this share of its width.
    LeftOf(&'static str, f32),
    /// Right of the named panel, taking this share of its width.
    RightOf(&'static str, f32),
    /// Above the named panel, taking this share of its height.
    Above(&'static str, f32),
    /// Below the named panel, taking this share of its height.
    Below(&'static str, f32),
    /// Among the named panel's tabs; its group keeps showing the tab it showed.
    With(&'static str),
}
