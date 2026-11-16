//! What tests share: the shapes a pass of egui paints, among which the UI's tests find what they
//! drew, in development builds, which have the UI; the codes of the events logged, which tests of
//! what warns, or logs what it did, read; and the lock that keeps tests that load cores for seconds
//! from running beside those that pace themselves by the clock.

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

/// The shapes `shapes`, a pass's output, paint, in the order they're painted: each shape a
/// [`Shape::Vec`] holds in its place.
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

/// What keeps a test that loads cores for seconds from running beside those that run peers, a
/// server and its clients, or a link, which pace themselves by the clock and allow a busy machine
/// only so much: [`real_time`] keeps it shared, [`alone`] alone. It guards no data, so a test that
/// panicked keeping it leaves it as usable as before.
static REAL_TIME: RwLock<()> = RwLock::new(());

/// Kept for its whole run by a test that runs peers, a server and its clients, or a link: such
/// tests run beside each other and the rest, but never beside one keeping [`alone`]. Taken once, as
/// the test starts: a second, while another test waits for [`alone`], may wait for good.
pub(crate) fn real_time() -> RwLockReadGuard<'static, ()> {
    REAL_TIME.read().unwrap_or_else(PoisonError::into_inner)
}

/// Kept for its whole run by a test that loads cores for seconds, as a 3D world's physics and
/// animation do in a debug build: it runs beside no test keeping [`real_time`], whose threads it
/// would starve, on a hosted runner of few cores, past what they allow.
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

/// The codes of the events logged on a thread while it's kept, as [`logged`] starts it: the
/// receiver first, so that the guards drop after it, the later set first.
pub(crate) struct Logged {
    pub(crate) codes: std::sync::mpsc::Receiver<String>,
    _codes: tracing::subscriber::DefaultGuard,
    _other: tracing::subscriber::DefaultGuard,
}

/// Has the code of each event logged on this thread go to the [`Logged`] it returns, while that's
/// kept, whichever other tests log the same events from their threads meanwhile. Two dispatchers
/// stay registered, so that tracing-core combines every live one's interest in a callsite another
/// thread registers first, where with one registered it would ask that thread's alone, which has
/// none, and keep the callsite off for good (tracing-core 0.1.36's `has_just_one`).
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
