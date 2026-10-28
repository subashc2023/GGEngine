//! Profiling: zones, plots and frame marks for Tracy, which compile to nothing unless gg-core's
//! `tracy` feature is on. A zone times the code from where it starts to where its guard drops,
//! and Tracy lays each frame's zones out on a timeline. Zones are for work of about 10 µs or more,
//! never one for each draw, sprite or other item of a loop over a frame's content, which would
//! cost more than it measures.
//!
//! The macros are defined here twice, under gg-core's feature, so that it decides for every crate
//! that calls them. tracy-client's macros expand to `unsafe` blocks, sound for the literal names
//! these take; rustc doesn't lint code expanded from another crate's macro, so callers that deny
//! or forbid unsafe code still compile.

use std::fmt::Display;

/// Whether zones reach Tracy: gg-core's `tracy` feature.
pub const ENABLED: bool = cfg!(feature = "tracy");

#[doc(hidden)]
#[cfg(feature = "tracy")]
pub use tracy_client as __tracy;

/// A zone, which ends when this drops.
#[must_use = "the zone ends when this drops; bind it to `_zone`, not `_`"]
pub struct Zone {
    #[cfg(feature = "tracy")]
    span: tracy_client::Span,
    /// Neither `Send` nor `Sync`, as Tracy's zones aren't, so that both builds take the same code.
    #[cfg(not(feature = "tracy"))]
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Zone {
    #[doc(hidden)]
    #[cfg(feature = "tracy")]
    pub fn __span(span: tracy_client::Span) -> Self {
        Self { span }
    }

    #[doc(hidden)]
    #[cfg(not(feature = "tracy"))]
    pub fn __off() -> Self {
        Self {
            _not_send: std::marker::PhantomData,
        }
    }

    /// Shows `text` with the zone, such as the path of the file it loads. Tracy copies it each
    /// time, so it's for zones that run rarely, or a few times a tick at most; without the feature,
    /// it isn't even formatted.
    pub fn text(&self, text: impl Display) {
        #[cfg(feature = "tracy")]
        self.span.emit_text(&text.to_string());
        #[cfg(not(feature = "tracy"))]
        let _ = text;
    }
}

/// Ends a frame in Tracy's list of frames, and starts the next.
pub fn frame_mark() {
    #[cfg(feature = "tracy")]
    tracy_client::frame_mark();
}

/// Starts a zone, which ends when the guard it returns drops. `let _zone = zone!();` names it
/// after the function it's in, such as `gg_gpu::device::Device::new`; `zone!("wait for the GPU")`
/// gives it a name of its own, a literal, since Tracy keeps every name it's given.
#[cfg(feature = "tracy")]
#[macro_export]
macro_rules! zone {
    () => {
        $crate::profile::Zone::__span($crate::profile::__tracy::span!())
    };
    ($name:literal) => {
        $crate::profile::Zone::__span($crate::profile::__tracy::span!($name))
    };
}

/// Starts a zone, which ends when the guard it returns drops. `let _zone = zone!();` names it
/// after the function it's in, such as `gg_gpu::device::Device::new`; `zone!("wait for the GPU")`
/// gives it a name of its own, a literal, since Tracy keeps every name it's given.
#[cfg(not(feature = "tracy"))]
#[macro_export]
macro_rules! zone {
    () => {
        $crate::profile::Zone::__off()
    };
    ($name:literal) => {{
        let _: &'static str = $name;
        $crate::profile::Zone::__off()
    }};
}

/// Adds `value`, an `f64`, to the plot named `name`, a literal, which Tracy draws under the
/// frames, such as `plot!("draws", draws as f64)`.
#[cfg(feature = "tracy")]
#[macro_export]
macro_rules! plot {
    ($name:literal, $value:expr) => {
        $crate::profile::__tracy::plot!($name, $value)
    };
}

/// Adds `value`, an `f64`, to the plot named `name`, a literal, which Tracy draws under the
/// frames, such as `plot!("draws", draws as f64)`. Without the feature, `value` is checked but
/// not evaluated.
#[cfg(not(feature = "tracy"))]
#[macro_export]
macro_rules! plot {
    ($name:literal, $value:expr) => {{
        let _: &'static str = $name;
        let _ = || -> f64 { $value };
    }};
}
