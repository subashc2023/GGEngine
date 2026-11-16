//! A model's default scene: its nodes, their transforms and lights.

use std::collections::BTreeMap;
use std::io;
use std::mem;
use std::path::Path;
use std::sync::Arc;

use gg_core::Result;
use gg_core::math::{Mat4, Quat, Vec3, Vec4};

use super::{Part, Reader, json};
use crate::{Light, LightKind, Transform};

/// How far a matrix may be from the one its translation, rotation and scale compose back to, as a
/// fraction of each column's length: past what rounding gives, as of a matrix printed to four
/// places, and short of a shear that would show.
const DECOMPOSES_WITHIN: f32 = 1e-3;

impl<'a, F: Fn(&Path, u64) -> io::Result<Vec<u8>>> Reader<'a, F> {
    /// Reads the default scene into parts, from its roots down, each before its children.
    pub(super) fn scene(&mut self) -> Result<()> {
        let (model, gltf) = (self.model, self.gltf);
        let index = (gltf.scene)
            .or((!gltf.scenes.is_empty()).then_some(0))
            .ok_or_else(|| {
                model.unsupported("has no scene: it's a library of assets rather than a model")
            })?;
        let scene = model.item(&gltf.scenes, "scene", index)?;
        let mut reached = vec![false; gltf.nodes.len()];
        self.parts_of = vec![None; gltf.nodes.len()];
        // Each skinned part, and its skin in glTF's order, which are read once every joint's part
        // is known.
        let mut skinned: Vec<(usize, u32)> = Vec::new();
        // A stack of its own rather than recursion, so that no chain of nodes is too deep.
        let mut stack: Vec<(u32, Option<usize>)> =
            scene.nodes.iter().rev().map(|&node| (node, None)).collect();
        while let Some((index, parent)) = stack.pop() {
            let node = model.item(&gltf.nodes, "node", index)?;
            if mem::replace(&mut reached[index as usize], true) {
                return Err(model.bad(format!(
                    "reaches node {index} twice: it's in a cycle, or has two parents"
                )));
            }
            // A node's morph weights replace its mesh's, which are only their default.
            let weights = match (node.mesh, node.weights.is_empty()) {
                (Some(mesh), true) => &model.item(&gltf.meshes, "mesh", mesh)?.weights,
                _ => &node.weights,
            };
            if weights.iter().any(|&weight| weight != 0.0) {
                return Err(model.unsupported(format!(
                    "has node {index} with morph weights other than 0, which the engine doesn't \
                     draw"
                )));
            }
            let transform = self.transform(index, node)?;
            let primitives = match node.mesh {
                Some(mesh) => self.mesh(mesh)?,
                None => 0..0,
            };
            let light = match &node.extensions.lights_punctual {
                Some(light) => Some(self.light(light.light)?),
                None => None,
            };
            let name = node.name.as_deref().filter(|name| !name.is_empty());
            self.memory
                .hold(size_of::<Part>() + name.map_or(0, str::len))?;
            let part = self.imported.parts.len();
            self.parts_of[index as usize] = Some(part);
            let one = (primitives.len() == 1).then_some(primitives.start);
            self.imported.parts.push(Part {
                name: name.map(Arc::from),
                transform,
                parent,
                primitive: one,
                light,
                skin: None,
            });
            if let (Some(skin), Some(_)) = (node.skin, one) {
                skinned.push((part, skin));
            }
            if primitives.len() > 1 {
                // An entity draws one primitive, so each of the mesh's is a child of the node's.
                self.memory.hold(primitives.len() * size_of::<Part>())?;
                for primitive in primitives {
                    if let Some(skin) = node.skin {
                        skinned.push((self.imported.parts.len(), skin));
                    }
                    self.imported.parts.push(Part {
                        name: None,
                        transform: Transform::IDENTITY,
                        parent: Some(part),
                        primitive: Some(primitive),
                        light: None,
                        skin: None,
                    });
                }
            }
            stack.extend(node.children.iter().rev().map(|&child| (child, Some(part))));
        }
        // Each skin once, as it's first reached, and the parts it moves checked against it.
        let mut read: BTreeMap<u32, usize> = BTreeMap::new();
        for (part, index) in skinned {
            let skin = match read.get(&index) {
                Some(&skin) => skin,
                None => {
                    let skin = self.skin(index)?;
                    self.imported.skins.push(skin);
                    read.insert(index, self.imported.skins.len() - 1);
                    self.imported.skins.len() - 1
                }
            };
            self.imported.parts[part].skin = Some(skin);
            let primitive = self.imported.parts[part]
                .primitive
                .expect("a skinned part draws");
            let primitive = &self.imported.primitives[primitive];
            let joints = self.imported.skins[skin].joints.len();
            if primitive.joints.is_empty() {
                return Err(model.bad(format!(
                    "has primitive {}, which a node skinned by skin {index} draws, without \
                     JOINTS_0 and WEIGHTS_0",
                    primitive.name
                )));
            }
            let past = primitive
                .joints
                .iter()
                .flatten()
                .find(|&&j| usize::from(j) >= joints);
            if let Some(past) = past {
                return Err(model.bad(format!(
                    "has primitive {} moved by joint {past}, past the {joints} of skin {index}, \
                     which skins it",
                    primitive.name
                )));
            }
        }
        Ok(())
    }

