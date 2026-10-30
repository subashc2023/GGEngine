//! What tests share: the shapes a pass of egui paints, among which the UI's tests find what they
//! drew; and the codes of the events logged, which tests of what warns read.

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
