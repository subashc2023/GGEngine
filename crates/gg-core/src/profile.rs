//! Profiling: zones, plots and frame marks for Tracy, which compile to nothing unless gg-core's
//! `tracy` feature is on. A zone times the code from where it starts to where its guard drops,
//! and Tracy lays each frame's zones out on a timeline. Zones are for work of about 10 µs or more,
//! never one for each draw, sprite or other item of a loop over a frame's content, which would
//! cost more than it measures.
//!
//! The GPU's work shows on a timeline of its own: a pass, named by a [`Label`], is a zone there,
//! timed by the GPU's own clock, which a [`GpuTimeline`] takes once the GPU has run it.
//!
//! The macros are defined here twice, under gg-core's feature, so that it decides for every crate
//! that calls them. tracy-client's macros expand to `unsafe` blocks, sound for the literal names
//! these take; rustc doesn't lint code expanded from another crate's macro, so callers that deny
//! or forbid unsafe code still compile.

use std::ffi::CStr;
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

/// The name of some of the GPU's work, such as a pass, which [`label!`](crate::label) makes from a
/// literal: what captures show it as, its debug label, and, with the feature, where Tracy's GPU
/// timeline shows it, a zone named so.
#[derive(Clone, Copy)]
pub struct Label {
    name: &'static CStr,
    #[cfg(feature = "tracy")]
    location: &'static tracy_client::SpanLocation,
}

impl Label {
    #[doc(hidden)]
    #[cfg(feature = "tracy")]
    pub fn __new(name: &'static CStr, location: &'static tracy_client::SpanLocation) -> Self {
        Self { name, location }
    }

    #[doc(hidden)]
    #[cfg(not(feature = "tracy"))]
    pub fn __new(name: &'static CStr) -> Self {
        Self { name }
    }

    /// The name, as debug labels take it.
    pub fn name(&self) -> &'static CStr {
        self.name
    }
}

impl std::fmt::Debug for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(f)
    }
}

impl PartialEq for Label {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

/// A [`Label`] named `name`, a literal: `label!("view")`.
#[cfg(feature = "tracy")]
#[macro_export]
macro_rules! label {
    ($name:literal) => {{
        const NAME: &::core::ffi::CStr = $crate::profile::__c_name(concat!($name, "\0"));
        $crate::profile::Label::__new(NAME, $crate::profile::__tracy::span_location!($name))
    }};
}

/// A [`Label`] named `name`, a literal: `label!("view")`.
#[cfg(not(feature = "tracy"))]
#[macro_export]
macro_rules! label {
    ($name:literal) => {{
        const NAME: &::core::ffi::CStr = $crate::profile::__c_name(concat!($name, "\0"));
        $crate::profile::Label::__new(NAME)
    }};
}

/// `name`, which ends in its only nul, as a C string.
#[doc(hidden)]
pub const fn __c_name(name: &'static str) -> &'static CStr {
    match CStr::from_bytes_with_nul(name.as_bytes()) {
        Ok(name) => name,
        Err(_) => panic!("a label's name has no nul of its own"),
    }
}

/// Tracy's timeline of one GPU queue's work, where each zone begins and ends at timestamps that
/// the GPU writes, numbered by the query each goes in, which come back once it has run them.
pub struct GpuTimeline {
    #[cfg(feature = "tracy")]
    context: tracy_client::GpuContext,
}

impl GpuTimeline {
    /// The timeline named `name` of a GPU whose clock read `timestamp` just now and ticks every
    /// `period` nanoseconds; `None` without the feature, or once Tracy has made all the 255 it
    /// makes in a process.
    pub fn new(name: &str, timestamp: i64, period: f32) -> Option<Self> {
        #[cfg(feature = "tracy")]
        {
            let client = tracy_client::Client::start();
            let kind = tracy_client::GpuContextType::Vulkan;
            let context = client.new_gpu_context(Some(name), kind, timestamp, period);
            context.ok().map(|context| Self { context })
        }
        #[cfg(not(feature = "tracy"))]
        {
            let _ = (name, timestamp, period);
            None
        }
    }

    /// A zone named `label` begins at the timestamp the GPU writes into `query`, which must be no
    /// other zone's that hasn't had its times yet.
    pub fn begin(&self, label: Label, query: u16) {
        #[cfg(feature = "tracy")]
        self.context.begin_span(label.location, query);
        #[cfg(not(feature = "tracy"))]
        let _ = (label, query);
    }

    /// The zone begun last ends at the timestamp the GPU writes into `query`.
    pub fn end(&self, query: u16) {
        #[cfg(feature = "tracy")]
        self.context.end_span(query);
        #[cfg(not(feature = "tracy"))]
        let _ = query;
    }

    /// The GPU wrote `timestamp` into `query`: given in the order the GPU wrote them.
    pub fn time(&self, query: u16, timestamp: i64) {
        #[cfg(feature = "tracy")]
        self.context.upload_gpu_timestamp(query, timestamp);
        #[cfg(not(feature = "tracy"))]
        let _ = (query, timestamp);
    }

    /// The GPU's clock read `timestamp` just now: Tracy places the timeline's timestamps from here
    /// on by it, against its own clock as this is called, so that the two don't drift apart.
    pub fn sync(&self, timestamp: i64) {
        #[cfg(feature = "tracy")]
        self.context.sync_gpu_time(timestamp);
        #[cfg(not(feature = "tracy"))]
        let _ = timestamp;
    }
}
