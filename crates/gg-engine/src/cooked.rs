//! Cooked models: a model as the cooker writes it into a game's package, which a run from the
//! package reads in place of its glTF file and the files that names. It holds what [`gltf::read`]
//! makes of them, its primitives' meshes as the cooker made them, their triangles optimized, with
//! their levels of detail, their vertices packed and their levels cut into meshlets, as
//! [`ModelMesh`] holds them, and its images compressed in KTX2 as what each holds says, BC7 of colors or of values,
//! or BC5 of a normal map, or BC7 of one, as a DDS image holds it, with their mip levels, as
//! [`read_ktx2`] reads them, so that a run neither decodes PNGs and JPEGs nor makes levels for
//! them, but takes the levels' blocks out of zstd, as the loader does.
//!
//! The format, every number little-endian, every count and length 32 bits, a string its length
//! then its UTF-8:
//!
//! ```text
//! ignored     a count, then each thing the model has that the engine ignores, as a string
//! parts       a count, then each part's translation (3 f32s), rotation (4) and scale (3); its
//!             parent's index and its primitive's, each u32::MAX for none; and its light's kind,
//!             a byte (0 for none, then directional, point, spot), and, if it has one, its color
//!             (3 f32s), intensity, range (0 for none), inner cone and outer cone (an f32 each)
//! primitives  a count, then each one's name, as a string; its mesh: its packing (8 f32s: offset,
//!             step, offset and scale of its texture coordinates), its vertices' count and each
//!             vertex as `PackedVertex` lays it out (3 u16s, an i16, 4 i16s, 2 u16s and 4 bytes),
//!             its indices' count and its indices, its levels' count and each one's start, count
//!             and error (an f32), its sphere's center (3 f32s) and radius, its meshlets' count and
//!             each one's first vertex, vertex count, first triangle and triangle count, its
//!             center (3 f32s) and radius and its cone (4 bytes), its meshlets' vertices' count and
//!             each, its meshlets' triangles' bytes' count and the bytes, and the count of its
//!             levels' meshlets, none or as many as its levels, and each one's first and count;
//!             its material's color (4 f32s), its workflow, a byte (0 for metallic-roughness, 1 for
//!             specular-glossiness),
//!             then its metallic and roughness (an f32 each) or its specular color (3 f32s) and
//!             glossiness (an f32), its normal scale and occlusion strength (an f32 each) and
//!             emission (3 f32s); each of its textures in `Slot`'s order, as its image (u32::MAX
//!             for none), whether it's nearest (a byte) and its wrap (a byte: clamp, repeat,
//!             mirror); its alpha mode (a byte: opaque, mask, blend), the mask's cutoff (an f32),
//!             and whether it's double-sided and unlit (a byte each)
//! images      a count, then each one's KTX2, as its length and its bytes
//! ```
//!
//! Its floats are the bits the reader made, so that a model's nodes, which reach the world hash,
//! are the same from a package as from the model's files.

use std::path::Path;

use gg_core::math::{Quat, Vec2, Vec3};
use gg_core::{Error, Result};
use gg_renderer::{
    AlphaMode, Lod, Lods, Meshlet, Meshlets, ModelMesh, PackedVertex, Packing, Workflow, Wrap,
};

use crate::gltf::{self, Imported, Material, Part, Primitive, Sampling};
use crate::image::read_ktx2;
use crate::model;
use crate::package::{Files, Kind};
use crate::{Light, LightKind, Transform};

/// What stands for none, where the format holds an index.
const NONE: u32 = u32::MAX;

