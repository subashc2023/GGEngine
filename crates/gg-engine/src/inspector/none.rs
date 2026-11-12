//! What a build without `dev`, which has no UI, has of the Inspector: the items games name, so that
//! a game's code compiles unchanged in both builds. The Inspector is a layer with [`Layer`]'s
//! defaults, which does nothing, and its link a handle whose calls change nothing; [`Placed`] is a
//! development build's, which the link never gives.

use crate::reflect::{Ident, Value};
use crate::{EntityId, Layer};

#[path = "placed.rs"]
mod placed;

pub use placed::Placed;

/// The Entities and Inspector panels in a development build, which an app adds as an overlay,
/// `.overlay(gg_engine::Inspector::default())`; in this one, which has no UI, a layer that does
/// nothing. A game makes one through `default` alone, as in a development build.
#[derive(Default)]
#[non_exhaustive]
pub struct Inspector;

impl Layer for Inspector {}

impl Inspector {
    /// A link to the Inspector, as [`InspectorLink`] says.
    pub fn link(&self) -> InspectorLink {
        InspectorLink
    }
}

/// A link to an [`Inspector`], through which another layer picks and edits as its panels do, in a
/// development build; in this one, which has no panels, a handle whose calls change nothing:
/// nothing is ever picked, placed or framed (`None`, or no picks), nothing waits (`waiting` is
/// false, `idle` true), nothing is stepped through (0), and nothing is pending.
#[derive(Clone)]
#[non_exhaustive]
pub struct InspectorLink;

impl InspectorLink {
    pub fn picked(&self) -> Option<EntityId> {
        None
    }

    pub fn picks(&self) -> Vec<EntityId> {
        Vec::new()
    }

    pub fn pick(&self, _: Option<EntityId>) {}

    pub fn pick_all(&self, _: &[EntityId]) {}

    pub fn toggle(&self, _: EntityId) {}

    pub fn set(&self, _: EntityId, _: &str, _: &str, _: Value) {}

    pub fn create(&self, _: Vec<(Ident, Value)>) {}

    pub fn seal(&self) {}

    pub fn waiting(&self) -> bool {
        false
    }

    pub fn idle(&self) -> bool {
        true
    }

    pub fn placed(&self) -> Option<Placed> {
        None
    }

    pub fn along(&self) -> Option<Vec<Placed>> {
        None
    }

    pub fn framed(&self) -> Option<EntityId> {
        None
    }

    pub fn stepped(&self) -> u64 {
        0
    }

    #[doc(hidden)]
    pub fn pending(&self) -> Vec<Option<(EntityId, String, String, Value)>> {
        Vec::new()
    }

    #[doc(hidden)]
    pub fn creates(&self) -> Vec<Vec<(Ident, Value)>> {
        Vec::new()
    }
}
