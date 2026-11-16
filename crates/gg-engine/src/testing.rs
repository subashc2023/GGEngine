//! What tests share: the shapes an egui pass paints (development builds, which have the UI), the
//! codes of logged events, and the lock that keeps core-hungry tests from running beside
//! clock-paced ones.

use std::fmt;
use std::sync::mpsc::Sender;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cfg(feature = "dev")]
use egui::Shape;
#[cfg(feature = "dev")]
use egui::epaint::ClippedShape;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{self, Layer};

/// The shapes a pass's `shapes` paint, in painting order, each [`Shape::Vec`] flattened in place.
#[cfg(feature = "dev")]
pub(crate) fn painted(shapes: &[ClippedShape]) -> Vec<&Shape> {
    fn add<'a>(shape: &'a Shape, painted: &mut Vec<&'a Shape>) {
        match shape {
            Shape::Vec(shapes) => {
                for shape in shapes {
                    add(shape, painted);
                }
            }
            shape => painted.push(shape),
        }
    }
    let mut painted = Vec::new();
    for clipped in shapes {
        add(&clipped.shape, &mut painted);
    }
    painted
}

/// Keeps core-hungry tests from running beside clock-paced ones (peers, a server and its clients,
/// a link), which tolerate a busy machine only so far. [`real_time`] holds it shared, [`alone`]
/// exclusive. It guards no data, so a panic while holding it leaves it usable.
static REAL_TIME: RwLock<()> = RwLock::new(());

/// Held for a whole test of peers, a server and its clients, or a link. These run beside each
/// other and the rest, never beside [`alone`]. Take it once, at the start: a second take, while
/// another test waits for [`alone`], may wait for good.
pub(crate) fn real_time() -> RwLockReadGuard<'static, ()> {
    REAL_TIME.read().unwrap_or_else(PoisonError::into_inner)
}

/// Held for a whole test that loads cores for seconds (a 3D world's physics and animation in a
/// debug build). It runs beside no [`real_time`] test: on a runner of few cores it would starve
/// their threads.
pub(crate) fn alone() -> RwLockWriteGuard<'static, ()> {
    REAL_TIME.write().unwrap_or_else(PoisonError::into_inner)
}

/// Sends the code of each event that reaches it.
pub(crate) struct Codes(pub(crate) Sender<String>);

impl<S: Subscriber> Layer<S> for Codes {
    fn on_event(&self, event: &Event<'_>, _: layer::Context<'_, S>) {
        let mut code = Code::default();
        event.record(&mut code);
        let _ = self.0.send(code.0);
    }
}

/// An event's `code` field, empty if it has none.
#[derive(Default)]
struct Code(String);

impl Visit for Code {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "code" {
            value.clone_into(&mut self.0);
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn fmt::Debug) {}
}

/// The codes of events logged on a thread while it's held, from [`logged`]. The receiver is first
/// so the guards drop after it, the later set first.
pub(crate) struct Logged {
    pub(crate) codes: std::sync::mpsc::Receiver<String>,
    _codes: tracing::subscriber::DefaultGuard,
    _other: tracing::subscriber::DefaultGuard,
}

/// Sends the code of each event logged on this thread to the returned [`Logged`] while it's held,
/// whatever other tests log from their threads. Two dispatchers stay registered so tracing-core
/// combines every live one's interest in a callsite another thread registers first. With one it
/// would ask that thread's dispatcher alone, which has none, and keep the callsite off for good
/// (tracing-core 0.1.36's `has_just_one`).
pub(crate) fn logged() -> Logged {
    use tracing_subscriber::layer::SubscriberExt;

    let other = tracing::subscriber::set_default(tracing_subscriber::registry());
    let (sender, codes) = std::sync::mpsc::channel();
    let subscriber = tracing_subscriber::registry().with(Codes(sender));
    let set = tracing::subscriber::set_default(subscriber);
    Logged {
        codes,
        _codes: set,
        _other: other,
    }
}
