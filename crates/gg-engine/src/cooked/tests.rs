use gg_core::math::DVec3;

use super::*;
use crate::image::tests::ktx2;
use gg_renderer::ModelVertex;

/// A model of two parts, the first a spot light and the second under it drawing a primitive of
/// two levels with a masked material that samples its two images, colors and a normal map; and
/// the KTX2 of those images, each a block 4 texels across and down, BC7 of sRGB and BC5.
fn model() -> (Imported, Vec<Vec<u8>>) {
    let colors = ktx2(ktx2::Format::BC7_SRGB_BLOCK, [4, 4], &[vec![7; 16]], true);
    let normals = ktx2(ktx2::Format::BC5_UNORM_BLOCK, [4, 4], &[vec![9; 16]], true);
    let sampled = |image, wrap| {
        Some(Sampling {
            image,
            nearest: true,
            wrap,
        })
    };
    let corners = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
    let colors_of = [[1.0; 4], [0.5; 4], [0.25; 4], [0.0, 0.0, 0.0, 1.0]];
    let vertices: Vec<_> = (corners.iter().zip(colors_of))
        .map(|(&[x, y], color)| ModelVertex {
            position: Vec3::new(x, y, 0.0),
            uv: Vec2::new(x, y),
            color: color.into(),
            normal: Vec3::Z,
            tangent: gg_core::math::Vec4::new(1.0, 0.0, 0.0, -1.0),
        })
        .collect();
    let levels = vec![
        Lod {
            start: 0,
            count: 6,
            error: 0.0,
        },
        Lod {
            start: 6,
            count: 3,
            error: 0.5,
        },
    ];
    let indices = vec![0, 1, 2, 2, 1, 3, 0, 1, 2];
    let mut mesh = ModelMesh::pack("square", &vertices, indices, levels).expect("a mesh");
    // A meshlet for each level: both triangles, then one.
    let meshlet = |vertices, vertex_count, triangles, triangle_count| Meshlet {
        vertices,
        vertex_count,
        triangles,
        triangle_count,
        center: Vec3::new(0.5, 0.5, 0.0),
        radius: 0.75,
        cone: [0, 0, 127, -20],
    };
    mesh.meshlets = Meshlets {
        meshlets: vec![meshlet(0, 4, 0, 2), meshlet(4, 3, 8, 1)],
        vertices: vec![0, 1, 2, 3, 0, 1, 2],
        triangles: vec![0, 1, 2, 2, 1, 3, 0, 0, 0, 1, 2, 0],
        levels: vec![[0, 1], [1, 1]],
    };
    let primitive = Primitive {
        name: "square/0".to_owned(),
        positions: Vec::new(),
        uvs: Vec::new(),
        colors: Vec::new(),
        normals: Vec::new(),
        tangents: Vec::new(),
        indices: Vec::new(),
        mesh: Some(mesh),
        material: Material {
            color: [1.0, 0.5, 0.25, 1.0],
            workflow: Workflow::MetallicRoughness {
                metallic: 0.25,
                roughness: 0.75,
            },
            normal_scale: 0.5,
            occlusion_strength: 0.125,
            emissive: [2.0, 0.0, 1.0],
            textures: [
                sampled(0, Wrap::Mirror),
                None,
                sampled(1, Wrap::Clamp),
                None,
                sampled(0, Wrap::Repeat),
            ],
            alpha: AlphaMode::Mask { cutoff: 0.25 },
            double_sided: true,
            unlit: true,
        },
        joints: Vec::new(),
        weights: Vec::new(),
        reach: None,
    };
    let turned = Transform {
        translation: DVec3::new(1.0, 2.0, 3.0),
        rotation: Quat::from_rotation_y(0.1),
        scale: Vec3::new(1.0, -2.0, 0.5),
    };
    let spot = Light {
        kind: LightKind::Spot,
        color: Vec3::new(1.0, 0.5, 0.25),
        intensity: 30.0,
        range: Some(8.0),
        inner_cone: 0.25,
        outer_cone: 0.5,
        ..Light::default()
    };
    let parts = vec![
        Part {
            name: Some(Arc::from("lamp")),
            transform: turned,
            parent: None,
            primitive: None,
            light: Some(spot),
            skin: None,
        },
        Part {
            name: None,
            transform: Transform::IDENTITY,
            parent: Some(0),
            primitive: Some(0),
            light: None,
            skin: Some(0),
        },
    ];
    // And the same again, of specular and glossiness, which no part draws.
    let mut glossy = primitive.clone();
    glossy.name = "square/1".to_owned();
    glossy.material.workflow = Workflow::SpecularGlossiness {
        specular: Vec3::new(0.5, 0.25, 0.125),
        glossiness: 0.625,
    };
    let read = |image: &Vec<u8>| crate::image::read_ktx2(image.clone()).expect("a texture");
    let imported = Imported {
        parts,
        primitives: vec![primitive, glossy],
        images: vec![read(&colors), read(&normals)],
        ignored: vec!["1 camera".to_owned()],
        skins: vec![Skin {
            joints: vec![0, 1],
            inverse_binds: vec![Mat4::IDENTITY, Mat4::from_rotation_x(0.5)],
        }],
        clips: vec![Clip {
            name: "Wave".to_owned(),
            duration: 2.0,
            channels: vec![
                Channel {
                    part: 1,
                    path: Moved::Rotation,
                    interpolation: Interpolation::CubicSpline,
                    times: vec![0.0, 2.0],
                    values: [[0.0; 4], [0.0, 0.0, 0.0, 1.0], [0.0; 4]]
                        .repeat(2)
                        .concat(),
                },
                Channel {
                    part: 0,
                    path: Moved::Scale,
                    interpolation: Interpolation::Step,
                    times: vec![0.5],
                    values: vec![2.0, 2.0, 2.0],
                },
            ],
        }],
    };
    (imported, vec![colors, normals])
}

