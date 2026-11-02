//! What tests share: the shapes a pass of egui paints, among which the UI's tests find what they
//! drew; and the codes of the events logged, which tests of what warns, or logs what it did, read.

use std::fmt;
use std::sync::mpsc::Sender;

use egui::Shape;
use egui::epaint::ClippedShape;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{self, Layer};

/// The shapes `shapes`, a pass's output, paint, in the order they're painted: each shape a
/// [`Shape::Vec`] holds in its place.
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
