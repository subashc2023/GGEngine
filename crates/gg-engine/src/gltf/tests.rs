use std::cell::Cell;

use serde_json::{Value, json};

use gg_core::math::{DVec3, Mat4, Quat, Vec3, Vec4};
use gg_renderer::{AlphaMode, Workflow, Wrap};

use super::buffers::{FLOAT, UNSIGNED_BYTE, percent_decode};
use super::*;
use crate::LightKind;
use crate::animation::{Influences, Interpolation, Path as Moved, place};
use crate::image;

/// A BIN chunk as tests build it, and its buffer views, each starting at a multiple of 4 bytes.
#[derive(Default)]
struct Bin {
    bytes: Vec<u8>,
    views: Vec<Value>,
}

impl Bin {
    /// Adds `bytes` as a buffer view of buffer 0, its elements `stride` apart if that's given,
    /// and returns its index.
    fn view(&mut self, bytes: &[u8], stride: Option<u32>) -> usize {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        let mut view = json!({
            "buffer": 0,
            "byteOffset": self.bytes.len(),
            "byteLength": bytes.len(),
        });
        if let Some(stride) = stride {
            view["byteStride"] = json!(stride);
        }
        self.bytes.extend_from_slice(bytes);
        self.views.push(view);
        self.views.len() - 1
    }
}

/// `values` as glTF stores floats.
fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

const PIXELS: [u8; 8] = [10, 20, 30, 255, 40, 50, 60, 128];

/// A PNG of `PIXELS`, two pixels in a row.
fn png() -> Vec<u8> {
    image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 2, &PIXELS, None)
}

/// `bytes` in base64, padded.
fn encode(bytes: &[u8]) -> String {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let bits = (chunk.iter().enumerate())
            .fold(0, |bits, (i, &byte)| bits | u32::from(byte) << (16 - 8 * i));
        for i in 0..4 {
            let sextet = (bits >> (18 - 6 * i) & 63) as usize;
            text.push(if i <= chunk.len() {
                char::from(alphabet[sextet])
            } else {
                '='
            });
        }
    }
    text
}

/// A GLB of `json`, and of `bin` as its BIN chunk unless it's empty, each padded as glTF pads
/// them.
fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let pad = |bytes: &[u8], with: u8| {
        let mut padded = bytes.to_vec();
        while !padded.len().is_multiple_of(4) {
            padded.push(with);
        }
        padded
    };
    let mut chunks = vec![(JSON_CHUNK, pad(json.as_bytes(), b' '))];
    if !bin.is_empty() {
        chunks.push((BIN_CHUNK, pad(bin, 0)));
    }
    let length = 12 + chunks.iter().map(|(_, data)| 8 + data.len()).sum::<usize>();
    let mut file = Vec::new();
    for word in [GLB_MAGIC, 2, length as u32] {
        file.extend(word.to_le_bytes());
    }
    for (kind, data) in chunks {
        file.extend((data.len() as u32).to_le_bytes());
        file.extend(kind.to_le_bytes());
        file.extend(data);
    }
    file
}

/// Reads `file` as model "m" at `path` in folder "models", with `files` beside it there, each
/// as far as it's asked, as `dirs::read_file` reads.
fn read_files(path: &str, file: &[u8], files: &[(&str, &[u8])]) -> Result<Imported> {
    let folder = Path::new("models");
    let read_file = |asked: &Path, most: u64| {
        let bytes = if asked == folder.join(path) {
            file
        } else {
            (files.iter().find(|(name, _)| folder.join(name) == asked))
                .map(|&(_, bytes)| bytes)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?
        };
        Ok(bytes[..bytes.len().min(usize::try_from(most).unwrap_or(usize::MAX))].to_vec())
    };
    read("m", &folder.join(path), read_file, MAX_MEMORY)
}

fn read_glb(file: &[u8]) -> Result<Imported> {
    read_files("m.glb", file, &[])
}

/// A GLB of a model of each kind of node, primitive and material that reading takes.
pub(crate) fn model() -> Vec<u8> {
    let mut bin = Bin::default();
    // Three vertices' positions and texture coordinates, interleaved.
    let interleaved = bin.view(
        &floats(&[0., 0., 0., 0., 0., 1., 0., 0., 1., 0., 0., 1., 0., 0., 1.]),
        Some(20),
    );
    let bytes = bin.view(&[0, 1, 2], None);
    let shorts = bin.view(&[2, 0, 1, 0, 0, 0], None);
    // Normalized RGB, with a byte after each, as glTF aligns vertices' attributes.
    let colors = bin.view(&[255, 0, 0, 7, 0, 255, 0, 7, 0, 0, 255, 7], Some(4));
    let positions = bin.view(&floats(&[0., 0., 1., 1., 0., 1., 0., 1., 1.]), None);
    // Normalized 16-bit RGBA, little-endian: 1, 0 and 0x8000 / 0xffff.
    let wide = bin.view(
        &[
            0xff, 0xff, 0, 0, 0, 0x80, 0xff, 0xff, // the first vertex
            0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, // the second
            0, 0x80, 0, 0x80, 0, 0x80, 0, 0, // the third
        ],
        None,
    );
    let image = bin.view(&png(), None);
    let normals = bin.view(&floats(&[0., 0., 1., 0., 0., 1., 0., 0., 1.]), None);
    let gltf = json!({
        "asset": {"version": "2.0", "generator": "a test"},
        "extensionsUsed": [
            "KHR_materials_unlit",
            "KHR_texture_transform",
            "KHR_lights_punctual",
            "KHR_materials_emissive_strength",
        ],
        "extensionsRequired": ["KHR_materials_unlit"],
        "extensions": {"KHR_lights_punctual": {"lights": [
            {"type": "point", "color": [1, 0.5, 0.25], "intensity": 40, "range": 6},
        ]}},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [
            // Mirrored and moved, by a matrix.
            {
                "name": "frame",
                "matrix": [-2, 0, 0, 0, 0, 3, 0, 0, 0, 0, 4, 0, 1, 2, 3, 1],
                "mesh": 0,
                "children": [1, 2],
            },
            // A rotation of length 2, which is the identity once it's unit, and morph weights
            // of 0, which replace its mesh's; named nothing.
            {
                "name": "",
                "translation": [0, 5, 0],
                "rotation": [0, 0, 0, 2],
                "mesh": 1,
                "weights": [0],
            },
            // The first mesh again, and a light.
            {
                "translation": [7, 0, 0],
                "mesh": 0,
                "extensions": {"KHR_lights_punctual": {"light": 0}},
            },
        ],
        "meshes": [
            {"name": "pair", "primitives": [
                {
                    "attributes": {"POSITION": 0, "TEXCOORD_0": 1, "NORMAL": 7},
                    "indices": 2,
                    "material": 0,
                },
                {
                    "attributes": {"POSITION": 0, "TEXCOORD_0": 1, "COLOR_0": 4},
                    "indices": 3,
                    "material": 1,
                    "mode": 4,
                },
            ]},
            // A primitive without positions, which isn't drawn, then one without indices or
            // a material, with 16-bit colors.
            {"weights": [0.5], "primitives": [
                {"attributes": {}},
                {"attributes": {"POSITION": 5, "COLOR_0": 6}},
            ]},
        ],
        "accessors": [
            {"bufferView": interleaved, "componentType": 5126, "count": 3, "type": "VEC3"},
            {
                "bufferView": interleaved,
                "byteOffset": 12,
                "componentType": 5126,
                "count": 3,
                "type": "VEC2",
            },
            {"bufferView": bytes, "componentType": 5121, "count": 3, "type": "SCALAR"},
            {"bufferView": shorts, "componentType": 5123, "count": 3, "type": "SCALAR"},
            {
                "bufferView": colors,
                "componentType": 5121,
                "normalized": true,
                "count": 3,
                "type": "VEC3",
            },
            {"bufferView": positions, "componentType": 5126, "count": 3, "type": "VEC3"},
            {
                "bufferView": wide,
                "componentType": 5123,
                "normalized": true,
                "count": 3,
                "type": "VEC4",
            },
            {"bufferView": normals, "componentType": 5126, "count": 3, "type": "VEC3"},
        ],
        "materials": [
            {
                "pbrMetallicRoughness": {
                    "baseColorFactor": [1, 0.5, 0.25, 1],
                    "baseColorTexture": {"index": 0},
                    "metallicFactor": 0.25,
                    "roughnessFactor": 0.5,
                },
                // Of the image that its base color samples as colors, so left out.
                "occlusionTexture": {"index": 1, "strength": 0.5},
                "emissiveFactor": [1, 0.5, 0],
                "extensions": {
                    "KHR_materials_emissive_strength": {"emissiveStrength": 2},
                    "KHR_materials_unlit": {},
                },
                "alphaMode": "MASK",
                "alphaCutoff": 0.25,
            },
            {
                "pbrMetallicRoughness": {"baseColorTexture": {"index": 1, "texCoord": 0}},
                "normalTexture": {"index": 2, "scale": 0.5},
                // Of another set of texture coordinates, so left out.
                "emissiveTexture": {"index": 1, "texCoord": 1},
                "alphaMode": "BLEND",
                "doubleSided": true,
            },
        ],
        // Two textures of one image, and one of a second.
        "textures": [{"source": 0, "sampler": 0}, {"source": 0}, {"source": 1}],
        "samplers": [{"magFilter": 9728, "minFilter": 9986, "wrapS": 33071, "wrapT": 33071}],
        // Said to be a JPEG, but a PNG, as its bytes say; and the same PNG again.
        "images": [{"bufferView": image, "mimeType": "image/jpeg"}, {"bufferView": image}],
        "buffers": [{"byteLength": bin.bytes.len()}],
        "bufferViews": bin.views,
        "animations": [{"channels": [], "samplers": []}],
        "cameras": [{"type": "perspective"}, {"type": "orthographic"}],
    });
    glb(&gltf.to_string(), &bin.bytes)
}

