//! What a build without `dev` has of the debug overlays: a layer that does nothing.

use crate::{Context, Layer};

/// A development build's debug overlays; here, a layer that does nothing. It keeps the app's
/// layers, which a recording names, the same in both builds.
#[derive(Default)]
#[non_exhaustive]
pub struct DebugOverlays;

impl Layer for DebugOverlays {}

/// `--overlays` names what a development build shows; here, nothing.
pub(crate) fn start(_: &mut Context, _: Option<&[String]>) {}
