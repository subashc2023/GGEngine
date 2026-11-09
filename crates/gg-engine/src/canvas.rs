//! The canvas: a game's own UI, its HUDs and menus, apart from the development UI, which layers lay
//! out as they update, each frame, in the game's view's own space, anchored to its edges and scaled
//! with it, as text and sprites that the engine draws into the view over the world.

use std::mem;

use gg_core::math::{Mat4, Vec2, Vec3};
use gg_renderer::{Draws, Shaders, Sprite, Viewport};

use crate::font::Fonts;
use crate::{Align, TextStyle};

/// How tall the game's view is in the canvas's units, however many pixels it is: what's drawn on
/// the canvas scales with the view's height, and the view is as many units across as its aspect
/// ratio makes, 1,920 at 16:9.
pub const CANVAS_HEIGHT: f32 = 1080.0;

/// What layers drew on the canvas this frame, until it's handed off: the draws, each in the
/// canvas's units from its anchor, and where each run of them of one anchor starts; and the clip
/// of the draws from here on, likewise.
#[derive(Default)]
pub(crate) struct Drawn {
    draws: Draws,
    anchors: Vec<(usize, [Align; 2])>,
    clip: Option<[Vec2; 2]>,
}

impl Drawn {
    /// Has the draws from here on placed from `anchor`, until another's, and clipped to `clip`, in
    /// the canvas's units from it, until another: a clip set again where the anchor changes, so
    /// that its draws' run starts there, and its clip is placed with them.
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

    /// Places this frame's draws in the pixels of a view `size` across and down, from its bottom
    /// left, as the renderer's canvas takes them, and swaps them for `into`'s, which it forgets,
    /// keeping their memory for the next frame's.
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

/// Where `anchor` is on a view `size` pixels across and down, from its bottom left, and how many of
/// its pixels a unit of the canvas spans: [`CANVAS_HEIGHT`] units to the view's height.
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

/// The matrix from the canvas's space at `anchor` to the pixels of a view `size` across and down,
/// from its bottom left, as [`anchored`] places and scales it.
fn from_anchor(anchor: [Align; 2], size: [u32; 2]) -> Mat4 {
    let (at, scale) = anchored(anchor, size);
    Mat4::from_translation(at.extend(0.0)) * Mat4::from_scale(Vec3::new(scale, scale, 1.0))
}

/// Where a layer draws on the game's view's canvas, from one of the view's edges, corners or its
/// middle, as [`Context::canvas`](crate::Context::canvas) gives it.
pub struct Canvas<'a> {
    drawn: &'a mut Drawn,
    fonts: &'a mut Fonts,
    shaders: &'a mut Shaders,
    anchor: [Align; 2],
    /// What it draws within, as [`set_clip`](Self::set_clip) set it.
    clip: Option<[Vec2; 2]>,
    /// Where the game's view was as the last frame handed it off, and the cursor, in the main
    /// window's pixels.
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

    /// Has what this canvas draws from here on show only within `clip`, a rectangle in the
    /// canvas's units from its anchor, +Y up, as transforms place draws, its least corner then its
    /// greatest: what of a draw falls outside it isn't drawn, to the view's pixel, each edge at the
    /// nearest pixel's, as a list that scrolls hides what's scrolled out of it. `None`, as a canvas
    /// starts, clips nothing. Each clip is drawn as a scissor of its own, so a canvas that changes
    /// its clip at each draw draws each apart, rather than as one instanced draw of them all.
    pub fn set_clip(&mut self, clip: Option<[Vec2; 2]>) {
        self.clip = clip;
    }

    /// Draws `sprite` on the canvas this frame, where `transform` places it from the canvas's
    /// anchor, over everything drawn before it, the world's nearest entities too, within its clip.
    /// Unscaled, a sprite is a unit square around the transform's translation, as [`Sprite`] says,
    /// so a sprite 400 units across is scaled by 400. It's of no entity, so a
    /// [pick](crate::Context::pick) where it's on top finds none.
    pub fn draw_sprite(&mut self, transform: Mat4, sprite: Sprite) {
        self.drawn.anchor(self.anchor, self.clip);
        self.drawn.draws.push_sprite(transform, sprite);
    }

    /// Draws `text` on the canvas this frame in `style`, where `transform` places its origin from
    /// the canvas's anchor, over everything drawn before it, the world's nearest entities too,
    /// within its clip, laid out as [`Context::draw_text`](crate::Context::draw_text) lays it out,
    /// its em `style.size` of the canvas's units. It's of no entity, so a
    /// [pick](crate::Context::pick) where it's on top finds none.
    ///
    /// # Panics
    ///
    /// If `style.font` is another context's.
    pub fn draw_text(&mut self, transform: Mat4, text: &str, style: &TextStyle) {
        self.drawn.anchor(self.anchor, self.clip);
        let glyphs = self.fonts.lay_out(transform, text, style, 0);
        self.drawn.draws.extend_glyphs(self.shaders, glyphs);
    }

    /// Where the cursor is on the canvas, in its units from its anchor, +Y up, as the view showed
    /// it in the last frame handed off, which is what the cursor was over; `None` while the cursor
    /// isn't over the view, as [`Context::view`](crate::Context::view) gives it, or the view has no
    /// area, and in headless runs, which have no cursor but what the tool API's `input` moves.
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

    /// Draws on the canvas from each anchor, interleaved, and checks that handing them off places
    /// each in the view's pixels from its anchor, scaled to the view's height, whatever its size:
    /// the draws laid out once are placed again for a view of another size; and that the next
    /// frame's start afresh, in the memory the last's had.
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
            // 1,080 pixels high, a pixel a unit: top left, middle, top left, bottom right.
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
        // Text joins its anchor's run, which another anchor's sprite ends.
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

    /// A canvas's clip goes with its draws, placed from its anchor as they are, in the view's
    /// pixels: a clip of one rectangle from two anchors is two, each placed from its own, and the
    /// draws after it's taken away are clipped no more.
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

    /// The cursor over the view is where the canvas draws at its pixel, from any anchor, the view's
    /// origin in the window aside; outside the view, or without a cursor or a view, it's nowhere.
    #[test]
    fn the_cursor_is_where_the_canvas_draws_under_it() {
        let mut textures = Textures::default();
        let mut fonts = Fonts::new(&mut textures);
        let mut shaders = Shaders::default();
        let mut drawn = Drawn::default();
        // 540 pixels high, two to a unit, 100 from the window's left and 50 from its top.
        let view = Viewport {
            origin: [100, 50],
            size: [800, 540],
        };
        let mut found = |anchor, view, cursor| {
            Canvas::new(&mut drawn, &mut fonts, &mut shaders, anchor, view, cursor).cursor()
        };
        // 300 right of the view's left and 90 above its bottom.
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