/// A model reads into its parts, each node's name and light among them; its primitives, with
/// their normals, or made flat without them, and their tangents, made by MikkTSpace for a
/// normal map's, and their materials, with each texture and factor and the extensions the
/// engine reads; and its images; leaving out what it says it leaves out.
#[test]
fn a_model_reads_into_parts_primitives_and_images() {
    let imported = read_glb(&model()).expect("a model");
    let part = |transform, parent, primitive| Part {
        name: None,
        transform,
        parent,
        primitive,
        light: None,
        skin: None,
    };
    let lit = |transform, parent, primitive| Part {
        light: Some(Light {
            kind: LightKind::Point,
            color: Vec3::new(1.0, 0.5, 0.25),
            intensity: 40.0,
            range: Some(6.0),
            ..Light::default()
        }),
        ..part(transform, parent, primitive)
    };
    let moved = |x, y, z| Transform {
        translation: DVec3::new(x, y, z),
        ..Transform::IDENTITY
    };
    let mirrored = Transform {
        translation: DVec3::new(1.0, 2.0, 3.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::new(-2.0, 3.0, 4.0),
    };
    let identity = Transform::IDENTITY;
    assert_eq!(
        imported.parts,
        [
            // The first node, with each of its mesh's two primitives as a child, unnamed.
            Part {
                name: Some(Arc::from("frame")),
                ..part(mirrored, None, None)
            },
            part(identity, Some(0), Some(0)),
            part(identity, Some(0), Some(1)),
            part(moved(0.0, 5.0, 0.0), Some(0), Some(2)),
            // The same primitives again, read once.
            lit(moved(7.0, 0.0, 0.0), Some(0), None),
            part(identity, Some(4), Some(0)),
            part(identity, Some(4), Some(1)),
        ]
    );
    let triangle = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    let white = vec![[1.0; 4]; 3];
    let half = 32768.0 / 65535.0;
    let (front, back) = (vec![[0.0, 0.0, 1.0]; 3], vec![[0.0, 0.0, -1.0]; 3]);
    let unused = vec![[1.0, 0.0, 0.0, 1.0]; 3];
    let sampled = |image, nearest, wrap| {
        Some(Sampling {
            image,
            nearest,
            wrap,
        })
    };
    let mut primitives = imported.primitives.clone();
    // MikkTSpace's, along +X, the bitangent cross(normal, tangent) * W along +Y, as V runs up
    // the triangle facing -Z: compared within rounding, then as exactly that.
    let made = &mut primitives[1].tangents;
    for tangent in made.iter() {
        let near = Vec4::from(*tangent).abs_diff_eq(Vec4::new(1.0, 0.0, 0.0, -1.0), 1e-6);
        assert!(near, "{tangent:?}");
    }
    *made = vec![[1.0, 0.0, 0.0, -1.0]; 3];
    assert_eq!(
        primitives,
        [
            Primitive {
                name: "pair/0".to_owned(),
                positions: triangle.clone(),
                uvs: uvs.clone(),
                colors: white,
                normals: front.clone(),
                tangents: unused.clone(),
                indices: vec![0, 1, 2],
                mesh: None,
                material: Material {
                    color: [1.0, 0.5, 0.25, 1.0],
                    workflow: Workflow::MetallicRoughness {
                        metallic: 0.25,
                        roughness: 0.5,
                    },
                    occlusion_strength: 0.5,
                    emissive: [2.0, 1.0, 0.0],
                    textures: [sampled(0, true, Wrap::Clamp), None, None, None, None],
                    alpha: AlphaMode::Mask { cutoff: 0.25 },
                    unlit: true,
                    ..Material::default()
                },
                joints: Vec::new(),
                weights: Vec::new(),
                reach: None,
            },
            // Without normals, so flat: each triangle's corners its own, in its order.
            Primitive {
                name: "pair/1".to_owned(),
                positions: triangle.into_iter().rev().collect(),
                uvs: uvs.into_iter().rev().collect(),
                colors: vec![
                    [0.0, 0.0, 1.0, 1.0],
                    [0.0, 1.0, 0.0, 1.0],
                    [1.0, 0.0, 0.0, 1.0],
                ],
                normals: back,
                tangents: vec![[1.0, 0.0, 0.0, -1.0]; 3],
                indices: vec![0, 1, 2],
                mesh: None,
                material: Material {
                    normal_scale: 0.5,
                    textures: [
                        sampled(0, false, Wrap::Repeat),
                        None,
                        sampled(1, false, Wrap::Repeat),
                        None,
                        None,
                    ],
                    alpha: AlphaMode::Blend,
                    double_sided: true,
                    ..Material::default()
                },
                joints: Vec::new(),
                weights: Vec::new(),
                reach: None,
            },
            Primitive {
                name: "1/1".to_owned(),
                positions: vec![[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]],
                uvs: vec![[0.0; 2]; 3],
                colors: vec![
                    [1.0, 0.0, half, 1.0],
                    [0.0, 1.0, 0.0, 1.0],
                    [half, half, half, 0.0]
                ],
                normals: front,
                tangents: unused,
                indices: vec![0, 1, 2],
                mesh: None,
                material: Material::default(),
                joints: Vec::new(),
                weights: Vec::new(),
                reach: None,
            },
        ]
    );
    // Read as far as their headers, the PNG they are, which decodes as the loader decodes it.
    let [colors, normals] = &imported.images[..] else {
        panic!("two images: {:?}", imported.images);
    };
    for read in [colors, normals] {
        assert_eq!((read.size, read.format), ([2, 1], image::Format::Png));
    }
    let image = Image {
        size: [2, 1],
        pixels: PIXELS.to_vec(),
        format: image::Format::Rgba8,
    };
    let mut decoded = imported.clone();
    decoded
        .decode_images("m", Path::new("m.glb"))
        .expect("its images decode");
    assert_eq!(decoded.images, [image.clone(), image]);
    // Its animation of no channels, a clip that moves nothing, named by its index.
    let clips: Vec<(&str, f32, usize)> = (imported.clips.iter())
        .map(|clip| (clip.name.as_str(), clip.duration, clip.channels.len()))
        .collect();
    assert_eq!(clips, [("0", 0.0, 0)]);
    assert_eq!(
        imported.ignored,
        [
            "2 cameras",
            "extension KHR_texture_transform",
            "material 0's occlusionTexture, which samples image 0 as values where another \
                 texture samples it as colors",
            "material 1's emissiveTexture, which reads TEXCOORD_1 where its others read \
                 TEXCOORD_0",
        ]
    );
}

#[test]
fn a_model_reads_the_same_every_time() {
    let model = model();
    let read = || read_glb(&model).expect("a model");
    assert_eq!(read(), read());
}

#[test]
fn a_model_cut_short_or_with_a_byte_changed_fails_without_panicking() {
    let model = model();
    for length in 0..model.len() {
        assert!(read_glb(&model[..length]).is_err(), "{length} bytes");
    }
    // Some changes, such as to padding, leave a model that reads.
    for at in 0..model.len() {
        let mut changed = model.clone();
        changed[at] = 0xff;
        let _ = read_glb(&changed);
    }
}

#[test]
fn a_gltf_reads_buffers_and_images_from_data_uris_and_files_beside_it() {
    let positions_and_uvs = floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 1.]);
    // A file a byte longer than its buffer.
    let indices: Vec<u8> = ([0u32, 2, 1].iter().flat_map(|index| index.to_le_bytes()))
        .chain([9])
        .collect();
    let gltf = json!({
        "asset": {"version": "2.0"},
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"name": "triangle", "primitives": [{
            "attributes": {"POSITION": 0, "TEXCOORD_0": 1},
            "indices": 2,
            "material": 0,
        }]}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
            {
                "bufferView": 0,
                "byteOffset": 36,
                "componentType": 5126,
                "count": 3,
                "type": "VEC2",
            },
            {"bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR"},
        ],
        "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
        "textures": [{"source": 0}],
        "images": [{"uri": "textures/a%20b.png"}],
        "buffers": [
            {
                "byteLength": 60,
                "uri": format!(
                    "data:application/octet-stream;base64,{}",
                    encode(&positions_and_uvs)
                ),
            },
            {"byteLength": 12, "uri": "my%20data.bin"},
        ],
        "bufferViews": [{"buffer": 0, "byteLength": 60}, {"buffer": 1, "byteLength": 12}],
    });
    let png = png();
    let files: [(&str, &[u8]); 2] = [("my data.bin", &indices), ("textures/a b.png", &png)];
    let mut imported = read_files("m.gltf", gltf.to_string().as_bytes(), &files).expect("a model");
    let decoded = imported.decode_images("m", Path::new("m.gltf"));
    decoded.expect("its image decodes");
    let part = Part {
        name: None,
        transform: Transform::IDENTITY,
        parent: None,
        primitive: Some(0),
        light: None,
        skin: None,
    };
    assert_eq!(imported.parts, [part]);
    // Without normals, so flat: each corner its own, in the triangle's order.
    let primitive = Primitive {
        name: "triangle/0".to_owned(),
        positions: vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
        uvs: vec![[0.0, 0.0], [0.0, 1.0], [1.0, 0.0]],
        colors: vec![[1.0; 4]; 3],
        normals: vec![[0.0, 0.0, -1.0]; 3],
        tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
        indices: vec![0, 1, 2],
        mesh: None,
        material: Material {
            textures: [
                Some(Sampling {
                    image: 0,
                    nearest: false,
                    wrap: Wrap::Repeat,
                }),
                None,
                None,
                None,
                None,
            ],
            ..Material::default()
        },
        joints: Vec::new(),
        weights: Vec::new(),
        reach: None,
    };
    assert_eq!(imported.primitives, [primitive]);
    assert_eq!(imported.images[0].pixels, PIXELS);

    // From the disk, listing the files read, its own first, as hot reload watches them.
    let scratch = crate::dirs::tests::Scratch::new("gltf-files");
    let folder = scratch.0.join("models");
    std::fs::create_dir_all(folder.join("textures")).expect("a directory");
    let model = folder.join("m.gltf");
    std::fs::write(&model, gltf.to_string()).expect("the model");
    for (name, bytes) in files {
        std::fs::write(folder.join(name), bytes).expect("a file it names");
    }
    let (mut read, listed) = read_listing("m", &model, &Files::default()).expect("a model");
    let decoded = read.decode_images("m", &model);
    decoded.expect("its image decodes");
    assert_eq!(read, imported);
    let mut named: Vec<PathBuf> = listed[1..].to_vec();
    named.sort();
    let beside = ["my data.bin", "textures/a b.png"].map(|name| folder.join(name));
    assert_eq!((&listed[0], named.as_slice()), (&model, beside.as_slice()));
}