/// Model `imported`, as the cooker writes it into a game's package, its images given as `images`,
/// their KTX2 in its images' order, in place of those `imported` has, and each primitive's mesh as
/// the cooker made it.
///
/// # Panics
///
/// If `images` aren't as many as `imported`'s, a primitive hasn't its mesh, or a count or a length
/// is past 32 bits.
pub(crate) fn encode(imported: &Imported, images: &[Vec<u8>]) -> Vec<u8> {
    assert_eq!(
        imported.images.len(),
        images.len(),
        "a cooked model holds each of its images"
    );
    let mut bytes = Vec::new();
    let count = |bytes: &mut Vec<u8>, count: usize| {
        let count = u32::try_from(count).expect("a count of 32 bits");
        bytes.extend_from_slice(&count.to_le_bytes());
    };
    let floats = |bytes: &mut Vec<u8>, floats: &[f32]| {
        for float in floats {
            bytes.extend_from_slice(&float.to_le_bytes());
        }
    };
    let index = |bytes: &mut Vec<u8>, index: Option<usize>| {
        let index = index.map_or(NONE, |index| {
            u32::try_from(index).expect("an index of 32 bits")
        });
        bytes.extend_from_slice(&index.to_le_bytes());
    };
    let string = |bytes: &mut Vec<u8>, string: &str| {
        count(bytes, string.len());
        bytes.extend_from_slice(string.as_bytes());
    };
    count(&mut bytes, imported.ignored.len());
    for ignored in &imported.ignored {
        string(&mut bytes, ignored);
    }
    count(&mut bytes, imported.parts.len());
    for part in &imported.parts {
        let Transform {
            translation,
            rotation,
            scale,
        } = part.transform;
        floats(&mut bytes, &translation.to_array());
        floats(&mut bytes, &rotation.to_array());
        floats(&mut bytes, &scale.to_array());
        index(&mut bytes, part.parent);
        index(&mut bytes, part.primitive);
        let Some(light) = &part.light else {
            bytes.push(0);
            continue;
        };
        bytes.push(match light.kind {
            LightKind::Directional => 1,
            LightKind::Point => 2,
            LightKind::Spot => 3,
        });
        floats(&mut bytes, &light.color.to_array());
        floats(
            &mut bytes,
            &[
                light.intensity,
                light.range.unwrap_or(0.0),
                light.inner_cone,
                light.outer_cone,
            ],
        );
    }
    count(&mut bytes, imported.primitives.len());
    for primitive in &imported.primitives {
        string(&mut bytes, &primitive.name);
        let mesh =
            (primitive.mesh.as_ref()).expect("a cooked model's primitives have their meshes");
        let words = |bytes: &mut Vec<u8>, words: &[u32]| {
            for word in words {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
        };
        let Packing {
            offset,
            step,
            uv_offset,
            uv_scale,
        } = mesh.packing;
        floats(&mut bytes, &offset.to_array());
        floats(&mut bytes, &[step]);
        floats(&mut bytes, &uv_offset.to_array());
        floats(&mut bytes, &uv_scale.to_array());
        count(&mut bytes, mesh.vertices.len());
        for vertex in &mesh.vertices {
            for half in vertex.position {
                bytes.extend_from_slice(&half.to_le_bytes());
            }
            let signed = [
                vertex.sign,
                vertex.normal[0],
                vertex.normal[1],
                vertex.tangent[0],
            ];
            for half in signed.into_iter().chain([vertex.tangent[1]]) {
                bytes.extend_from_slice(&half.to_le_bytes());
            }
            for half in vertex.uv {
                bytes.extend_from_slice(&half.to_le_bytes());
            }
            bytes.extend_from_slice(&vertex.color);
        }
        count(&mut bytes, mesh.indices.len());
        words(&mut bytes, &mesh.indices);
        count(&mut bytes, mesh.lods.levels.len());
        for lod in &mesh.lods.levels {
            words(&mut bytes, &[lod.start, lod.count]);
            floats(&mut bytes, &[lod.error]);
        }
        floats(&mut bytes, &mesh.lods.center.to_array());
        floats(&mut bytes, &[mesh.lods.radius]);
        let meshlets = &mesh.meshlets;
        count(&mut bytes, meshlets.meshlets.len());
        for meshlet in &meshlets.meshlets {
            let Meshlet {
                vertices,
                vertex_count,
                triangles,
                triangle_count,
                center,
                radius,
                cone,
            } = *meshlet;
            words(
                &mut bytes,
                &[vertices, vertex_count, triangles, triangle_count],
            );
            floats(&mut bytes, &center.to_array());
            floats(&mut bytes, &[radius]);
            bytes.extend(cone.map(|byte| byte as u8));
        }
        count(&mut bytes, meshlets.vertices.len());
        words(&mut bytes, &meshlets.vertices);
        count(&mut bytes, meshlets.triangles.len());
        bytes.extend_from_slice(&meshlets.triangles);
        count(&mut bytes, meshlets.levels.len());
        words(&mut bytes, meshlets.levels.as_flattened());
        let material = &primitive.material;
        floats(&mut bytes, &material.color);
        match material.workflow {
            Workflow::MetallicRoughness {
                metallic,
                roughness,
            } => {
                bytes.push(0);
                floats(&mut bytes, &[metallic, roughness]);
            }
            Workflow::SpecularGlossiness {
                specular,
                glossiness,
            } => {
                bytes.push(1);
                floats(&mut bytes, &specular.to_array());
                floats(&mut bytes, &[glossiness]);
            }
        }
        floats(
            &mut bytes,
            &[material.normal_scale, material.occlusion_strength],
        );
        floats(&mut bytes, &material.emissive);
        for texture in material.textures {
            index(&mut bytes, texture.map(|texture| texture.image));
            let (nearest, wrap) = texture.map_or((false, Wrap::Repeat), |texture| {
                (texture.nearest, texture.wrap)
            });
            let wrap = match wrap {
                Wrap::Clamp => 0,
                Wrap::Repeat => 1,
                Wrap::Mirror => 2,
            };
            bytes.extend_from_slice(&[u8::from(nearest), wrap]);
        }
        let (alpha, cutoff) = match material.alpha {
            AlphaMode::Opaque => (0, 0.0),
            AlphaMode::Mask { cutoff } => (1, cutoff),
            AlphaMode::Blend => (2, 0.0),
        };
        bytes.push(alpha);
        floats(&mut bytes, &[cutoff]);
        bytes.extend_from_slice(&[u8::from(material.double_sided), u8::from(material.unlit)]);
    }
    count(&mut bytes, images.len());
    for image in images {
        count(&mut bytes, image.len());
        bytes.extend_from_slice(image);
    }
    bytes
}

/// Reads model `name`'s file at `path` as the game's package holds it, cooked, through `files`, as
/// [`decode`] reads it.
///
/// Fails with `engine.bad_model` if the package hasn't it as a cooked model, and as `decode` does.
pub(crate) fn read(name: &str, path: &Path, files: &Files) -> Result<Imported> {
    let zone = gg_core::zone!();
    zone.text(path.display());
    let bytes = (files.read_as(path, Kind::Model, u64::MAX)).map_err(|error| {
        let message = format!("model {name:?} at {} can't be read", path.display());
        Error::new("engine.bad_model", message).with_source(error)
    })?;
    decode(name, path, &bytes)
}

/// Reads model `name`, as the cooker writes it, from `bytes`, its file at `path`, checking that it
/// holds what [`gltf::read`] would give: each part after its parent, primitives whose meshes hold
/// together, as [`ModelMesh::check`] checks them, and images that
/// read, each the KTX2 of BC7 that [`read_ktx2`] takes, read as far as its header, taking no more
/// memory than [`gltf::MAX_MEMORY`], what its images decode to among it.
///
/// Fails with `engine.bad_model` for what doesn't, saying where.
pub(crate) fn decode(name: &str, path: &Path, bytes: &[u8]) -> Result<Imported> {
    let bad = |problem: String| {
        let path = path.display();
        let message = format!("model {name:?} at {path} doesn't read as a cooked model: {problem}");
        Error::new("engine.bad_model", message)
    };
    let mut reader = Reader { bytes };
    let imported = reader.imported().map_err(bad)?;
    if !reader.bytes.is_empty() {
        let message = format!("{} bytes follow its last image", reader.bytes.len());
        return Err(bad(message));
    }
    Ok(imported)
}

/// What's left to read of a cooked model's bytes.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// The model, but for any bytes after it.
    fn imported(&mut self) -> Result<Imported, String> {
        let ignored = (0..self.count(4)?)
            .map(|_| self.string())
            .collect::<Result<_, _>>()?;
        let mut parts: Vec<Part> = Vec::new();
        // A transform, two indices and a light's kind.
        let part_count = self.count(49)?;
        let mut drawn = Vec::with_capacity(part_count);
        for i in 0..part_count {
            let [x, y, z, rx, ry, rz, rw, sx, sy, sz] = self.floats()?;
            let transform = Transform {
                translation: Vec3::new(x, y, z),
                rotation: Quat::from_xyzw(rx, ry, rz, rw),
                scale: Vec3::new(sx, sy, sz),
            };
            let parent = self.index()?;
            if parent.is_some_and(|parent| parent >= i) {
                return Err(format!("its part {i} comes before its parent"));
            }
            let primitive = self.index()?;
            drawn.extend(primitive);
            let light = self
                .light()
                .map_err(|problem| format!("its part {i} {problem}"))?;
            parts.push(Part {
                transform,
                parent,
                primitive,
                light,
            });
        }
        let mut held = 0;
        let mut primitives = Vec::new();
        for i in 0..self.count(1)? {
            let primitive = self
                .primitive()
                .map_err(|problem| format!("its primitive {i} {problem}"))?;
            let mesh = primitive.mesh.as_ref().expect("a cooked primitive's mesh");
            let meshlets = &mesh.meshlets;
            held += size_of_val(mesh.vertices.as_slice())
                + size_of_val(mesh.indices.as_slice())
                + size_of_val(meshlets.meshlets.as_slice())
                + size_of_val(meshlets.vertices.as_slice())
                + meshlets.triangles.len();
            primitives.push(primitive);
        }
        if let Some(i) = drawn.iter().find(|&&i| i >= primitives.len()) {
            return Err(format!("a part draws primitive {i}, which it hasn't"));
        }
        let mut images = Vec::new();
        for i in 0..self.count(4)? {
            let len = self.count(1)?;
            let image = read_ktx2(self.take(len)?.to_vec())
                .map_err(|problem| format!("its image {i} doesn't read: {problem}"))?;
            // Its file, held until the loader decodes it, and what it decodes to.
            held += image.pixels.len() + image.decoded_len();
            if held as u64 > gltf::MAX_MEMORY {
                return Err(format!(
                    "it takes more than {} GiB to read",
                    gltf::MAX_MEMORY >> 30
                ));
            }
            images.push(image);
        }
        let mut samples = (primitives.iter())
            .flat_map(|primitive| primitive.material.textures.into_iter().flatten())
            .map(|texture| texture.image);
        if let Some(i) = samples.find(|&i| i >= images.len()) {
            return Err(format!("a material samples image {i}, which it hasn't"));
        }
        // Each image compressed as what its materials sample it as, which reading the model left
        // one of: colors, values or a normal map.
        for (i, image) in images.iter().enumerate() {
            let texels = model::texels(&primitives, i);
            if !image.compressed().is_some_and(|held| held.holds(texels)) {
                return Err(format!(
                    "its image {i} holds {:?}, where its materials sample it as {texels:?}",
                    image.compressed()
                ));
            }
        }
        Ok(Imported {
            parts,
            primitives,
            images,
            ignored,
        })
    }

    /// A part's light, if it has one. The error says what's wrong with it.
    fn light(&mut self) -> Result<Option<Light>, String> {
        let [kind]: [u8; 1] = self.take(1)?.try_into().expect("a byte");
        let kind = match kind {
            0 => return Ok(None),
            1 => LightKind::Directional,
            2 => LightKind::Point,
            3 => LightKind::Spot,
            _ => return Err(format!("has a light of kind {kind}")),
        };
        let [r, g, b, intensity, range, inner_cone, outer_cone] = self.floats()?;
        Ok(Some(Light {
            kind,
            color: Vec3::new(r, g, b),
            intensity,
            range: (range > 0.0).then_some(range),
            inner_cone,
            outer_cone,
            // As glTF's, which says nothing of shadows.
            ..Light::default()
        }))
    }

    /// A primitive. The error says what's wrong with it.
    fn primitive(&mut self) -> Result<Primitive, String> {
        let name = self.string()?;
        let mesh = self.mesh()?;
        let color = self.floats()?;
        let [workflow]: [u8; 1] = self.take(1)?.try_into().expect("a byte");
        let workflow = match workflow {
            0 => {
                let [metallic, roughness] = self.floats()?;
                Workflow::MetallicRoughness {
                    metallic,
                    roughness,
                }
            }
            1 => {
                let [r, g, b, glossiness] = self.floats()?;
                Workflow::SpecularGlossiness {
                    specular: Vec3::new(r, g, b),
                    glossiness,
                }
            }
            _ => return Err(format!("has a material of workflow {workflow}")),
        };
        let [normal_scale, occlusion_strength] = self.floats()?;
        let emissive = self.floats()?;
        let flag = |byte: u8, what: &str| match byte {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(format!("has a material whose {what} is {byte}")),
        };
        let mut textures = [None; 5];
        for texture in &mut textures {
            let image = self.index()?;
            let [nearest, wrap]: [u8; 2] = self.take(2)?.try_into().expect("two bytes");
            let wrap = match wrap {
                0 => Wrap::Clamp,
                1 => Wrap::Repeat,
                2 => Wrap::Mirror,
                _ => return Err(format!("has a material that wraps by {wrap}")),
            };
            let nearest = flag(nearest, "nearest")?;
            *texture = image.map(|image| Sampling {
                image,
                nearest,
                wrap,
            });
        }
        let [alpha]: [u8; 1] = self.take(1)?.try_into().expect("a byte");
        let [cutoff] = self.floats()?;
        let [double_sided, unlit]: [u8; 2] = self.take(2)?.try_into().expect("two bytes");
        let alpha = match alpha {
            0 => AlphaMode::Opaque,
            1 => AlphaMode::Mask { cutoff },
            2 => AlphaMode::Blend,
            _ => return Err(format!("has a material of alpha mode {alpha}")),
        };
        Ok(Primitive {
            name,
            positions: Vec::new(),
            uvs: Vec::new(),
            colors: Vec::new(),
            normals: Vec::new(),
            tangents: Vec::new(),
            indices: Vec::new(),
            mesh: Some(mesh),
            material: Material {
                color,
                workflow,
                normal_scale,
                occlusion_strength,
                emissive,
                textures,
                alpha,
                double_sided: flag(double_sided, "double-sided")?,
                unlit: flag(unlit, "unlitness")?,
            },
        })
    }

    /// A primitive's mesh, which holds together as [`ModelMesh::check`] checks it. The error says
    /// what's wrong with it.
    fn mesh(&mut self) -> Result<ModelMesh, String> {
        let [x, y, z, step, u, v, u_scale, v_scale] = self.floats()?;
        let packing = Packing {
            offset: Vec3::new(x, y, z),
            step,
            uv_offset: Vec2::new(u, v),
            uv_scale: Vec2::new(u_scale, v_scale),
        };
        let vertices = (0..self.count(size_of::<PackedVertex>())?)
            .map(|_| {
                let bytes = self.take(size_of::<PackedVertex>())?;
                let (halves, color) = bytes.split_at(20);
                let (halves, _) = halves.as_chunks::<2>();
                let half = |i: usize| u16::from_le_bytes(halves[i]);
                let signed = |i: usize| i16::from_le_bytes(halves[i]);
                Ok(PackedVertex {
                    position: [half(0), half(1), half(2)],
                    sign: signed(3),
                    normal: [signed(4), signed(5)],
                    tangent: [signed(6), signed(7)],
                    uv: [half(8), half(9)],
                    color: color.try_into().expect("4 bytes"),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let indices = self.words(4)?;
        let levels = (0..self.count(12)?)
            .map(|_| {
                let [start, count] = [self.u32()?, self.u32()?];
                let [error] = self.floats()?;
                Ok(Lod {
                    start,
                    count,
                    error,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let [cx, cy, cz, radius] = self.floats()?;
        // Its first vertex and triangle, their counts, its sphere and its cone.
        let meshlets = (0..self.count(36)?)
            .map(|_| {
                let [vertices, vertex_count, triangles, triangle_count] =
                    [self.u32()?, self.u32()?, self.u32()?, self.u32()?];
                let [x, y, z, radius] = self.floats()?;
                let cone: [u8; 4] = self.take(4)?.try_into().expect("4 bytes");
                Ok(Meshlet {
                    vertices,
                    vertex_count,
                    triangles,
                    triangle_count,
                    center: Vec3::new(x, y, z),
                    radius,
                    cone: cone.map(|byte| byte as i8),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let meshlet_vertices = self.words(4)?;
        let len = self.count(1)?;
        let triangles = self.take(len)?.to_vec();
        let meshlet_levels = (0..self.count(8)?)
            .map(|_| Ok([self.u32()?, self.u32()?]))
            .collect::<Result<Vec<_>, String>>()?;
        let mesh = ModelMesh {
            packing,
            vertices,
            indices,
            lods: Lods {
                levels,
                center: Vec3::new(cx, cy, cz),
                radius,
            },
            meshlets: Meshlets {
                meshlets,
                vertices: meshlet_vertices,
                triangles,
                levels: meshlet_levels,
            },
        };
        // What's wrong with it, which follows the primitive's own name in the error.
        mesh.check("its").map_err(|error| {
            let message = error.to_string();
            let problem = message.strip_prefix("mesh \"its\" ").unwrap_or(&message);
            format!("has a mesh that {problem}")
        })?;
        Ok(mesh)
    }

    /// A count of words, then the words, the count checked against the bytes left, `each` bytes a
    /// word.
    fn words(&mut self, each: usize) -> Result<Vec<u32>, String> {
        (0..self.count(each)?).map(|_| self.u32()).collect()
    }

    /// The next `len` bytes.
    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        if len > self.bytes.len() {
            return Err(format!("it ends {} bytes short", len - self.bytes.len()));
        }
        let (taken, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(taken)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?.try_into().expect("four bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    /// A count of what follows, each of which takes at least `each` bytes: no more than the bytes
    /// left hold, so that nothing is allocated for more than the file has.
    fn count(&mut self, each: usize) -> Result<usize, String> {
        let count = self.u32()? as usize;
        if count.saturating_mul(each) > self.bytes.len() {
            return Err(format!(
                "it counts {count} of something, more than its {} bytes left hold",
                self.bytes.len()
            ));
        }
        Ok(count)
    }

    /// An index, or none.
    fn index(&mut self) -> Result<Option<usize>, String> {
        let index = self.u32()?;
        Ok((index != NONE).then_some(index as usize))
    }

    fn floats<const N: usize>(&mut self) -> Result<[f32; N], String> {
        let bytes = self.take(N * 4)?;
        let (floats, _) = bytes.as_chunks::<4>();
        Ok(std::array::from_fn(|i| f32::from_le_bytes(floats[i])))
    }

    fn string(&mut self) -> Result<String, String> {
        let len = self.count(1)?;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| "it has a name that isn't UTF-8".to_owned())
    }
}

#[cfg(test)]
mod tests {
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
        };
        let turned = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
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
                transform: turned,
                parent: None,
                primitive: None,
                light: Some(spot),
            },
            Part {
                transform: Transform::IDENTITY,
                parent: Some(0),
                primitive: Some(0),
                light: None,
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
            ignored: vec!["1 animation".to_owned()],
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
                // The light's kind is the byte after the first part's transform and indices, past
                // the ignored's count and string and the parts' count.
                let at = 4 + 4 + "1 animation".len() + 4 + 48;
                if i == at { 9 } else { byte }
            })
            .collect(),
        ));
        for (case, bytes) in cases {
            let error = decode("m", Path::new("m.glb"), &bytes).expect_err(case);
            assert_eq!(error.code(), "engine.bad_model", "{case}");
            assert!(error.to_string().contains("\"m\""), "{case}: {error}");
        }
    }
}
