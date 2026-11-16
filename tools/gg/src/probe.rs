//! `gg probe`: what shading takes and makes at one pixel of a game's view, as JSON.

use gg_api::Failure;
#[cfg(test)]
use gg_api::probe::{CAPTURES, linear};
#[cfg(test)]
use gg_api::{Captured, Looked, Picked, Projection};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use serde_json::value::RawValue;

#[cfg(test)]
pub(crate) use gg_api::probe::SHADING;

use crate::game::{self, Pick};

/// `at`, a pixel `[x, y]` from the view's top left.
///
/// Fails with `gg.args` for a number not finite or negative.
pub(crate) fn pixel(at: [f64; 2]) -> Result<[f64; 2], Failure> {
    if at.iter().all(|axis| axis.is_finite() && *axis >= 0.0) {
        return Ok(at);
    }
    let message = format!("a pixel's X and Y are numbers, not negative, not {at:?}");
    Err(Failure::new("gg.args", message))
}

/// Probes pixel `at` of the game's view, or its own with `own`.
///
/// Fails as the calls do, and with `api.protocol` for an answer the API doesn't define.
pub(crate) fn run(pick: &Pick, at: [f64; 2], own: bool) -> Result<Value, Failure> {
    let mut client = game::connect(pick)?;
    let call = |method: &str, params: Value| {
        (client.call(method, params)).map_err(|failure| game::waits(pick, method, failure))
    };
    probed(call, at, own)
}

/// [`run`] through `call`.
fn probed(
    call: impl FnMut(&str, Value) -> Result<Box<RawValue>, Failure>,
    at: [f64; 2],
    own: bool,
) -> Result<Value, Failure> {
    gg_api::probe::probe(call, at, own).map(|probe| printed(&probe))
}

#[cfg(test)]
fn decode(
    captured: &[Captured; CAPTURES],
    at: [f64; 2],
    picked: &Picked,
    looked: &Looked,
) -> Value {
    printed(&gg_api::probe::decode(captured, at, picked, looked))
}

/// `probe` as JSON, fields in name order.
fn printed(probe: &gg_api::probe::Probe) -> Value {
    serde_json::to_value(probe).expect("a probe always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capture of a 4 by 2 view.
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

    /// The probe of captures of `bytes`, in `decode`'s order.
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

    /// Each field reads its bytes as the `Shown` docs say: sRGB colors as linear, normals in -1
    /// to 1, scalars from red, lights as sRGB bytes, depth reversed, distance from the near plane.
    #[test]
    fn a_probe_reads_each_part_as_its_bytes_say() {
        let mut bytes = [Some([0, 0, 0, 255]); CAPTURES];
        // The view, shading's parts, the depth.
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
        // Halfway from the near plane's white to black: 2^-12 of its depth.
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
        // Red alone.
        close(&probe["metallic"], 1.0, 1e-9);
        close(&probe["roughness"], 0.2, 1e-9);
        close(&probe["occlusion"], 0.4, 1e-9);
        close(&probe["emission"][0], linear(102), 1e-9);
        assert_eq!(probe["direct_light"], json!([0, 0, 0]));
        assert_eq!(probe["sky_light"], json!([1, 2, 3]));
        assert_eq!(probe["reflection"], json!([4, 5, 6]));
        // A dielectric's 0.04: sRGB byte 56, in red.
        close(&probe["specular_color"][0], 0.04, 0.001);
        close(&probe["specular_color"][2], 1.0, 1e-9);
        assert_eq!(probe["diffuse_light"], json!([7, 8, 9]));
        assert_eq!(probe["specular_light"], json!([10, 11, 12]));
        // Byte 128 of 255: 2^-12 of the depth, a little off from the 12 it rounds.
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

    /// Depth 255 is the near plane, at its distance; 0, nothing drawn, has none; an orthographic
    /// view, or none seen, has none.
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

    /// A pixel outside the view is `null` in every value; the size and entity still answer.
    #[test]
    fn a_pixel_outside_the_view_has_no_values() {
        let probe = probe_of([None; CAPTURES], &looked(None));
        let nulls = SHADING.iter().chain(&["shown", "depth", "distance"]);
        for name in nulls {
            assert!(probe[*name].is_null(), "{name}");
        }
        assert_eq!(probe["size"], json!([4, 2]));
    }

    /// A probe captures the view, shading's parts and the depth, then picks and looks, in one
    /// paused world; its pixel can't be negative or not finite.
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
