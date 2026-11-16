//! The canvas: a game's own UI (HUDs, menus), apart from the development UI. Layers lay it out
//! each frame as text and sprites in the view's space, anchored to its edges and scaled with it.
//! The engine draws it over the world.

use std::mem;

use gg_core::math::{Mat4, Vec2, Vec3};
use gg_renderer::{Draws, Shaders, Sprite, Viewport};

use crate::font::Fonts;
use crate::{Align, TextStyle};

/// The view's height in canvas units, whatever its pixels. Drawings scale with the height; the
/// width follows the aspect ratio: 1,920 at 16:9.
pub const CANVAS_HEIGHT: f32 = 1080.0;

/// This frame's canvas draws until handed off: each in canvas units from its anchor, where each
/// anchor's run starts, and the clip of later draws.
#[derive(Default)]
pub(crate) struct Drawn {
    draws: Draws,
    anchors: Vec<(usize, [Align; 2])>,
    clip: Option<[Vec2; 2]>,
}

impl Drawn {
    /// Places later draws from `anchor` and clips them to `clip` (canvas units from the anchor).
    /// A new anchor starts a run and sets the clip again, so the clip is placed with its run.
    fn anchor(&mut self, anchor: [Align; 2], clip: Option<[Vec2; 2]>) {
        let moved = self.anchors.last().is_none_or(|&(_, last)| last != anchor);
        if moved {
            self.anchors.push((self.draws.len(), anchor));
        }
        if clip != self.clip || moved && clip.is_some() {
            self.draws.clip(clip);
            self.clip = clip;
        }
    }

    /// Places this frame's draws in the pixels of a view `size`, from its bottom left, and swaps
    /// them into `into`, forgetting its old draws but keeping their memory for the next frame.
    pub(crate) fn place(&mut self, size: [u32; 2], into: &mut Draws) {
        let len = self.draws.len();
        for (i, &(start, anchor)) in self.anchors.iter().enumerate() {
            let end = self.anchors.get(i + 1).map_or(len, |&(next, _)| next);
            self.draws.place(start..end, from_anchor(anchor, size));
        }
        self.anchors.clear();
        self.clip = None;
        mem::swap(into, &mut self.draws);
        self.draws.clear();
    }
}

/// Where `anchor` is on a view `size`, from its bottom left, and the pixels a canvas unit spans:
/// [`CANVAS_HEIGHT`] units to the view's height.
fn anchored(anchor: [Align; 2], size: [u32; 2]) -> (Vec2, f32) {
    let [width, height] = size.map(|side| side as f32);
    let across = match anchor[0] {
        Align::Start => 0.0,
        Align::Center => width / 2.0,
        Align::End => width,
    };
    let up = match anchor[1] {
        Align::Start => height,
        Align::Center => height / 2.0,
        Align::End => 0.0,
    };
    (Vec2::new(across, up), height / CANVAS_HEIGHT)
}

/// The matrix from canvas space at `anchor` to a view `size`'s pixels, per [`anchored`].
fn from_anchor(anchor: [Align; 2], size: [u32; 2]) -> Mat4 {
    let (at, scale) = anchored(anchor, size);
    Mat4::from_translation(at.extend(0.0)) * Mat4::from_scale(Vec3::new(scale, scale, 1.0))
}

/// Where a layer draws on the game view's canvas, from an edge, corner or the middle of the view.
/// [`Context::canvas`](crate::Context::canvas) gives it.
pub struct Canvas<'a> {
    drawn: &'a mut Drawn,
    fonts: &'a mut Fonts,
    shaders: &'a mut Shaders,
    anchor: [Align; 2],
    /// Set by [`set_clip`](Self::set_clip).
    clip: Option<[Vec2; 2]>,
    /// The view as the last frame handed it off, and the cursor, in the main window's pixels.
    view: Option<Viewport>,
    cursor: Option<[f64; 2]>,
}

impl<'a> Canvas<'a> {
    pub(crate) fn new(
        drawn: &'a mut Drawn,
        fonts: &'a mut Fonts,
        shaders: &'a mut Shaders,
        anchor: [Align; 2],
        view: Option<Viewport>,
        cursor: Option<[f64; 2]>,
    ) -> Self {
        Self {
            drawn,
            fonts,
            shaders,
            anchor,
            clip: None,
            view,
            cursor,
        }
    }