#[test]
fn a_glb_that_doesnt_hold_together_is_refused() {
    let json = r#"{"asset": {"version": "2.0"}, "scenes": [{"nodes": []}]}"#;
    let whole = glb(json, &[]);
    assert_eq!(
        read_glb(&whole).expect("an empty scene"),
        Imported::default()
    );
    let with = |at: usize, word: u32| {
        let mut file = whole.clone();
        file[at..at + 4].copy_from_slice(&word.to_le_bytes());
        file
    };
    let (unsupported, bad) = ("engine.unsupported_model", "engine.bad_model");
    let cases = [
        ("too short", whole[..10].to_vec(), bad, "too short"),
        (
            "another magic",
            with(0, u32::from_le_bytes(*b"glTX")),
            bad,
            "isn't glTF's JSON",
        ),
        ("version 1", with(4, 1), unsupported, "version 1"),
        ("version 3", with(4, 3), bad, "version 3"),
        (
            "longer than its bytes",
            with(8, whole.len() as u32 + 4),
            bad,
            "header says",
        ),
        ("a chunk past its end", with(12, 1000), bad, "past its end"),
        ("no chunks", with(8, 12), bad, "without a JSON chunk"),
        (
            "a BIN chunk first",
            with(16, BIN_CHUNK),
            bad,
            "doesn't start with a JSON chunk",
        ),
    ];
    for (case, file, code, problem) in cases {
        let error = read_glb(&file).expect_err(case);
        assert_eq!(error.code(), code, "{case}: {error}");
        assert!(error.to_string().contains(problem), "{case}: {error}");
    }
}

#[test]
fn base64_decodes_as_rfc_4648_has_it() {
    let vectors = [
        ("", ""),
        ("Zg==", "f"),
        ("Zm8=", "fo"),
        ("Zm9v", "foo"),
        ("Zm9vYg==", "foob"),
        ("Zm9vYmE=", "fooba"),
        ("Zm9vYmFy", "foobar"),
    ];
    for (text, bytes) in vectors {
        assert_eq!(base64(text).as_deref(), Some(bytes.as_bytes()), "{text}");
    }
    let every_byte: Vec<u8> = (0..=255).collect();
    for length in [254, 255, 256] {
        let bytes = &every_byte[..length];
        assert_eq!(base64(&encode(bytes)).as_deref(), Some(bytes));
    }
    // Characters outside the alphabet, such as URL-safe base64's, and padding missing or out
    // of place.
    for text in [
        "Zm9v!A==",
        "Zm9v YmFy",
        "Zm9v-_==",
        "Zg",
        "Zg=",
        "Z===",
        "Zg==Zg==",
        "Zm=v",
    ] {
        assert_eq!(base64(text), None, "{text}");
    }
}

#[test]
fn uris_percent_decode() {
    let decoded = [
        ("my%20data.bin", "my data.bin"),
        ("grande_sph%C3%A8re.png", "grande_sph\u{e8}re.png"),
        ("grande_sph\u{e8}re.png", "grande_sph\u{e8}re.png"),
    ];
    for (uri, path) in decoded {
        assert_eq!(percent_decode(uri).as_deref(), Some(path), "{uri}");
    }
    for uri in ["%zz", "a%2", "a%", "%+f", "%FF"] {
        assert_eq!(percent_decode(uri), None, "{uri}");
    }
}

/// The JSON and BIN chunk of a model of one textured triangle, for tests to break.
fn triangle() -> (Value, Vec<u8>) {
    let mut bin = Bin::default();
    let positions = bin.view(&floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]), None);
    let uvs = bin.view(&floats(&[0., 0., 1., 0., 0., 1.]), None);
    // A fourth index, past the vertices, for a test to read.
    let indices = bin.view(&[0, 0, 1, 0, 2, 0, 3, 0], None);
    let image = bin.view(&png(), None);
    // View 4: NaNs, for a test to read as positions.
    bin.view(&[0xff; 12], None);
    let gltf = json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{
            "attributes": {"POSITION": 0, "TEXCOORD_0": 1},
            "indices": 2,
            "material": 0,
        }]}],
        "accessors": [
            {"bufferView": positions, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": uvs, "componentType": 5126, "count": 3, "type": "VEC2"},
            {"bufferView": indices, "componentType": 5123, "count": 3, "type": "SCALAR"},
        ],
        "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
        "textures": [{"source": 0, "sampler": 0}],
        "samplers": [{}],
        "images": [{"bufferView": image, "mimeType": "image/png"}],
        "buffers": [{"byteLength": bin.bytes.len()}],
        "bufferViews": bin.views,
    });
    (gltf, bin.bytes)
}

