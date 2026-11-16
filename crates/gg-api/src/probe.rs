//! A probe: what shading takes and makes at one pixel of a game's view, for `gg probe` and the
//! editor's Probe tool. It reads captures of the pixel, one per part of shading, a `pick` and a
//! `look`, all of one world.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::{Captured, Failure, Looked, Picked, Projection, while_paused};

/// The parts of shading a probe captures, as a capture's `show` and [`Probe`]'s fields name them.
pub const SHADING: [&str; 13] = [
    "base_color",
    "normal",
    "surface_normal",
    "metallic",
    "roughness",
    "occlusion",
    "emission",
    "direct_light",
    "sky_light",
    "reflection",
    "specular_color",
    "diffuse_light",
    "specular_light",
];

/// The image whose pixel gives the depth.
const DEPTH: &str = "view depth";

/// How many captures a probe takes: the view as shown, each part of shading, and the depth.
pub const CAPTURES: usize = SHADING.len() + 2;

/// What the answer says of the precision of its numbers.
const QUANTIZED: &str = "every value is read from an 8-bit pixel, so it's good to about 1/255; \
                         `distance` to about 3%";

/// What shading took and made at a pixel of the view, as [`probe`] answers; each value `None` where
/// the pixel is outside the view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    /// The pixel, `[x, y]` from the view's top left.
    pub at: [f64; 2],
    /// The view's `[width, height]` in pixels.
    pub size: [u32; 2],
    /// The ticks the simulation had run in the frame the captures show.
    pub ticks: u64,
    /// Linear RGB, each channel from 0, its sRGB byte decoded; past 1 is shown as 1.
    pub base_color: Option<[f64; 3]>,
    /// With its normal map: a world-space unit vector, `[x, y, z]`, each axis from -1 to 1, its
    /// byte over 255, times 2, less 1. Read from bytes, it isn't exactly of unit length.
    pub normal: Option<[f64; 3]>,
    /// As `normal`, without its normal map.
    pub surface_normal: Option<[f64; 3]>,
    /// 0 to 1, its byte over 255, read from red.
    pub metallic: Option<f64>,
    /// As `metallic`, from 0.045, the least shading takes.
    pub roughness: Option<f64>,
    /// As `metallic`: the share of the light from all around that reaches the point.
    pub occlusion: Option<f64>,
    /// As `base_color`.
    pub emission: Option<[f64; 3]>,
    /// What the point reflects head-on, as `base_color`.
    pub specular_color: Option<[f64; 3]>,
    /// The frame's lights' light as the view shows it, exposed and tonemapped: sRGB bytes,
    /// `[r, g, b]`, not linear nor in physical units.
    pub direct_light: Option<[u8; 3]>,
    /// As `direct_light`, the light from all around.
    pub sky_light: Option<[u8; 3]>,
    /// As `direct_light`, the light from the view's reflection.
    pub reflection: Option<[u8; 3]>,
    /// As `direct_light`, the light the point diffuses, of all three.
    pub diffuse_light: Option<[u8; 3]>,
    /// As `direct_light`, the light the point reflects, of all three.
    pub specular_light: Option<[u8; 3]>,
    /// The view's pixel as it's drawn, with all the view adds to the light, as `direct_light`.
    pub shown: Option<[u8; 3]>,
    /// The reversed depth the view drew, 1 at the near plane and smaller farther off:
    /// `2^(24 * (byte / 255 - 1))`. 0 where the byte is 0: nothing drew, or past about 2^-24.
    pub depth: Option<f64>,
    /// `near / depth` in world units: how far the point is along the way the viewpoint faces, not
    /// from its position. `None` where `depth` is 0, and for a view without a perspective
    /// projection, whose depth isn't reversed this way.
    pub distance: Option<f64>,
    /// The entity there, as `pick` answers it.
    pub entity: Picked,
    /// A note that the values are 8-bit.
    pub quantized: String,
}

