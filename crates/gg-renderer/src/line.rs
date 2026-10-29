//! Lines: straight lines a width in the view's pixels, which the engine draws on its quad with a
//! shader of its own.

use bytemuck::NoUninit;
use gg_core::math::{Vec3, Vec4};

/// The line shader, which the build compiled from `shaders/line.slang`.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/line.spv"));

/// A straight line from `from` to `to`, points in the space a draw's transform takes to the
/// world's, drawn as wide as `width` says in the view's pixels however near or far it is, rather
/// than as the transform scales its points. Its ends are squared off half its width past its
/// points, so that lines that meet at a point, as a rectangle's sides do, leave no gap there,
/// though where they overlap a translucent color shows darker; its edges are smoothed over a pixel.
/// It's drawn by straight alpha over what's drawn before it, but where an opaque mesh is nearer,
/// testing depth without writing it, and cut where it crosses the near plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub from: Vec3,
    pub to: Vec3,
    /// In linear RGBA, alpha not multiplied in.
    pub color: Vec4,
    /// How wide it's drawn, in the view's pixels. One thinner than a pixel draws a pixel wide, as
    /// much fainter as it's thinner, so that it neither breaks up nor vanishes.
    pub width: f32,
}

impl Line {
    /// What the line shader takes for the line, an instance of its draw.
    pub(crate) fn params(&self) -> Params {
        Params {
            from: self.from,
            to: self.to,
            color: self.color.to_array(),
            width: self.width,
        }
    }
}

/// What `shaders/line.slang` takes for each line, an instance of its draw.
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Params {
    from: Vec3,
    to: Vec3,
    color: [f32; 4],
    width: f32,
}