/// A model of a triangle without normals, so drawn flat, that a skin of two joints moves, a
/// root's and its child's, a unit along X: its first corner the root's alone, its second half
/// each, its third the child's; and an animation, "Turn", that turns the child a quarter about
/// Z over a second, along a line, lifts the root two units at once at half a second, by steps,
/// and moves node 3, outside the scene.
fn skinned() -> (Value, Vec<u8>) {
    let mut bin = Bin::default();
    let positions = bin.view(&floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]), None);
    let joints = bin.view(&[0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0], None);
    let weights = floats(&[1., 0., 0., 0., 0.5, 0.5, 0., 0., 1., 0., 0., 0.]);
    let weights = bin.view(&weights, None);
    let mut binds = Mat4::IDENTITY.to_cols_array().to_vec();
    binds.extend(Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0)).to_cols_array());
    let binds = bin.view(&floats(&binds), None);
    let times = bin.view(&floats(&[0.0, 1.0]), None);
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let turns = bin.view(&floats(&[0., 0., 0., 1., 0., 0., half, half]), None);
    let steps = bin.view(&floats(&[0.0, 0.5]), None);
    let lifts = bin.view(&floats(&[0., 0., 0., 0., 2., 0.]), None);
    let gltf = json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0, 2]}],
        "nodes": [
            {"children": [1]},
            {"translation": [1.0, 0.0, 0.0]},
            {"mesh": 0, "skin": 0},
            {},
        ],
        "meshes": [{"primitives": [{
            "attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2},
        }]}],
        "skins": [{"joints": [0, 1], "inverseBindMatrices": 3}],
        "animations": [{
            "name": "Turn",
            "channels": [
                {"sampler": 0, "target": {"node": 1, "path": "rotation"}},
                {"sampler": 1, "target": {"node": 0, "path": "translation"}},
                {"sampler": 1, "target": {"node": 3, "path": "translation"}},
            ],
            "samplers": [
                {"input": 4, "output": 5},
                {"input": 6, "output": 7, "interpolation": "STEP"},
            ],
        }],
        "accessors": [
            {"bufferView": positions, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": joints, "componentType": 5121, "count": 3, "type": "VEC4"},
            {"bufferView": weights, "componentType": 5126, "count": 3, "type": "VEC4"},
            {"bufferView": binds, "componentType": 5126, "count": 2, "type": "MAT4"},
            {"bufferView": times, "componentType": 5126, "count": 2, "type": "SCALAR"},
            {"bufferView": turns, "componentType": 5126, "count": 2, "type": "VEC4"},
            {"bufferView": steps, "componentType": 5126, "count": 2, "type": "SCALAR"},
            {"bufferView": lifts, "componentType": 5126, "count": 2, "type": "VEC3"},
        ],
        "buffers": [{"byteLength": bin.bytes.len()}],
        "bufferViews": bin.views,
    });
    (gltf, bin.bytes)
}

/// A skinned model reads its skin, its joints' parts and their inverse bind matrices; its
/// primitive's joints and weights, which follow its corners as it's made flat; its skinned
/// part's skin; and its animation's channels of nodes in its scene, as clips, sampled along
/// their keys, leaving out the one of a node outside it. Its primitive reaches as far as its
/// joints carry it in every pose, with room to spare.
#[test]
fn a_skinned_model_reads_its_skin_its_weights_and_its_clips() {
    let (gltf, bin) = skinned();
    let imported = read_glb(&glb(&gltf.to_string(), &bin)).expect("a skinned model");
    let parts: Vec<(Option<usize>, Option<usize>, Option<usize>)> = (imported.parts.iter())
        .map(|part| (part.parent, part.primitive, part.skin))
        .collect();
    assert_eq!(
        parts,
        [
            (None, None, None),
            (Some(0), None, None),
            (None, Some(0), Some(0))
        ]
    );
    let unbound = Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0));
    let skin = Skin {
        joints: vec![0, 1],
        inverse_binds: vec![Mat4::IDENTITY, unbound],
    };
    assert_eq!(imported.skins, [skin]);
    let primitive = &imported.primitives[0];
    assert_eq!(primitive.joints, [[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]]);
    let weights = [
        [1.0, 0.0, 0.0, 0.0],
        [0.5, 0.5, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
    ];
    assert_eq!(primitive.weights, weights);
    assert_eq!(primitive.normals, [[0.0, 0.0, 1.0]; 3], "flat");
    let [clip] = &imported.clips[..] else {
        panic!("a clip: {:?}", imported.clips);
    };
    assert_eq!((clip.name.as_str(), clip.duration), ("Turn", 1.0));
    let moved: Vec<(u32, Moved, Interpolation)> = (clip.channels.iter())
        .map(|channel| (channel.part, channel.path, channel.interpolation))
        .collect();
    let (rotation, translation) = (Moved::Rotation, Moved::Translation);
    let (linear, step) = (Interpolation::Linear, Interpolation::Step);
    assert_eq!(moved, [(1, rotation, linear), (0, translation, step)]);
    let eighth = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
    let turned = Quat::from_vec4(clip.channels[0].sample(0.5));
    assert!(turned.abs_diff_eq(eighth, 1e-6), "{turned}");
    let lift = |time| clip.channels[1].sample(time).truncate();
    assert_eq!(
        [lift(0.49), lift(0.5), lift(9.0)],
        [Vec3::ZERO, Vec3::Y * 2.0, Vec3::Y * 2.0]
    );
    let ignored = "channel 2 of animation 0, of node 3 outside the scene";
    assert_eq!(imported.ignored, [ignored]);
    // Lifted two at half a second, its first corner at (0, 2) and its second at (1, 2); and
    // its third, the child turned an eighth, at (1 - √2, 2): within its reach. That's the box
    // round what the child moves, its second corner and its third, turned with it, a unit
    // square turned an eighth, which reaches √½ past the child's center at (1 - √½, 2); with
    // a tenth of its longest side, 2 + √½, to spare each way.
    let [low, high] = primitive.reach.expect("a skinned primitive reaches");
    let turned = Vec3::new(1.0 - std::f32::consts::SQRT_2, 2.0, 0.0);
    let held = [Vec3::ZERO, Vec3::Y * 2.0, Vec3::new(1.0, 2.0, 0.0), turned];
    for point in held {
        assert!(
            point.cmpge(low).all() && point.cmple(high).all(),
            "{point} in {low}..{high}"
        );
    }
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let spare = Vec3::splat((2.0 + half) / 10.0);
    let expected = [
        Vec3::new(turned.x, 0.0, 0.0),
        Vec3::new(1.0, 2.0 + half, 0.0),
    ];
    assert!(
        (low - (expected[0] - spare)).abs().max_element() < 1e-5,
        "{low}"
    );
    assert!(
        (high - (expected[1] + spare)).abs().max_element() < 1e-5,
        "{high}"
    );
}

/// A skin, a skinned primitive or a clip that doesn't hold together refuses the model, saying
/// how.
#[test]
fn a_skin_or_clip_that_doesnt_hold_together_is_refused() {
    let (gltf, bin) = skinned();
    type Change = fn(&mut Value);
    let cases: [(&str, Change); 7] = [
        ("one of JOINTS_0 and WEIGHTS_0 without the other", |g| {
            remove(
                &mut g["meshes"][0]["primitives"][0]["attributes"],
                "WEIGHTS_0",
            );
        }),
        ("without JOINTS_0 and WEIGHTS_0", |g| {
            let attributes = &mut g["meshes"][0]["primitives"][0]["attributes"];
            remove(attributes, "WEIGHTS_0");
            remove(attributes, "JOINTS_0");
        }),
        ("moved by joint 1, past the 1 of skin 0", |g| {
            g["skins"][0]["joints"] = json!([0])
        }),
        ("of joint node 3, which isn't in its scene", |g| {
            g["skins"][0]["joints"] = json!([0, 3]);
        }),
        ("of 2 joints and 1 inverse bind matrices", |g| {
            g["accessors"][3]["count"] = json!(1)
        }),
        ("of times that don't go up from 0", |g| {
            g["accessors"][4]["bufferView"] = json!(5)
        }),
        ("of 2 times and 1 values", |g| {
            g["accessors"][7]["count"] = json!(1)
        }),
    ];
    for (problem, change) in cases {
        let mut gltf = gltf.clone();
        change(&mut gltf);
        let error = read_glb(&glb(&gltf.to_string(), &bin)).expect_err(problem);
        assert_eq!(error.code(), "engine.bad_model", "{problem}: {error}");
        assert!(error.to_string().contains(problem), "{problem}: {error}");
    }
}

