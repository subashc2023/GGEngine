//! What the UI's tests share: the shapes a pass of egui paints, among which they find what they
//! drew.

use egui::Shape;
use egui::epaint::ClippedShape;

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
