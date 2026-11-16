//! Placing on a UI's canvas, and painting there: looks, whole or cut into nine, and text.

use gg_core::math::{Mat4, Vec2, Vec3, Vec4};
use gg_renderer::{Sprite, Texture};

use super::layout::{Rect, share};
use super::{Look, Node};
use crate::canvas::Canvas;

/// Where `point`, in the canvas's units from the view's top left, +Y down, is on the canvas from
/// `origin`, likewise, +Y up.
pub(super) fn on_canvas(point: Vec2, origin: Vec2) -> Vec3 {
    Vec3::new(point.x - origin.x, origin.y - point.y, 0.0)
}

/// `rect`'s corners on the canvas from `origin`, its least then its greatest, as a clip is.
pub(super) fn corners(rect: Rect, origin: Vec2) -> [Vec2; 2] {
    let low = on_canvas(Vec2::new(rect.at.x, rect.at.y + rect.size.y), origin);
    let high = on_canvas(Vec2::new(rect.at.x + rect.size.x, rect.at.y), origin);
    [low.truncate(), high.truncate()]
}

/// The transform of a sprite over `rect`, on the canvas from `origin`.
pub(super) fn over(rect: Rect, origin: Vec2) -> Mat4 {
    Mat4::from_translation(on_canvas(rect.middle(), origin))
        * Mat4::from_scale(rect.size.extend(1.0))
}

/// Draws `look` over `rect` on `canvas`, from `origin`: its texture cut into nine as `slice` says,
/// or else whole, or without a texture, its corners rounded `radius` by quarters of `disc`; and
/// nothing of a color with no alpha.
pub(super) fn paint(
    canvas: &mut Canvas<'_>,
    rect: Rect,
    origin: Vec2,
    look: &Look,
    radius: f32,
    slice: Option<(f32, f32)>,
    disc: Option<Texture>,
) {
    if look.color.w <= 0.0 || rect.size.cmple(Vec2::ZERO).any() {
        return;
    }
    let texture = look.texture.as_ref().and_then(|texture| texture.handle);
    let sprite = Sprite {
        color: look.color,
        texture,
        ..Sprite::default()
    };
    match (texture, slice, disc) {
        (Some(_), Some((fraction, units)), _) => {
            nine(canvas, rect, origin, sprite, fraction, units)
        }
        (None, _, Some(disc)) if radius > 0.0 => {
            let rounded = Sprite {
                texture: Some(disc),
                ..sprite
            };
            nine(canvas, rect, origin, rounded, 0.5, radius);
        }
        _ => canvas.draw_sprite(over(rect, origin), sprite),
    }
}

/// Draws `sprite`'s texture cut into nine over `rect` on `canvas`, from `origin`: its corners and
/// edges `fraction` of it in from its sides, drawn `units` wide, or half `rect`'s smaller side if
/// that's less, and the rest stretched between them.
fn nine(
    canvas: &mut Canvas<'_>,
    rect: Rect,
    origin: Vec2,
    sprite: Sprite,
    fraction: f32,
    units: f32,
) {
    let border = units.min(rect.size.x / 2.0).min(rect.size.y / 2.0);
    let end = rect.at + rect.size;
    let xs = [rect.at.x, rect.at.x + border, end.x - border, end.x];
    let ys = [rect.at.y, rect.at.y + border, end.y - border, end.y];
    let cuts = [0.0, fraction, 1.0 - fraction, 1.0];
    for row in 0..3 {
        for column in 0..3 {
            let at = Vec2::new(xs[column], ys[row]);
            let size = Vec2::new(xs[column + 1], ys[row + 1]) - at;
            if size.cmple(Vec2::ZERO).any() {
                continue;
            }
            let piece = Sprite {
                rect: [
                    Vec2::new(cuts[column], cuts[row]),
                    Vec2::new(cuts[column + 1], cuts[row + 1]),
                ],
                ..sprite
            };
            canvas.draw_sprite(over(Rect { at, size }, origin), piece);
        }
    }
}

/// Draws `text` of `node`'s style in `color` in `rect` less its padding on `canvas`, from `origin`,
/// aligned there as the node says.
pub(super) fn write(
    canvas: &mut Canvas<'_>,
    node: &Node,
    text: &str,
    rect: Rect,
    origin: Vec2,
    color: Vec4,
) {
    if text.is_empty() || color.w <= 0.0 {
        return;
    }
    let room = rect.inside(node.padding);
    let at = room.at + room.size * Vec2::new(share(node.align[0]), share(node.align[1]));
    let place = Mat4::from_translation(on_canvas(at, origin));
    canvas.draw_text(place, text, &node.style(color));
}
