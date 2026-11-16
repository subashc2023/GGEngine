//! Tracy's zones, plots and frame marks, compiled out without the `tracy` feature. Zones are for
//! work of about 10 µs or more, never one per item of a frame's content: that costs more than it
//! measures.
//!
//! Each macro is defined twice, by gg-core's feature, so that gg-core decides for every caller.
//! tracy-client's macros expand to `unsafe` blocks, sound for the literal names these take. rustc
//! doesn't lint another crate's macro expansions, so callers that forbid `unsafe_code` compile.

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
    /// `!Send` and `!Sync`, as Tracy's zones are, so both builds accept the same code.
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

    /// Shows `text` with the zone. Tracy copies it each time, so it's for a few times a tick at
    /// most. Without the feature, it isn't formatted.
    pub fn text(&self, text: impl Display) {
        #[cfg(feature = "tracy")]
        self.span.emit_text(&text.to_string());
        #[cfg(not(feature = "tracy"))]
        let _ = text;
    }
}

/// Ends a frame in Tracy, starting the next.
pub fn frame_mark() {
    #[cfg(feature = "tracy")]
    tracy_client::frame_mark();
}

/// Starts a zone that ends as its guard drops. `let _zone = zone!();` names it after its function,
/// and `zone!("wait for the GPU")` by a literal, as Tracy keeps every name.
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

/// Starts a zone that ends as its guard drops. `let _zone = zone!();` names it after its function,
/// and `zone!("wait for the GPU")` by a literal, as Tracy keeps every name.
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

/// Adds an `f64` to a plot Tracy draws under the frames: `plot!("draws", draws as f64)`.
#[cfg(feature = "tracy")]
#[macro_export]
macro_rules! plot {
    ($name:literal, $value:expr) => {
        $crate::profile::__tracy::plot!($name, $value)
    };
}

/// Adds an `f64` to a plot Tracy draws under the frames: `plot!("draws", draws as f64)`. Without
/// the feature, the value is checked but not evaluated.
#[cfg(not(feature = "tracy"))]
#[macro_export]
macro_rules! plot {
    ($name:literal, $value:expr) => {{
        let _: &'static str = $name;
        let _ = || -> f64 { $value };
    }};
}

/// The name of some GPU work, such as a pass, from [`label!`](crate::label): its debug label, and
/// with the feature its zone on Tracy's GPU timeline.
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

/// Tracy's timeline of one GPU queue, its zones timed by timestamps the GPU writes into queries.
pub struct GpuTimeline {
    #[cfg(feature = "tracy")]
    context: tracy_client::GpuContext,
}

impl GpuTimeline {
    /// A timeline for a GPU whose clock read `timestamp` just now and ticks every `period`
    /// nanoseconds. `None` without the feature, or past Tracy's 255 in a process.
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

    /// Begins a zone at the timestamp the GPU writes into `query`, which no zone awaiting its times
    /// may hold.
    pub fn begin(&self, label: Label, query: u16) {
        #[cfg(feature = "tracy")]
        self.context.begin_span(label.location, query);
        #[cfg(not(feature = "tracy"))]
        let _ = (label, query);
    }

    /// Ends the zone begun last at the timestamp the GPU writes into `query`.
    pub fn end(&self, query: u16) {
        #[cfg(feature = "tracy")]
        self.context.end_span(query);
        #[cfg(not(feature = "tracy"))]
        let _ = query;
    }

    /// Gives `query`'s `timestamp`, in the order the GPU wrote them.
    pub fn time(&self, query: u16, timestamp: i64) {
        #[cfg(feature = "tracy")]
        self.context.upload_gpu_timestamp(query, timestamp);
        #[cfg(not(feature = "tracy"))]
        let _ = (query, timestamp);
    }

    /// Realigns the timeline with Tracy's clock against drift: the GPU's read `timestamp` just now.
    pub fn sync(&self, timestamp: i64) {
        #[cfg(feature = "tracy")]
        self.context.sync_gpu_time(timestamp);
        #[cfg(not(feature = "tracy"))]
        let _ = timestamp;
    }
}