/// A `.gltf` of [`triangle`]'s model, its buffer in a data URI, with `edit` made to its JSON:
/// for tests that edit a model's file as people do.
#[cfg(feature = "dev")]
pub(crate) fn triangle_gltf(edit: impl FnOnce(&mut Value)) -> String {
    let (mut gltf, bin) = triangle();
    let uri = format!("data:application/octet-stream;base64,{}", encode(&bin));
    gltf["buffers"][0]["uri"] = json!(uri);
    edit(&mut gltf);
    gltf.to_string()
}

/// Removes `key` from the object `value`.
fn remove(value: &mut Value, key: &str) {
    value.as_object_mut().expect("an object").remove(key);
}

/// A material of KHR_materials_pbrSpecularGlossiness's is that, its diffuse color and texture
/// in place of a base color's and its specular-glossiness map, which holds colors, in place of
/// its metallic-roughness, the extension's fallback, which is ignored. A texture whose
/// MSFT_texture_dds names a DDS image samples that, not its source, which isn't there: a DDS of
/// BC7, read as far as its headers. Neither extension is ignored; a DDS that isn't of BC7 is
/// refused, naming its image.
#[test]
fn specular_glossiness_and_dds_images_read_as_their_extensions_say() {
    let (mut gltf, bin) = triangle();
    gltf["extensionsUsed"] = json!(["KHR_materials_pbrSpecularGlossiness", "MSFT_texture_dds"]);
    gltf["materials"] = json!([{
        "pbrMetallicRoughness": {"baseColorFactor": [0.0, 0.0, 0.0, 1.0], "metallicFactor": 0.5},
        "extensions": {"KHR_materials_pbrSpecularGlossiness": {
            "diffuseFactor": [1.0, 0.5, 0.25, 1.0],
            "diffuseTexture": {"index": 0},
            "specularFactor": [0.5, 0.25, 0.125],
            "glossinessFactor": 0.75,
            "specularGlossinessTexture": {"index": 1},
        }},
    }]);
    let dds_of =
        |source: u32| json!({"source": 1, "extensions": {"MSFT_texture_dds": {"source": source}}});
    gltf["textures"] = json!([dds_of(2), dds_of(3)]);
    let png = gltf["images"][0].clone();
    gltf["images"] = json!([png, {"uri": "missing.png"}, {"uri": "a.dds"}, {"uri": "b.dds"}]);
    gltf["buffers"][0]["uri"] = json!("m.bin");
    let read = |gltf: &Value, b: &[u8]| {
        let a = crate::image::tests::dds(99, [4, 4], &[vec![1; 16]]);
        let files = [("m.bin", &bin[..]), ("a.dds", &a[..]), ("b.dds", b)];
        read_files("m.gltf", gltf.to_string().as_bytes(), &files)
    };
    let b = crate::image::tests::dds(98, [4, 4], &[vec![2; 16]]);
    let imported = read(&gltf, &b).expect("a model");
    let material = imported.primitives[0].material;
    assert_eq!(material.color, [1.0, 0.5, 0.25, 1.0]);
    let workflow = Workflow::SpecularGlossiness {
        specular: Vec3::new(0.5, 0.25, 0.125),
        glossiness: 0.75,
    };
    assert_eq!(material.workflow, workflow);
    let sampled = material
        .textures
        .map(|texture| texture.map(|texture| texture.image));
    assert_eq!(sampled, [Some(0), Some(1), None, None, None]);
    assert_eq!(material.map(Slot::Surface), None, "colors");
    let dds = |compressed| crate::image::Format::Dds {
        compressed,
        mipmapped: false,
    };
    let formats: Vec<_> = (imported.images.iter())
        .map(|image| (image.size, image.format))
        .collect();
    use crate::image::Compressed::{Bc7Srgb, Bc7Unorm};
    assert_eq!(formats, [([4, 4], dds(Bc7Srgb)), ([4, 4], dds(Bc7Unorm))]);
    assert!(imported.ignored.is_empty(), "{:?}", imported.ignored);
    let bc1 = crate::image::tests::dds(71, [4, 4], &[vec![2; 8]]);
    let error = read(&gltf, &bc1).expect_err("BC1");
    assert_eq!(error.code(), "engine.bad_model");
    let message = error.to_string();
    assert!(
        message.contains("image 3") && message.contains("DXGI format 71"),
        "{message}"
    );
}