    /// Shows later draws only within `clip`: a rectangle in canvas units from the anchor, +Y up,
    /// least corner then greatest. Edges snap to the nearest pixel. `None`, the start, clips
    /// nothing. Each clip is its own scissor, so changing it at each draw costs a draw each
    /// instead of one instanced draw.
    pub fn set_clip(&mut self, clip: Option<[Vec2; 2]>) {
        self.clip = clip;
    }

    /// Draws `sprite` this frame, placed by `transform` from the anchor, within the clip, over
    /// everything drawn before, the world too. Unscaled it's a unit square ([`Sprite`]), so 400
    /// units across is scale 400. It belongs to no entity: a [pick](crate::Context::pick) on top
    /// of it finds none.
    pub fn draw_sprite(&mut self, transform: Mat4, sprite: Sprite) {
        self.drawn.anchor(self.anchor, self.clip);
        self.drawn.draws.push_sprite(transform, sprite);
    }

    /// Like [`draw_sprite`](Self::draw_sprite) for `text` in `style`, its origin placed by
    /// `transform`. Laid out like [`Context::draw_text`](crate::Context::draw_text), its em
    /// `style.size` canvas units.
    ///
    /// # Panics
    ///
    /// If `style.font` is another context's.
    pub fn draw_text(&mut self, transform: Mat4, text: &str, style: &TextStyle) {
        self.drawn.anchor(self.anchor, self.clip);
        let glyphs = self.fonts.lay_out(transform, text, style, 0);
        self.drawn.draws.extend_glyphs(self.shaders, glyphs);
    }

    /// The cursor in canvas units from the anchor, +Y up, over the view as the last frame handed
    /// it off. `None` when it isn't over the view ([`Context::view`](crate::Context::view)), the
    /// view has no area, or in headless runs unless the tool API's `input` moved it.
    pub fn cursor(&self) -> Option<Vec2> {
        let view = self.view?;
        let [x, y] = self.cursor?;
        let [left, top] = view.origin.map(f64::from);
        let [width, height] = view.size.map(f64::from);
        let [x, y] = [x - left, y - top];
        if !((0.0..width).contains(&x) && (0.0..height).contains(&y)) {
            return None;
        }
        let (at, scale) = anchored(self.anchor, view.size);
        Some((Vec2::new(x as f32, (height - y) as f32) - at) / scale)
    }
}

#[cfg(test)]
mod tests {
    use gg_core::math::Vec4;
    use gg_renderer::Textures;

    use super::*;

    /// Handing off places interleaved draws from each anchor, scaled to the view's height, again
    /// for a view of another size; the next frame starts afresh in the last's memory.
    #[test]
    fn handing_off_places_each_draw_from_its_anchor() {
        let mut textures = Textures::default();
        let mut fonts = Fonts::new(&mut textures);
        let mut shaders = Shaders::default();
        let mut drawn = Drawn::default();
        // A sprite 100 units across, 10 right of and 20 above each anchor.
        let at = Mat4::from_translation(Vec3::new(10.0, 20.0, 0.0))
            * Mat4::from_scale(Vec3::splat(100.0));
        let anchors = [
            [Align::Start, Align::Start],
            [Align::Center, Align::Center],
            [Align::Start, Align::Start],
            [Align::End, Align::End],
        ];
        let mut draw = |drawn: &mut Drawn| {
            for anchor in anchors {
                let mut canvas = Canvas::new(drawn, &mut fonts, &mut shaders, anchor, None, None);
                canvas.draw_sprite(at, Sprite::default());
            }
        };
        let mut handed = Draws::default();
        for (size, scale, points) in [
            // 1,080 high, a pixel a unit: top left, middle, top left, bottom right.
            (
                [1920, 1080],
                1.0,
                [[0.0, 1080.0], [960.0, 540.0], [0.0, 1080.0], [1920.0, 0.0]],
            ),
            // Half as high and narrower, half a pixel a unit.
            (
                [600, 540],
                0.5,
                [[0.0, 540.0], [300.0, 270.0], [0.0, 540.0], [600.0, 0.0]],
            ),
        ] {
            draw(&mut drawn);
            drawn.place(size, &mut handed);
            let mut want = Draws::default();
            for [x, y] in points {
                let anchor = Mat4::from_translation(Vec3::new(x, y, 0.0));
                let scaled = Mat4::from_scale(Vec3::new(scale, scale, 1.0));
                want.push_sprite(anchor * scaled * at, Sprite::default());
            }
            assert_eq!(handed, want, "a view of {size:?}");
            assert!(drawn.draws.is_empty() && drawn.anchors.is_empty());
        }
        // Text joins its anchor's run; another anchor's sprite ends it.
        let style = TextStyle {
            font: None,
            size: 50.0,
            color: Vec4::ONE,
            align: [Align::Start; 2],
        };
        let top_right = [Align::End, Align::Start];
        let mut canvas = Canvas::new(&mut drawn, &mut fonts, &mut shaders, top_right, None, None);
        canvas.draw_text(Mat4::IDENTITY, "Hi", &style);
        canvas.draw_sprite(at, Sprite::default());
        let middle = [Align::Center; 2];
        Canvas::new(&mut drawn, &mut fonts, &mut shaders, middle, None, None)
            .draw_sprite(at, Sprite::default());
        assert_eq!(drawn.anchors, [(0, top_right), (3, middle)]);
    }

