//! Grids: lines a power of ten apart on one of the world's axis planes, and the plane's two axes in
//! their colors, which the engine draws with a mesh and a shader of its own.

use std::mem::offset_of;

use bytemuck::NoUninit;
use gg_core::math::{Vec2, Vec3};

/// The grid shader, which the build compiled from `shaders/grid.slang`.
pub(crate) static SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/grid.spv"));

/// The unit square, about its center: its center, then its corners counterclockwise.
pub(crate) const SQUARE: [Vec2; 5] = [
    Vec2::ZERO,
    Vec2::new(-0.5, -0.5),
    Vec2::new(0.5, -0.5),
    Vec2::new(0.5, 0.5),
    Vec2::new(-0.5, 0.5),
];

/// The square as four triangles from its center, where perspective puts the camera's foot: the
/// lines are finest there, and the plane's coordinates are interpolated most precisely near a
/// vertex.
pub(crate) const FAN: [u32; 12] = [0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 1];

/// A grid on one of the world's axis planes, XY, XZ or YZ, seen through either projection: lines a
/// power of ten apart, and the plane's two axes in their colors, on a unit square about its center
/// in its XY plane, which a draw's transform lays in the plane: around the camera's foot in
/// perspective, and over the view orthographically, as the engine's grid places it. Lines a power
/// of ten apart show once they're 8 pixels apart, fading in until they're 80 apart, and brighten
/// from there to the major lines' gray by 800. Crisp lines are a pixel each; anti-aliased ones are
/// drawn each family of lines by its own footprint, and fade toward grazing angles. It's drawn by
/// straight alpha over what's drawn before it, but where an opaque mesh is nearer, testing depth
/// without writing it, and where it draws nothing, picks find what's behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// The color of the plane's first axis, linear: X in XY and XZ, Y in YZ.
    pub first: Vec3,
    /// Where the square's center is along the first axis, modulo the coarsest spacing drawn, and
    /// within half of it of 0: the lines are where they'd be from the plane's origin, and as
    /// precise however far from it they are.
    pub first_center: f32,
    /// The color of its second axis: Y in XY, Z in XZ and YZ.
    pub second: Vec3,
    /// Where the square's center is along the second axis, as `first_center` is along the first.
    pub second_center: f32,
    /// In perspective, where the camera is from the square's center; orthographically, the way it
    /// faces.
    pub eye: Vec3,
    /// The power of ten that the coarsest spacing drawn is, which the center is taken modulo.
    pub coarsest: f32,
    /// The plane's normal: Z for XY, Y for XZ, X for YZ.
    pub normal: Vec3,
    /// How the lines draw.
    pub lines: GridLines,
}

/// How a [`Grid`]'s lines draw, as the camera sees its plane.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridLines {
    /// Anti-aliased, through perspective from a point.
    Perspective,
    /// Anti-aliased, orthographically along a direction.
    Orthographic,
    /// Crisp, a pixel each, orthographically, for a camera that faces the plane squarely, its
    /// right along one of the plane's axes.
    Crisp,
}

impl Grid {
    /// What the grid shader takes for the grid, an instance of its draw.
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

/// What `shaders/grid.slang` takes for each grid, an instance of its draw, which `Grid` there
/// matches byte for byte: `lines` is its `PERSPECTIVE`, `ORTHOGRAPHIC` or `CRISP`, as
/// [`GridLines`] numbers them.
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

// Each `Vec3` begins a row of 16 bytes, which a 4-byte field ends, so that `Params` is laid out
// alike in C, as Slang lays out what an address points at, and in std430, which aligns a float3 to
// 16 bytes.
const _: () = {
    assert!(offset_of!(Params, second) == 16);
    assert!(offset_of!(Params, eye) == 32);
    assert!(offset_of!(Params, normal) == 48);
    assert!(size_of::<Params>() == 64);
};