/// A primitive whose TANGENT holds numbers that aren't finite, as a tool can leave them where a
/// triangle's texture coordinates have no area, is drawn as one without tangents, its normal
/// map's from MikkTSpace, which `ignored` says; its other attributes still may hold none.
#[test]
fn tangents_that_arent_finite_are_ignored() {
    let (mut gltf, mut bin) = triangle();
    let mut view = |bytes: &[u8]| {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let at = bin.len();
        bin.extend_from_slice(bytes);
        let views = gltf["bufferViews"].as_array_mut().expect("views");
        views.push(json!({"buffer": 0, "byteOffset": at, "byteLength": bytes.len()}));
        views.len() - 1
    };
    let nan = f32::NAN;
    let normals = view(&floats(&[0., 0., 1., 0., 0., 1., 0., 0., 1.]));
    let tangents = view(&floats(&[
        1., 0., 0., 1., nan, nan, nan, 0., 1., 0., 0., 1.,
    ]));
    let nans = view(&floats(&[nan; 9]));
    gltf["buffers"][0]["byteLength"] = json!(bin.len());
    let accessors = gltf["accessors"].as_array_mut().expect("accessors");
    for (view, kind) in [(normals, "VEC3"), (tangents, "VEC4"), (nans, "VEC3")] {
        accessors
            .push(json!({"bufferView": view, "componentType": 5126, "count": 3, "type": kind}));
    }
    let attributes = &mut gltf["meshes"][0]["primitives"][0]["attributes"];
    attributes["NORMAL"] = json!(3);
    attributes["TANGENT"] = json!(4);
    // Its own image, of the same PNG, which the base color samples as colors.
    let png = gltf["images"][0].clone();
    gltf["images"].as_array_mut().expect("images").push(png);
    let texture = json!({"source": 1});
    gltf["textures"]
        .as_array_mut()
        .expect("textures")
        .push(texture);
    gltf["materials"][0]["normalTexture"] = json!({"index": 1});
    let read = |gltf: &Value| read_glb(&glb(&gltf.to_string(), &bin));
    let imported = read(&gltf).expect("a model");
    let ignored = "primitive 0 of mesh 0's TANGENT, which holds numbers that aren't finite: \
                       it's drawn as without one";
    assert_eq!(imported.ignored, [ignored]);
    // MikkTSpace's, along U, which runs along X.
    let made = &imported.primitives[0].tangents;
    assert!(
        made.iter().all(|tangent| tangent == &[1.0, 0.0, 0.0, 1.0]),
        "{made:?}"
    );
    gltf["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(5);
    let error = read(&gltf).expect_err("normals that aren't finite");
    assert!(error.to_string().contains("isn't finite"), "{error}");
}

#[test]
fn a_matrix_rounded_as_it_was_printed_still_decomposes() {
    let (mut gltf, bin) = triangle();
    // A turn of 30 degrees about Z, to four places.
    let matrix = [
        0.866, 0.5, 0.0, 0.0, -0.5, 0.866, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    gltf["nodes"][0]["matrix"] = json!(matrix);
    let imported = read_glb(&glb(&gltf.to_string(), &bin)).expect("a model");
    let rotation = imported.parts[0].transform.rotation;
    let turn = Quat::from_rotation_z(gg_core::math::to_radians(30.0));
    assert!(rotation.abs_diff_eq(turn, 1e-4), "{rotation}");
}

/// The code of the error that a change to a model's JSON makes, what its message says, and the
/// change.
type Refusal = (&'static str, &'static str, fn(&mut Value));

#[test]
fn what_the_engine_doesnt_draw_or_that_isnt_gltf_is_refused_with_its_code() {
    let (gltf, bin) = triangle();
    read_glb(&glb(&gltf.to_string(), &bin)).expect("a triangle");
    let (unsupported, bad) = ("engine.unsupported_model", "engine.bad_model");
    let cases: &[Refusal] = &[
        // What the engine doesn't draw.
        (unsupported, "glTF 1.0", |g| {
            g["asset"]["version"] = json!("1.0")
        }),
        (unsupported, "glTF 2.1 at least", |g| {
            g["asset"]["minVersion"] = json!("2.1");
        }),
        (unsupported, "no scene", |g| {
            remove(g, "scene");
            remove(g, "scenes");
        }),
        (unsupported, "triangle lists", |g| {
            g["meshes"][0]["primitives"][0]["mode"] = json!(5);
        }),
        (unsupported, "sparse", |g| {
            g["accessors"][0]["sparse"] = json!({"count": 1});
        }),
        (unsupported, "no buffer view", |g| {
            remove(&mut g["accessors"][0], "bufferView");
        }),
        (unsupported, "morph weights", |g| {
            g["meshes"][0]["weights"] = json!([0.5]);
        }),
        (unsupported, "morph weights", |g| {
            g["nodes"][0]["weights"] = json!([0, 1]);
        }),
        (unsupported, "wraps S and T differently", |g| {
            g["samplers"][0]["wrapS"] = json!(33071);
        }),
        (unsupported, "no source", |g| {
            remove(&mut g["textures"][0], "source")
        }),
        (unsupported, "KHR_draco_mesh_compression", |g| {
            g["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
        }),
        // What isn't glTF.
        (bad, "isn't glTF's JSON", |g| {
            g["nodes"] = json!({"root": {}})
        }),
        (bad, "glTF version", |g| {
            g["asset"]["version"] = json!("two")
        }),
        (bad, "glTF version", |g| remove(g, "asset")),
        (bad, "has no scene 1", |g| g["scene"] = json!(1)),
        (bad, "has no node 5", |g| {
            g["scenes"][0]["nodes"] = json!([5])
        }),
        (bad, "has no mesh 1", |g| g["nodes"][0]["mesh"] = json!(1)),
        (bad, "node 0 twice", |g| {
            g["nodes"][0]["children"] = json!([0])
        }),
        (bad, "node 0 twice", |g| {
            g["scenes"][0]["nodes"] = json!([1, 2]);
            let nodes = g["nodes"].as_array_mut().expect("nodes");
            nodes.extend([json!({"children": [0]}), json!({"children": [0]})]);
        }),
        (bad, "a matrix and", |g| {
            g["nodes"][0]["matrix"] = json!([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
            g["nodes"][0]["scale"] = json!([1, 1, 1]);
        }),
        // A shear, a projection, and a column of 0.
        (bad, "doesn't decompose", |g| {
            g["nodes"][0]["matrix"] = json!([1, 0, 0, 0, 0.5, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
        }),
        (bad, "doesn't decompose", |g| {
            g["nodes"][0]["matrix"] = json!([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, -1, 0, 0, 0, 1]);
        }),
        (bad, "doesn't decompose", |g| {
            g["nodes"][0]["matrix"] = json!([0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);
        }),
        (bad, "rotation is 0", |g| {
            g["nodes"][0]["rotation"] = json!([0, 0, 0, 0])
        }),
        // A rotation so near 0 that its length, squared, loses its precision, which then
        // isn't a unit as the world takes one.
        (bad, "rotation is 0", |g| {
            g["nodes"][0]["rotation"] = json!([2e-22, 0, 0, 0])
        }),
        // A number past an f32's range, which `float_roundtrip` refuses, where it would read
        // as infinite; and an emissive factor within it, times a strength, past it.
        (bad, "isn't glTF's JSON", |g| {
            g["nodes"][0]["translation"] = json!([1e39, 0, 0]);
        }),
        (bad, "material 0 has a number that isn't finite", |g| {
            g["materials"][0]["emissiveFactor"] = json!([3e38, 0, 0]);
            g["materials"][0]["extensions"] = json!({
                "KHR_materials_emissive_strength": {"emissiveStrength": 2},
            });
        }),
        (bad, "mode 9", |g| {
            g["meshes"][0]["primitives"][0]["mode"] = json!(9)
        }),
        (bad, "reads POSITION from accessor 0", |g| {
            g["accessors"][0]["type"] = json!("VEC2");
        }),
        (bad, "reads POSITION from accessor 0", |g| {
            g["accessors"][0]["componentType"] = json!(5121);
            g["accessors"][0]["normalized"] = json!(true);
        }),
        (bad, "reads TEXCOORD_0 from accessor 1", |g| {
            g["accessors"][1]["componentType"] = json!(5122);
        }),
        (bad, "no TEXCOORD_0", |g| {
            let attributes = &mut g["meshes"][0]["primitives"][0]["attributes"];
            remove(attributes, "TEXCOORD_0");
        }),
        (bad, "no TEXCOORD_1", |g| {
            let texture = &mut g["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"];
            texture["texCoord"] = json!(1);
        }),
        (bad, "TEXCOORD_0 has 2 elements", |g| {
            g["accessors"][1]["count"] = json!(2)
        }),
        (bad, "reads indices from accessor 2", |g| {
            g["accessors"][2]["componentType"] = json!(5126);
        }),
        (bad, "past its 3 vertices", |g| {
            g["accessors"][2]["byteOffset"] = json!(2)
        }),
        (bad, "isn't whole triangles", |g| {
            g["accessors"][2]["count"] = json!(4)
        }),
        (bad, "isn't whole triangles", |g| {
            remove(&mut g["meshes"][0]["primitives"][0], "indices");
            g["accessors"][0]["count"] = json!(2);
        }),
        (bad, "isn't finite", |g| {
            g["accessors"][0]["bufferView"] = json!(4);
            g["accessors"][0]["count"] = json!(1);
        }),
        (bad, "has no elements", |g| {
            g["accessors"][0]["count"] = json!(0)
        }),
        // A byte short of the accessor.
        (bad, "reads past buffer view 0", |g| {
            g["bufferViews"][0]["byteLength"] = json!(35);
        }),
        // A count far past the bytes there are, which fails before anything is allocated for
        // it.
        (bad, "reads past buffer view 0", |g| {
            g["accessors"][0]["count"] = json!(4_000_000_000u64);
        }),
        (bad, "elements are 12 bytes", |g| {
            g["bufferViews"][0]["byteStride"] = json!(8)
        }),
        (bad, "reads past buffer 0", |g| {
            g["bufferViews"][0]["byteOffset"] = json!(1_000_000);
        }),
        (bad, "short of its byteLength", |g| {
            g["buffers"][0]["byteLength"] = json!(1_000_000);
        }),
        (bad, "has no buffer 1", |g| {
            g["bufferViews"][0]["buffer"] = json!(1)
        }),
        (bad, "alphaMode", |g| {
            g["materials"][0]["alphaMode"] = json!("CUTOUT")
        }),
        (bad, "alphaCutoff", |g| {
            g["materials"][0]["alphaMode"] = json!("MASK");
            g["materials"][0]["alphaCutoff"] = json!(-1);
        }),
        (bad, "wrap mode 1234", |g| {
            g["samplers"][0] = json!({"wrapS": 1234, "wrapT": 1234});
        }),
        (bad, "both a uri and a bufferView", |g| {
            g["images"][0]["uri"] = json!("a.png");
        }),
        (bad, "doesn't decode as a PNG or a JPEG", |g| {
            g["images"][0]["bufferView"] = json!(0);
        }),
        (bad, "isn't base64", |g| {
            g["buffers"][0]["uri"] = json!("data:application/octet-stream,AAAA");
        }),
        (bad, "isn't valid base64", |g| {
            g["buffers"][0]["uri"] = json!("data:application/octet-stream;base64,AAA");
        }),
        (bad, "two hex digits", |g| {
            g["buffers"][0]["uri"] = json!("m%zz.bin")
        }),
        (bad, "can't be read", |g| {
            g["buffers"][0]["uri"] = json!("missing.bin")
        }),
        // URIs with a scheme, a root, a drive or a backslash, as they are or percent-encoded.
        (bad, "isn't a relative path", |g| {
            g["buffers"][0]["uri"] = json!("https://example.com/m.bin");
        }),
        (bad, "isn't a relative path", |g| {
            g["buffers"][0]["uri"] = json!("/m.bin")
        }),
        (bad, "isn't a relative path", |g| {
            g["buffers"][0]["uri"] = json!("C:/m.bin")
        }),
        (bad, "isn't a relative path", |g| {
            g["buffers"][0]["uri"] = json!("models\\m.bin");
        }),
        (bad, "isn't a relative path", |g| {
            g["buffers"][0]["uri"] = json!("%5C%5Cserver%5Cm.bin");
        }),
        (bad, "isn't a relative path", |g| {
            g["buffers"][0]["uri"] = json!("C%3A/m.bin")
        }),
        (bad, "isn't a relative path", |g| {
            g["buffers"][0]["uri"] = json!("//server/share/m.bin");
        }),
        // URIs that go up a folder, as they are or percent-encoded.
        (bad, "has a `..`", |g| {
            g["buffers"][0]["uri"] = json!("../m.bin")
        }),
        (bad, "has a `..`", |g| {
            g["buffers"][0]["uri"] = json!("textures/../../m.bin");
        }),
        (bad, "has a `..`", |g| g["buffers"][0]["uri"] = json!("..")),
        (bad, "has a `..`", |g| {
            g["buffers"][0]["uri"] = json!("%2E%2e/m.bin")
        }),
        (bad, "has a `..`", |g| {
            g["buffers"][0]["uri"] = json!("..%2Fm.bin")
        }),
        (bad, "has a `..`", |g| {
            g["images"][0] = json!({"uri": "../../../../dev/zero"});
        }),
    ];
    for (code, problem, change) in cases {
        let mut gltf = gltf.clone();
        change(&mut gltf);
        let error = read_glb(&glb(&gltf.to_string(), &bin)).expect_err(problem);
        assert_eq!(error.code(), *code, "{problem}: {error}");
        assert!(error.to_string().contains(problem), "{problem}: {error}");
    }
}

#[test]
fn a_buffers_file_is_read_no_further_than_its_byte_length_or_the_memory_left() {
    // The triangle's buffer in a file of its own, with bytes past its byteLength.
    let (mut gltf, bin) = triangle();
    gltf["buffers"][0]["uri"] = json!("m.bin");
    let model = glb(&gltf.to_string(), &[]);
    let length = bin.len() as u64;
    let read_with = |file: &[u8], memory| {
        let asked = Cell::new(0);
        let read_file = |path: &Path, most: u64| {
            if path == Path::new("models/m.glb") {
                return Ok(model.clone());
            }
            asked.set(most);
            Ok(file[..file.len().min(most as usize)].to_vec())
        };
        let read = read("m", Path::new("models/m.glb"), read_file, memory);
        (read, asked.get())
    };
    let longer = [&bin[..], &[7; 64]].concat();
    let (read, asked) = read_with(&longer, MAX_MEMORY);
    read.expect("a triangle");
    assert_eq!(asked, length);
    // A file short of its byteLength.
    let (read, _) = read_with(&bin[..bin.len() - 1], MAX_MEMORY);
    let error = read.expect_err("a short file");
    assert_eq!(error.code(), "engine.bad_model");
    let message = error.to_string();
    assert!(message.contains("short of its byteLength"), "{message}");
    // A byteLength past the memory reading may take, which reads a byte past it.
    let memory = length / 2;
    let (read, asked) = read_with(&longer, memory);
    let error = read.expect_err("a buffer past the memory");
    assert_eq!(error.code(), "engine.unsupported_model");
    assert_eq!(asked, memory + 1);
}

/// The Fox's vertices, each where the skin pass moves it, in each pose its reach is found from,
/// are within the box its leaves bound that pose by, which is little larger than the box round
/// the vertices themselves.
#[test]
fn a_skinned_meshs_leaves_hold_its_vertices_in_every_pose() {
    let fox = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/fox/Fox.gltf");
    let imported = read("fox", &fox, crate::dirs::read_file, MAX_MEMORY).expect("the Fox");
    let (part, skinned) = (imported.parts.iter().enumerate())
        .find(|(_, part)| part.skin.is_some())
        .expect("a skinned part");
    let skin = &imported.skins[skinned.skin.expect("a skin")];
    let primitive = &imported.primitives[skinned.primitive.expect("a primitive")];
    let (positions, joints) = (&primitive.positions, &primitive.joints);
    let influences = Influences::new(skin, positions, joints, &primitive.weights);
    let rest: Vec<Transform> = (imported.parts.iter()).map(|p| p.transform).collect();
    let parents: Vec<Option<usize>> = imported.parts.iter().map(|p| p.parent).collect();
    let all: Vec<usize> = (0..rest.len()).collect();
    let mut poses = vec![rest.clone()];
    let mut times = Vec::new();
    for clip in &imported.clips {
        clip.pose_times(&mut times);
        for &time in &times {
            let mut locals = rest.clone();
            for channel in &clip.channels {
                channel.apply(&mut locals[channel.part as usize], channel.sample(time));
            }
            poses.push(locals);
        }
    }
    let [mut exact, mut bound] = [[Vec3::INFINITY, Vec3::NEG_INFINITY]; 2];
    for locals in &poses {
        let mut placed = vec![Mat4::IDENTITY; rest.len()];
        place(locals, &parents, &all, &mut placed);
        let [low, high] = influences.posed(&placed, part, skin);
        let into_mesh = placed[part].inverse();
        for ((&position, joints), weights) in positions.iter().zip(joints).zip(&primitive.weights) {
            let total: f32 = weights.iter().sum();
            let moved: Vec3 = (joints.iter().zip(weights))
                .map(|(&joint, &weight)| {
                    let joint = usize::from(joint);
                    let matrix =
                        into_mesh * placed[skin.joints[joint] as usize] * skin.inverse_binds[joint];
                    matrix.transform_point3(Vec3::from_array(position)) * (weight / total)
                })
                .sum();
            let within = 1e-3 * (high - low).max_element();
            assert!(
                moved.cmpge(low - within).all() && moved.cmple(high + within).all(),
                "{moved} in {low}..{high}"
            );
            exact = [exact[0].min(moved), exact[1].max(moved)];
        }
        bound = [bound[0].min(low), bound[1].max(high)];
    }
    // Within a twentieth of its longest side, which the reach spares a tenth of each way.
    let longest = (exact[1] - exact[0]).max_element();
    let past = (exact[0] - bound[0]).max(bound[1] - exact[1]).max_element();
    assert!(past < longest / 20.0, "{bound:?} round {exact:?}");
}

/// [`skinned`]'s model, as a GLB, with `clips` clips that each turn the child joint along 65
/// keys, all from one pair of accessors, in place of its own; `parts` more parts, that place
/// nothing; and `drawn` more nodes that draw its mesh, skinned by its skin.
fn many(clips: usize, parts: usize, drawn: usize) -> Vec<u8> {
    let (mut gltf, bin) = skinned();
    let mut view = Bin {
        bytes: bin,
        views: serde_json::from_value(gltf["bufferViews"].take()).expect("views"),
    };
    let times: Vec<f32> = (0..65).map(|key| key as f32 / 64.0).collect();
    let turns: Vec<f32> = (0..65)
        .flat_map(|key| Quat::from_rotation_z(key as f32 / 64.0).to_array())
        .collect();
    let times = view.view(&floats(&times), None);
    let turns = view.view(&floats(&turns), None);
    let accessors = gltf["accessors"].as_array_mut().expect("accessors");
    let accessor =
        |view, kind| json!({"bufferView": view, "componentType": 5126, "count": 65, "type": kind});
    accessors.push(accessor(times, "SCALAR"));
    accessors.push(accessor(turns, "VEC4"));
    let (times, turns) = (accessors.len() - 2, accessors.len() - 1);
    let clip = json!({
        "channels": [{"sampler": 0, "target": {"node": 1, "path": "rotation"}}],
        "samplers": [{"input": times, "output": turns}],
    });
    gltf["animations"] = json!(vec![clip; clips]);
    let nodes = gltf["nodes"].as_array_mut().expect("nodes");
    let first = nodes.len();
    nodes.extend(std::iter::repeat_n(json!({}), parts));
    nodes.extend(std::iter::repeat_n(json!({"mesh": 0, "skin": 0}), drawn));
    let scene = (gltf["scenes"][0]["nodes"].as_array_mut()).expect("the scene's nodes");
    scene.extend((first..first + parts + drawn).map(|node| json!(node)));
    gltf["bufferViews"] = json!(view.views);
    gltf["buffers"][0]["byteLength"] = json!(view.bytes.len());
    glb(&gltf.to_string(), &view.bytes)
}

/// A small file of many clips and many parts, which would take the product of the poses its
/// clips' keys make and its parts, each pose's 64 bytes a part, to find its skinned mesh's
/// reach, as some 33 GB, reads within the memory reading may take; and what finding its reach
/// holds counts against that memory, so that the least that reads the model with fewer clips
/// fails, a byte less, as its skinned meshes' poses are bound.
#[test]
fn a_skinned_model_of_many_clips_and_parts_reaches_within_the_memory_reading_may_take() {
    let read_with =
        |model: &[u8], memory| read("m", Path::new("m.glb"), |_, _| Ok(model.to_vec()), memory);
    let model = many(2_000, 4_000, 0);
    assert!(model.len() < 1 << 20, "{} bytes", model.len());
    let imported = read_with(&model, MAX_MEMORY).expect("the model");
    assert_eq!((imported.clips.len(), imported.parts.len()), (2_000, 4_003));
    let [low, high] = imported.primitives[0].reach.expect("a reach");
    for corner in [Vec3::ZERO, Vec3::X, Vec3::Y] {
        assert!(
            corner.cmpge(low).all() && corner.cmple(high).all(),
            "{low}..{high}"
        );
    }
    let model = many(2, 4_000, 0);
    let (mut fails, mut reads) = (0, MAX_MEMORY);
    while reads - fails > 1 {
        let memory = fails + (reads - fails) / 2;
        match read_with(&model, memory) {
            Ok(_) => reads = memory,
            Err(_) => fails = memory,
        }
    }
    let error = read_with(&model, fails).expect_err("a byte less than it reads in");
    assert_eq!(error.code(), "engine.unsupported_model", "{error}");
    let poses = "MiB to read, of its skinned meshes' poses";
    assert!(error.to_string().contains(poses), "{error}");
}

/// A small file of many clips and many nodes that draw a skinned mesh, whose poses would take
/// minutes to bound, is refused as it would take more than the engine takes, before it poses
/// any; with fewer, it reads.
#[test]
fn a_skinned_model_whose_poses_take_too_long_to_bound_is_refused() {
    let read_with = |model: Vec<u8>| {
        read(
            "m",
            Path::new("m.glb"),
            |_, _| Ok(model.clone()),
            MAX_MEMORY,
        )
    };
    let error = read_with(many(100, 0, 10_000)).expect_err("too long to bound");
    assert_eq!(error.code(), "engine.unsupported_model", "{error}");
    let past = "to bound its skinned meshes as its clips pose them, past the 134217728";
    assert!(error.to_string().contains(past), "{error}");
    let imported = read_with(many(100, 0, 100)).expect("fewer");
    assert_eq!(imported.parts.len(), 103);
}

#[test]
fn data_drawn_many_times_counts_each_time_against_the_memory_reading_may_take() {
    // A mesh of `primitives` that share `vertices`, each holding a copy, drawn by `nodes`, each
    // a part for each primitive: 30 copies of 3,000 vertices, 120 KB each; and 2,000 nodes of a
    // mesh of 8 triangles, 648 bytes of parts each.
    let model = |vertices: usize, primitives: usize, nodes: usize| {
        let mut bin = Bin::default();
        let positions = bin.view(&vec![0; vertices * 12], None);
        let primitive = json!({"attributes": {"POSITION": 0}});
        let gltf = json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": (0..nodes).collect::<Vec<_>>()}],
            "nodes": vec![json!({"mesh": 0}); nodes],
            "meshes": [{"primitives": vec![primitive; primitives]}],
            "accessors": [{
                "bufferView": positions,
                "componentType": 5126,
                "count": vertices,
                "type": "VEC3",
            }],
            "buffers": [{"byteLength": bin.bytes.len()}],
            "bufferViews": bin.views,
        });
        glb(&gltf.to_string(), &bin.bytes)
    };
    for (vertices, primitives, nodes) in [(3000, 30, 1), (3, 8, 2000)] {
        let model = model(vertices, primitives, nodes);
        let read_with = |memory| read("m", Path::new("m.glb"), |_, _| Ok(model.clone()), memory);
        read_with(MAX_MEMORY).expect("a model");
        let error = read_with(1 << 20).expect_err("a model past 1 MiB");
        assert_eq!(error.code(), "engine.unsupported_model", "{error}");
    }
}

#[test]
fn images_that_share_a_source_each_count_against_the_memory_reading_may_take() {
    // Four triangles, each sampling an image of its own, all four the same PNG: 1-bit gray, 256
    // pixels square, a few hundred bytes that decode to 256 KiB of RGBA each.
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, 256, 256);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::One);
    let mut writer = encoder.write_header().expect("a header");
    let rows = [0; 256 * 256 / 8];
    writer.write_image_data(&rows).expect("the pixels");
    writer.finish().expect("a whole PNG");
    let mut bin = Bin::default();
    let positions = bin.view(&floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]), None);
    let uvs = bin.view(&floats(&[0., 0., 1., 0., 0., 1.]), None);
    let image = bin.view(&png, None);
    let primitive = |i| json!({"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "material": i});
    let material = |i| json!({"pbrMetallicRoughness": {"baseColorTexture": {"index": i}}});
    let gltf = json!({
        "asset": {"version": "2.0"},
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": (0..4).map(primitive).collect::<Vec<_>>()}],
        "accessors": [
            {"bufferView": positions, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": uvs, "componentType": 5126, "count": 3, "type": "VEC2"},
        ],
        "materials": (0..4).map(material).collect::<Vec<_>>(),
        "textures": (0..4).map(|i| json!({"source": i})).collect::<Vec<_>>(),
        "images": vec![json!({"bufferView": image}); 4],
        "buffers": [{"byteLength": bin.bytes.len()}],
        "bufferViews": bin.views,
    });
    let model = glb(&gltf.to_string(), &bin.bytes);
    let read_with = |memory| read("m", Path::new("m.glb"), |_, _| Ok(model.clone()), memory);
    let imported = read_with(2 << 20).expect("four images in 2 MiB");
    assert_eq!(imported.images.len(), 4);
    let error = read_with(1 << 20).expect_err("four images past 1 MiB");
    assert_eq!(error.code(), "engine.unsupported_model");
    assert!(error.to_string().contains("more than 1 MiB"), "{error}");
}

/// What reading holds on the way counts against the memory it may take, as it's about to be
/// made, as well as what it holds once read: the indices as they're read, beside the
/// primitive's copy, and the vertices of a primitive drawn flat, or parted for MikkTSpace's
/// tangents, beside those they're made from, with MikkTSpace's tangent for each corner. A
/// primitive's file holds a byte for each of its corners, which are made each a vertex of 64
/// bytes, so that a gigabyte of file would otherwise take some 70 before it was counted.
#[test]
fn what_reading_holds_on_the_way_counts_against_the_memory_it_may_take() {
    // A triangle drawn 1,000 times, by indices of a byte each: flat, for want of normals; or
    // with normals and a normal map, without tangents.
    let model = |normal_map: bool| {
        let mut bin = Bin::default();
        let positions = bin.view(&floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]), None);
        let normals = bin.view(&floats(&[0., 0., 1., 0., 0., 1., 0., 0., 1.]), None);
        let uvs = bin.view(&floats(&[0., 0., 1., 0., 0., 1.]), None);
        let indices = bin.view(&[0, 1, 2].repeat(1_000), None);
        let image = bin.view(&png(), None);
        let accessor = |view, kind, count, component| json!({"bufferView": view, "type": kind, "count": count, "componentType": component});
        let mut gltf = json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 3}]}],
            "accessors": [
                accessor(positions, "VEC3", 3, FLOAT),
                accessor(normals, "VEC3", 3, FLOAT),
                accessor(uvs, "VEC2", 3, FLOAT),
                accessor(indices, "SCALAR", 3_000, UNSIGNED_BYTE),
            ],
            "buffers": [{"byteLength": bin.bytes.len()}],
            "bufferViews": bin.views,
        });
        if normal_map {
            let primitive = &mut gltf["meshes"][0]["primitives"][0];
            primitive["attributes"] = json!({"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2});
            primitive["material"] = json!(0);
            gltf["materials"] = json!([{"normalTexture": {"index": 0}}]);
            gltf["textures"] = json!([{"source": 0}]);
            gltf["images"] = json!([{"bufferView": image}]);
        }
        glb(&gltf.to_string(), &bin.bytes)
    };
    // What a model holds once read: its parts, its primitives' vertices, indices and names,
    // and its images' files and what they decode to.
    let held = |imported: &Imported| {
        let primitives =
            (imported.primitives.iter()).map(|primitive| primitive.held() + primitive.name.len());
        let images = (imported.images.iter()).map(|image| image.pixels.len() + image.decoded_len());
        (size_of_val(imported.parts.as_slice()) + primitives.chain(images).sum::<usize>()) as u64
    };
    for normal_map in [false, true] {
        let model = model(normal_map);
        let read_with = |memory| read("m", Path::new("m.glb"), |_, _| Ok(model.clone()), memory);
        let imported = read_with(MAX_MEMORY).expect("a model");
        assert_eq!(imported.primitives[0].indices.len(), 3_000);
        let error = read_with(held(&imported)).expect_err("past what it holds once read");
        assert_eq!(error.code(), "engine.unsupported_model", "{error}");
    }
}