    /// Light `index` of KHR_lights_punctual's, which a node holds, checked as the extension says,
    /// of a type it has, and as [`allowed`] says. Its intensity and range are in its units, candela
    /// or lux, and meters, the engine's.
    fn light(&self, index: u32) -> Result<Light> {
        let (model, gltf) = (self.model, self.gltf);
        let lights = gltf.extensions.lights_punctual.as_ref();
        let light = model.item(lights.map_or(&[][..], |l| &l.lights), "light", index)?;
        let kind = match light.kind.as_str() {
            "directional" => LightKind::Directional,
            "point" => LightKind::Point,
            "spot" => LightKind::Spot,
            other => {
                return Err(model.bad(format!(
                    "has light {index} of type {other:?}, which KHR_lights_punctual doesn't have"
                )));
            }
        };
        let spot = light.spot.as_ref();
        let read = Light {
            kind,
            color: light.color.map_or(Vec3::ONE, Vec3::from_array),
            intensity: light.intensity.unwrap_or(1.0),
            range: light.range,
            inner_cone: spot.and_then(|spot| spot.inner_cone_angle).unwrap_or(0.0),
            outer_cone: (spot.and_then(|spot| spot.outer_cone_angle))
                .unwrap_or(std::f32::consts::FRAC_PI_4),
            // KHR_lights_punctual says nothing of shadows.
            ..Light::default()
        };
        if !allowed(&read) {
            return Err(model.bad(format!(
                "has light {index} of a negative color or intensity, a range of 0 or less, a \
                 cone's angles out of order or past a quarter turn, or a number that isn't finite"
            )));
        }
        Ok(read)
    }

    /// Node `index`'s transform: its matrix decomposed, or its translation, rotation and scale, its
    /// rotation made unit; one [`takes`] takes, as JSON's numbers are finite, and serde_json's
    /// `float_roundtrip`, which the workspace turns on, refuses one past an f32's range.
    fn transform(&self, index: u32, node: &json::Node) -> Result<Transform> {
        let model = self.model;
        let Some(matrix) = node.matrix else {
            let rotation = match node.rotation {
                Some(rotation) => unit(Quat::from_array(rotation)).ok_or_else(|| {
                    model.bad(format!(
                        "has node {index} whose rotation is 0, or too near it or too large to \
                         make unit"
                    ))
                })?,
                None => Quat::IDENTITY,
            };
            return Ok(Transform {
                translation: node
                    .translation
                    .map_or(Vec3::ZERO, Vec3::from_array)
                    .as_dvec3(),
                rotation,
                scale: node.scale.map_or(Vec3::ONE, Vec3::from_array),
            });
        };
        if node.translation.is_some() || node.rotation.is_some() || node.scale.is_some() {
            return Err(model.bad(format!(
                "has node {index} with a matrix and a translation, rotation or scale too"
            )));
        }
        let doesnt_decompose = || {
            model.bad(format!(
                "has node {index} whose matrix doesn't decompose into translation, rotation and \
                 scale"
            ))
        };
        let matrix = Mat4::from_cols_array(&matrix);
        if matrix.row(3) != Vec4::W {
            return Err(doesnt_decompose());
        }
        let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
        let rotation = unit(rotation).ok_or_else(doesnt_decompose)?;
        if !scale.is_finite() {
            return Err(doesnt_decompose());
        }
        // A shear, or a column of 0, composes back to another matrix. Each column is compared with
        // its own length, so that a small scale on one axis hides no shear along it.
        let composed = Mat4::from_scale_rotation_translation(scale, rotation, translation);
        let decomposes = (0..3).all(|i| {
            let (column, back) = (matrix.col(i), composed.col(i));
            let within = Vec4::splat(DECOMPOSES_WITHIN * column.length());
            (back - column).abs().cmple(within).all()
        });
        if !decomposes {
            return Err(doesnt_decompose());
        }
        Ok(Transform {
            translation: translation.as_dvec3(),
            rotation,
            scale,
        })
    }
}

/// `rotation` scaled to unit length; `None` if it's 0, or too near it or too large to scale to a
/// unit, as the world takes one.
pub(super) fn unit(rotation: Quat) -> Option<Quat> {
    let length = rotation.length();
    let scaled = (length > 0.0 && length.is_finite()).then(|| rotation / length);
    scaled.filter(|unit| unit.is_normalized())
}

/// Whether `transform` is one a model's part may have, as the world takes it: of finite numbers,
/// its rotation a unit.
pub(crate) fn takes(transform: &Transform) -> bool {
    let Transform {
        translation,
        rotation,
        scale,
    } = transform;
    translation.is_finite() && scale.is_finite() && rotation.is_normalized()
}

/// Whether `light` is one a model's part may have, as KHR_lights_punctual allows it, of finite
/// numbers: of a color and an intensity not negative, a range more than 0 if it has one, and a
/// spot's cone's angles from 0 to the outer, which is more than the inner and at most a quarter
/// turn.
pub(crate) fn allowed(light: &Light) -> bool {
    let Light {
        color,
        intensity,
        range,
        inner_cone,
        outer_cone,
        ..
    } = *light;
    color.is_finite()
        && color.cmpge(Vec3::ZERO).all()
        && intensity.is_finite()
        && intensity >= 0.0
        && range.is_none_or(|range| range.is_finite() && range > 0.0)
        && (0.0..outer_cone).contains(&inner_cone)
        && outer_cone <= std::f32::consts::FRAC_PI_2
}