#[test]
fn a_cooked_model_reads_back_as_it_was_written() {
    let (imported, images) = model();
    let bytes = encode(&imported, &images);
    let read = decode("m", Path::new("m.glb"), &bytes).expect("a cooked model");
    assert_eq!(read, imported);
    // Its normal map as BC7 of values, as the cooker keeps a DDS image's blocks.
    let bc7 = ktx2(ktx2::Format::BC7_UNORM_BLOCK, [4, 4], &[vec![9; 16]], true);
    let mut kept = imported.clone();
    kept.images[1] = crate::image::read_ktx2(bc7.clone()).expect("a texture");
    let bytes = encode(&kept, &[images[0].clone(), bc7]);
    let read = decode("m", Path::new("m.glb"), &bytes).expect("a BC7 normal map");
    assert_eq!(read, kept);
    // Its floats' bits, which the world hash reads through its nodes.
    let rotation = read.parts[0]
        .transform
        .rotation
        .to_array()
        .map(f32::to_bits);
    assert_eq!(
        rotation,
        imported.parts[0]
            .transform
            .rotation
            .to_array()
            .map(f32::to_bits)
    );
}

#[test]
fn a_cooked_model_that_doesnt_hold_together_is_refused() {
    let (imported, images) = model();
    let good = encode(&imported, &images);
    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("cut short", good[..good.len() - 1].to_vec()),
        ("followed by more", [&good[..], &[0]].concat()),
        ("counting past its end", u32::MAX.to_le_bytes().to_vec()),
    ];
    let edited = |edit: &dyn Fn(&mut Imported)| {
        let mut imported = imported.clone();
        edit(&mut imported);
        encode(&imported, &images)
    };
    cases.push((
        "a part before its parent",
        edited(&|imported| imported.parts[0].parent = Some(1)),
    ));
    cases.push((
        "a part drawing what it hasn't",
        edited(&|imported| imported.parts[1].primitive = Some(2)),
    ));
    // What glTF's reader refuses too, which reaches the world as a node's.
    cases.push((
        "a part turned by a rotation that isn't a unit",
        edited(&|imported| {
            imported.parts[0].transform.rotation = Quat::from_xyzw(0.0, 0.0, 0.0, 2.0);
        }),
    ));
    cases.push((
        "a part moved by a NaN",
        edited(&|imported| imported.parts[1].transform.translation.x = f64::from(f32::NAN)),
    ));
    cases.push((
        "a part scaled infinitely",
        edited(&|imported| imported.parts[1].transform.scale.y = f32::INFINITY),
    ));
    cases.push((
        "a light of a negative intensity",
        edited(&|imported| {
            imported.parts[0].light.as_mut().expect("a light").intensity = -1.0;
        }),
    ));
    cases.push((
        "a spot's cone past a quarter turn",
        edited(&|imported| {
            imported.parts[0]
                .light
                .as_mut()
                .expect("a light")
                .outer_cone = 2.0;
        }),
    ));
    cases.push((
        "a material of an infinite color",
        edited(&|imported| imported.primitives[0].material.color[0] = f32::INFINITY),
    ));
    cases.push((
        "a material of a NaN glossiness",
        edited(&|imported| {
            imported.primitives[1].material.workflow = Workflow::SpecularGlossiness {
                specular: Vec3::ONE,
                glossiness: f32::NAN,
            };
        }),
    ));
    let mesh = |edit: fn(&mut ModelMesh)| {
        move |imported: &mut Imported| {
            edit(imported.primitives[0].mesh.as_mut().expect("a mesh"));
        }
    };
    cases.push((
        "an index past its vertices",
        edited(&mesh(|mesh| mesh.indices[8] = 4)),
    ));
    cases.push((
        "indices that aren't whole triangles",
        edited(&mesh(|mesh| {
            mesh.indices.pop();
            mesh.lods.levels.pop();
            mesh.meshlets.levels.pop();
        })),
    ));
    cases.push((
        "a level past its indices",
        edited(&mesh(|mesh| mesh.lods.levels[1].start = 7)),
    ));
    cases.push((
        "a level of no triangles",
        edited(&mesh(|mesh| mesh.lods.levels[1].count = 0)),
    ));
    cases.push((
        "an error that shrinks",
        edited(&mesh(|mesh| mesh.lods.levels[0].error = 1.0)),
    ));
    cases.push((
        "an error that isn't finite",
        edited(&mesh(|mesh| mesh.lods.levels[1].error = f32::NAN)),
    ));
    cases.push((
        "a meshlet's vertex past the mesh's",
        edited(&mesh(|mesh| mesh.meshlets.vertices[3] = 4)),
    ));
    cases.push((
        "a meshlet's corner past its vertices",
        edited(&mesh(|mesh| mesh.meshlets.triangles[9] = 3)),
    ));
    cases.push((
        "a meshlet past its triangles",
        edited(&mesh(|mesh| mesh.meshlets.meshlets[1].triangle_count = 2)),
    ));
    cases.push(("a step of 0", edited(&mesh(|mesh| mesh.packing.step = 0.0))));
    // Its skinned part's mesh skinned, without meshlets, which skinning would leave behind:
    // moved by the skin's two joints, it reads; by a third, it's refused, as glTF's would be.
    let skinned = |joint| {
        move |imported: &mut Imported| {
            let mesh = imported.primitives[0].mesh.as_mut().expect("a mesh");
            mesh.meshlets = Meshlets::default();
            let moved = SkinVertex::new([0, 1, joint, 0], [0.5, 0.25, 0.25, 0.0]);
            mesh.skin = vec![moved; mesh.vertices.len()];
        }
    };
    let read = decode("m", Path::new("m.glb"), &edited(&skinned(1)));
    assert_eq!(read.expect("moved by its skin's joints").primitives[0], {
        let mut imported = imported.clone();
        skinned(1)(&mut imported);
        imported.primitives[0].clone()
    });
    cases.push((
        "a vertex moved by a joint past its skin's",
        edited(&skinned(2)),
    ));
    cases.push((
        "a material sampling what it hasn't",
        edited(&|imported| {
            let material = &mut imported.primitives[0].material;
            material.textures[0] = material.textures[2].map(|texture| Sampling {
                image: 2,
                ..texture
            });
        }),
    ));
    let mut image = images.clone();
    image[0][0] ^= 1;
    cases.push(("an image that isn't KTX2", encode(&imported, &image)));
    // The normal map's BC5 where its colors' BC7 go.
    let mut swapped = imported.clone();
    swapped.images.swap(0, 1);
    let swapped_images = [images[1].clone(), images[0].clone()];
    cases.push((
        "an image compressed as other than its materials sample it",
        encode(&swapped, &swapped_images),
    ));
    cases.push((
        "a light of a kind it hasn't",
        edited(&|imported| {
            let light = imported.parts[0].light.as_mut().expect("a light");
            light.kind = LightKind::Point;
        })
        .into_iter()
        .enumerate()
        .map(|(i, byte)| {
            // The light's kind is the byte after the first part's name, its transform and its
            // three indices, past the ignored's count and string and the parts' count.
            let at = 4 + 4 + "1 camera".len() + 4 + 4 + "lamp".len() + 40 + 12;
            if i == at { 9 } else { byte }
        })
        .collect(),
    ));
    for (case, bytes) in cases {
        let error = decode("m", Path::new("m.glb"), &bytes).expect_err(case);
        assert_eq!(error.code(), "engine.bad_model", "{case}");
        assert!(error.to_string().contains("\"m\""), "{case}: {error}");
        if case == "a light of a kind it hasn't" {
            let kind = error.to_string().contains("has a light of kind 9");
            assert!(kind, "{case}: {error}");
        }
        if case == "a vertex moved by a joint past its skin's" {
            let past = "has primitive 0 moved by joint 2, past the 2 of skin 0, which skins it";
            assert!(error.to_string().contains(past), "{case}: {error}");
        }
    }
}