    /// A clip is placed with its draws: one rectangle from two anchors is two clips, each placed
    /// from its own anchor; draws after it's cleared are unclipped.
    #[test]
    fn a_clip_is_placed_with_its_draws() {
        let mut textures = Textures::default();
        let mut fonts = Fonts::new(&mut textures);
        let mut shaders = Shaders::default();
        let mut drawn = Drawn::default();
        let at = Mat4::from_translation(Vec3::new(10.0, 20.0, 0.0))
            * Mat4::from_scale(Vec3::splat(100.0));
        let clip = Some([Vec2::new(-50.0, -20.0), Vec2::new(50.0, 20.0)]);
        let middle = [Align::Center; 2];
        let mut canvas = Canvas::new(&mut drawn, &mut fonts, &mut shaders, middle, None, None);
        canvas.set_clip(clip);
        canvas.draw_sprite(at, Sprite::default());
        let corner = [Align::Start, Align::Start];
        let mut canvas = Canvas::new(&mut drawn, &mut fonts, &mut shaders, corner, None, None);
        canvas.set_clip(clip);
        canvas.draw_sprite(at, Sprite::default());
        canvas.set_clip(None);
        canvas.draw_sprite(at, Sprite::default());
        let mut handed = Draws::default();
        // Half a pixel a unit, the middle at (480, 270) and the top left at (0, 540).
        drawn.place([960, 540], &mut handed);
        let mut want = Draws::default();
        let scaled = Mat4::from_scale(Vec3::new(0.5, 0.5, 1.0));
        for (x, y, clip) in [
            (
                480.0,
                270.0,
                Some([Vec2::new(455.0, 260.0), Vec2::new(505.0, 280.0)]),
            ),
            (
                0.0,
                540.0,
                Some([Vec2::new(-25.0, 530.0), Vec2::new(25.0, 550.0)]),
            ),
            (0.0, 540.0, None),
        ] {
            want.clip(clip);
            let anchor = Mat4::from_translation(Vec3::new(x, y, 0.0));
            want.push_sprite(anchor * scaled * at, Sprite::default());
        }
        assert_eq!(handed, want);
    }

    /// The cursor is where the canvas draws at its pixel, from any anchor, offset by the view's
    /// origin; outside the view, or without a cursor or view, it's `None`.
    #[test]
    fn the_cursor_is_where_the_canvas_draws_under_it() {
        let mut textures = Textures::default();
        let mut fonts = Fonts::new(&mut textures);
        let mut shaders = Shaders::default();
        let mut drawn = Drawn::default();
        // 540 pixels high, two to a unit, at (100, 50) in the window.
        let view = Viewport {
            origin: [100, 50],
            size: [800, 540],
        };
        let mut found = |anchor, view, cursor| {
            Canvas::new(&mut drawn, &mut fonts, &mut shaders, anchor, view, cursor).cursor()
        };
        // 300 right of the view's left, 90 above its bottom.
        let over = Some([400.0, 500.0]);
        let at = [
            ([Align::Start, Align::Start], Vec2::new(600.0, -900.0)),
            ([Align::Center, Align::Center], Vec2::new(-200.0, -360.0)),
            ([Align::End, Align::End], Vec2::new(-1000.0, 180.0)),
        ];
        for (anchor, at) in at {
            assert_eq!(found(anchor, Some(view), over), Some(at), "{anchor:?}");
        }
        let anchor = [Align::Start; 2];
        for outside in [[99.0, 300.0], [900.0, 300.0], [400.0, 49.0], [400.0, 590.0]] {
            assert_eq!(
                found(anchor, Some(view), Some(outside)),
                None,
                "{outside:?}"
            );
        }
        assert_eq!(found(anchor, None, over), None);
        assert_eq!(found(anchor, Some(view), None), None);
    }
}
