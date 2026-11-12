//! `gg probe`: the numbers of what shading takes and makes at one pixel of a game's view, read from
//! captures of that pixel, each showing a part of shading, and a `pick` and a `look`, all of one
//! world.

use gg_api::{Captured, Failure, Looked, Picked, Projection};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::game::{self, Pick};

/// The parts of shading a probe captures, by the names a capture's `show` takes, which are the names
/// of the fields they answer.
pub(crate) const SHADING: [&str; 13] = [
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
const CAPTURES: usize = SHADING.len() + 2;

/// What the answer says of the precision of its numbers.
const QUANTIZED: &str = "every value is read from an 8-bit pixel, so it's good to about 1/255; \
                         `distance` to about 3%";

/// `at`, a pixel of the view, `[x, y]` from its top left.
///
/// Fails with `gg.args` for a number that isn't finite or is negative.
pub(crate) fn pixel(at: [f64; 2]) -> Result<[f64; 2], Failure> {
    if at.iter().all(|axis| axis.is_finite() && *axis >= 0.0) {
        return Ok(at);
    }
    let message = format!("a pixel's X and Y are numbers, not negative, not {at:?}");
    Err(Failure::new("gg.args", message))
}

/// What shading took and made at pixel `at` of the view of the game `pick` picks, as the game's
/// view shows it, or its own with `own`, as [`decode`] gives it, read from one world: the game is
/// paused for it if it plays, and runs again after.
///
/// Fails as the calls do, and with `api.protocol` for an answer that isn't what the API says.
pub(crate) fn run(pick: &Pick, at: [f64; 2], own: bool) -> Result<Value, Failure> {
    let mut client = game::connect(pick)?;
    let call = |method: &str, params: Value| {
        (client.call(method, params)).map_err(|failure| game::waits(pick, method, failure))
    };
    probed(call, at, own)
}

/// [`run`], sending each request through `call`: a `capture` of `at` as the view is shown, one for
/// each of [`SHADING`] and one of [`DEPTH`], then a `pick` of `at` and a `look`.
fn probed(
    call: impl FnMut(&str, Value) -> Result<Box<RawValue>, Failure>,
    at: [f64; 2],
    own: bool,
) -> Result<Value, Failure> {
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
    let (captured, picked, looked) = game::while_paused(call, |call| {
        let mut captured = Vec::with_capacity(CAPTURES);
        let view = call("capture", params.clone())?;
        captured
            .push(serde_json::from_str::<Captured>(view.get()).map_err(|e| unread("capture", e))?);
        for show in &shows {
            let taken = call("capture", game::with_show(&params, show))?;
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

/// The answer of a probe of pixel `at`: `captured`, the answers of its captures, the view as shown,
/// each of [`SHADING`] and the depth, in that order; and what `picked` and `looked`, a `pick` of it
/// and a `look`, answered. Its fields, each `null` where the pixel is outside the view:
///
/// - `at`, the pixel, `[x, y]` from the view's top left, and `size`, the view's `[width, height]`
///   in pixels; `ticks`, those the simulation had run in the frame the captures show.
/// - `base_color`, `emission` and `specular_color`, what the point reflects head-on: linear RGB,
///   each channel from 0, its sRGB byte decoded; past 1 is shown as 1.
/// - `normal`, with its normal map, and `surface_normal`, without: unit vectors in the world, `[x,
///   y, z]`, each axis from -1 to 1, as its byte over 255, times 2, less 1; they are read from
///   bytes, so aren't of unit length exactly.
/// - `metallic`, `roughness` and `occlusion`: 0 to 1, its byte over 255, read from red;
///   `roughness` is from 0.045, the least shading takes; `occlusion` is the share of the light from
///   all around that reaches the point.
/// - `direct_light`, `sky_light` and `reflection`: the light from the frame's lights, from all
///   around, and from the view's reflection; `diffuse_light` and `specular_light`, the light the
///   point diffuses and the light it reflects, of all three; each as the view shows it, exposed and
///   tonemapped, as sRGB bytes, `[r, g, b]`, 0 to 255; not linear, nor in physical units. `shown`:
///   the view's pixel as it's drawn, with all the view adds to the light, in the same bytes.
/// - `depth`: the reversed depth the view drew, 1 at the viewpoint's near plane and smaller the
///   farther it is, `2^(24 * (byte / 255 - 1))`; 0 where the byte is 0, where nothing drew, or past
///   about 2^-24. `distance`: `near / depth`, in world units, how far the point is in front of the
///   viewpoint, along the way it faces, not from its position; `null` where `depth` is 0, and for a
///   view that doesn't have a perspective projection, whose depth isn't reversed this way.
/// - `entity`: `{"id", "name"}`, as `pick` answers it, both `null` where none draws.
/// - `quantized`: a note that the values are 8-bit.
fn decode(
    captured: &[Captured; CAPTURES],
    at: [f64; 2],
    picked: &Picked,
    looked: &Looked,
) -> Value {
    let [view, shading @ .., depth] = captured;
    let bytes = |captured: &Captured| captured.pixel.map(|[r, g, b, _]| [r, g, b]);
    let read = |captured: &Captured, read: fn(u8) -> f64| bytes(captured).map(|rgb| rgb.map(read));
    let mut probe = json!({
        "at": at,
        "size": [view.width, view.height],
        "ticks": view.ticks,
    });
    for (name, captured) in SHADING.iter().zip(shading) {
        probe[*name] = match *name {
            "base_color" | "emission" | "specular_color" => json!(read(captured, linear)),
            "normal" | "surface_normal" => json!(read(captured, signed)),
            "metallic" | "roughness" | "occlusion" => {
                json!(captured.pixel.map(|[red, ..]| unit(red)))
            }
            _ => json!(bytes(captured)),
        };
    }
    probe["shown"] = json!(bytes(view));
    let reversed = captured_depth(depth);
    probe["depth"] = json!(reversed);
    let near = match looked.seen.map(|seen| seen.projection) {
        Some(Projection::Perspective { near, .. }) => Some(f64::from(near)),
        _ => None,
    };
    probe["distance"] = json!(
        reversed
            .filter(|depth| *depth > 0.0)
            .zip(near)
            .map(|(d, n)| n / d)
    );
    probe["entity"] = json!(picked);
    probe["quantized"] = json!(QUANTIZED);
    probe
}

/// The reversed depth `depth`, a capture of [`DEPTH`], has at its pixel: 0 for byte 0, where nothing
/// drew.
// The workspace bans std's powers for a simulation's bits to be the same everywhere; these are
// numbers for a reader of 8-bit bytes, good to 1/255, which the platform's precision doesn't touch.
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
fn linear(byte: u8) -> f64 {
    let encoded = unit(byte);
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capture's answer at a pixel of bytes `pixel`, of a view 4 pixels by 2.
    fn captured(pixel: Option<[u8; 4]>) -> Captured {
        Captured {
            path: None,
            width: 4,
            height: 2,
            pixel,
            ticks: 9,
            streaming: false,
        }
    }

    fn looked(projection: Option<Projection>) -> Looked {
        let seen = projection.map(|projection| gg_api::Viewpoint {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            projection,
        });
        let look = json!({
            "viewpoint": null, "outline": null, "grid": null, "colliders": null,
            "by": {}, "revision": 1, "run": 1, "seen": seen,
        });
        serde_json::from_value(look).expect("a look")
    }

    /// The probe of captures whose pixels are `bytes`, in the order `decode` has them.
    fn probe_of(bytes: [Option<[u8; 4]>; CAPTURES], looked: &Looked) -> Value {
        let captured = bytes.map(captured);
        let picked = Picked {
            id: Some("2b7e151628aed2a6".to_owned()),
            name: None,
        };
        decode(&captured, [1.0, 0.5], &picked, looked)
    }

    fn close(value: &Value, want: f64, tolerance: f64) {
        let value = value.as_f64().expect("a number");
        assert!((value - want).abs() <= tolerance, "{value} isn't {want}");
    }

    /// Each field reads its bytes as the `Shown` docs say: sRGB colors as linear, normals from -1
    /// to 1, values over 255 from red, lights as the sRGB bytes they are; the depth as reversed
    /// depth, and the distance from the perspective's near plane.
    #[test]
    fn a_probe_reads_each_part_as_its_bytes_say() {
        let mut bytes = [Some([0, 0, 0, 255]); CAPTURES];
        // The view, then each part of shading in order, then the depth.
        bytes[0] = Some([10, 20, 30, 255]);
        bytes[1] = Some([188, 255, 0, 255]);
        bytes[2] = Some([128, 255, 0, 255]);
        bytes[3] = Some([0, 128, 255, 255]);
        bytes[4] = Some([255, 0, 0, 255]);
        bytes[5] = Some([51, 51, 51, 255]);
        bytes[6] = Some([102, 0, 0, 255]);
        bytes[7] = Some([102, 0, 0, 255]);
        bytes[8] = Some([0, 0, 0, 255]);
        bytes[9] = Some([1, 2, 3, 255]);
        bytes[10] = Some([4, 5, 6, 255]);
        bytes[11] = Some([56, 0, 255, 255]);
        bytes[12] = Some([7, 8, 9, 255]);
        bytes[13] = Some([10, 11, 12, 255]);
        // Half the way from the near plane's white to black: 2^-12 of its depth.
        let half: f64 = 255.0 * (1.0 - 12.0 / 24.0);
        bytes[CAPTURES - 1] = Some([half.round() as u8, 0, 0, 255]);
        let perspective = Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        };
        let probe = probe_of(bytes, &looked(Some(perspective)));
        assert_eq!(probe["at"], json!([1.0, 0.5]));
        assert_eq!(probe["size"], json!([4, 2]));
        assert_eq!(probe["ticks"], 9);
        assert_eq!(probe["shown"], json!([10, 20, 30]));
        close(&probe["base_color"][0], 0.5, 0.01);
        close(&probe["base_color"][1], 1.0, 1e-9);
        close(&probe["base_color"][2], 0.0, 1e-9);
        close(&probe["normal"][0], 0.0, 0.005);
        close(&probe["normal"][1], 1.0, 1e-9);
        close(&probe["normal"][2], -1.0, 1e-9);
        close(&probe["surface_normal"][0], -1.0, 1e-9);
        close(&probe["surface_normal"][1], 0.0, 0.005);
        close(&probe["surface_normal"][2], 1.0, 1e-9);
        // Their value is red's alone.
        close(&probe["metallic"], 1.0, 1e-9);
        close(&probe["roughness"], 0.2, 1e-9);
        close(&probe["occlusion"], 0.4, 1e-9);
        close(&probe["emission"][0], linear(102), 1e-9);
        assert_eq!(probe["direct_light"], json!([0, 0, 0]));
        assert_eq!(probe["sky_light"], json!([1, 2, 3]));
        assert_eq!(probe["reflection"], json!([4, 5, 6]));
        // A dielectric's 0.04, sRGB's byte 56, in red.
        close(&probe["specular_color"][0], 0.04, 0.001);
        close(&probe["specular_color"][2], 1.0, 1e-9);
        assert_eq!(probe["diffuse_light"], json!([7, 8, 9]));
        assert_eq!(probe["specular_light"], json!([10, 11, 12]));
        // Byte 128 of 255 is half of the way, 2^-12 of the depth, a little off from the 12 it
        // rounds from.
        let depth = probe["depth"].as_f64().expect("a depth");
        assert!((1.0 / 4096.0..1.0 / 3800.0).contains(&depth), "{depth}");
        close(&probe["distance"], 0.1 / depth, 1e-4);
        assert_eq!(
            probe["entity"],
            json!({"id": "2b7e151628aed2a6", "name": null})
        );
        assert!(
            probe["quantized"]
                .as_str()
                .is_some_and(|note| note.contains("8-bit"))
        );
    }

    /// A depth byte of 255 is the near plane's, whose distance is the near plane; 0 is where nothing
    /// drew, with none; and an orthographic view, or none seen, has no distance at all.
    #[test]
    fn a_depth_has_a_distance_where_something_drew_in_a_perspective() {
        let perspective = Projection::Perspective {
            fov: 1.0,
            near: 0.1,
        };
        let orthographic = Projection::Orthographic {
            height: 2.0,
            near: -1.0,
            far: 1.0,
        };
        for (byte, projection, depth, distance) in [
            (255, Some(perspective), Some(1.0), Some(0.1)),
            (0, Some(perspective), Some(0.0), None),
            (255, Some(orthographic), Some(1.0), None),
            (255, None, Some(1.0), None),
        ] {
            let mut bytes = [Some([0, 0, 0, 255]); CAPTURES];
            bytes[CAPTURES - 1] = Some([byte, byte, byte, 255]);
            let probe = probe_of(bytes, &looked(projection));
            assert_eq!(probe["depth"].as_f64(), depth, "{byte} {projection:?}");
            match distance {
                Some(distance) => close(&probe["distance"], distance, 1e-6),
                None => assert!(probe["distance"].is_null(), "{byte} {projection:?}"),
            }
        }
    }

    /// A pixel outside the view is `null` in every value, the view's size and the entity still
    /// answered.
    #[test]
    fn a_pixel_outside_the_view_has_no_values() {
        let probe = probe_of([None; CAPTURES], &looked(None));
        let nulls = SHADING.iter().chain(&["shown", "depth", "distance"]);
        for name in nulls {
            assert!(probe[*name].is_null(), "{name}");
        }
        assert_eq!(probe["size"], json!([4, 2]));
    }

    /// A probe sends a capture of the pixel for the view, each part of shading and the depth, then a
    /// pick and a look, through one paused world, and answers what they decode to; and its pixel
    /// can't be negative or not finite.
    #[test]
    fn a_probe_captures_picks_and_looks_in_one_paused_world() {
        let mut got = Vec::new();
        let call = |method: &str, params: Value| {
            got.push(match params.get("show") {
                Some(show) => format!("{method} {show}"),
                None => method.to_owned(),
            });
            let answer = match method {
                "status" => json!({
                    "ticks": 0, "paused": true, "stopped": false, "frames": 0,
                    "entities": 0, "view": null, "scene": null, "unsaved": false,
                }),
                "pick" => json!({"id": null, "name": null}),
                "look" => json!({
                    "viewpoint": null, "outline": null, "grid": null, "colliders": null,
                    "by": {}, "revision": 0, "run": 1, "seen": null,
                }),
                _ => {
                    assert_eq!(params["at"], json!([3.0, 1.0]));
                    assert_eq!(params["own"], json!(true));
                    json!({"path": null, "width": 4, "height": 2, "pixel": [255, 0, 0, 255],
                           "ticks": 0})
                }
            };
            Ok(RawValue::from_string(answer.to_string()).expect("JSON"))
        };
        let probe = probed(call, [3.0, 1.0], true).expect("a probe");
        let mut want = vec!["status".to_owned(), "capture".to_owned()];
        want.extend(SHADING.iter().map(|name| format!("capture \"{name}\"")));
        want.extend([r#"capture {"image":"view depth"}"#, "pick", "look"].map(str::to_owned));
        assert_eq!(got, want);
        assert_eq!(probe["distance"], Value::Null);
        assert_eq!(probe["metallic"], json!(1.0));
        assert_eq!(pixel([3.0, 0.5]).ok(), Some([3.0, 0.5]));
        for at in [
            [-1.0, 0.0],
            [0.0, -0.5],
            [f64::NAN, 0.0],
            [0.0, f64::INFINITY],
        ] {
            let code = pixel(at).err().map(|failure| failure.code);
            assert_eq!(code.as_deref(), Some("gg.args"), "{at:?}");
        }
    }
}
