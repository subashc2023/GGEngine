//! Grids: lines a power of ten apart on one of the world's axis planes, and the plane's two axes in
//! their colors, drawn by the engine's own mesh and shader.

use std::mem::offset_of;

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3};

/// `shaders/grid.slang`, compiled by the build.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/grid.spv"));

/// The unit square: its center, then its corners counterclockwise.
pub(crate) const SQUARE: [Vec2; 5] = [
    Vec2::ZERO,
    Vec2::new(-0.5, -0.5),
    Vec2::new(0.5, -0.5),
    Vec2::new(0.5, 0.5),
    Vec2::new(-0.5, 0.5),
];

/// The square as four triangles from its center, where perspective puts the camera's foot: the
/// lines are finest there, and the plane's coordinates interpolate most precisely near a vertex.
pub(crate) const FAN: [u32; 12] = [0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 1];

/// A grid on the XY, XZ or YZ plane, in either projection.
///
/// A unit square about its center in its XY plane, which the draw's transform lays in the plane.
/// The engine's grid places it around the camera's foot in perspective, and over the view
/// orthographically.
///
/// Lines of each power of ten show once 8 pixels apart, fade in until 80 apart, and brighten to the
/// major lines' gray by 800. Crisp lines are a pixel each. Anti-aliased ones are filtered per
/// family by its footprint, and fade toward grazing angles. It blends by straight alpha over what's
/// drawn before it, testing depth without writing it. Where it draws nothing, picks find what's
/// behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// The first axis's color, linear: X in XY and XZ, Y in YZ.
    pub first: Vec3,
    /// The square's center along the first axis, modulo the coarsest spacing drawn, within half of
    /// it of 0. So lines sit where they would from the plane's origin, as precise however far from
    /// it.
    pub first_center: f32,
    /// The second axis's color, linear: Y in XY, Z in XZ and YZ.
    pub second: Vec3,
    /// As `first_center`, along the second axis.
    pub second_center: f32,
    /// In perspective, the camera's position from the square's center; orthographically, its
    /// forward direction.
    pub eye: Vec3,
    /// The coarsest spacing drawn, as a power of ten. The centers are taken modulo it.
    pub coarsest: f32,
    /// Z for XY, Y for XZ, X for YZ.
    pub normal: Vec3,
    pub lines: GridLines,
}

/// How a [`Grid`]'s lines draw.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridLines {
    /// Anti-aliased, in perspective.
    Perspective,
    /// Anti-aliased, orthographic.
    Orthographic,
    /// Crisp, a pixel each, orthographic, for a camera facing the plane squarely with its right
    /// along one of the plane's axes.
    Crisp,
}

impl Grid {
    pub(crate) fn params(&self) -> Params {
        Params {
            first: self.first,
            first_center: self.first_center,
            second: self.second,
            second_center: self.second_center,
            eye: self.eye,
            coarsest: self.coarsest,
            normal: self.normal,
            lines: self.lines as u32,
        }
    }
}

/// What `shaders/grid.slang` takes per grid, an instance of its draw. Its `Grid` matches this byte
/// for byte; `lines` is its `PERSPECTIVE`, `ORTHOGRAPHIC` or `CRISP`, numbered as [`GridLines`].
#[repr(C)]
#[derive(Clone, Copy, NoUninit)]
pub(crate) struct Params {
    first: Vec3,
    first_center: f32,
    second: Vec3,
    second_center: f32,
    eye: Vec3,
    coarsest: f32,
    normal: Vec3,
    lines: u32,
}

// Each `Vec3` starts a 16-byte row that a 4-byte field ends, so `Params` lays out alike in C (as
// Slang lays out what an address points at) and in std430 (which aligns a float3 to 16 bytes).
const _: () = {
    assert!(offset_of!(Params, second) == 16);
    assert!(offset_of!(Params, eye) == 32);
    assert!(offset_of!(Params, normal) == 48);
    assert!(size_of::<Params>() == 64);
};