/// Probes pixel `at` of the game's view as shown, or its own with `own`, through `call`. It pauses
/// a playing game meanwhile ([`while_paused`]), so that all is of one world, and takes a frame per
/// capture: the view as shown, each of [`SHADING`] and the depth; then a `pick` and a `look`.
///
/// Fails as the calls do, and with `api.protocol` for an answer that isn't what the API says.
pub fn probe(
    call: impl FnMut(&str, Value) -> Result<Box<RawValue>, Failure>,
    at: [f64; 2],
    own: bool,
) -> Result<Probe, Failure> {
    let unread = |method: &str, error: serde_json::Error| {
        Failure::new("api.protocol", format!("`{method}` answered {error}"))
    };
    let mut params = json!({"at": at});
    if own {
        params["own"] = json!(true);
    }
    let shows = (SHADING.iter())
        .map(|name| json!(name))
        .chain([json!({"image": DEPTH})]);
    let shows: Vec<Value> = shows.collect();
    let (captured, picked, looked) = while_paused(call, |call| {
        let mut captured = Vec::with_capacity(CAPTURES);
        let view = call("capture", params.clone())?;
        captured
            .push(serde_json::from_str::<Captured>(view.get()).map_err(|e| unread("capture", e))?);
        for show in &shows {
            let mut shown = params.clone();
            shown["show"] = show.clone();
            let taken = call("capture", shown)?;
            let taken = serde_json::from_str(taken.get()).map_err(|e| unread("capture", e))?;
            captured.push(taken);
        }
        let picked = call("pick", json!({"at": at}))?;
        let picked = serde_json::from_str::<Picked>(picked.get()).map_err(|e| unread("pick", e))?;
        let looked = call("look", Value::Null)?;
        let looked = serde_json::from_str::<Looked>(looked.get()).map_err(|e| unread("look", e))?;
        Ok((captured, picked, looked))
    })?;
    let captured: [Captured; CAPTURES] = captured
        .try_into()
        .expect("a capture of the view, of each part of shading and of the depth");
    Ok(decode(&captured, at, &picked, &looked))
}

/// The probe of pixel `at` from its captures' answers, `captured` (the view as shown, each of
/// [`SHADING`], then the depth), its `pick` and its `look`.
pub fn decode(
    captured: &[Captured; CAPTURES],
    at: [f64; 2],
    picked: &Picked,
    looked: &Looked,
) -> Probe {
    let [view, shading @ .., depth] = captured;
    let [
        base_color,
        normal,
        surface_normal,
        metallic,
        roughness,
        occlusion,
        emission,
        direct_light,
        sky_light,
        reflection,
        specular_color,
        diffuse_light,
        specular_light,
    ] = shading;
    let bytes = |captured: &Captured| captured.pixel.map(|[r, g, b, _]| [r, g, b]);
    let read = |captured: &Captured, read: fn(u8) -> f64| bytes(captured).map(|rgb| rgb.map(read));
    let value = |captured: &Captured| captured.pixel.map(|[red, ..]| unit(red));
    let reversed = captured_depth(depth);
    let near = match looked.seen.map(|seen| seen.projection) {
        Some(Projection::Perspective { near, .. }) => Some(f64::from(near)),
        _ => None,
    };
    Probe {
        at,
        size: [view.width, view.height],
        ticks: view.ticks,
        base_color: read(base_color, linear),
        normal: read(normal, signed),
        surface_normal: read(surface_normal, signed),
        metallic: value(metallic),
        roughness: value(roughness),
        occlusion: value(occlusion),
        emission: read(emission, linear),
        specular_color: read(specular_color, linear),
        direct_light: bytes(direct_light),
        sky_light: bytes(sky_light),
        reflection: bytes(reflection),
        diffuse_light: bytes(diffuse_light),
        specular_light: bytes(specular_light),
        shown: bytes(view),
        depth: reversed,
        distance: (reversed.filter(|depth| *depth > 0.0).zip(near)).map(|(d, n)| n / d),
        entity: picked.clone(),
        quantized: QUANTIZED.to_owned(),
    }
}

/// The reversed depth at a capture of the depth's pixel: 0 for byte 0, where nothing drew.
// The workspace bans std's powers for the simulation's bits. These read 8-bit bytes, good to 1/255
// on any platform.
#[allow(clippy::disallowed_methods)]
fn captured_depth(depth: &Captured) -> Option<f64> {
    depth.pixel.map(|[byte, ..]| match byte {
        0 => 0.0,
        byte => 2f64.powf(24.0 * (unit(byte) - 1.0)),
    })
}

/// `byte` over 255.
fn unit(byte: u8) -> f64 {
    f64::from(byte) / 255.0
}

/// `byte` over 255, times 2, less 1: from -1 to 1.
fn signed(byte: u8) -> f64 {
    unit(byte) * 2.0 - 1.0
}

/// The linear value of `byte`, an sRGB-encoded channel.
// As for `captured_depth`, the platform's precision of `powf` is beyond what a byte holds.
#[allow(clippy::disallowed_methods)]
pub fn linear(byte: u8) -> f64 {
    let encoded = unit(byte);
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}
